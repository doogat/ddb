//! redb-based NoSQL key-value index for fast lookups and prefix scans.
//!
//! Complements SQLite (which keeps FTS5/SQL). redb adds O(1) key lookups
//! and efficient prefix scans by type, tag, and backlinks.

use std::path::Path;

use redb::{Database, ReadableTable, TableDefinition};

use crate::error::{Result, ZettelError};
use crate::types::ParsedZettel;

/// Shorthand for mapping any redb/bincode error to ZettelError::Redb.
fn redb_err(e: impl std::fmt::Display) -> ZettelError {
    ZettelError::Redb(e.to_string())
}

// ── Table definitions ────────────────────────────────────────────

/// Primary store: zettel_id → bincode-serialized ParsedZettel
const ZETTELS: TableDefinition<&str, &[u8]> = TableDefinition::new("zettels");
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

    /// Index a single zettel (upsert).
    pub fn index_zettel(&self, zettel: &ParsedZettel) -> Result<()> {
        let id = zettel
            .meta
            .id
            .as_ref()
            .map(|z| z.0.as_str())
            .ok_or_else(|| ZettelError::Validation("zettel has no id".into()))?;

        let encoded = serde_json::to_vec(zettel).map_err(redb_err)?;

        let txn = self.db.begin_write().map_err(redb_err)?;

        // Remove old secondary entries before re-inserting
        self.remove_secondary_entries(&txn, id)?;

        {
            let mut t = txn.open_table(ZETTELS).map_err(redb_err)?;
            t.insert(id, encoded.as_slice()).map_err(redb_err)?;
        }

        // Type index
        if let Some(ref zt) = zettel.meta.zettel_type {
            let key = format!("{zt}/{id}");
            let mut t = txn.open_table(BY_TYPE).map_err(redb_err)?;
            t.insert(key.as_str(), [].as_slice()).map_err(redb_err)?;
        }

        // Tag index (frontmatter + body hashtags)
        {
            let mut t = txn.open_table(BY_TAG).map_err(redb_err)?;
            for tag in zettel.meta.tags.iter().chain(zettel.body_tags.iter()) {
                let key = format!("{tag}/{id}");
                t.insert(key.as_str(), [].as_slice()).map_err(redb_err)?;
            }
        }

        // Link index
        {
            let mut t = txn.open_table(LINKS).map_err(redb_err)?;
            for link in &zettel.wikilinks {
                let key = format!("{}/{id}", link.target);
                t.insert(key.as_str(), [].as_slice()).map_err(redb_err)?;
            }
        }

        txn.commit().map_err(redb_err)?;
        Ok(())
    }

    /// Remove a zettel from all tables.
    pub fn remove_zettel(&self, id: &str) -> Result<()> {
        let txn = self.db.begin_write().map_err(redb_err)?;

        self.remove_secondary_entries(&txn, id)?;

        {
            let mut t = txn.open_table(ZETTELS).map_err(redb_err)?;
            let _: Option<redb::AccessGuard<&[u8]>> = t.remove(id).map_err(redb_err)?;
        }

        txn.commit().map_err(redb_err)?;
        Ok(())
    }

    /// Get a single zettel by ID.
    pub fn get(&self, id: &str) -> Result<Option<ParsedZettel>> {
        let txn = self.db.begin_read().map_err(redb_err)?;
        let t = match txn.open_table(ZETTELS) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(redb_err(e)),
        };
        match t.get(id) {
            Ok(Some(val)) => {
                let z: ParsedZettel = serde_json::from_slice(val.value()).map_err(redb_err)?;
                Ok(Some(z))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(redb_err(e)),
        }
    }

    /// Prefix scan: all zettel IDs of a given type.
    pub fn scan_by_type(&self, type_name: &str) -> Result<Vec<String>> {
        self.prefix_scan(BY_TYPE, &format!("{type_name}/"))
    }

    /// Prefix scan: all zettel IDs with a given tag.
    pub fn scan_by_tag(&self, tag: &str) -> Result<Vec<String>> {
        self.prefix_scan(BY_TAG, &format!("{tag}/"))
    }

    /// Prefix scan: all zettel IDs that link TO the given target.
    pub fn backlinks(&self, target_id: &str) -> Result<Vec<String>> {
        self.prefix_scan(LINKS, &format!("{target_id}/"))
    }

    /// Rebuild the entire redb index from a git repo.
    pub fn rebuild<S: crate::traits::ZettelSource>(&self, source: &S) -> Result<usize> {
        let paths = source.list_zettels()?;
        let mut count = 0;

        for path in &paths {
            if let Ok(content) = source.read_file(path) {
                if let Ok(parsed) = crate::parser::parse(&content, path) {
                    self.index_zettel(&parsed)?;
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    // ── Internal helpers ─────────────────────────────────────────

    /// Remove secondary index entries for a zettel (before delete or re-index).
    fn remove_secondary_entries(&self, txn: &redb::WriteTransaction, id: &str) -> Result<()> {
        // Read existing zettel to know its type/tags/links
        if let Ok(t) = txn.open_table(ZETTELS) {
            if let Ok(Some(val)) = t.get(id) {
                if let Ok(old) = serde_json::from_slice::<ParsedZettel>(val.value()) {
                    // Remove type entry
                    if let Some(ref zt) = old.meta.zettel_type {
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
                        for link in &old.wikilinks {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{WikiLink, ZettelId, ZettelMeta, Zone};

    fn test_zettel(id: &str, title: &str) -> ParsedZettel {
        ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId(id.into())),
                title: Some(title.into()),
                zettel_type: Some("project".into()),
                tags: vec!["rust".into(), "test".into()],
                ..Default::default()
            },
            body: "body".into(),
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![WikiLink {
                target: "20240102000000".into(),
                display: None,
                zone: Zone::Reference,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: format!("zettelkasten/{id}.md"),
        }
    }

    #[test]
    fn crud_and_prefix_scan() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.redb");
        let idx = RedbIndex::open(&db_path).unwrap();

        let z = test_zettel("20240101120000", "Test Note");
        idx.index_zettel(&z).unwrap();

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
        idx.remove_zettel("20240101120000").unwrap();
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

        let mut z = test_zettel("20240101120000", "V1");
        idx.index_zettel(&z).unwrap();

        // Re-index with different type and tags
        z.meta.zettel_type = Some("contact".into());
        z.meta.tags = vec!["new-tag".into()];
        z.wikilinks = vec![];
        idx.index_zettel(&z).unwrap();

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
