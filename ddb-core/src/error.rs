use thiserror::Error;

pub type Result<T> = std::result::Result<T, DoogatError>;

#[derive(Debug, Error)]
pub enum DoogatError {
    #[error("git: {0}")]
    Git(String),

    #[error("yaml: {0}")]
    Yaml(String),

    #[error("sql: {0}")]
    Sql(String),

    #[error("automerge: {0}")]
    Automerge(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml: {0}")]
    Toml(String),

    #[error("parse: {0}")]
    Parse(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("validation: {0}")]
    Validation(String),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("sql engine: {0}")]
    SqlEngine(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("sync: {0}")]
    Sync(String),

    #[error("index: {0}")]
    Index(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("version mismatch: repo format v{repo}, driver supports up to v{driver}")]
    VersionMismatch { repo: u32, driver: u32 },

    #[cfg(feature = "nosql")]
    #[error("redb: {0}")]
    Redb(String),
}
