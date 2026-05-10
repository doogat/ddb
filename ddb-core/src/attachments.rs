//! File attachment operations for doogats.
//!
//! Attachments are stored in `reference/{doogat_id}/` and tracked
//! in the doogat's frontmatter `attachments` array.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::indexer::Index;
use crate::parser;
use crate::traits::GitBackend;
use crate::types::{AttachmentInfo, DoogatId, Value};

/// Validate that a filename is safe for use in attachment paths.
/// Rejects path traversal characters: `..`, `/`, `\`.
fn validate_attachment_filename(filename: &str) -> Result<()> {
    if filename.is_empty()
        || filename.contains("..")
        || filename.contains('/')
        || filename.contains('\\')
    {
        return Err(crate::error::DoogatError::Validation(format!(
            "invalid attachment filename: '{filename}'"
        )));
    }
    Ok(())
}

/// Validate that a doogat ID is a 14-digit numeric string.
fn validate_doogat_id_format(id: &DoogatId) -> Result<()> {
    if id.0.len() != 14 || !id.0.chars().all(|c| c.is_ascii_digit()) {
        return Err(crate::error::DoogatError::Validation(format!(
            "invalid doogat ID format: '{}' (expected 14 digits)",
            id.0
        )));
    }
    Ok(())
}

/// Build the relative path for an attachment file.
fn attachment_path(doogat_id: &DoogatId, filename: &str) -> String {
    format!("reference/{}/{}", doogat_id.0, filename)
}

/// Extract the `attachments` list from frontmatter extras.
fn parse_attachments(extra: &BTreeMap<String, Value>, doogat_id: &DoogatId) -> Vec<AttachmentInfo> {
    let Some(Value::List(items)) = extra.get("attachments") else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let Value::Map(map) = item else { return None };
            let name = map.get("name")?.as_str()?.to_owned();
            let mime = map
                .get("mime")
                .and_then(|v| v.as_str())
                .unwrap_or("application/octet-stream")
                .to_owned();
            let size = map.get("size").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64;
            Some(AttachmentInfo {
                path: attachment_path(doogat_id, &name),
                name,
                mime,
                size,
            })
        })
        .collect()
}

/// Build a `Value::Map` entry for one attachment in frontmatter.
fn attachment_to_value(info: &AttachmentInfo) -> Value {
    let mut map = BTreeMap::new();
    map.insert("name".into(), Value::String(info.name.clone()));
    map.insert("mime".into(), Value::String(info.mime.clone()));
    map.insert("size".into(), Value::Number(info.size as f64));
    Value::Map(map)
}

/// List attachments for a doogat by reading its frontmatter.
pub fn list_attachments(repo: &impl GitBackend, id: &DoogatId) -> Result<Vec<AttachmentInfo>> {
    validate_doogat_id_format(id)?;
    let doogat_path = format!("ddb/{}.md", id.0);
    let content = repo.read_file(&doogat_path)?;
    let parsed = parser::parse(&content, &doogat_path)?;
    Ok(parse_attachments(&parsed.meta.extra, id))
}

/// Attach a file to a doogat: store in `reference/{id}/`, update frontmatter, commit.
pub fn attach_file(
    repo: &impl GitBackend,
    index: &Index,
    id: &DoogatId,
    filename: &str,
    bytes: &[u8],
    mime: &str,
) -> Result<AttachmentInfo> {
    validate_doogat_id_format(id)?;
    validate_attachment_filename(filename)?;
    let doogat_path = format!("ddb/{}.md", id.0);
    let content = repo.read_file(&doogat_path)?;
    let mut parsed = parser::parse(&content, &doogat_path)?;

    let rel_path = attachment_path(id, filename);
    let info = AttachmentInfo {
        name: filename.to_owned(),
        mime: mime.to_owned(),
        size: bytes.len() as u64,
        path: rel_path.clone(),
    };

    // Update frontmatter attachments array
    let entry = attachment_to_value(&info);
    match parsed.meta.extra.get_mut("attachments") {
        Some(Value::List(list)) => {
            // Replace existing entry with same name, or append
            if let Some(pos) = list.iter().position(|v| {
                matches!(v, Value::Map(m) if m.get("name").and_then(|n| n.as_str()) == Some(filename))
            }) {
                list[pos] = entry;
            } else {
                list.push(entry);
            }
        }
        _ => {
            parsed
                .meta
                .extra
                .insert("attachments".into(), Value::List(vec![entry]));
        }
    }

    // Atomic commit: binary file + frontmatter update
    let serialized = parser::serialize(&parsed);
    repo.commit_binary_and_text(
        &rel_path,
        bytes,
        &[(&doogat_path, &serialized)],
        &format!("attach {} to {}", filename, id.0),
    )?;

    // Re-index
    index.index_doogat(&parsed)?;

    Ok(info)
}

