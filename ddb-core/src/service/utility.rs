use crate::error::Result;
use crate::git_ops;
use crate::parser;
use crate::types::{
    AttachmentInfo, CommitHash, DoogatId, FixReport, ParsedDoogat, RenameReport, TableSchema,
};

use super::DoogatService;

impl DoogatService {
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

    pub fn rename_doogat(&self, id: &str, new_path: &str) -> Result<RenameReport> {
        let old_path = self.index.resolve_path(id)?;
        git_ops::rename_doogat(&self.repo, &self.index, &old_path, new_path)
    }

    pub fn fix_all(&self, dry_run: bool) -> Result<FixReport> {
        crate::consistency::fix_all(&self.repo, &self.index, dry_run)
    }

    pub fn migrate_all(&self, dry_run: bool) -> Result<FixReport> {
        crate::consistency::migrate_all(&self.repo, dry_run)
    }

    pub fn zone_migrate_all(&self, dry_run: bool) -> Result<FixReport> {
        crate::consistency::zone_migrate_all(&self.repo, &self.index, dry_run)
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

    pub fn attach_file(
        &self,
        doogat_id: &str,
        filename: &str,
        bytes: &[u8],
        mime: &str,
    ) -> Result<AttachmentInfo> {
        let id = DoogatId(doogat_id.to_owned());
        crate::attachments::attach_file(&self.repo, &self.index, &id, filename, bytes, mime)
    }

    pub fn detach_file(&self, doogat_id: &str, filename: &str) -> Result<()> {
        let id = DoogatId(doogat_id.to_owned());
        crate::attachments::detach_file(&self.repo, &self.index, &id, filename)
    }

    pub fn list_attachments(&self, doogat_id: &str) -> Result<Vec<AttachmentInfo>> {
        let id = DoogatId(doogat_id.to_owned());
        crate::attachments::list_attachments(&self.repo, &id)
    }

    // ── NoSQL reads ─────────────────────────────────────────────────────

    #[cfg(feature = "nosql")]
    fn open_nosql(&self) -> Result<crate::nosql::RedbIndex> {
        let redb_path = self.repo_path.join(".ddb/nosql.redb");
        crate::nosql::RedbIndex::open(&redb_path)
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
        Err(DoogatError::NotFound("nosql not available".into()))
    }

    /// Scan by type in the NoSQL index.
    #[cfg(feature = "nosql")]
    pub fn nosql_scan_type(&self, type_name: &str) -> Result<Vec<String>> {
        self.open_nosql_fresh()?.scan_by_type(type_name)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_scan_type(&self, _type_name: &str) -> Result<Vec<String>> {
        Err(DoogatError::NotFound("nosql not available".into()))
    }

    /// Scan by tag in the NoSQL index.
    #[cfg(feature = "nosql")]
    pub fn nosql_scan_tag(&self, tag: &str) -> Result<Vec<String>> {
        self.open_nosql_fresh()?.scan_by_tag(tag)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_scan_tag(&self, _tag: &str) -> Result<Vec<String>> {
        Err(DoogatError::NotFound("nosql not available".into()))
    }

    /// Get backlinks from the NoSQL index.
    #[cfg(feature = "nosql")]
    pub fn nosql_backlinks(&self, id: &str) -> Result<Vec<String>> {
        self.open_nosql_fresh()?.backlinks(id)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_backlinks(&self, _id: &str) -> Result<Vec<String>> {
        Err(DoogatError::NotFound("nosql not available".into()))
    }

    /// Rebuild the NoSQL index from git.
    #[cfg(feature = "nosql")]
    pub fn nosql_rebuild(&self) -> Result<usize> {
        let ri = self.open_nosql()?;
        ri.rebuild(&self.repo)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_rebuild(&self) -> Result<usize> {
        Err(DoogatError::NotFound("nosql not available".into()))
    }
}
