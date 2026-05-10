use std::cmp::Ordering;
use std::collections::HashMap;

use crate::error::{DoogatError, Result};
use crate::hlc::{append_hlc_trailer, Hlc};
use crate::indexer::Index;
use crate::sql_engine::schema_from_parsed;
use crate::traits::{DoogatStore, GitHistory};
use crate::types::{DoogatFix, Fix};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SingletonSweepReport {
    pub conflicts_resolved: usize,
    pub details: Vec<(String, String, Vec<String>)>,
    pub fixes: Vec<DoogatFix>,
}

#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    path: String,
    content: String,
    hlc: Option<Hlc>,
}

pub fn singleton_sweep(
    repo: &(impl DoogatStore + GitHistory),
    index: &Index,
    resolved_at: &Hlc,
) -> Result<SingletonSweepReport> {
    let paths = repo.list_doogats()?;
    let head_oid = repo.head_oid()?;
    let mut typedef_schemas = index.load_all_typedefs(repo);
    let mut candidates_by_type: HashMap<String, Vec<Candidate>> = HashMap::new();

    for path in &paths {
        if path.starts_with("ddb/_conflicts/") {
            continue;
        }

        let content = match repo.read_file(path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let parsed = match crate::parser::parse(&content, path) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };

        match parsed.meta.doogat_type.as_deref() {
            Some("_typedef") => {
                if let Ok(schema) = schema_from_parsed(&parsed) {
                    typedef_schemas.insert(schema.table_name.clone(), schema);
                }
            }
            Some(type_name) => {
                let Some(id) = parsed.meta.id.as_ref().map(|id| id.0.clone()) else {
                    continue;
                };
                candidates_by_type
                    .entry(type_name.to_string())
                    .or_default()
                    .push(Candidate {
                        id,
                        path: path.clone(),
                        content,
                        hlc: repo.find_hlc_for_path(&head_oid.0, path),
                    });
            }
            None => {}
        }
    }

    let mut report = SingletonSweepReport::default();
    let mut writes: Vec<(String, String)> = Vec::new();
    let mut deletes: Vec<String> = Vec::new();

    for (table_name, mut candidates) in candidates_by_type {
        let Some(schema) = typedef_schemas.get(&table_name) else {
            continue;
        };
        if !schema.singleton || candidates.len() <= 1 {
            continue;
        }

        candidates.sort_by(compare_candidates);
        let Some(winner) = candidates.pop() else {
            tracing::error!(
                table = %table_name,
                "singleton sweep invariant violated: candidates unexpectedly empty after length check"
            );
            return Err(DoogatError::Conflict(format!(
                "singleton sweep invariant violated for table {table_name}: missing winner after candidate sort"
            )));
        };
        let winner_id = winner.id.clone();
        let loser_ids: Vec<String> = candidates
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect();

        for loser in candidates {
            writes.push((
                format!("ddb/_conflicts/{}.md", loser.id),
                quarantine_content(&loser.content, &winner_id, &table_name, resolved_at)?,
            ));
            deletes.push(loser.path);
        }

        report.conflicts_resolved += loser_ids.len();
        report
            .details
            .push((table_name.clone(), winner_id.clone(), loser_ids.clone()));
        report.fixes.push(DoogatFix {
            path: winner.path,
            applied: vec![Fix::SingletonConflictResolved {
                table: table_name,
                winner: winner_id,
                losers: loser_ids,
            }],
        });
    }

    if !writes.is_empty() {
        let write_refs: Vec<(&str, &str)> = writes
            .iter()
            .map(|(path, content)| (path.as_str(), content.as_str()))
            .collect();
        let delete_refs: Vec<&str> = deletes.iter().map(String::as_str).collect();
        let message = append_hlc_trailer(
            &format!(
                "fix: resolve {} singleton conflicts across {} tables",
                report.conflicts_resolved,
                report.details.len()
            ),
            resolved_at,
        );
        repo.commit_batch(&write_refs, &delete_refs, &message)?;
    }

    Ok(report)
}

fn compare_candidates(left: &Candidate, right: &Candidate) -> Ordering {
    match (&left.hlc, &right.hlc) {
        (Some(left_hlc), Some(right_hlc)) if left_hlc != right_hlc => left_hlc.cmp(right_hlc),
        _ => left.id.cmp(&right.id),
    }
}

