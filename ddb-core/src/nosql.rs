//! redb-based NoSQL key-value index for fast lookups and prefix scans.
//!
//! Complements SQLite (which keeps FTS5/SQL). redb adds O(1) key lookups
//! and efficient prefix scans by type, tag, and backlinks.

use std::path::Path;

use redb::{Database, ReadableTable, TableDefinition};

use crate::error::{DoogatError, Result};
use crate::types::ParsedDoogat;

/// Shorthand for mapping any redb/bincode error to DoogatError::Redb.
fn redb_err(e: impl std::fmt::Display) -> DoogatError {
    DoogatError::Redb(e.to_string())
}

// ── Table definitions ────────────────────────────────────────────

/// Primary store: doogat_id → bincode-serialized ParsedDoogat
const DOOGATS: TableDefinition<&str, &[u8]> = TableDefinition::new("doogats");
/// Secondary index: "{type}/{id}" → empty value
const BY_TYPE: TableDefinition<&str, &[u8]> = TableDefinition::new("by_type");
/// Secondary index: "{tag}/{id}" → empty value
const BY_TAG: TableDefinition<&str, &[u8]> = TableDefinition::new("by_tag");
/// Link index: "{target_id}/{source_id}" → empty value
const LINKS: TableDefinition<&str, &[u8]> = TableDefinition::new("links");

// ── Public API ───────────────────────────────────────────────────

pub struct RedbIndex {
    db: Database,
}

