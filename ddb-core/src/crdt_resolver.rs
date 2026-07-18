use std::collections::BTreeMap;

use automerge::{transaction::Transactable, AutoCommit, ObjType, ReadDoc};
use sha2::{Digest, Sha256};

use crate::error::{DoogatError, Result};
use crate::hlc::Hlc;
use crate::parser;
use crate::types::{ConflictFile, Doogat, ResolvedFile};

impl From<automerge::AutomergeError> for DoogatError {
    fn from(e: automerge::AutomergeError) -> Self {
        Self::Automerge(e.to_string())
    }
}

/// Derive a stable Automerge actor from content bytes.
/// Symmetric under ours/theirs swap: the SAME physical content always maps to the SAME actor on
/// every node, so Automerge's actor-ordered tie-break picks the same winner regardless of which
/// side a node calls "ours". 16 bytes = valid ActorId.
fn derive_actor(content: &str) -> automerge::ActorId {
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

fn parse_zones(content: &str) -> Result<Doogat> {
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

/// Merge body text using Automerge text CRDT with line-level diffs.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn merge_body(ancestor: &str, ours: &str, theirs: &str) -> Result<String> {
    let mut doc = AutoCommit::new();
    doc.set_actor(derive_actor(ancestor));
    let text_id = doc.put_object(automerge::ROOT, "body", ObjType::Text)?;
    doc.splice_text(&text_id, 0, 0, ancestor)?;

    // Fork for ours
    let mut doc_ours = doc.fork();
    doc_ours.set_actor(derive_actor(ours));
    let ours_text_id = doc_ours
        .get(&automerge::ROOT, "body")?
        .map(|(_, id)| id)
        .ok_or_else(|| DoogatError::Parse("missing body text".into()))?;
    apply_text_diff(&mut doc_ours, &ours_text_id, ancestor, ours)?;

    // Fork for theirs
    let mut doc_theirs = doc.fork();
    doc_theirs.set_actor(derive_actor(theirs));
    let theirs_text_id = doc_theirs
        .get(&automerge::ROOT, "body")?
        .map(|(_, id)| id)
        .ok_or_else(|| DoogatError::Parse("missing body text".into()))?;
    apply_text_diff(&mut doc_theirs, &theirs_text_id, ancestor, theirs)?;

    // Merge
    doc_ours.merge(&mut doc_theirs)?;

    // Extract merged text
    let merged_text_id = doc_ours
        .get(&automerge::ROOT, "body")?
        .map(|(_, id)| id)
        .ok_or_else(|| DoogatError::Parse("missing body text after merge".into()))?;
    let merged = doc_ours.text(&merged_text_id)?;

    Ok(merged)
}

fn apply_text_diff(
    doc: &mut AutoCommit,
    text_id: &automerge::ObjId,
    old: &str,
    new: &str,
) -> Result<()> {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_chars(old, new);

    // Consolidate consecutive same-tag changes into single ops
    struct Op {
        pos: usize,
        delete: usize,
        insert: String,
    }

    let mut ops: Vec<Op> = Vec::new();
    let mut orig_pos = 0usize;

    for change in diff.iter_all_changes() {
        let ch = change.value();
        match change.tag() {
            ChangeTag::Equal => {
                orig_pos += 1;
            }
            ChangeTag::Delete => {
                if let Some(last) = ops.last_mut() {
                    if last.pos + last.delete == orig_pos && last.insert.is_empty() {
                        last.delete += 1;
                        orig_pos += 1;
                        continue;
                    }
                }
                ops.push(Op {
                    pos: orig_pos,
                    delete: 1,
                    insert: String::new(),
                });
                orig_pos += 1;
            }
            ChangeTag::Insert => {
                if let Some(last) = ops.last_mut() {
                    if last.pos == orig_pos && last.delete == 0 {
                        last.insert.push_str(ch);
                        continue;
                    }
                }
                ops.push(Op {
                    pos: orig_pos,
                    delete: 0,
                    insert: ch.to_string(),
                });
            }
        }
    }

    // Apply in reverse order so positions stay valid
    for op in ops.iter().rev() {
        doc.splice_text(text_id, op.pos, op.delete as isize, &op.insert)?;
    }

    Ok(())
}

/// Merge reference sections using Automerge List CRDT (fork-diff-merge).
/// Each `- key:: value` line becomes a List element; merged list is sorted alphabetically on export.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn merge_reference(ancestor: &str, ours: &str, theirs: &str) -> Result<String> {
    let ancestor_lines = ref_lines(ancestor);
    let ours_lines = ref_lines(ours);
    let theirs_lines = ref_lines(theirs);

    // Seed Automerge doc with ancestor state as a List
    let mut doc = AutoCommit::new();
    doc.set_actor(derive_actor(ancestor));
    let list_id = doc.put_object(automerge::ROOT, "refs", ObjType::List)?;
    for (i, line) in ancestor_lines.iter().enumerate() {
        doc.insert(&list_id, i, line.as_str())?;
    }

    // Fork for ours — apply diffs
    let mut doc_ours = doc.fork();
    doc_ours.set_actor(derive_actor(ours));
    let ours_list = refs_list_id(&mut doc_ours)?;
    apply_list_diff(&mut doc_ours, &ours_list, &ancestor_lines, &ours_lines)?;

    // Fork for theirs — apply diffs
    let mut doc_theirs = doc.fork();
    doc_theirs.set_actor(derive_actor(theirs));
    let theirs_list = refs_list_id(&mut doc_theirs)?;
    apply_list_diff(
        &mut doc_theirs,
        &theirs_list,
        &ancestor_lines,
        &theirs_lines,
    )?;

    // Merge
    doc_ours.merge(&mut doc_theirs)?;

    // Extract merged list
    let merged_list = refs_list_id(&mut doc_ours)?;
    let len = doc_ours.length(&merged_list);
    let mut merged: Vec<String> = Vec::with_capacity(len);
    for i in 0..len {
        if let Some((value, _)) = doc_ours.get(&merged_list, i)? {
            merged.push(value.to_string().trim_matches('"').to_string());
        }
    }

    // Sort alphabetically and deduplicate
    merged.sort();
    merged.dedup();

    Ok(merged.join("\n"))
}

