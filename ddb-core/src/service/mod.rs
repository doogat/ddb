use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::git_ops::GitRepo;
use crate::indexer::Index;
use crate::sql_engine::TransactionBuffer;

mod crud;
mod discovery;
mod ops;
mod search;
mod sql;
mod utility;

/// Extra frontmatter fields to set or remove during an update.
pub struct ExtraFieldUpdates<'a> {
    pub set: &'a std::collections::BTreeMap<String, crate::types::Value>,
    pub unset: &'a [String],
}

impl Default for ExtraFieldUpdates<'_> {
    fn default() -> Self {
        static EMPTY_MAP: std::sync::LazyLock<
            std::collections::BTreeMap<String, crate::types::Value>,
        > = std::sync::LazyLock::new(std::collections::BTreeMap::new);
        Self {
            set: &EMPTY_MAP,
            unset: &[],
        }
    }
}

/// Unified orchestration layer composing GitRepo, Index, and optional NoSQL
/// index into a single entry point for all high-level operations.
///
/// CLI, FFI, and server consumers delegate to `DoogatService` instead of
/// independently composing core modules. This ensures consistent behaviour
/// (e.g. NoSQL dual-write) across all entry points.
pub struct DoogatService {
    pub(super) repo: GitRepo,
    pub(super) index: Index,
    pub(super) txn: Option<TransactionBuffer>,
    pub(super) repo_path: PathBuf,
    pub(super) skip_stale_check: bool,
}

impl DoogatService {
    /// Open an existing Doogat DB repository.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = GitRepo::open(path)?;
        let db_dir = path.join(".ddb");
        std::fs::create_dir_all(&db_dir)?;
        let index = Index::open(&db_dir.join("index.db"))?;
        Ok(Self {
            repo,
            index,
            txn: None,
            repo_path: path.to_path_buf(),
            skip_stale_check: false,
        })
    }

    /// Initialize a new Doogat DB repository at `path` and open it.
    pub fn init(path: &Path) -> Result<Self> {
        GitRepo::init(path)?;
        Self::open(path)
    }

    /// Path to the repository root.
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Skip rebuild_if_stale checks. Use for read-only connections
    /// where another writer (e.g. actor) keeps the index fresh.
    pub fn set_skip_stale_check(&mut self, skip: bool) {
        self.skip_stale_check = skip;
    }

    fn ensure_fresh(&self) -> Result<()> {
        if !self.skip_stale_check {
            self.index.rebuild_if_stale(&self.repo)?;
        }
        // skip_stale_check: ReadPool path — actor keeps the index
        // current, so readers trust WAL visibility without querying
        // _ddb_meta (avoids SQLite lock contention on Windows).
        Ok(())
    }
}

#[cfg(test)]
mod tests;
