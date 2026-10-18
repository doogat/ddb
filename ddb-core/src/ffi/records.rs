//! UniFFI `Record`/`Error` types and their conversions from core types.
//!
//! Split out of `ffi.rs` per PRD 00156.

use crate::app_contract::AppOutput;
use crate::error::{DoogatError, ErrorContext, ErrorValue};
use crate::sql_engine::SqlResult;
use crate::types::{ConflictAction, ParsedDoogat, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DdbErrorContextEntry {
    pub key: String,
    pub value: Option<String>,
    pub values: Vec<String>,
}

pub(super) fn empty_error_context() -> Vec<DdbErrorContextEntry> {
    Vec::new()
}

fn ffi_error_context(context: ErrorContext) -> Vec<DdbErrorContextEntry> {
    context
        .into_iter()
        .map(|(key, value)| match value {
            ErrorValue::String(value) => DdbErrorContextEntry {
                key,
                value: Some(value),
                values: Vec::new(),
            },
            ErrorValue::List(values) => DdbErrorContextEntry {
                key,
                value: None,
                values,
            },
        })
        .collect()
}

/// FFI error enum exposed to Swift/Kotlin via UniFFI.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum DdbError {
    #[error("Git: {msg}")]
    Git { msg: String },
    #[error("Yaml: {msg}")]
    Yaml { msg: String },
    #[error("Sql: {msg}")]
    Sql { msg: String },
    #[error("Automerge: {msg}")]
    Automerge { msg: String },
    #[error("Io: {msg}")]
    Io { msg: String },
    #[error("Parse: {msg}")]
    Parse { msg: String },
    #[error("NotFound: {msg}")]
    NotFound { msg: String },
    #[error("Config: {msg}")]
    Config { msg: String },
    #[error("Validation: {msg}")]
    Validation {
        msg: String,
        code: Option<String>,
        context: Vec<DdbErrorContextEntry>,
    },
    #[error("SqlEngine: {msg}")]
    SqlEngine {
        msg: String,
        code: Option<String>,
        context: Vec<DdbErrorContextEntry>,
    },
    #[error("Conflict: {msg}")]
    Conflict {
        msg: String,
        code: Option<String>,
        context: Vec<DdbErrorContextEntry>,
    },
    #[error("VersionMismatch: {msg}")]
    VersionMismatch { msg: String },
}

impl From<DoogatError> for DdbError {
    fn from(e: DoogatError) -> Self {
        if let DoogatError::VersionMismatch { repo, driver } = &e {
            return DdbError::VersionMismatch {
                msg: format!("repo format v{repo}, driver supports up to v{driver}"),
            };
        }
        #[cfg(feature = "nosql")]
        if let DoogatError::Redb(msg) = &e {
            return DdbError::Io { msg: msg.clone() };
        }
        category_variant(e)
    }
}

fn category_variant(err: DoogatError) -> DdbError {
    use crate::app_contract::{error_rendering, AppError, FfiVariant};
    let context = match &err {
        DoogatError::Structured { context, .. } => ffi_error_context(context.clone()),
        _ => empty_error_context(),
    };
    let msg = err.to_string();
    let rendering = error_rendering(AppError::from(&err));
    match rendering.ffi_variant {
        FfiVariant::NotFound => DdbError::NotFound { msg },
        FfiVariant::Conflict => DdbError::Conflict {
            msg,
            code: Some(rendering.code.to_string()),
            context,
        },
        FfiVariant::Validation => DdbError::Validation {
            msg,
            code: Some(rendering.code.to_string()),
            context,
        },
        FfiVariant::Internal => DdbError::Io { msg },
    }
}

/// FFI-safe frontmatter field entry (one key/value pair of a search hit).
#[derive(uniffi::Record)]
pub struct FieldEntry {
    pub key: String,
    pub value: String,
}

/// FFI-safe search result.
#[derive(uniffi::Record)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub path: String,
    pub snippet: String,
    pub rank: f64,
    pub updated_at: String,
    pub tags: Vec<String>,
    pub doogat_type: Option<String>,
    pub fields: Vec<FieldEntry>,
    pub created_at: Option<String>,
}

impl From<crate::types::SearchResult> for SearchResult {
    fn from(r: crate::types::SearchResult) -> Self {
        Self {
            id: r.id,
            title: r.title,
            path: r.path,
            snippet: r.snippet,
            rank: r.rank,
            updated_at: r.updated_at,
            tags: r.tags,
            doogat_type: r.doogat_type,
            fields: r
                .fields
                .unwrap_or_default()
                .into_iter()
                .map(|(key, value)| FieldEntry { key, value })
                .collect(),
            created_at: r.created_at,
        }
    }
}

/// FFI-safe paginated search result.
#[derive(uniffi::Record)]
pub struct PaginatedSearchResult {
    pub hits: Vec<SearchResult>,
    pub total_count: u64,
    pub warnings: Vec<RebuildWarningRecord>,
}

/// FFI-safe reindex warning (one per skipped/poisoned file).
#[derive(Debug, uniffi::Record)]
pub struct RebuildWarningRecord {
    pub code: String,
    pub message: String,
}