fn ref_lines(content: &str) -> Vec<String> {
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect()
}

fn refs_list_id(doc: &mut AutoCommit) -> Result<automerge::ObjId> {
    doc.get(&automerge::ROOT, "refs")?
        .map(|(_, id)| id)
        .ok_or_else(|| DoogatError::Parse("missing refs list".into()))
}

/// Apply list-level diff: delete removed entries (backwards for index stability), append new ones.
fn apply_list_diff(
    doc: &mut AutoCommit,
    list_id: &automerge::ObjId,
    ancestor: &[String],
    current: &[String],
) -> Result<()> {
    // Delete entries removed in current (backwards to keep indices stable)
    for i in (0..ancestor.len()).rev() {
        if !current.contains(&ancestor[i]) {
            doc.delete(list_id, i)?;
        }
    }
    // Append entries new in current
    for line in current {
        if !ancestor.contains(line) {
            let len = doc.length(list_id);
            doc.insert(list_id, len, line.as_str())?;
        }
    }
    Ok(())
}

/// Resolve conflicts using Last-Writer-Wins by HLC comparison.
/// Higher HLC wins. Tie-break: higher node string wins.
/// If no HLC available, falls back to "ours".
pub fn resolve_lww(conflicts: Vec<ConflictFile>) -> Result<Vec<ResolvedFile>> {
    let mut resolved = Vec::new();
    for conflict in conflicts {
        let content = pick_lww_winner(&conflict);
        resolved.push(ResolvedFile {
            path: conflict.path,
            content,
            fm_crdt_bytes: None,
        });
    }
    Ok(resolved)
}

/// Which side of a conflict wins an LWW decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LwwSide {
    Ours,
    Theirs,
}

/// The one missing/tie HLC rule, applied at every LWW decision site.
///
/// Higher HLC wins. When an HLC is absent on either side OR the two HLCs are
/// equal, the higher content key wins — a role-independent, content-deterministic
/// fallback that converges under an ours/theirs swap.
pub(crate) fn lww_pick(
    ours_hlc: Option<&Hlc>,
    theirs_hlc: Option<&Hlc>,
    ours_key: &str,
    theirs_key: &str,
) -> LwwSide {
    match (ours_hlc, theirs_hlc) {
        (Some(o), Some(t)) if o != t => {
            if t > o {
                LwwSide::Theirs
            } else {
                LwwSide::Ours
            }
        }
        _ => {
            if theirs_key > ours_key {
                LwwSide::Theirs
            } else {
                LwwSide::Ours
            }
        }
    }
}

/// Pick the winner based on HLC comparison.
fn pick_lww_winner(conflict: &ConflictFile) -> String {
    match lww_pick(
        conflict.ours_hlc.as_ref(),
        conflict.theirs_hlc.as_ref(),
        &conflict.ours,
        &conflict.theirs,
    ) {
        LwwSide::Ours => conflict.ours.clone(),
        LwwSide::Theirs => conflict.theirs.clone(),
    }
}