/// Detach a file from a doogat: remove from git, update frontmatter, commit.
pub fn detach_file(
    repo: &impl GitBackend,
    index: &Index,
    id: &DoogatId,
    filename: &str,
) -> Result<()> {
    validate_doogat_id_format(id)?;
    validate_attachment_filename(filename)?;
    let doogat_path = format!("ddb/{}.md", id.0);
    let content = repo.read_file(&doogat_path)?;
    let mut parsed = parser::parse(&content, &doogat_path)?;

    // Remove from frontmatter
    let removed = match parsed.meta.extra.get_mut("attachments") {
        Some(Value::List(list)) => {
            let before = list.len();
            list.retain(|v| {
                !matches!(v, Value::Map(m) if m.get("name").and_then(|n| n.as_str()) == Some(filename))
            });
            let after = list.len();
            if list.is_empty() {
                parsed.meta.extra.remove("attachments");
            }
            before != after
        }
        _ => false,
    };

    if !removed {
        return Err(crate::error::DoogatError::NotFound(format!(
            "attachment '{}' not found on doogat {}",
            filename, id.0
        )));
    }

    // Atomic commit: delete file + frontmatter update
    let rel_path = attachment_path(id, filename);
    let serialized = parser::serialize(&parsed);
    repo.commit_batch(
        &[(&doogat_path, &serialized)],
        &[&rel_path],
        &format!("detach {} from {}", filename, id.0),
    )?;

    // Re-index
    index.index_doogat(&parsed)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_ops::GitRepo;
    use crate::indexer::Index;
    use tempfile::TempDir;

    fn setup() -> (TempDir, GitRepo, Index) {
        let dir = TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        let index = Index::open(std::path::Path::new(":memory:")).unwrap();
        (dir, repo, index)
    }

    fn create_doogat(repo: &GitRepo, index: &Index, id: &str) {
        let content = format!("---\nid: {id}\ntitle: Test\n---\nBody text\n");
        let path = format!("ddb/{id}.md");
        repo.commit_file(&path, &content, "add doogat").unwrap();
        let parsed = parser::parse(&content, &path).unwrap();
        index.index_doogat(&parsed).unwrap();
    }

    #[test]
    fn attach_and_list() {
        let (_dir, repo, index) = setup();
        let id = DoogatId("20260301130000".into());
        create_doogat(&repo, &index, &id.0);

        let info = attach_file(
            &repo,
            &index,
            &id,
            "photo.jpg",
            b"\xFF\xD8\xFF",
            "image/jpeg",
        )
        .unwrap();
        assert_eq!(info.name, "photo.jpg");
        assert_eq!(info.mime, "image/jpeg");
        assert_eq!(info.size, 3);
        assert_eq!(info.path, "reference/20260301130000/photo.jpg");

        let list = list_attachments(&repo, &id).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "photo.jpg");
    }

    #[test]
    fn attach_multiple_and_detach() {
        let (_dir, repo, index) = setup();
        let id = DoogatId("20260301130000".into());
        create_doogat(&repo, &index, &id.0);

        attach_file(&repo, &index, &id, "a.txt", b"aaa", "text/plain").unwrap();
        attach_file(&repo, &index, &id, "b.pdf", b"pdf", "application/pdf").unwrap();
        assert_eq!(list_attachments(&repo, &id).unwrap().len(), 2);

        detach_file(&repo, &index, &id, "a.txt").unwrap();
        let list = list_attachments(&repo, &id).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "b.pdf");
    }

    #[test]
    fn detach_nonexistent_errors() {
        let (_dir, repo, index) = setup();
        let id = DoogatId("20260301130000".into());
        create_doogat(&repo, &index, &id.0);

        let err = detach_file(&repo, &index, &id, "nope.txt");
        assert!(err.is_err());
    }

    #[test]
    fn attach_overwrites_same_name() {
        let (_dir, repo, index) = setup();
        let id = DoogatId("20260301130000".into());
        create_doogat(&repo, &index, &id.0);

        attach_file(&repo, &index, &id, "file.txt", b"v1", "text/plain").unwrap();
        attach_file(&repo, &index, &id, "file.txt", b"v2data", "text/plain").unwrap();

        let list = list_attachments(&repo, &id).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].size, 6); // "v2data".len()
    }

    #[test]
    fn reject_path_traversal_filename() {
        let (_dir, repo, index) = setup();
        let id = DoogatId("20260301130000".into());
        create_doogat(&repo, &index, &id.0);

        for bad in &["../etc/passwd", "foo/bar.txt", "foo\\bar.txt", "..", ""] {
            let err = attach_file(&repo, &index, &id, bad, b"x", "text/plain");
            assert!(err.is_err(), "should reject filename: {bad:?}");

            // detach should also reject
            let err = detach_file(&repo, &index, &id, bad);
            assert!(err.is_err(), "detach should reject filename: {bad:?}");
        }
    }

    #[test]
    fn reject_invalid_doogat_id_format() {
        let (_dir, repo, _index) = setup();

        for bad_id in &[
            "short",
            "../../etc/pass",
            "12345678901234/",
            "abcdefghijklmn",
        ] {
            let id = DoogatId(bad_id.to_string());
            let err = list_attachments(&repo, &id);
            assert!(err.is_err(), "should reject doogat ID: {bad_id:?}");
        }
    }

    #[test]
    fn list_empty_attachments() {
        let (_dir, repo, _index) = setup();
        let id = DoogatId("20260301130000".into());
        create_doogat(&repo, &_index, &id.0);

        let list = list_attachments(&repo, &id).unwrap();
        assert!(list.is_empty());
    }
}
