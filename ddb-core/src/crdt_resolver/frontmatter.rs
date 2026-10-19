use std::collections::BTreeMap;

use automerge::{transaction::Transactable, AutoCommit, ObjType, ReadDoc};
use sha2::{Digest, Sha256};

use super::append_log::resolve_append_log;
use super::body::{merge_body, merge_reference};
use super::lww::resolve_lww;
use crate::error::{DoogatError, Result};
use crate::parser;
use crate::types::{ConflictFile, Doogat, ResolvedFile};

/// Derive a stable Automerge actor from content bytes.
/// Symmetric under ours/theirs swap: the SAME physical content always maps to the SAME actor on
/// every node, so Automerge's actor-ordered tie-break picks the same winner regardless of which
/// side a node calls "ours". 16 bytes = valid ActorId.
pub(super) fn derive_actor(content: &str) -> automerge::ActorId {
    let digest = Sha256::digest(content.as_bytes());
    automerge::ActorId::from(&digest[..16])
}

/// Resolve all conflict files using per-zone CRDT merge strategies.
/// If `crdt_strategy` is set to something other than `preset:default`, a warning is logged
/// since only the default strategy is currently implemented.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn resolve_conflicts(
    conflicts: Vec<ConflictFile>,
    crdt_strategy: Option<&str>,
) -> Result<Vec<ResolvedFile>> {
    if let Some(strategy) = crdt_strategy {
        match strategy {
            "preset:default" => {}
            "preset:last-writer-wins" => return resolve_lww(conflicts),
            "preset:append-log" => return resolve_append_log(conflicts),
            other => {
                tracing::warn!("crdt_strategy '{}' not recognized; using default", other);
            }
        }
    }

    let mut resolved = Vec::new();

    for conflict in conflicts {
        let ancestor_content = conflict.ancestor.as_deref().unwrap_or("");

        let ancestor = parse_zones(ancestor_content)?;
        let ours = parse_zones(&conflict.ours)?;
        let theirs = parse_zones(&conflict.theirs)?;

        let (merged_fm, fm_crdt_bytes) = merge_frontmatter(
            &ancestor.raw_frontmatter,
            &ours.raw_frontmatter,
            &theirs.raw_frontmatter,
        )?;
        let merged_body = merge_body(&ancestor.body, &ours.body, &theirs.body)?;
        let merged_ref = merge_reference(
            &ancestor.reference_section,
            &ours.reference_section,
            &theirs.reference_section,
        )?;

        // Reassemble via parser
        let meta = parser::parse_frontmatter(&merged_fm, &conflict.path)?;
        let inline_fields = parser::extract_inline_fields(&merged_body, &merged_ref)?;
        let wikilinks = parser::extract_links(&merged_fm, &merged_body, &merged_ref);

        let parsed = crate::types::ParsedDoogat {
            meta,
            body: merged_body,
            sections: vec![],
            reference_section: merged_ref,
            inline_fields,
            links: wikilinks,
            body_tags: vec![],
            checkboxes: vec![],
            path: conflict.path.clone(),
            updated_at: None,
        };

        let content = parser::serialize(&parsed);
        resolved.push(ResolvedFile {
            path: conflict.path,
            content,
            fm_crdt_bytes: Some(fm_crdt_bytes),
        });
    }

    Ok(resolved)
}

pub(super) fn parse_zones(content: &str) -> Result<Doogat> {
    if content.is_empty() {
        return Ok(Doogat {
            raw_frontmatter: String::new(),
            body: String::new(),
            reference_section: String::new(),
        });
    }
    parser::split_zones(content)
}

/// A frontmatter value: either a scalar string or a list of strings.
#[derive(Debug, Clone, PartialEq)]
enum FmValue {
    Scalar(String),
    List(Vec<String>),
}