/// Resolve conflicts using append-log strategy.
/// Body log sections: parse entries, dedup, union, sort chronologically.
/// Non-log body sections: default text CRDT merge.
/// Frontmatter + references: same as default.
pub fn resolve_append_log(conflicts: Vec<ConflictFile>) -> Result<Vec<ResolvedFile>> {
    let mut resolved = Vec::new();

    for conflict in conflicts {
        let ancestor_content = conflict.ancestor.as_deref().unwrap_or("");

        let ancestor = parse_zones(ancestor_content)?;
        let ours = parse_zones(&conflict.ours)?;
        let theirs = parse_zones(&conflict.theirs)?;

        // Frontmatter: same as default (Automerge Map)
        let (merged_fm, fm_crdt_bytes) = merge_frontmatter(
            &ancestor.raw_frontmatter,
            &ours.raw_frontmatter,
            &theirs.raw_frontmatter,
        )?;

        // Body: section-aware merge
        let merged_body = merge_body_append_log(
            &ancestor.body,
            &ours.body,
            &theirs.body,
            &conflict.ours_hlc,
            &conflict.theirs_hlc,
        )?;

        // Reference: same as default (set merge)
        let merged_ref = merge_reference(
            &ancestor.reference_section,
            &ours.reference_section,
            &theirs.reference_section,
        )?;

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

        resolved.push(ResolvedFile {
            path: conflict.path,
            content: parser::serialize(&parsed),
            fm_crdt_bytes: Some(fm_crdt_bytes),
        });
    }

    Ok(resolved)
}

/// Split body into sections by `## ` headings.
fn split_body_sections(body: &str) -> Vec<(Option<String>, String)> {
    let mut sections: Vec<(Option<String>, String)> = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in body.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            // Push previous section
            if !current_lines.is_empty() || current_heading.is_some() {
                sections.push((current_heading.take(), current_lines.join("\n")));
                current_lines.clear();
            }
            current_heading = Some(heading.trim().to_string());
        } else {
            current_lines.push(line);
        }
    }
    // Push final section
    sections.push((current_heading, current_lines.join("\n")));

    sections
}

/// Check if a section body is a log section (entries matching `- [x] YYYY-MM-DD` or `- [ ] YYYY-MM-DD`).
fn is_log_section(body: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let log_pattern = RE
        .get_or_init(|| regex::Regex::new(r"^- \[[ xi]\] \d{4}-\d{2}-\d{2}").expect("valid regex"));
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .any(|l| log_pattern.is_match(l))
}

/// Parse log entries: each entry starts with `- [` and may have continuation lines.
fn parse_log_entries(body: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut current = String::new();

    for line in body.lines() {
        if line.starts_with("- [") {
            if !current.is_empty() {
                entries.push(current.trim_end().to_string());
            }
            current = line.to_string();
        } else if !current.is_empty() && !line.trim().is_empty() {
            current.push('\n');
            current.push_str(line);
        } else if current.is_empty() && !line.trim().is_empty() {
            // Non-entry content before first entry — skip
        } else if !current.is_empty() && line.trim().is_empty() {
            current.push('\n');
        }
    }
    if !current.is_empty() {
        entries.push(current.trim_end().to_string());
    }
    entries
}

/// Dedup key: (date, first line of entry text after the checkbox).
fn entry_dedup_key(entry: &str) -> String {
    let first_line = entry.lines().next().unwrap_or("");
    // Extract the date and text after `- [x] `
    if first_line.len() > 6 {
        first_line[6..].to_string() // skip "- [x] " prefix
    } else {
        first_line.to_string()
    }
}

/// Merge log sections: union entries from both sides, dedup, sort chronologically.
fn merge_log_section(ancestor: &str, ours: &str, theirs: &str) -> String {
    let ancestor_entries = parse_log_entries(ancestor);
    let ours_entries = parse_log_entries(ours);
    let theirs_entries = parse_log_entries(theirs);

    let mut seen = std::collections::BTreeSet::new();
    let mut merged = Vec::new();

    // Add all unique entries (by dedup key)
    for entry in ours_entries
        .iter()
        .chain(theirs_entries.iter())
        .chain(ancestor_entries.iter())
    {
        let key = entry_dedup_key(entry);
        if seen.insert(key) {
            merged.push(entry.clone());
        }
    }

    // Sort chronologically by the date in the entry
    merged.sort_by(|a, b| {
        let date_a = extract_entry_date(a).unwrap_or_default();
        let date_b = extract_entry_date(b).unwrap_or_default();
        date_a.cmp(&date_b)
    });

    merged.join("\n")
}

fn extract_entry_date(entry: &str) -> Option<String> {
    let first_line = entry.lines().next()?;
    // Pattern: `- [x] YYYY-MM-DD`
    if first_line.len() >= 16 {
        Some(first_line[6..16].to_string())
    } else {
        None
    }
}

