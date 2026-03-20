use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::git_ops::{self, GitRepo};
use crate::indexer::Index;
use crate::parser;
use crate::sql_engine::TransactionBuffer;
use crate::types::{ParsedZettel, ZettelId, ZettelMeta};

/// Unified orchestration layer composing GitRepo, Index, and optional NoSQL
/// index into a single entry point for all high-level operations.
///
/// CLI, FFI, and server consumers delegate to `ZettelService` instead of
/// independently composing core modules. This ensures consistent behaviour
/// (e.g. NoSQL dual-write) across all entry points.
pub struct ZettelService {
    repo: GitRepo,
    index: Index,
    txn: Option<TransactionBuffer>,
    repo_path: PathBuf,
}

impl ZettelService {
    /// Open an existing ZettelDB repository.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = GitRepo::open(path)?;
        let db_dir = path.join(".zdb");
        std::fs::create_dir_all(&db_dir)?;
        let index = Index::open(&db_dir.join("index.db"))?;
        Ok(Self {
            repo,
            index,
            txn: None,
            repo_path: path.to_path_buf(),
        })
    }

    /// Initialize a new ZettelDB repository at `path` and open it.
    pub fn init(path: &Path) -> Result<Self> {
        GitRepo::init(path)?;
        Self::open(path)
    }

    /// Path to the repository root.
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Borrow the underlying `GitRepo`.
    pub fn repo(&self) -> &GitRepo {
        &self.repo
    }

    /// Borrow the underlying `Index`.
    pub fn index(&self) -> &Index {
        &self.index
    }

    // ── CRUD ────────────────────────────────────────────────────────────

    /// Create a new zettel from individual fields.
    ///
    /// Generates a unique ID, determines the storage path (flat or folder),
    /// commits to git, indexes in SQLite, and dual-writes to NoSQL.
    /// Returns the new zettel ID.
    pub fn create_zettel(
        &self,
        title: &str,
        tags: &[String],
        zettel_type: Option<&str>,
        body: &str,
    ) -> Result<String> {
        let id = self.unique_id();
        let id_str = id.to_string();

        let folder = zettel_type
            .map(|t| self.index.type_uses_folder(t, &self.repo))
            .unwrap_or(false);
        let path = git_ops::zettel_path(&id_str, zettel_type, folder);

        let meta = ZettelMeta {
            id: Some(id),
            title: Some(title.to_owned()),
            date: Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
            zettel_type: zettel_type.map(str::to_owned),
            tags: tags.to_vec(),
            extra: Default::default(),
        };

        let parsed = ParsedZettel {
            meta,
            body: body.to_owned(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: path.clone(),
        };

        let content = parser::serialize(&parsed);
        self.repo
            .commit_file(&path, &content, &format!("create zettel {id_str}"))?;
        self.index.index_zettel(&parsed)?;
        self.nosql_index_zettel(&parsed);

        Ok(id_str)
    }

    /// Create a zettel from raw Markdown content (for FFI consumers).
    ///
    /// Parses the content to extract/generate an ID, determines storage path,
    /// commits, indexes, and dual-writes. Returns the zettel ID.
    pub fn create_zettel_raw(&self, content: &str, message: &str) -> Result<String> {
        let parsed = parser::parse(content, "new.md")?;
        let id = parsed
            .meta
            .id
            .as_ref()
            .map(|z| z.0.clone())
            .unwrap_or_else(|| parser::generate_id().0);

        let folder = parsed
            .meta
            .zettel_type
            .as_deref()
            .map(|t| self.index.type_uses_folder(t, &self.repo))
            .unwrap_or(false);
        let rel_path = git_ops::zettel_path(&id, parsed.meta.zettel_type.as_deref(), folder);

        self.repo.commit_file(&rel_path, content, message)?;
        let parsed = parser::parse(content, &rel_path)?;
        self.index.index_zettel(&parsed)?;
        self.nosql_index_zettel(&parsed);

        Ok(id)
    }

    /// Read a zettel's raw content by ID.
    pub fn read_zettel(&self, id: &str) -> Result<String> {
        self.index.rebuild_if_stale(&self.repo)?;
        let path = self.index.resolve_path(id)?;
        self.repo.read_file(&path)
    }

    /// Update a zettel, merging provided fields into the existing content.
    pub fn update_zettel(
        &self,
        id: &str,
        title: Option<&str>,
        tags: Option<&[String]>,
        zettel_type: Option<&str>,
        body: Option<&str>,
    ) -> Result<()> {
        self.index.rebuild_if_stale(&self.repo)?;
        let path = self.index.resolve_path(id)?;
        let content = self.repo.read_file(&path)?;
        let mut parsed = parser::parse(&content, &path)?;

        if let Some(t) = title {
            parsed.meta.title = Some(t.to_owned());
        }
        if let Some(t) = tags {
            parsed.meta.tags = t.to_vec();
        }
        if let Some(t) = zettel_type {
            parsed.meta.zettel_type = Some(t.to_owned());
        }
        if let Some(b) = body {
            parsed.body = b.to_owned();
        }

        let new_content = parser::serialize(&parsed);
        self.repo
            .commit_file(&path, &new_content, &format!("update zettel {id}"))?;
        self.index.index_zettel(&parsed)?;
        self.nosql_index_zettel(&parsed);
        Ok(())
    }

    /// Update a zettel from raw content (for FFI consumers).
    pub fn update_zettel_raw(&self, id: &str, content: &str, message: &str) -> Result<()> {
        let rel_path = self.index.resolve_path(id)?;
        self.repo.commit_file(&rel_path, content, message)?;
        let parsed = parser::parse(content, &rel_path)?;
        self.index.index_zettel(&parsed)?;
        self.nosql_index_zettel(&parsed);
        Ok(())
    }

    /// Delete a zettel by ID. Returns broken backlinks `(source_id, source_path)`.
    pub fn delete_zettel(&self, id: &str) -> Result<Vec<(String, String)>> {
        self.index.rebuild_if_stale(&self.repo)?;
        let path = self.index.resolve_path(id)?;
        let broken = self.index.backlinking_zettel_paths(id)?;
        self.repo
            .delete_file(&path, &format!("delete zettel {id}"))?;
        self.index.remove_zettel(id)?;
        self.nosql_remove_zettel(id);
        Ok(broken)
    }

    // ── Private helpers ─────────────────────────────────────────────────

    /// Generate a unique zettel ID, checking the filesystem for collisions.
    fn unique_id(&self) -> ZettelId {
        let zk = self.repo_path.join("zettelkasten");
        parser::generate_unique_id(|candidate| {
            let filename = format!("{candidate}.md");
            if zk.join(&filename).exists() {
                return true;
            }
            if let Ok(entries) = std::fs::read_dir(&zk) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() && entry.path().join(&filename).exists() {
                        return true;
                    }
                }
            }
            false
        })
    }

    /// Best-effort dual-write to NoSQL index.
    #[cfg(feature = "nosql")]
    fn nosql_index_zettel(&self, zettel: &ParsedZettel) {
        let redb_path = self.repo_path.join(".zdb/nosql.redb");
        if let Ok(ri) = crate::nosql::RedbIndex::open(&redb_path) {
            let _ = ri.index_zettel(zettel);
        }
    }

    #[cfg(not(feature = "nosql"))]
    fn nosql_index_zettel(&self, _zettel: &ParsedZettel) {}

    /// Best-effort removal from NoSQL index.
    #[cfg(feature = "nosql")]
    fn nosql_remove_zettel(&self, id: &str) {
        let redb_path = self.repo_path.join(".zdb/nosql.redb");
        if let Ok(ri) = crate::nosql::RedbIndex::open(&redb_path) {
            let _ = ri.remove_zettel(id);
        }
    }

    #[cfg(not(feature = "nosql"))]
    fn nosql_remove_zettel(&self, _id: &str) {}
}