impl RedbIndex {
    /// Open or create a redb database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path).map_err(redb_err)?;
        Ok(Self { db })
    }

    /// Index a single doogat (upsert).
    pub fn index_doogat(&self, doogat: &ParsedDoogat) -> Result<()> {
        let id = doogat
            .meta
            .id
            .as_ref()
            .map(|z| z.0.as_str())
            .ok_or_else(|| DoogatError::Validation("doogat has no id".into()))?;

        let encoded = serde_json::to_vec(doogat).map_err(redb_err)?;

        let txn = self.db.begin_write().map_err(redb_err)?;

        // Remove old secondary entries before re-inserting
        self.remove_secondary_entries(&txn, id)?;

        {
            let mut t = txn.open_table(DOOGATS).map_err(redb_err)?;
            t.insert(id, encoded.as_slice()).map_err(redb_err)?;
        }

        // Type index
        if let Some(ref zt) = doogat.meta.doogat_type {
            let key = format!("{zt}/{id}");
            let mut t = txn.open_table(BY_TYPE).map_err(redb_err)?;
            t.insert(key.as_str(), [].as_slice()).map_err(redb_err)?;
        }

        // Tag index (frontmatter + body hashtags)
        {
            let mut t = txn.open_table(BY_TAG).map_err(redb_err)?;
            for tag in doogat.meta.tags.iter().chain(doogat.body_tags.iter()) {
                let key = format!("{tag}/{id}");
                t.insert(key.as_str(), [].as_slice()).map_err(redb_err)?;
            }
        }

        // Link index
        {
            let mut t = txn.open_table(LINKS).map_err(redb_err)?;
            for link in &doogat.links {
                let key = format!("{}/{id}", link.target);
                t.insert(key.as_str(), [].as_slice()).map_err(redb_err)?;
            }
        }

        txn.commit().map_err(redb_err)?;
        Ok(())
    }

    /// Remove a doogat from all tables.
    pub fn remove_doogat(&self, id: &str) -> Result<()> {
        let txn = self.db.begin_write().map_err(redb_err)?;

        self.remove_secondary_entries(&txn, id)?;

        {
            let mut t = txn.open_table(DOOGATS).map_err(redb_err)?;
            let _: Option<redb::AccessGuard<&[u8]>> = t.remove(id).map_err(redb_err)?;
        }

        txn.commit().map_err(redb_err)?;
        Ok(())
    }

    /// Get a single doogat by ID.
    pub fn get(&self, id: &str) -> Result<Option<ParsedDoogat>> {
        let txn = self.db.begin_read().map_err(redb_err)?;
        let t = match txn.open_table(DOOGATS) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(redb_err(e)),
        };
        match t.get(id) {
            Ok(Some(val)) => {
                let z: ParsedDoogat = serde_json::from_slice(val.value()).map_err(redb_err)?;
                Ok(Some(z))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(redb_err(e)),
        }
    }

    /// Prefix scan: all doogat IDs of a given type.
    pub fn scan_by_type(&self, type_name: &str) -> Result<Vec<String>> {
        self.prefix_scan(BY_TYPE, &format!("{type_name}/"))
    }

    /// Prefix scan: all doogat IDs with a given tag.
    pub fn scan_by_tag(&self, tag: &str) -> Result<Vec<String>> {
        self.prefix_scan(BY_TAG, &format!("{tag}/"))
    }

    /// Prefix scan: all doogat IDs that link TO the given target.
    pub fn backlinks(&self, target_id: &str) -> Result<Vec<String>> {
        self.prefix_scan(LINKS, &format!("{target_id}/"))
    }

    /// Rebuild the entire redb index from a git repo.
    pub fn rebuild<S: crate::traits::DoogatSource>(&self, source: &S) -> Result<usize> {
        let paths = source.list_doogats()?;
        let mut count = 0;

        for path in &paths {
            if let Ok(content) = source.read_file(path) {
                if let Ok(parsed) = crate::parser::parse(&content, path) {
                    self.index_doogat(&parsed)?;
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    // ── Internal helpers ─────────────────────────────────────────

    /// Remove secondary index entries for a doogat (before delete or re-index).
    fn remove_secondary_entries(&self, txn: &redb::WriteTransaction, id: &str) -> Result<()> {
        // Read existing doogat to know its type/tags/links
        if let Ok(t) = txn.open_table(DOOGATS) {
            if let Ok(Some(val)) = t.get(id) {
                if let Ok(old) = serde_json::from_slice::<ParsedDoogat>(val.value()) {
                    // Remove type entry
                    if let Some(ref zt) = old.meta.doogat_type {
                        let key = format!("{zt}/{id}");
                        if let Ok(mut tt) = txn.open_table(BY_TYPE) {
                            let _: std::result::Result<Option<redb::AccessGuard<&[u8]>>, _> =
                                tt.remove(key.as_str());
                        }
                    }
                    // Remove tag entries (frontmatter + body hashtags)
                    if let Ok(mut tt) = txn.open_table(BY_TAG) {
                        for tag in old.meta.tags.iter().chain(old.body_tags.iter()) {
                            let key = format!("{tag}/{id}");
                            let _: std::result::Result<Option<redb::AccessGuard<&[u8]>>, _> =
                                tt.remove(key.as_str());
                        }
                    }
                    // Remove link entries
                    if let Ok(mut tt) = txn.open_table(LINKS) {
                        for link in &old.links {
                            let key = format!("{}/{id}", link.target);
                            let _: std::result::Result<Option<redb::AccessGuard<&[u8]>>, _> =
                                tt.remove(key.as_str());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Generic prefix scan on a secondary index table.
    /// Returns the ID portion (after the "/") of matching keys.
    fn prefix_scan(
        &self,
        table_def: TableDefinition<&str, &[u8]>,
        prefix: &str,
    ) -> Result<Vec<String>> {
        let txn = self.db.begin_read().map_err(redb_err)?;
        let t = match txn.open_table(table_def) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(redb_err(e)),
        };

        let mut ids = Vec::new();
        let range = t.range(prefix..).map_err(redb_err)?;

        for entry in range {
            let (key, _val) = entry.map_err(redb_err)?;
            let k: &str = key.value();
            if !k.starts_with(prefix) {
                break;
            }
            if let Some(id) = k.strip_prefix(prefix) {
                ids.push(id.to_string());
            }
        }
        Ok(ids)
    }
}

/// Production `NoSqlMirrorPort` backed by the redb index.
///
/// Opens the redb database on each mirror call rather than holding it open,
/// preserving the exact open-on-write behavior the service used before the
/// port boundary (`service/crud.rs` `nosql_index_doogat`/`nosql_remove_doogat`).
/// redb takes an exclusive file lock while open, so a long-lived handle would
/// block other processes; opening per call keeps the mirror best-effort and
/// non-blocking. PRD 00142.
pub struct RedbMirror {
    path: std::path::PathBuf,
}

impl RedbMirror {
    /// Create a mirror that writes to the redb database at `path`.
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }
}

impl crate::traits::NoSqlMirrorPort for RedbMirror {
    fn mirror_index_doogat(&self, doogat: &ParsedDoogat) -> Result<()> {
        RedbIndex::open(&self.path)?.index_doogat(doogat)
    }

    fn mirror_remove_doogat(&self, id: &str) -> Result<()> {
        RedbIndex::open(&self.path)?.remove_doogat(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DoogatId, DoogatMeta, Link, Zone};

    fn test_doogat(id: &str, title: &str) -> ParsedDoogat {
        ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId(id.into())),
                title: Some(title.into()),
                doogat_type: Some("project".into()),
                tags: vec!["rust".into(), "test".into()],
                ..Default::default()
            },
            body: "body".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![Link {
                target: "20240102000000".into(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: Zone::Reference,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: format!("ddb/{id}.md"),
            updated_at: None,
        }
    }

    #[test]
    fn crud_and_prefix_scan() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.redb");
        let idx = RedbIndex::open(&db_path).unwrap();

        let z = test_doogat("20240101120000", "Test Note");
        idx.index_doogat(&z).unwrap();

        // Get
        let got = idx.get("20240101120000").unwrap().unwrap();
        assert_eq!(got.meta.title.as_deref(), Some("Test Note"));

        // Scan by type
        let ids = idx.scan_by_type("project").unwrap();
        assert_eq!(ids, vec!["20240101120000"]);

        // Scan by tag
        let ids = idx.scan_by_tag("rust").unwrap();
        assert_eq!(ids, vec!["20240101120000"]);

        // Backlinks
        let ids = idx.backlinks("20240102000000").unwrap();
        assert_eq!(ids, vec!["20240101120000"]);

        // Remove
        idx.remove_doogat("20240101120000").unwrap();
        assert!(idx.get("20240101120000").unwrap().is_none());
        assert!(idx.scan_by_type("project").unwrap().is_empty());
        assert!(idx.scan_by_tag("rust").unwrap().is_empty());
        assert!(idx.backlinks("20240102000000").unwrap().is_empty());
    }

    #[test]
    fn upsert_updates_secondary_indices() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.redb");
        let idx = RedbIndex::open(&db_path).unwrap();

        let mut z = test_doogat("20240101120000", "V1");
        idx.index_doogat(&z).unwrap();

        // Re-index with different type and tags
        z.meta.doogat_type = Some("contact".into());
        z.meta.tags = vec!["new-tag".into()];
        z.links = vec![];
        idx.index_doogat(&z).unwrap();

        // Old type/tag/link gone
        assert!(idx.scan_by_type("project").unwrap().is_empty());
        assert!(idx.scan_by_tag("rust").unwrap().is_empty());
        assert!(idx.backlinks("20240102000000").unwrap().is_empty());

        // New type/tag present
        assert_eq!(idx.scan_by_type("contact").unwrap(), vec!["20240101120000"]);
        assert_eq!(idx.scan_by_tag("new-tag").unwrap(), vec!["20240101120000"]);
    }

    #[test]
    fn get_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.redb");
        let idx = RedbIndex::open(&db_path).unwrap();
        assert!(idx.get("nonexistent").unwrap().is_none());
    }
}