/// Merge body using append-log strategy.
fn merge_body_append_log(
    ancestor: &str,
    ours: &str,
    theirs: &str,
    _ours_hlc: &Option<Hlc>,
    _theirs_hlc: &Option<Hlc>,
) -> Result<String> {
    let ancestor_sections = split_body_sections(ancestor);
    let ours_sections = split_body_sections(ours);
    let theirs_sections = split_body_sections(theirs);

    // Build section maps keyed by heading
    let ours_map: std::collections::BTreeMap<Option<String>, String> =
        ours_sections.into_iter().collect();
    let theirs_map: std::collections::BTreeMap<Option<String>, String> =
        theirs_sections.into_iter().collect();
    let ancestor_map: std::collections::BTreeMap<Option<String>, String> =
        ancestor_sections.into_iter().collect();

    // Collect all headings in order (ours first, then new from theirs)
    let mut headings: Vec<Option<String>> = Vec::new();
    for key in ours_map.keys() {
        headings.push(key.clone());
    }
    for key in theirs_map.keys() {
        if !headings.contains(key) {
            headings.push(key.clone());
        }
    }

    let mut output = String::new();
    for heading in headings {
        if let Some(h) = &heading {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&format!("## {h}\n"));
        }

        let a = ancestor_map.get(&heading).map(|s| s.as_str()).unwrap_or("");
        let o = ours_map.get(&heading).map(|s| s.as_str()).unwrap_or("");
        let t = theirs_map.get(&heading).map(|s| s.as_str()).unwrap_or("");

        let merged_section = if is_log_section(o) || is_log_section(t) {
            merge_log_section(a, o, t)
        } else {
            merge_body(a, o, t)?
        };

        output.push_str(&merged_section);
        if !merged_section.ends_with('\n') {
            output.push('\n');
        }
    }

    // Trim trailing whitespace
    Ok(output.trim_end().to_string())
}

/// Default CRDT conflict resolver using Automerge.
pub struct DefaultResolver;

