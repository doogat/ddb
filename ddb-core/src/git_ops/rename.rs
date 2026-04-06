use std::path::Path;

use crate::error::Result;
use crate::types::RenameReport;

/// Rename a doogat and rewrite all backlinks pointing to it.
///
/// 1. Moves the file via `rename_file()` (first commit).
/// 2. Finds all doogats linking to the old path or bare ID.
/// 3. Rewrites wikilinks in each backlinking file.
/// 4. Commits all rewritten files (second commit).
/// 5. Detects remaining broken references via `broken_backlinks()` (FR-10a).
pub fn rename_doogat(
    repo: &impl crate::traits::GitBackend,
    index: &crate::indexer::Index,
    old_path: &str,
    new_path: &str,
) -> Result<RenameReport> {
    // Step 1: move the file
    repo.rename_file(
        old_path,
        new_path,
        &format!("rename: {old_path} → {new_path}"),
    )?;

    // Extract the bare ID from the old path (filename without .md)
    let old_id = Path::new(old_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // Step 2: find backlinks for both old path and bare ID
    let mut backlinks = index.backlinking_doogat_paths(old_path)?;
    if !old_id.is_empty() && old_id != old_path {
        let by_id = index.backlinking_doogat_paths(old_id)?;
        for entry in by_id {
            if !backlinks.iter().any(|(id, _)| *id == entry.0) {
                backlinks.push(entry);
            }
        }
    }

    let mut report = RenameReport::default();
    let old_target_for_path = old_path.trim_end_matches(".md");

    // Step 3-4: rewrite backlinks and commit
    if !backlinks.is_empty() {
        let new_target_for_path = new_path.trim_end_matches(".md");

        let mut writes: Vec<(String, String)> = Vec::new();
        for (_source_id, source_path) in &backlinks {
            let content = repo.read_file(source_path)?;
            let mut rewritten = content.clone();

            // Rewrite path-qualified links (without .md, as wikilinks typically omit it)
            rewritten =
                crate::parser::rewrite_links(&rewritten, old_target_for_path, new_target_for_path);

            // Rewrite bare ID links
            if !old_id.is_empty() {
                rewritten = crate::parser::rewrite_links(&rewritten, old_id, new_target_for_path);
            }

            if rewritten != content {
                writes.push((source_path.clone(), rewritten));
                report.updated.push(source_path.clone());
            }
        }

        if !writes.is_empty() {
            let write_refs: Vec<(&str, &str)> = writes
                .iter()
                .map(|(p, c)| (p.as_str(), c.as_str()))
                .collect();
            repo.commit_files(
                &write_refs,
                &format!("refactor: rewrite links after rename {old_path}"),
            )?;
        }
    }

    // Step 5: detect remaining broken references to old target (runs unconditionally)
    index.rebuild_if_stale(repo)?;
    let old_targets = [old_path, old_target_for_path, old_id];
    report.unresolvable = index
        .broken_backlinks()?
        .into_iter()
        .filter(|(_src, target)| old_targets.contains(&target.as_str()))
        .filter_map(|(src_id, _)| index.resolve_path(&src_id).ok())
        .collect();

    Ok(report)
}
