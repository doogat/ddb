use crate::error::Result;
use crate::parser;
use crate::traits::{GitBackend, IndexPort};
use crate::types::{AttachmentInfo, CommitHash, DoogatId, FixReport, ParsedDoogat, TableSchema};

use super::DoogatService;

/// Whether `e` is redb's `DatabaseAlreadyOpen` conflict rather than a real
/// failure. `RedbIndex::open` flattens every redb error to a `DoogatError::Redb(String)`
/// (`nosql.rs`'s `redb_err`), so the only signal left is the message text.
#[cfg(feature = "nosql")]
fn is_transient_open_conflict(e: &crate::error::DoogatError) -> bool {
    matches!(e, crate::error::DoogatError::Redb(msg) if msg.contains("already open"))
}

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    // ── Utility ─────────────────────────────────────────────────────────

    /// Verify index is reachable.
    pub fn health_check(&self) -> Result<bool> {
        Ok(self.index.query_raw("SELECT 1").is_ok())
    }

    pub fn list_doogats(&self) -> Result<Vec<String>> {
        self.repo.list_doogats()
    }

    pub fn resolve_path(&self, id: &str) -> Result<String> {
        self.index.resolve_path(id)
    }

    pub fn head_oid(&self) -> Result<CommitHash> {
        self.repo.head_oid()
    }

    /// Commit an arbitrary file to the git repository.
    pub fn commit_file(&self, path: &str, content: &str, message: &str) -> Result<CommitHash> {
        self.repo.commit_file(path, content, message)
    }

    pub fn is_index_stale(&self) -> Result<bool> {
        self.index.is_stale(&self.repo)
    }

    pub fn load_config(&self) -> Result<crate::types::RepoConfig> {
        self.repo.load_config()
    }

    pub fn list_type_schemas(&self) -> Result<Vec<TableSchema>> {
        let rows = self
            .index
            .query_raw("SELECT path FROM doogats WHERE type = '_typedef'")?;
        let mut schemas = Vec::new();
        for row in rows {
            if let Some(path) = row.first() {
                let content = match self.repo.read_file(path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("typedef {path}: read failed: {e}");
                        continue;
                    }
                };
                let parsed = match parser::parse(&content, path) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("typedef {path}: parse failed: {e}");
                        continue;
                    }
                };
                match crate::sql_engine::schema_from_parsed(&parsed) {
                    Ok(schema) => schemas.push(schema),
                    Err(e) => {
                        tracing::warn!("typedef {path}: schema extraction failed: {e}");
                    }
                }
            }
        }
        Ok(schemas)
    }

    pub fn infer_schema(&self, name: &str) -> Result<TableSchema> {
        self.ensure_fresh()?;
        self.index.infer_schema(name, &self.repo)
    }

    pub fn migrate_all(&self, dry_run: bool) -> Result<FixReport> {
        crate::consistency::migrate_all(&self.repo, dry_run)
    }

    pub fn resurrected_doogats(&self) -> Result<Vec<(String, String)>> {
        self.index.resurrected_doogats()
    }

    pub fn broken_backlinks(&self) -> Result<Vec<(String, String)>> {
        self.index.broken_backlinks()
    }

    pub fn backlinking_doogat_paths(&self, id: &str) -> Result<Vec<(String, String)>> {
        self.index.backlinking_doogat_paths(id)
    }

    // ── Attachments ─────────────────────────────────────────────────────

    pub fn list_attachments(&self, doogat_id: &str) -> Result<Vec<AttachmentInfo>> {
        let id = DoogatId(doogat_id.to_owned());
        crate::attachments::list_attachments(&self.repo, &id)
    }

    // ── NoSQL reads ─────────────────────────────────────────────────────

    /// Open the NoSQL redb handle, retrying briefly on a transient
    /// `DatabaseAlreadyOpen` conflict. redb allows only one open handle on
    /// the file at a time (`nosql.rs`'s `RedbMirror` doc comment); since
    /// 00174 split the write actor and maintenance worker onto separate
    /// threads sharing one mirror, a NoSQL read opening its own handle here
    /// can now race a mirror write's own transient open on the other
    /// thread. Both opens are brief (one redb transaction), so a short
    /// bounded retry resolves the race without a cross-thread lock spanning
    /// two independently-owned `DoogatService` instances.
    #[cfg(feature = "nosql")]
    fn open_nosql(&self) -> Result<crate::nosql::RedbIndex> {
        let redb_path = self.repo_path.join(".ddb/nosql.redb");
        let mut last_err = None;
        for attempt in 0..20 {
            match crate::nosql::RedbIndex::open(&redb_path) {
                Ok(idx) => return Ok(idx),
                Err(e) if is_transient_open_conflict(&e) => {
                    last_err = Some(e);
                    if attempt < 19 {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.expect("loop always runs at least once"))
    }

    /// Open and rebuild the NoSQL index, returning a ready-to-query handle.
    #[cfg(feature = "nosql")]
    fn open_nosql_fresh(&self) -> Result<crate::nosql::RedbIndex> {
        let ri = self.open_nosql()?;
        ri.rebuild(&self.repo)?;
        Ok(ri)
    }

    /// Get a doogat by ID from the NoSQL index.
    #[cfg(feature = "nosql")]
    pub fn nosql_get(&self, id: &str) -> Result<Option<ParsedDoogat>> {
        self.open_nosql_fresh()?.get(id)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_get(&self, _id: &str) -> Result<Option<ParsedDoogat>> {
        Err(crate::error::DoogatError::NotFound(
            "nosql not available".into(),
        ))
    }

    /// Scan by type in the NoSQL index.
    #[cfg(feature = "nosql")]
    pub fn nosql_scan_type(&self, type_name: &str) -> Result<Vec<String>> {
        self.open_nosql_fresh()?.scan_by_type(type_name)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_scan_type(&self, _type_name: &str) -> Result<Vec<String>> {
        Err(crate::error::DoogatError::NotFound(
            "nosql not available".into(),
        ))
    }

    /// Scan by tag in the NoSQL index.
    #[cfg(feature = "nosql")]
    pub fn nosql_scan_tag(&self, tag: &str) -> Result<Vec<String>> {
        self.open_nosql_fresh()?.scan_by_tag(tag)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_scan_tag(&self, _tag: &str) -> Result<Vec<String>> {
        Err(crate::error::DoogatError::NotFound(
            "nosql not available".into(),
        ))
    }

    /// Get backlinks from the NoSQL index.
    #[cfg(feature = "nosql")]
    pub fn nosql_backlinks(&self, id: &str) -> Result<Vec<String>> {
        self.open_nosql_fresh()?.backlinks(id)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_backlinks(&self, _id: &str) -> Result<Vec<String>> {
        Err(crate::error::DoogatError::NotFound(
            "nosql not available".into(),
        ))
    }

    /// Rebuild the NoSQL index from git.
    #[cfg(feature = "nosql")]
    pub fn nosql_rebuild(&self) -> Result<usize> {
        let ri = self.open_nosql()?;
        ri.rebuild(&self.repo)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_rebuild(&self) -> Result<usize> {
        Err(crate::error::DoogatError::NotFound(
            "nosql not available".into(),
        ))
    }
}
