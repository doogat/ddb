use automerge::{transaction::Transactable, AutoCommit, ObjType, ReadDoc};

use super::frontmatter::derive_actor;
use crate::error::{DoogatError, Result};

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