/// FFI-safe rebuild report.
#[derive(uniffi::Record)]
pub struct RebuildReport {
    pub indexed: u64,
    pub tables_materialized: u64,
    pub types_inferred: Vec<String>,
    pub warnings: Vec<RebuildWarningRecord>,
}

/// FFI-safe attachment metadata.
#[derive(uniffi::Record)]
pub struct AttachmentInfo {
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub path: String,
}

impl From<crate::types::AttachmentInfo> for AttachmentInfo {
    fn from(a: crate::types::AttachmentInfo) -> Self {
        Self {
            name: a.name,
            mime: a.mime,
            size: a.size,
            path: a.path,
        }
    }
}

/// FFI-safe column definition for type schema discovery.
#[derive(uniffi::Record)]
pub struct ColumnDefRecord {
    pub name: String,
    pub data_type: String,
    pub references: Option<String>,
    pub required: bool,
}

impl From<&crate::types::ColumnDef> for ColumnDefRecord {
    fn from(c: &crate::types::ColumnDef) -> Self {
        Self {
            name: c.name.clone(),
            data_type: c.data_type.clone(),
            references: c.references.clone(),
            required: c.required,
        }
    }
}

/// FFI-safe type schema for typedef discovery.
#[derive(uniffi::Record)]
pub struct TypeSchemaRecord {
    pub table_name: String,
    pub columns: Vec<ColumnDefRecord>,
    pub crdt_strategy: Option<String>,
    pub template_sections: Vec<String>,
}

impl From<crate::types::TableSchema> for TypeSchemaRecord {
    fn from(s: crate::types::TableSchema) -> Self {
        Self {
            table_name: s.table_name,
            columns: s.columns.iter().map(ColumnDefRecord::from).collect(),
            crdt_strategy: s.crdt_strategy,
            template_sections: s.template_sections,
        }
    }
}

/// FFI-safe SQL execution result.
///
/// Flat record suitable for UniFFI export. Queries populate `columns`/`rows`;
/// mutations populate `affected_rows`; DDL sets `message`.
#[derive(uniffi::Record)]
pub struct SqlResultRecord {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub affected_rows: i64,
    pub message: String,
}

impl From<SqlResult> for SqlResultRecord {
    fn from(r: SqlResult) -> Self {
        match r {
            SqlResult::Rows { columns, rows, .. } => {
                let affected_rows = rows.len() as i64;
                Self {
                    columns,
                    rows,
                    affected_rows,
                    message: String::new(),
                }
            }
            SqlResult::Affected(n) => Self {
                columns: Vec::new(),
                rows: Vec::new(),
                affected_rows: n as i64,
                message: String::new(),
            },
            SqlResult::Ok(msg) => Self {
                columns: Vec::new(),
                rows: Vec::new(),
                affected_rows: 0,
                message: msg,
            },
        }
    }
}

/// FFI-safe single schema-plan operation report.
#[derive(Debug, uniffi::Record)]
pub struct SchemaPlanOpRecord {
    pub kind: String,
    pub table: String,
    pub detail: String,
    pub destructive: bool,
    pub sql: String,
}

impl From<crate::schema_diff::plan::PlanOpReport> for SchemaPlanOpRecord {
    fn from(op: crate::schema_diff::plan::PlanOpReport) -> Self {
        Self {
            kind: op.kind,
            table: op.table,
            detail: op.detail,
            destructive: op.destructive,
            sql: op.sql,
        }
    }
}

/// FFI-safe schema-apply warning.
#[derive(Debug, uniffi::Record)]
pub struct SchemaWarningRecord {
    pub code: String,
    pub message: String,
}

/// FFI-safe schema-apply report.
#[derive(Debug, uniffi::Record)]
pub struct SchemaApplyReportRecord {
    pub dry_run: bool,
    pub applied: bool,
    pub ops: Vec<SchemaPlanOpRecord>,
    pub unsupported: Vec<String>,
    pub warnings: Vec<SchemaWarningRecord>,
}

impl SchemaApplyReportRecord {
    pub(super) fn from_output(
        out: crate::app_contract::AppOutput<crate::schema_diff::plan::SchemaApplyReport>,
    ) -> Self {
        Self {
            dry_run: out.value.dry_run,
            applied: out.value.applied,
            ops: out
                .value
                .ops
                .into_iter()
                .map(SchemaPlanOpRecord::from)
                .collect(),
            unsupported: out.value.unsupported,
            warnings: out
                .warnings
                .into_iter()
                .map(|w| SchemaWarningRecord {
                    code: w.code.to_string(),
                    message: w.message,
                })
                .collect(),
        }
    }
}

/// FFI-safe scalar field value (one element of a [`FieldValue::List`]).
/// Non-recursive on purpose: uniffi 0.29.5 cannot express recursion
/// through `Vec`.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum FieldScalar {
    Str(String),
    Num(f64),
    Bool(bool),
}

/// FFI-safe structured field value.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum FieldValue {
    Str(String),
    Num(f64),
    Bool(bool),
    List(Vec<FieldScalar>),
}

