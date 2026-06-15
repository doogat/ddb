use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::git_ops::GitRepo;
use crate::indexer::Index;
use crate::sql_engine::TransactionBuffer;
use crate::traits::{GitBackend, IndexPort, NoSqlMirrorPort};

mod batch;
mod concrete_index;
mod create;
mod delete;
mod discovery;
mod ops;
mod read;
mod search;
mod sql;
mod update;
mod utility;
mod validation;
mod write_helpers;

pub use create::UpsertOutcome;
pub use search::SORTABLE_COLUMNS;

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

/// Unified orchestration layer for high-level application operations.
///
/// Production uses the default `GitRepo` and SQLite `Index`; tests can inject a
/// different git backend, index port, and NoSQL mirror.
pub struct DoogatService<G: GitBackend = GitRepo, I: IndexPort = Index> {
    pub(super) repo: G,
    pub(super) index: I,
    pub(super) nosql: Box<dyn NoSqlMirrorPort + Send + Sync>,
    pub(super) txn: Option<TransactionBuffer>,
    pub(super) repo_path: PathBuf,
    pub(super) skip_stale_check: bool,
}

impl DoogatService<GitRepo, Index> {
    /// Open an existing Doogat DB repository with the default adapters.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = GitRepo::open(path)?;
        let db_dir = path.join(".ddb");
        std::fs::create_dir_all(&db_dir)?;
        let index = Index::open(&db_dir.join("index.db"))?;
        let nosql = Self::default_mirror(&db_dir);
        Ok(Self::from_parts(repo, index, nosql, path.to_path_buf()))
    }

    /// Initialize a new Doogat DB repository at `path` and open it.
    pub fn init(path: &Path) -> Result<Self> {
        GitRepo::init(path)?;
        Self::open(path)
    }

    /// Build the default NoSQL mirror for production wiring.
    #[cfg(feature = "nosql")]
    fn default_mirror(db_dir: &Path) -> Box<dyn NoSqlMirrorPort + Send + Sync> {
        Box::new(crate::nosql::RedbMirror::new(db_dir.join("nosql.redb")))
    }

    #[cfg(not(feature = "nosql"))]
    fn default_mirror(_db_dir: &Path) -> Box<dyn NoSqlMirrorPort + Send + Sync> {
        Box::new(crate::traits::NoopMirror)
    }
}

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    /// Inject pre-constructed dependencies.
    pub fn from_parts(
        repo: G,
        index: I,
        nosql: Box<dyn NoSqlMirrorPort + Send + Sync>,
        repo_path: PathBuf,
    ) -> Self {
        Self {
            repo,
            index,
            nosql,
            txn: None,
            repo_path,
            skip_stale_check: false,
        }
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
mod create_unregistered_policy_tests;
#[cfg(test)]
mod update_facade_tests;
#[cfg(test)]
mod mock_index_tests;
#[cfg(test)]
mod tests;