fn quarantine_content(
    content: &str,
    winner_id: &str,
    table_name: &str,
    resolved_at: &Hlc,
) -> Result<String> {
    let zones = crate::parser::split_zones(content)?;
    let mut quarantined = String::from("---\n");
    quarantined.push_str(&zones.raw_frontmatter);
    if !zones.raw_frontmatter.is_empty() && !zones.raw_frontmatter.ends_with('\n') {
        quarantined.push('\n');
    }
    quarantined.push_str(&format!("singleton_conflict_loser: {winner_id}\n"));
    quarantined.push_str(&format!("singleton_conflict_table: {table_name}\n"));
    quarantined.push_str(&format!(
        "singleton_conflict_resolved_at: {}\n",
        resolved_at
    ));
    quarantined.push_str("---\n");
    quarantined.push_str(&zones.body);
    if !zones.reference_section.is_empty() {
        quarantined.push_str("\n---\n");
        quarantined.push_str(&zones.reference_section);
    }
    Ok(quarantined)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::git_ops::GitRepo;

    fn temp_repo() -> (TempDir, GitRepo) {
        let dir = TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        (dir, repo)
    }

    fn test_index() -> Index {
        Index::open_in_memory().unwrap()
    }

    fn test_hlc(wall_ms: u64, counter: u32, node: &str) -> Hlc {
        Hlc {
            wall_ms,
            counter,
            node: node.to_string(),
        }
    }

    fn singleton_typedef() -> &'static str {
        "\
---
id: 20260510120000
title: app_config
type: _typedef
singleton: true
columns:
  - name: theme
    data_type: TEXT
    zone: frontmatter
---
"
    }

    fn non_singleton_typedef() -> &'static str {
        "\
---
id: 20260510121000
title: app_config
type: _typedef
columns:
  - name: theme
    data_type: TEXT
    zone: frontmatter
