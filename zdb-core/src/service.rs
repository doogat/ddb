use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::git_ops::GitRepo;
use crate::indexer::Index;
use crate::sql_engine::TransactionBuffer;

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
}