/// Merge YAML frontmatter at field granularity.
/// Scalar fields use Automerge Map CRDT. List fields (e.g. tags) use three-way set merge.
/// Returns `(resolved_yaml, automerge_doc_bytes)` for CRDT state persistence.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn merge_frontmatter(ancestor: &str, ours: &str, theirs: &str) -> Result<(String, Vec<u8>)> {
    let ancestor_map = yaml_to_map(ancestor)?;
    let ours_map = yaml_to_map(ours)?;
    let theirs_map = yaml_to_map(theirs)?;

    // Partition into scalars and lists
    let (ancestor_scalars, ancestor_lists) = partition_fm(&ancestor_map);
    let (ours_scalars, ours_lists) = partition_fm(&ours_map);
    let (theirs_scalars, theirs_lists) = partition_fm(&theirs_map);

    // Merge scalars via Automerge Map CRDT
    let mut doc = AutoCommit::new();
    doc.set_actor(derive_actor(ancestor));
    let map_id = doc.put_object(automerge::ROOT, "frontmatter", ObjType::Map)?;
    for (k, v) in &ancestor_scalars {
        doc.put(&map_id, k.as_str(), v.as_str())?;
    }

    let mut doc_ours = doc.fork();
    doc_ours.set_actor(derive_actor(ours));
    let ours_map_id = doc_ours
        .get(&automerge::ROOT, "frontmatter")?
        .map(|(_, id)| id)
        .ok_or_else(|| DoogatError::Parse("missing frontmatter map".into()))?;
    apply_scalar_diff(
        &mut doc_ours,
        &ours_map_id,
        &ancestor_scalars,
        &ours_scalars,
    )?;

    let mut doc_theirs = doc.fork();
    doc_theirs.set_actor(derive_actor(theirs));
    let theirs_map_id = doc_theirs
        .get(&automerge::ROOT, "frontmatter")?
        .map(|(_, id)| id)
        .ok_or_else(|| DoogatError::Parse("missing frontmatter map".into()))?;
    apply_scalar_diff(
        &mut doc_theirs,
        &theirs_map_id,
        &ancestor_scalars,
        &theirs_scalars,
    )?;

    doc_ours.merge(&mut doc_theirs)?;

    let merged_map_id = doc_ours
        .get(&automerge::ROOT, "frontmatter")?
        .map(|(_, id)| id)
        .ok_or_else(|| DoogatError::Parse("missing frontmatter map after merge".into()))?;

    let mut merged = BTreeMap::new();
    for key in doc_ours.keys(&merged_map_id) {
        if let Some((value, _)) = doc_ours.get(&merged_map_id, key.as_str())? {
            let scalar = value
                .to_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| value.to_string());
            merged.insert(key, FmValue::Scalar(scalar));
        }
    }

    // Merge lists via three-way set merge
    for (k, v) in merge_list_fields(&ancestor_lists, &ours_lists, &theirs_lists) {
        merged.insert(k, v);
    }

    let doc_bytes = doc_ours.save();
    Ok((map_to_yaml(&merged), doc_bytes))
}

/// Partition an FmValue map into scalar and list components.
fn partition_fm(
    map: &BTreeMap<String, FmValue>,
) -> (BTreeMap<String, String>, BTreeMap<String, Vec<String>>) {
    let mut scalars = BTreeMap::new();
    let mut lists = BTreeMap::new();
    for (k, v) in map {
        match v {
            FmValue::Scalar(s) => {
                scalars.insert(k.clone(), s.clone());
            }
            FmValue::List(l) => {
                lists.insert(k.clone(), l.clone());
            }
        }
    }
    (scalars, lists)
}