---
"
    }

    fn app_config_row(id: &str, theme: &str, note: &str) -> String {
        format!(
            "\
---
id: {id}
title: Config {id}
type: app_config
theme: {theme}
---
Body for {id}
---
- source:: {note}
"
        )
    }

    fn commit_with_hlc(repo: &GitRepo, path: &str, content: &str, wall_ms: u64) {
        let msg = append_hlc_trailer(
            "write singleton candidate",
            &test_hlc(wall_ms, 0, "node0001"),
        );
        repo.commit_file(path, content, &msg).unwrap();
    }

    #[test]
    fn singleton_sweep_no_typedef_no_op() {
        let (_dir, repo) = temp_repo();
        let index = test_index();

        let report = singleton_sweep(&repo, &index, &test_hlc(9000, 0, "sweep001")).unwrap();

        assert_eq!(report.conflicts_resolved, 0);
        assert!(report.details.is_empty());
    }

    #[test]
    fn singleton_sweep_zero_rows_no_op() {
        let (_dir, repo) = temp_repo();
        let index = test_index();
        repo.commit_file(
            "ddb/_typedef/20260510120000.md",
            singleton_typedef(),
            "add singleton typedef",
        )
        .unwrap();

        let report = singleton_sweep(&repo, &index, &test_hlc(9000, 0, "sweep001")).unwrap();

        assert_eq!(report.conflicts_resolved, 0);
        assert!(report.details.is_empty());
    }

    #[test]
    fn singleton_sweep_one_row_no_op() {
        let (_dir, repo) = temp_repo();
        let index = test_index();
        repo.commit_file(
            "ddb/_typedef/20260510120000.md",
            singleton_typedef(),
            "add singleton typedef",
        )
        .unwrap();
        commit_with_hlc(
            &repo,
            "ddb/20260601000000.md",
            &app_config_row("20260601000000", "dark", "one"),
            1000,
        );

        let report = singleton_sweep(&repo, &index, &test_hlc(9000, 0, "sweep001")).unwrap();

        assert_eq!(report.conflicts_resolved, 0);
        assert!(report.details.is_empty());
        assert!(repo.read_file("ddb/20260601000000.md").is_ok());
    }

    #[test]
    fn singleton_sweep_two_rows_resolves_one() {
        let (_dir, repo) = temp_repo();
        let index = test_index();
        repo.commit_file(
            "ddb/_typedef/20260510120000.md",
            singleton_typedef(),
            "add singleton typedef",
        )
        .unwrap();
        let loser_path = "ddb/20260601000000.md";
        let loser_content = app_config_row("20260601000000", "dark", "alpha");
        commit_with_hlc(&repo, loser_path, &loser_content, 1000);
        commit_with_hlc(
            &repo,
            "ddb/20260601000010.md",
            &app_config_row("20260601000010", "light", "beta"),
            2000,
        );

        let resolved_at = test_hlc(9000, 0, "sweep001");
        let report = singleton_sweep(&repo, &index, &resolved_at).unwrap();

        assert_eq!(report.conflicts_resolved, 1);
        assert_eq!(
            report.details,
            vec![(
                "app_config".to_string(),
                "20260601000010".to_string(),
                vec!["20260601000000".to_string()]
            )]
        );
        assert!(repo.read_file(loser_path).is_err());
        let quarantined = repo.read_file("ddb/_conflicts/20260601000000.md").unwrap();
        assert!(quarantined.contains("singleton_conflict_loser: 20260601000010"));
        assert!(quarantined.contains("singleton_conflict_table: app_config"));
        assert!(quarantined.contains("singleton_conflict_resolved_at: 9000-0000-sweep001"));
        let original = crate::parser::split_zones(&loser_content).unwrap();
        let quarantined_zones = crate::parser::split_zones(&quarantined).unwrap();
        assert_eq!(quarantined_zones.body, original.body);
        assert_eq!(
            quarantined_zones.reference_section,
            original.reference_section
        );
        assert!(
            report.fixes.iter().any(|doogat_fix| {
                matches!(
                    doogat_fix.applied.as_slice(),
                    [Fix::SingletonConflictResolved { table, winner, losers }]
                        if table == "app_config"
                            && winner == "20260601000010"
                            && losers == &vec!["20260601000000".to_string()]
                )
            }),
            "expected singleton conflict warning event: {:?}",
            report.fixes
        );
    }

    #[test]
    fn singleton_sweep_emits_fix_event_for_materialized_singleton_conflict() {
        let (_dir, repo) = temp_repo();
        let index = test_index();
        repo.commit_file(
            "ddb/_typedef/20260510120000.md",
            non_singleton_typedef(),
            "add non-singleton typedef",
        )
        .unwrap();
        let loser_path = "ddb/20260601000000.md";
        let loser_content = app_config_row("20260601000000", "dark", "alpha");
        commit_with_hlc(&repo, loser_path, &loser_content, 1000);
        commit_with_hlc(
            &repo,
            "ddb/20260601000010.md",
            &app_config_row("20260601000010", "light", "beta"),
            2000,
        );
        index.rebuild(&repo).unwrap();
        assert_eq!(
            index
                .query_raw("SELECT id FROM app_config ORDER BY id")
                .unwrap(),
            vec![
                vec!["20260601000000".to_string()],
                vec!["20260601000010".to_string()]
            ]
        );
        repo.commit_file(
            "ddb/_typedef/20260510120000.md",
            singleton_typedef(),
            "upgrade typedef to singleton",
        )
        .unwrap();

        let resolved_at = test_hlc(9000, 0, "sweep001");
        let report = singleton_sweep(&repo, &index, &resolved_at).unwrap();

        assert_eq!(report.conflicts_resolved, 1);
        assert!(repo.read_file(loser_path).is_err());
        let quarantined = repo.read_file("ddb/_conflicts/20260601000000.md").unwrap();
        assert!(quarantined.contains("singleton_conflict_loser: 20260601000010"));
        assert!(quarantined.contains("singleton_conflict_table: app_config"));
        assert!(matches!(
            report.fixes.as_slice(),
            [DoogatFix {
                applied,
                ..
            }] if matches!(
                applied.as_slice(),
                [Fix::SingletonConflictResolved { table, winner, losers }]
                    if table == "app_config"
                        && winner == "20260601000010"
                        && losers == &vec!["20260601000000".to_string()]
            )
        ));
    }

    #[test]
    fn singleton_sweep_three_rows_resolves_two() {
        let (_dir, repo) = temp_repo();
        let index = test_index();
        repo.commit_file(
            "ddb/_typedef/20260510120000.md",
            singleton_typedef(),
            "add singleton typedef",
        )
        .unwrap();
        commit_with_hlc(
            &repo,
            "ddb/20260601000000.md",
            &app_config_row("20260601000000", "dark", "alpha"),
            1000,
        );
        commit_with_hlc(
            &repo,
            "ddb/20260601000010.md",
            &app_config_row("20260601000010", "light", "beta"),
            2000,
        );
        commit_with_hlc(
            &repo,
            "ddb/20260601000020.md",
            &app_config_row("20260601000020", "blue", "gamma"),
            3000,
        );

        let report = singleton_sweep(&repo, &index, &test_hlc(9000, 0, "sweep001")).unwrap();

        assert_eq!(report.conflicts_resolved, 2);
        assert_eq!(
            report.details,
            vec![(
                "app_config".to_string(),
                "20260601000020".to_string(),
                vec!["20260601000000".to_string(), "20260601000010".to_string()]
            )]
        );
        assert!(repo.read_file("ddb/_conflicts/20260601000000.md").is_ok());
        assert!(repo.read_file("ddb/_conflicts/20260601000010.md").is_ok());
        assert!(repo.read_file("ddb/20260601000020.md").is_ok());
    }

    #[test]
    fn singleton_sweep_skips_already_quarantined() {
        let (_dir, repo) = temp_repo();
        let index = test_index();
        repo.commit_file(
            "ddb/_typedef/20260510120000.md",
            singleton_typedef(),
            "add singleton typedef",
        )
        .unwrap();
        commit_with_hlc(
            &repo,
            "ddb/20260601000010.md",
            &app_config_row("20260601000010", "light", "beta"),
            2000,
        );
        repo.commit_file(
            "ddb/_conflicts/20260601000000.md",
            "\
---
id: 20260601000000
title: Config 20260601000000
type: app_config
theme: dark
singleton_conflict_loser: 20260601000010
singleton_conflict_table: app_config
singleton_conflict_resolved_at: 9000-0000-sweep001
---
Body for 20260601000000
---
- source:: alpha
",
            "already quarantined",
        )
        .unwrap();

        let report = singleton_sweep(&repo, &index, &test_hlc(9100, 0, "sweep001")).unwrap();

        assert_eq!(report.conflicts_resolved, 0);
        assert!(report.details.is_empty());
        assert!(repo.read_file("ddb/_conflicts/20260601000000.md").is_ok());
    }

    #[test]
    fn singleton_sweep_skips_non_singleton_typedef() {
        let (_dir, repo) = temp_repo();
        let index = test_index();
        repo.commit_file(
            "ddb/_typedef/20260510121000.md",
            non_singleton_typedef(),
            "add non-singleton typedef",
        )
        .unwrap();
        commit_with_hlc(
            &repo,
            "ddb/20260601000000.md",
            &app_config_row("20260601000000", "dark", "alpha"),
            1000,
        );
        commit_with_hlc(
            &repo,
            "ddb/20260601000010.md",
            &app_config_row("20260601000010", "light", "beta"),
            2000,
        );

        let report = singleton_sweep(&repo, &index, &test_hlc(9000, 0, "sweep001")).unwrap();

        assert_eq!(report.conflicts_resolved, 0);
        assert!(repo.read_file("ddb/20260601000000.md").is_ok());
        assert!(repo.read_file("ddb/20260601000010.md").is_ok());
    }

    #[test]
    fn singleton_sweep_idempotent() {
        let (_dir, repo) = temp_repo();
        let index = test_index();
        repo.commit_file(
            "ddb/_typedef/20260510120000.md",
            singleton_typedef(),
            "add singleton typedef",
        )
        .unwrap();
        commit_with_hlc(
            &repo,
            "ddb/20260601000000.md",
            &app_config_row("20260601000000", "dark", "alpha"),
            1000,
        );
        commit_with_hlc(
            &repo,
            "ddb/20260601000010.md",
            &app_config_row("20260601000010", "light", "beta"),
            2000,
        );

        let first = singleton_sweep(&repo, &index, &test_hlc(9000, 0, "sweep001")).unwrap();
        let second = singleton_sweep(&repo, &index, &test_hlc(9100, 0, "sweep001")).unwrap();

        assert_eq!(first.conflicts_resolved, 1);
        assert_eq!(second.conflicts_resolved, 0);
        assert!(repo.read_file("ddb/_conflicts/20260601000000.md").is_ok());
    }
}
