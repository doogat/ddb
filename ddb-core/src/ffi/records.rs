//! UniFFI `Record`/`Error` types and their conversions from core types.
//!
//! Split out of `ffi.rs` per PRD 00156.

use crate::error::{DoogatError, ErrorContext, ErrorValue};
use crate::sql_engine::SqlResult;

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
    #[error("VersionMismatch: {msg}")]
    VersionMismatch { msg: String },
}

impl From<DoogatError> for DdbError {
    fn from(e: DoogatError) -> Self {
        match e {
            DoogatError::Git(msg) => DdbError::Git { msg },
            DoogatError::Yaml(msg) => DdbError::Yaml { msg },
            DoogatError::Sql(msg) => DdbError::Sql { msg },
            DoogatError::Automerge(msg) => DdbError::Automerge { msg },
            DoogatError::Io(e) => DdbError::Io { msg: e.to_string() },
            DoogatError::Toml(msg) => DdbError::Config { msg },
            DoogatError::Parse(msg) => DdbError::Parse { msg },
            DoogatError::NotFound(msg) => DdbError::NotFound { msg },
            DoogatError::Validation(msg) => DdbError::Validation {
                msg,
                code: None,
                context: empty_error_context(),
            },
            DoogatError::InvalidPath(msg) => DdbError::Validation {
                msg,
                code: None,
                context: empty_error_context(),
            },
            DoogatError::SqlEngine(msg) => DdbError::SqlEngine {
                msg,
                code: None,
                context: empty_error_context(),
            },
            DoogatError::Conflict(msg) => DdbError::Git { msg },
            DoogatError::Sync(msg) => DdbError::Git { msg },
            DoogatError::Index(msg) => DdbError::Sql { msg },
            DoogatError::BadRequest(msg) => DdbError::Validation {
                msg,
                code: None,
                context: empty_error_context(),
            },
            DoogatError::VersionMismatch { repo, driver } => DdbError::VersionMismatch {
                msg: format!("repo format v{repo}, driver supports up to v{driver}"),
            },
            #[cfg(feature = "nosql")]
            DoogatError::Redb(msg) => DdbError::Io { msg },
            DoogatError::Structured {
                code,
                message,
                context,
            } => match code {
                "REFERENCES_VIOLATION" | "CASCADE_CYCLE" => DdbError::SqlEngine {
                    msg: message,
                    code: Some(code.to_string()),
                    context: ffi_error_context(context),
                },
                _ => DdbError::Validation {
                    msg: message,
                    code: Some(code.to_string()),
                    context: ffi_error_context(context),
                },
            },
        }
    }
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
        }
    }
}

/// FFI-safe paginated search result.
#[derive(uniffi::Record)]
pub struct PaginatedSearchResult {
    pub hits: Vec<SearchResult>,
    pub total_count: u64,
}

impl From<crate::types::PaginatedSearchResult> for PaginatedSearchResult {
    fn from(r: crate::types::PaginatedSearchResult) -> Self {
        Self {
            hits: r.hits.into_iter().map(Into::into).collect(),
            total_count: r.total_count as u64,
        }
    }
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
