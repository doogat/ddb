//! Shared, read-only planning for service and SQL cascade deletion.

use std::collections::BTreeSet;

use crate::error::{DoogatError, Result};

pub(crate) struct CascadeNode {
    pub path: String,
    pub children: Vec<(String, String)>,
}

enum Visit {
    Enter { id: String, table: String },
    Leave(String),
}

/// Inspect the whole graph before mutation. Completed nodes deduplicate
/// diamonds; only a node still on the active DFS path forms a cycle.
pub(crate) fn plan(
    roots: &[String],
    mut inspect: impl FnMut(&str) -> Result<CascadeNode>,
) -> Result<Vec<(String, String)>> {
    let mut visits: Vec<Visit> = roots
        .iter()
        .rev()
        .map(|id| Visit::Enter {
            id: id.clone(),
            table: String::new(),
        })
        .collect();
    let mut active = BTreeSet::new();
    let mut complete = BTreeSet::new();
    let mut tables = Vec::new();
    let mut result = Vec::new();
    while let Some(visit) = visits.pop() {
        match visit {
            Visit::Leave(id) => {
                active.remove(&id);
                complete.insert(id);
                tables.pop();
            }
            Visit::Enter { id, table } => {
                if complete.contains(&id) {
                    continue;
                }
                if !active.insert(id.clone()) {
                    tables.push(table);
                    return Err(DoogatError::cascade_cycle(
                        tables.into_iter().filter(|table| !table.is_empty()),
                    ));
                }
                let node = inspect(&id)?;
                result.push((id.clone(), node.path));
                tables.push(table);
                visits.push(Visit::Leave(id));
                visits.extend(
                    node.children
                        .into_iter()
                        .rev()
                        .map(|(table, id)| Visit::Enter { id, table }),
                );
            }
        }
    }
    Ok(result)
}

pub(crate) struct ReferenceEdit {
    pub id: String,
    pub content: String,
    pub parsed: crate::types::ParsedDoogat,
}

struct ReferenceSource<'a> {
    id: String,
    targets: Vec<(&'a str, &'a str)>,
}

/// Compose all removed targets before editing a surviving source once.
/// The SQL caller supplies its buffered view; the service supplies Git reads.
pub(crate) fn reference_edits(
    index: &dyn crate::traits::SqlBackend,
    plan: &[(String, String)],
    mut read_content: impl FnMut(&str) -> Result<String>,
) -> Result<Vec<ReferenceEdit>> {
    use std::collections::BTreeMap;
    let deleted_paths: BTreeSet<&str> = plan.iter().map(|(_, path)| path.as_str()).collect();
    let mut sources: BTreeMap<String, ReferenceSource<'_>> = BTreeMap::new();
    for (id, path) in plan {
        for (source_id, source_path) in index.backlinks_by_target(id, path)? {
            if !deleted_paths.contains(source_path.as_str()) {
                sources
                    .entry(source_path)
                    .or_insert_with(|| ReferenceSource {
                        id: source_id,
                        targets: Vec::new(),
                    })
                    .targets
                    .push((id, path));
            }
        }
    }
    let mut edits = Vec::new();
    for (path, ReferenceSource { id, targets }) in sources {
        let content = read_content(&path)?;
        let mut parsed = crate::parser::parse(&content, &path)?;
        let old = &parsed.reference_section;
        let retained: Vec<&str> = old
            .lines()
            .filter(|line| {
                !targets.iter().any(|(id, path)| {
                    line.contains(&format!("[[{id}]]")) || line.contains(&format!("[[{path}]]"))
                })
            })
            .collect();
        let section = if retained.is_empty() {
            String::new()
        } else {
            format!("{}\n", retained.join("\n"))
        };
        if section == *old {
            continue;
        }
        parsed.reference_section = section;
        let content = crate::parser::serialize(&parsed);
        let parsed = crate::parser::parse(&content, &path)?;
        edits.push(ReferenceEdit {
            id,
            content,
            parsed,
        });
    }
    Ok(edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(roots: &[&str], edges: &[(&str, &[&str])]) -> Result<Vec<String>> {
        let roots: Vec<String> = roots.iter().map(|s| (*s).into()).collect();
        plan(&roots, |id| {
            let children = edges
                .iter()
                .find(|(from, _)| *from == id)
                .map(|(_, to)| *to)
                .unwrap_or_default();
            Ok(CascadeNode {
                path: format!("{id}.md"),
                children: children
                    .iter()
                    .map(|child| ("node".into(), (*child).into()))
                    .collect(),
            })
        })
        .map(|nodes| nodes.into_iter().map(|(id, _)| id).collect())
    }

    #[test]
    fn diamonds_duplicates_and_overlapping_roots_are_deleted_once() {
        assert_eq!(
            graph(
                &["root", "b"],
                &[
                    ("root", &["a", "a", "b"]),
                    ("a", &["shared"]),
                    ("b", &["shared"])
                ]
            )
            .unwrap(),
            ["root", "a", "shared", "b"]
        );
    }

    #[test]
    fn cross_sibling_edges_cannot_hide_a_cycle() {
        assert!(matches!(
            graph(
                &["root"],
                &[("root", &["a", "b"]), ("a", &["b"]), ("b", &["a"])]
            ),
            Err(DoogatError::Structured {
                code: crate::error::codes::CASCADE_CYCLE,
                ..
            })
        ));
    }

    #[test]
    fn self_and_root_cycles_are_refused() {
        for edges in [
            vec![("root", &["root"][..])],
            vec![("root", &["a"][..]), ("a", &["root"][..])],
        ] {
            assert!(matches!(
                graph(&["root"], &edges),
                Err(DoogatError::Structured {
                    code: crate::error::codes::CASCADE_CYCLE,
                    ..
                })
            ));
        }
    }

    #[test]
    fn descendant_inspection_errors_abort_the_whole_plan() {
        let error = plan(&["root".into()], |id| {
            if id == "child" {
                return Err(DoogatError::NotFound(id.into()));
            }
            Ok(CascadeNode {
                path: "root.md".into(),
                children: vec![("node".into(), "child".into())],
            })
        })
        .unwrap_err();
        assert!(matches!(error, DoogatError::NotFound(id) if id == "child"));
    }

    #[test]
    fn long_chains_do_not_depend_on_the_call_stack() {
        let nodes = plan(&["0".into()], |id| {
            let next = id.parse::<usize>().unwrap() + 1;
            Ok(CascadeNode {
                path: format!("{id}.md"),
                children: if next < 2000 {
                    vec![("node".into(), next.to_string())]
                } else {
                    vec![]
                },
            })
        })
        .unwrap();
        assert_eq!(nodes.len(), 2000);
        assert_eq!(nodes.last().unwrap().0, "1999");
    }
}