impl crate::traits::ConflictResolver for DefaultResolver {
    fn resolve_conflicts(
        &self,
        conflicts: Vec<ConflictFile>,
        strategy: Option<&str>,
    ) -> Result<Vec<ResolvedFile>> {
        resolve_conflicts(conflicts, strategy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_different_fields() {
        let ancestor = "title: Original\ndate: 2026-01-01";
        let ours = "title: Changed by us\ndate: 2026-01-01";
        let theirs = "title: Original\ndate: 2026-01-01\nauthor: Them";

        let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
        assert!(merged.contains("Changed by us"));
        assert!(merged.contains("author"));
    }

    #[test]
    fn frontmatter_same_field_conflict() {
        let ancestor = "title: Original";
        let ours = "title: Ours";
        let theirs = "title: Theirs";

        // Automerge picks one deterministically
        let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
        assert!(merged.contains("title:"));
    }

    #[test]
    fn frontmatter_field_removal() {
        let ancestor = "title: Test\nauthor: Bob";
        let ours = "title: Test"; // removed author
        let theirs = "title: Test\nauthor: Bob";

        let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
        assert!(merged.contains("title:"));
        // author should be removed since ours removed it
        assert!(!merged.contains("author"));
    }

    #[test]
    fn frontmatter_preserves_quotes_for_yaml_special_chars() {
        let ancestor = "title: Original";
        let ours = "title: \"[[note|My Note]]\"";
        let theirs = "title: Original";

        let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
        assert!(merged.contains("title: \"[[note|My Note]]\""));
    }

    #[test]
    fn frontmatter_unquotes_flow_sequence_values() {
        let ancestor = "tags:\n  - shared";
        let ours = "tags:\n  - shared\n  - ours";
        let theirs = "tags:\n  - shared";

        let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
        let parsed = parser::parse_frontmatter(&merged, "test.md").unwrap();
        assert_eq!(parsed.tags, vec!["shared", "ours"]);
    }

    #[test]
    fn frontmatter_concurrent_tag_additions_merge() {
        let ancestor = "title: Test\ntags:\n  - shared";
        let ours = "title: Test\ntags:\n  - shared\n  - ours-tag";
        let theirs = "title: Test\ntags:\n  - shared\n  - theirs-tag";

        let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
        let parsed = parser::parse_frontmatter(&merged, "test.md").unwrap();
        // Both concurrent additions must survive
        assert!(parsed.tags.contains(&"shared".to_string()));
        assert!(parsed.tags.contains(&"ours-tag".to_string()));
        assert!(parsed.tags.contains(&"theirs-tag".to_string()));
        assert_eq!(parsed.tags.len(), 3);
    }

    #[test]
    fn frontmatter_tag_removal_honored() {
        let ancestor = "tags:\n  - keep\n  - remove";
        let ours = "tags:\n  - keep"; // removed 'remove'
        let theirs = "tags:\n  - keep\n  - remove";

        let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
        let parsed = parser::parse_frontmatter(&merged, "test.md").unwrap();
        assert_eq!(parsed.tags, vec!["keep"]);
    }

    #[test]
    fn frontmatter_returns_loadable_automerge_bytes() {
        let ancestor = "title: Original\ndate: 2026-01-01";
        let ours = "title: Changed\ndate: 2026-01-01";
        let theirs = "title: Original\ndate: 2026-01-01\nauthor: Them";

        let (yaml, doc_bytes) = merge_frontmatter(ancestor, ours, theirs).unwrap();
        assert!(!yaml.is_empty());
        assert!(!doc_bytes.is_empty());

        // Bytes must be a valid automerge document
        let loaded = automerge::AutoCommit::load(&doc_bytes);
        assert!(loaded.is_ok(), "automerge doc bytes should be loadable");
    }

    #[test]
    fn body_non_overlapping_edits() {
        let ancestor = "Line 1\nLine 2\nLine 3\n";
        let ours = "Line 1 edited\nLine 2\nLine 3\n";
        let theirs = "Line 1\nLine 2\nLine 3 edited\n";

        let merged = merge_body(ancestor, ours, theirs).unwrap();
        assert!(merged.contains("Line 1 edited"));
        assert!(merged.contains("Line 3 edited"));
    }

    #[test]
    fn body_append_from_both() {
        let ancestor = "Line 1\n";
        let ours = "Line 1\nOurs appended\n";
        let theirs = "Line 1\nTheirs appended\n";

        let merged = merge_body(ancestor, ours, theirs).unwrap();
        assert!(merged.contains("Line 1"));
        // Both appends should be present in some order
        assert!(merged.contains("appended"));
    }

    #[test]
    fn reference_union_additions() {
        let ancestor = "- source:: Wikipedia";
        let ours = "- source:: Wikipedia\n- author:: Bob";
        let theirs = "- source:: Wikipedia\n- year:: 2026";

        let merged = merge_reference(ancestor, ours, theirs).unwrap();
        assert!(merged.contains("- source:: Wikipedia"));
        assert!(merged.contains("- author:: Bob"));
        assert!(merged.contains("- year:: 2026"));
    }

    #[test]
    fn reference_removal() {
        let ancestor = "- source:: Wikipedia\n- author:: Bob";
        let ours = "- source:: Wikipedia"; // removed author
        let theirs = "- source:: Wikipedia\n- author:: Bob";

        let merged = merge_reference(ancestor, ours, theirs).unwrap();
        assert!(merged.contains("- source:: Wikipedia"));
        assert!(!merged.contains("author"));
    }

    #[test]
    fn reference_same_key_conflict() {
        let ancestor = "- source:: Wikipedia";
        let ours = "- source:: Ours Source";
        let theirs = "- source:: Theirs Source";

        let merged = merge_reference(ancestor, ours, theirs).unwrap();
        assert!(merged.contains("- source::"));
    }

    #[test]
    fn reference_concurrent_additions_both_present() {
        let ancestor = "- source:: Wikipedia";
        let ours = "- source:: Wikipedia\n- author:: Alice";
        let theirs = "- source:: Wikipedia\n- year:: 2026";

        let merged = merge_reference(ancestor, ours, theirs).unwrap();
        assert!(merged.contains("- source:: Wikipedia"));
        assert!(merged.contains("- author:: Alice"));
        assert!(merged.contains("- year:: 2026"));
    }

    #[test]
    fn full_pipeline_resolve() {
        let ancestor = "---\ntitle: Original\n---\nOriginal body.\n---\n- source:: Wikipedia";
        let ours = "---\ntitle: Changed\n---\nOurs body.\n---\n- source:: Wikipedia";
        let theirs =
            "---\ntitle: Original\n---\nOriginal body.\n---\n- source:: Wikipedia\n- author:: Bob";

        let conflicts = vec![ConflictFile {
            path: "ddb/20260226120000.md".into(),
            ancestor: Some(ancestor.into()),
            ours: ours.into(),
            theirs: theirs.into(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }];

        let resolved = resolve_conflicts(conflicts, None).unwrap();
        assert_eq!(resolved.len(), 1);

        // Should be valid parseable markdown
        let parsed = parser::parse(&resolved[0].content, &resolved[0].path).unwrap();
        assert!(parsed.meta.title.is_some());
    }

    #[test]
    fn full_pipeline_no_ancestor() {
        let ours = "---\ntitle: Ours\n---\nOurs body.";
        let theirs = "---\ntitle: Theirs\n---\nTheirs body.";

        let conflicts = vec![ConflictFile {
            path: "ddb/new.md".into(),
            ancestor: None,
            ours: ours.into(),
            theirs: theirs.into(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }];

        let resolved = resolve_conflicts(conflicts, None).unwrap();
        assert_eq!(resolved.len(), 1);
        let parsed = parser::parse(&resolved[0].content, &resolved[0].path).unwrap();
        assert!(parsed.meta.title.is_some());
    }

    #[test]
    fn body_intra_line_char_level_merge() {
        let ancestor = "hello world";
        let ours = "hello brave world";
        let theirs = "hello world!";

        let merged = merge_body(ancestor, ours, theirs).unwrap();
        assert!(merged.contains("brave"));
        assert!(merged.contains("!"));
    }

    // --- lww_pick: the single tie-break rule shared by all three sites ---

    #[test]
    fn lww_pick_both_hlc_differ_higher_hlc_wins_theirs() {
        let ours_hlc = Hlc {
            wall_ms: 100,
            counter: 0,
            node: "aaaaaaaa".into(),
        };
        let theirs_hlc = Hlc {
            wall_ms: 200,
            counter: 0,
            node: "bbbbbbbb".into(),
        };
        // Content keys deliberately favour ours; the higher HLC must still decide.
        let pick = lww_pick(Some(&ours_hlc), Some(&theirs_hlc), "zzz_ours", "aaa_theirs");
        assert_eq!(pick, LwwSide::Theirs);
    }

    #[test]
    fn lww_pick_both_hlc_differ_higher_hlc_wins_ours() {
        let ours_hlc = Hlc {
            wall_ms: 300,
            counter: 0,
            node: "aaaaaaaa".into(),
        };
        let theirs_hlc = Hlc {
            wall_ms: 200,
            counter: 0,
            node: "bbbbbbbb".into(),
        };
        // Content keys deliberately favour theirs; the higher HLC must still decide.
        let pick = lww_pick(Some(&ours_hlc), Some(&theirs_hlc), "aaa_ours", "zzz_theirs");
        assert_eq!(pick, LwwSide::Ours);
    }

    #[test]
    fn lww_pick_equal_hlc_picks_higher_content_key() {
        let hlc = Hlc {
            wall_ms: 100,
            counter: 0,
            node: "samenode".into(),
        };
        // Exactly-equal HLCs → content key decides. "theirs_key" > "ours_key" by str `>`.
        assert_eq!(
            lww_pick(Some(&hlc), Some(&hlc), "ours_key", "theirs_key"),
            LwwSide::Theirs
        );
        // And the other direction: higher key on ours.
        assert_eq!(
            lww_pick(Some(&hlc), Some(&hlc), "zzz_key", "aaa_key"),
            LwwSide::Ours
        );
    }

    #[test]
    fn lww_pick_equal_wall_ms_and_node_orders_by_counter_not_content() {
        // Same wall_ms AND same node: the HLC `counter` component must break the
        // tie, NOT the content key. The higher-`counter` side is given the
        // LEXICALLY-LOWER content key, so "order by counter" and "fall back to
        // content key" DISAGREE. Guards against an impl that compares
        // wall_ms + node but drops counter.
        let ours_hlc = Hlc {
            wall_ms: 100,
            counter: 3, // lower counter
            node: "samenode".into(),
        };
        let theirs_hlc = Hlc {
            wall_ms: 100,
            counter: 5, // higher counter
            node: "samenode".into(),
        };
        // Content key favours ours ("zzz" > "aaa"); the higher counter must win.
        assert_eq!(
            lww_pick(Some(&ours_hlc), Some(&theirs_hlc), "zzz", "aaa"),
            LwwSide::Theirs
        );
    }

    #[test]
    fn lww_pick_cross_magnitude_wall_ms_compares_numerically_not_lexically() {
        // Cross-magnitude wall_ms: the numerically-later side is LEXICALLY lower
        // in the HLC Display form ("900-..." vs "1000-...", '9' > '1'). The rule
        // must compare wall_ms numerically (Hlc::Ord), not as a string. Content
        // key favours ours ("z" > "a") so a fallback would also pick the wrong side.
        let ours = Hlc {
            wall_ms: 900,
            counter: 0,
            node: "aaaaaaaa".into(),
        };
        let theirs = Hlc {
            wall_ms: 1000,
            counter: 0,
            node: "bbbbbbbb".into(),
        };
        assert_eq!(lww_pick(Some(&ours), Some(&theirs), "z", "a"), LwwSide::Theirs);
    }

    #[test]
    fn lww_pick_missing_hlc_picks_higher_content_key() {
        let hlc = Hlc {
            wall_ms: 100,
            counter: 0,
            node: "somenode".into(),
        };
        // Ours None → content key decides.
        assert_eq!(lww_pick(None, Some(&hlc), "aaa", "bbb"), LwwSide::Theirs);
        // Theirs None → content key decides.
        assert_eq!(lww_pick(Some(&hlc), None, "bbb", "aaa"), LwwSide::Ours);
        // Both None → content key decides.
        assert_eq!(lww_pick(None, None, "aaa", "bbb"), LwwSide::Theirs);
        assert_eq!(lww_pick(None, None, "bbb", "aaa"), LwwSide::Ours);
    }

    #[test]
    fn lww_pick_role_swap_converges_on_same_content_key() {
        // Missing/equal case: the winning CONTENT KEY is invariant under role swap.
        let ours_key = "alpha";
        let theirs_key = "omega"; // omega > alpha by str `>`

        let orig = lww_pick(None, None, ours_key, theirs_key);
        let winning_key_orig = match orig {
            LwwSide::Ours => ours_key,
            LwwSide::Theirs => theirs_key,
        };

        // Swap the (hlc, key) argument pairs; the winning key must not change.
        let swapped = lww_pick(None, None, theirs_key, ours_key);
        let winning_key_swapped = match swapped {
            LwwSide::Ours => theirs_key,
            LwwSide::Theirs => ours_key,
        };

        assert_eq!(
            winning_key_orig, winning_key_swapped,
            "winning content key must be invariant under role swap"
        );
        assert_eq!(winning_key_orig, theirs_key, "the higher content key wins");
    }

    #[test]
    fn lww_picks_later_hlc() {
        let ours_hlc = Hlc {
            wall_ms: 100,
            counter: 0,
            node: "aaaaaaaa".into(),
        };
        let theirs_hlc = Hlc {
            wall_ms: 200,
            counter: 0,
            node: "bbbbbbbb".into(),
        };
        let conflicts = vec![ConflictFile {
            path: "ddb/test.md".into(),
            ancestor: None,
            ours: "---\ntitle: Ours\n---\nOurs body.".into(),
            theirs: "---\ntitle: Theirs\n---\nTheirs body.".into(),
            ours_hlc: Some(ours_hlc),
            theirs_hlc: Some(theirs_hlc),
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }];

        let resolved = resolve_lww(conflicts).unwrap();
        assert!(resolved[0].content.contains("Theirs"));
    }

    #[test]
    fn lww_equal_wall_ms_orders_by_hlc_node_not_content() {
        // Same wall_ms AND same counter: the HLC `node` component must break the
        // tie, NOT the content key. To tell the two rules apart, the higher-`node`
        // side is given the LEXICALLY-LOWER content, so "order by HLC node" and
        // "fall back to content key" DISAGREE on the winner.
        let ours_hlc = Hlc {
            wall_ms: 100,
            counter: 0,
            node: "aaaaaaaa".into(), // lower node
        };
        let theirs_hlc = Hlc {
            wall_ms: 100,
            counter: 0,
            node: "zzzzzzzz".into(), // higher node
        };
        let conflicts = vec![ConflictFile {
            path: "ddb/test.md".into(),
            ancestor: None,
            ours: "zzz body".into(),   // higher content key, but LOWER HLC node
            theirs: "aaa body".into(), // lower content key, but HIGHER HLC node
            ours_hlc: Some(ours_hlc),
            theirs_hlc: Some(theirs_hlc),
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }];

        let resolved = resolve_lww(conflicts).unwrap();
        // Higher HLC node (theirs) wins even though its content key is lower.
        // A wall_ms-only impl would treat this as a tie, fall back to the content
        // key, and return "zzz body" (ours) — which is WRONG.
        assert_eq!(resolved[0].content, "aaa body");
    }

    #[test]
    fn lww_missing_hlc_picks_higher_content_key() {
        // resolve_lww's content key is the side's full text. With both HLCs
        // absent, the higher text (str `>`) wins: "theirs content" > "ours content".
        let conflicts = vec![ConflictFile {
            path: "ddb/test.md".into(),
            ancestor: None,
            ours: "ours content".into(),
            theirs: "theirs content".into(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }];

        let resolved = resolve_lww(conflicts).unwrap();
        assert_eq!(resolved[0].content, "theirs content");

        // Role-swap convergence: swap ours/theirs → same winning content.
        let swapped = vec![ConflictFile {
            path: "ddb/test.md".into(),
            ancestor: None,
            ours: "theirs content".into(),
            theirs: "ours content".into(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }];

        let resolved_swapped = resolve_lww(swapped).unwrap();
        assert_eq!(resolved_swapped[0].content, "theirs content");
    }

    #[test]
    fn append_log_different_entries_both_survive() {
        let ancestor =
            "---\ntitle: Project\ntype: project\n---\n## Log\n- [x] 2026-01-01 Setup project";
        let ours = "---\ntitle: Project\ntype: project\n---\n## Log\n- [x] 2026-01-01 Setup project\n- [x] 2026-01-05 Laptop entry";
        let theirs = "---\ntitle: Project\ntype: project\n---\n## Log\n- [x] 2026-01-01 Setup project\n- [x] 2026-01-03 Desktop entry";

        let conflicts = vec![ConflictFile {
            path: "ddb/project/test.md".into(),
            ancestor: Some(ancestor.into()),
            ours: ours.into(),
            theirs: theirs.into(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }];

        let resolved = resolve_append_log(conflicts).unwrap();
        let content = &resolved[0].content;
        assert!(content.contains("Laptop entry"), "missing laptop entry");
        assert!(content.contains("Desktop entry"), "missing desktop entry");
        // Sorted chronologically: desktop (01-03) before laptop (01-05)
        let desktop_pos = content.find("Desktop entry").unwrap();
        let laptop_pos = content.find("Laptop entry").unwrap();
        assert!(
            desktop_pos < laptop_pos,
            "entries not sorted chronologically"
        );
    }

    #[test]
    fn append_log_dedup_same_entry() {
        let ancestor = "---\ntitle: P\n---\n## Log\n- [x] 2026-01-01 Init";
        let ours = "---\ntitle: P\n---\n## Log\n- [x] 2026-01-01 Init\n- [x] 2026-02-01 Same";
        let theirs = "---\ntitle: P\n---\n## Log\n- [x] 2026-01-01 Init\n- [x] 2026-02-01 Same";

        let conflicts = vec![ConflictFile {
            path: "ddb/test.md".into(),
            ancestor: Some(ancestor.into()),
            ours: ours.into(),
            theirs: theirs.into(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }];

        let resolved = resolve_append_log(conflicts).unwrap();
        let content = &resolved[0].content;
        // "Same" should appear only once
        assert_eq!(content.matches("Same").count(), 1);
    }

    #[test]
    fn append_log_non_log_sections_use_text_crdt() {
        let ancestor = "---\ntitle: P\n---\n## Description\nOriginal description.\n## Log\n- [x] 2026-01-01 Init";
        let ours = "---\ntitle: P\n---\n## Description\nUpdated description from laptop.\n## Log\n- [x] 2026-01-01 Init";
        let theirs = "---\ntitle: P\n---\n## Description\nOriginal description.\n## Log\n- [x] 2026-01-01 Init\n- [x] 2026-02-01 New entry";

        let conflicts = vec![ConflictFile {
            path: "ddb/test.md".into(),
            ancestor: Some(ancestor.into()),
            ours: ours.into(),
            theirs: theirs.into(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }];

        let resolved = resolve_append_log(conflicts).unwrap();
        let content = &resolved[0].content;
        assert!(content.contains("Updated description from laptop"));
        assert!(content.contains("New entry"));
    }

    #[test]
    fn append_log_empty_log_section() {
        let ancestor = "---\ntitle: P\n---\n## Log\n";
        let ours = "---\ntitle: P\n---\n## Log\n- [x] 2026-01-01 First entry";
        let theirs = "---\ntitle: P\n---\n## Log\n";

        let conflicts = vec![ConflictFile {
            path: "ddb/test.md".into(),
            ancestor: Some(ancestor.into()),
            ours: ours.into(),
            theirs: theirs.into(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }];

        let resolved = resolve_append_log(conflicts).unwrap();
        assert!(resolved[0].content.contains("First entry"));
    }

    #[test]
    fn resolve_with_non_default_strategy_still_works() {
        let ancestor = "---\ntitle: Base\n---\nBody.";
        let ours = "---\ntitle: Ours\n---\nBody.";
        let theirs = "---\ntitle: Base\n---\nBody changed.";

        let conflicts = vec![ConflictFile {
            path: "ddb/20260226120000.md".into(),
            ancestor: Some(ancestor.into()),
            ours: ours.into(),
            theirs: theirs.into(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }];

        // Should succeed (using default strategy) and log a warning
        let resolved = resolve_conflicts(conflicts, Some("preset:append-log")).unwrap();
        assert_eq!(resolved.len(), 1);
        let parsed = parser::parse(&resolved[0].content, &resolved[0].path).unwrap();
        assert!(parsed.meta.title.is_some());
    }

    #[test]
    fn resolve_errors_on_unparseable_content() {
        // Content without frontmatter markers → parse_zones fails
        // This proves the LWW fallback path in sync_manager is reachable
        let conflicts = vec![ConflictFile {
            path: "ddb/broken.md".into(),
            ancestor: Some("no frontmatter here".into()),
            ours: "also no frontmatter".into(),
            theirs: "still no frontmatter".into(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }];

        let result = resolve_conflicts(conflicts, None);
        assert!(result.is_err());
    }

    #[test]
    fn derive_actor_same_content_same_actor() {
        let content = "title: A\nx: 1";
        let a = derive_actor(content);
        let b = derive_actor(content);
        assert_eq!(a, b);
    }

    #[test]
    fn derive_actor_different_content_different_actor() {
        let a = derive_actor("title: A\nx: 1");
        let b = derive_actor("title: B\nx: 2");
        assert_ne!(a, b);
    }

    #[test]
    fn derive_actor_produces_valid_actor_id() {
        let actor = derive_actor("title: A\nx: 1");
        assert_eq!(actor.to_bytes().len(), 16);
    }

    #[test]
    fn merge_frontmatter_is_byte_identical_on_re_resolution() {
        let ancestor = "title: Base\nstatus: open";
        let ours = "title: Base\nstatus: ours";
        let theirs = "title: Base\nstatus: theirs";

        let (yaml_a, bytes_a) = merge_frontmatter(ancestor, ours, theirs).unwrap();
        let (yaml_b, bytes_b) = merge_frontmatter(ancestor, ours, theirs).unwrap();

        assert_eq!(yaml_a, yaml_b);
        assert_eq!(bytes_a, bytes_b);
    }
}
