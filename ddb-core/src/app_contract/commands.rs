//! Command types for the application contract layer.

use crate::types::{ConflictAction, Value};
use std::collections::BTreeMap;

/// Command to create a new doogat. `title` and `body` are optional so
/// transports can omit them and rely on service-level defaults (e.g.
/// `title_template`). `on_conflict` controls behaviour when a doogat with
/// the same identity already exists.
#[derive(Debug, Clone)]
pub struct CreateCommand {
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub doogat_type: Option<String>,
    pub body: Option<String>,
    pub fields: BTreeMap<String, Value>,
    pub on_conflict: ConflictAction,
}

/// Command to fetch a single doogat by its 14-digit ID.
#[derive(Debug, Clone)]
pub struct ReadCommand {
    pub id: String,
}

/// Command to modify an existing doogat identified by `id`. Only `Some`
/// fields are applied; `None` leaves the current value unchanged. An empty
/// `fields` map means no field changes.
#[derive(Debug, Clone)]
pub struct UpdateCommand {
    pub id: String,
    pub title: Option<String>,
    pub tags: Option<Vec<String>>,
    pub doogat_type: Option<String>,
    pub body: Option<String>,
    /// Fields to set or update. An empty map means no field changes.
    pub fields: BTreeMap<String, Value>,
}

/// Command to delete a doogat by its 14-digit ID. The service returns a
/// result that includes any broken backlinks created by the deletion, which
/// transports should surface rather than discard.
#[derive(Debug, Clone)]
pub struct DeleteCommand {
    pub id: String,
}

/// Command to run a full-text search against the SQLite FTS5 index.
///
/// `query` is passed through the search-query normalizer. `limit` and
/// `offset` enable pagination; transports that do not expose pagination
/// may leave them `None` to use service defaults.
#[derive(Debug, Clone)]
pub struct SearchCommand {
    pub query: String,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}