/// Three-way set merge for list-typed frontmatter fields.
fn merge_list_fields(
    ancestor: &BTreeMap<String, Vec<String>>,
    ours: &BTreeMap<String, Vec<String>>,
    theirs: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, FmValue> {
    use std::collections::HashSet;
    let mut result = BTreeMap::new();
    let all_keys: HashSet<&String> = ancestor
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();

    for key in all_keys {
        let a: HashSet<&str> = ancestor
            .get(key)
            .map(|v| v.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let o: HashSet<&str> = ours
            .get(key)
            .map(|v| v.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let t: HashSet<&str> = theirs
            .get(key)
            .map(|v| v.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();

        // Key presence: if both removed it, skip. If one removed, honor removal.
        let ours_present = ours.contains_key(key);
        let theirs_present = theirs.contains_key(key);
        let ancestor_present = ancestor.contains_key(key);

        if ancestor_present && !ours_present && !theirs_present {
            continue;
        } // both removed
        if ancestor_present && !ours_present {
            continue;
        } // ours removed
        if ancestor_present && !theirs_present {
            continue;
        } // theirs removed

        // Three-way set merge: start with ancestor, add new items from each side, remove items removed by each side
        let ours_added: HashSet<&str> = o.difference(&a).copied().collect();
        let theirs_added: HashSet<&str> = t.difference(&a).copied().collect();
        let ours_removed: HashSet<&str> = a.difference(&o).copied().collect();
        let theirs_removed: HashSet<&str> = a.difference(&t).copied().collect();

        let mut merged: HashSet<&str> = a.clone();
        merged.extend(ours_added);
        merged.extend(theirs_added);
        for r in &ours_removed {
            merged.remove(r);
        }
        for r in &theirs_removed {
            merged.remove(r);
        }

        // Survivors in ANCESTOR order (the shared merge base → identical on every node),
        // then all added items sorted lexicographically (deterministic AND role-independent).
        let ancestor_list = ancestor.get(key).map(|v| v.as_slice()).unwrap_or_default();
        let mut ordered: Vec<String> = ancestor_list
            .iter()
            .filter(|s| merged.contains(s.as_str()))
            .cloned()
            .collect();
        let mut additions: Vec<String> = merged
            .iter()
            .filter(|s| !ancestor_list.iter().any(|a| a == *s))
            .map(|s| s.to_string())
            .collect();
        additions.sort();
        ordered.extend(additions);
        result.insert(key.clone(), FmValue::List(ordered));
    }
    result
}

fn yaml_to_map(yaml: &str) -> Result<BTreeMap<String, FmValue>> {
    if yaml.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let value: serde_yaml::Value = serde_yaml::from_str(yaml)?;
    let mut map = BTreeMap::new();
    if let serde_yaml::Value::Mapping(m) = value {
        for (k, v) in m {
            let key = match k {
                serde_yaml::Value::String(s) => s,
                other => serde_yaml::to_string(&other)?.trim().to_string(),
            };
            let val = match &v {
                serde_yaml::Value::Sequence(seq) => {
                    let items: Vec<String> = seq
                        .iter()
                        .map(|item| match item {
                            serde_yaml::Value::String(s) => s.clone(),
                            other => serde_yaml::to_string(other)
                                .unwrap_or_default()
                                .trim()
                                .to_string(),
                        })
                        .collect();
                    FmValue::List(items)
                }
                serde_yaml::Value::String(s) => FmValue::Scalar(s.clone()),
                other => FmValue::Scalar(
                    serde_yaml::to_string(other)
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                ),
            };
            map.insert(key, val);
        }
    }
    Ok(map)
}

fn map_to_yaml(map: &BTreeMap<String, FmValue>) -> String {
    let mut out = String::new();
    for (k, v) in map {
        match v {
            FmValue::Scalar(s) => {
                out.push_str(&format!("{k}: {}\n", crate::parser::yaml_quote(s)));
            }
            FmValue::List(items) => {
                out.push_str(&format!("{k}:\n"));
                for item in items {
                    out.push_str(&format!("  - {}\n", crate::parser::yaml_quote(item)));
                }
            }
        }
    }
    out
}

fn apply_scalar_diff(
    doc: &mut AutoCommit,
    map_id: &automerge::ObjId,
    ancestor: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> Result<()> {
    for (k, v) in current {
        if ancestor.get(k) != Some(v) {
            doc.put(map_id, k.as_str(), v.as_str())?;
        }
    }
    for k in ancestor.keys() {
        if !current.contains_key(k) {
            doc.delete(map_id, k.as_str())?;
        }
    }
    Ok(())
}
