//! Command types for the application contract layer.

use crate::types::{ConflictAction, Value};
use std::collections::BTreeMap;

/// Whether `DoogatService::create` rejects an unregistered `doogat_type` or
/// falls back to a base-only create. Makes create strictness an explicit
/// per-caller input (PRD 00155) instead of an accident of shared routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnregisteredTypePolicy {
    /// Reject an unregistered `doogat_type` with `TYPE_NOT_REGISTERED`.
    /// GraphQL `createDoogat` and the REST POST handler use this (default).
    #[default]
    Strict,
    /// Create a base doogat (no typed validation) when `doogat_type` names a
    /// type with no registered `_typedef`, attaching an
    /// `UNREGISTERED_TYPE_BASE_ONLY` warning. The CLI uses this to preserve
    /// the released (≤ v0.2.5) contract.
    BaseOnly,
}

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
    pub unregistered_type_policy: UnregisteredTypePolicy,
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
