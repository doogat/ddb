//! Command types for the application contract layer.

use crate::types::{ConflictAction, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct CreateCommand {
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub doogat_type: Option<String>,
    pub body: Option<String>,
    pub fields: BTreeMap<String, Value>,
    pub on_conflict: ConflictAction,
}

#[derive(Debug, Clone)]
pub struct ReadCommand {
    pub id: String,
}

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

#[derive(Debug, Clone)]
pub struct DeleteCommand {
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct SearchCommand {
    pub query: String,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}