/// FFI-safe frontmatter field input. A `Vec` of these stands in for a map
/// because UniFFI records need a concrete field order.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FieldEntryValue {
    pub key: String,
    pub value: FieldValue,
}

fn field_scalar_to_core(v: FieldScalar) -> Value {
    match v {
        FieldScalar::Str(s) => Value::String(s),
        FieldScalar::Num(n) => Value::Number(n),
        FieldScalar::Bool(b) => Value::Bool(b),
    }
}

#[cfg(test)]
fn core_to_field_scalar(v: Value) -> Option<FieldScalar> {
    match v {
        Value::String(s) => Some(FieldScalar::Str(s)),
        Value::Number(n) => Some(FieldScalar::Num(n)),
        Value::Bool(b) => Some(FieldScalar::Bool(b)),
        Value::List(_) | Value::Map(_) => None,
    }
}

pub(super) fn field_value_to_core(v: FieldValue) -> Value {
    match v {
        FieldValue::Str(s) => Value::String(s),
        FieldValue::Num(n) => Value::Number(n),
        FieldValue::Bool(b) => Value::Bool(b),
        FieldValue::List(items) => {
            Value::List(items.into_iter().map(field_scalar_to_core).collect())
        }
    }
}

/// Convert a core value to its FFI form. Nested lists/maps inside a list are
/// dropped with a warning; a top-level map degrades to an empty list.
#[cfg(test)]
pub(super) fn core_to_field_value(v: Value) -> FieldValue {
    match v {
        Value::String(s) => FieldValue::Str(s),
        Value::Number(n) => FieldValue::Num(n),
        Value::Bool(b) => FieldValue::Bool(b),
        Value::List(items) => FieldValue::List(
            items
                .into_iter()
                .filter_map(|item| {
                    let scalar = core_to_field_scalar(item);
                    if scalar.is_none() {
                        tracing::warn!("dropping nested list/map element from FFI field list");
                    }
                    scalar
                })
                .collect(),
        ),
        Value::Map(_) => {
            tracing::warn!("dropping map field value at the FFI boundary");
            FieldValue::List(Vec::new())
        }
    }
}

pub(super) fn fields_to_core(fields: Vec<FieldEntryValue>) -> BTreeMap<String, Value> {
    fields
        .into_iter()
        .map(|e| (e.key, field_value_to_core(e.value)))
        .collect()
}

/// FFI-safe mirror of [`ConflictAction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ConflictActionRecord {
    Error,
    Ignore,
}

impl From<ConflictActionRecord> for ConflictAction {
    fn from(a: ConflictActionRecord) -> Self {
        match a {
            ConflictActionRecord::Error => ConflictAction::Error,
            ConflictActionRecord::Ignore => ConflictAction::Ignore,
        }
    }
}

impl From<ConflictAction> for ConflictActionRecord {
    fn from(a: ConflictAction) -> Self {
        match a {
            ConflictAction::Error => ConflictActionRecord::Error,
            ConflictAction::Ignore => ConflictActionRecord::Ignore,
        }
    }
}

/// FFI-safe create result.
#[derive(Debug, uniffi::Record)]
pub struct CreateResultRecord {
    pub id: String,
    pub title: Option<String>,
    pub doogat_type: Option<String>,
    pub tags: Vec<String>,
    pub path: String,
    pub warnings: Vec<RebuildWarningRecord>,
}

/// FFI-safe update result.
#[derive(Debug, uniffi::Record)]
pub struct UpdateResultRecord {
    pub id: String,
    pub title: Option<String>,
    pub doogat_type: Option<String>,
    pub tags: Vec<String>,
    pub path: String,
    pub warnings: Vec<RebuildWarningRecord>,
}

/// Fields shared by the create and update result records, copied verbatim.
type DoogatResultParts = (
    String,
    Option<String>,
    Option<String>,
    Vec<String>,
    String,
    Vec<RebuildWarningRecord>,
);

fn doogat_result_parts(out: AppOutput<ParsedDoogat>) -> DoogatResultParts {
    let meta = out.value.meta;
    (
        meta.id.map(|id| id.0).unwrap_or_default(),
        meta.title,
        meta.doogat_type,
        meta.tags,
        out.value.path,
        out.warnings
            .into_iter()
            .map(|w| RebuildWarningRecord {
                code: w.code.to_string(),
                message: w.message,
            })
            .collect(),
    )
}

impl CreateResultRecord {
    pub(super) fn from_output(out: AppOutput<ParsedDoogat>) -> Self {
        let (id, title, doogat_type, tags, path, warnings) = doogat_result_parts(out);
        Self {
            id,
            title,
            doogat_type,
            tags,
            path,
            warnings,
        }
    }
}

impl UpdateResultRecord {
    pub(super) fn from_output(out: AppOutput<ParsedDoogat>) -> Self {
        let (id, title, doogat_type, tags, path, warnings) = doogat_result_parts(out);
        Self {
            id,
            title,
            doogat_type,
            tags,
            path,
            warnings,
        }
    }
}
