use crate::error::{codes, DoogatError, ErrorValue};

/// Warning code for a desired-vs-live schema difference that has no targeted
/// DDL path (PRD 00161 §3.4/§3.5). Attached to `AppWarning.code`.
pub const SCHEMA_UNSUPPORTED_CHANGE: &str = "SCHEMA_UNSUPPORTED_CHANGE";

/// Broad classification of an application-layer error, transport-agnostic.
///
/// Transports map these categories to their own status codes or error kinds
/// (e.g. HTTP 404 for `NotFound`, 409 for `Conflict`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppErrorCategory {
    NotFound,
    InvalidInput,
    Conflict,
    Internal,
}

/// Adapter-neutral mirror of [`crate::error::ErrorValue`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppErrorDetail {
    String(String),
    List(Vec<String>),
}

impl From<ErrorValue> for AppErrorDetail {
    fn from(v: ErrorValue) -> Self {
        match v {
            ErrorValue::String(s) => AppErrorDetail::String(s),
            ErrorValue::List(l) => AppErrorDetail::List(l),
        }
    }
}

/// Adapter-neutral application error returned when a command fails.
///
/// `code` is a stable static string for programmatic handling; `category`
/// lets transports map to their own status conventions without inspecting
/// `code`; `field` and `details` carry structured context when available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub code: &'static str,
    pub message: String,
    pub category: AppErrorCategory,
    pub field: Option<String>,
    pub details: Vec<(String, AppErrorDetail)>,
}

/// One row of the single error policy: what an error code implies about its
/// category, its HTTP status, and the thiserror `Display` prefix its message
/// carries (`None` when the code's message has no outer prefix).
struct ErrorPolicy {
    code: &'static str,
    category: AppErrorCategory,
    http_status: u16,
    display_prefix: Option<&'static str>,
}

/// The one table every error-policy decision derives from. Registering a new
/// error code is a single new row here: the conversions read `category`, and
/// [`error_rendering`] reads `http_status` and `display_prefix`. A code absent
/// from the table is unregistered - it classifies `Internal`, takes its status
/// from the category, and keeps its message unstripped (then redacted).
static ERROR_POLICY: &[ErrorPolicy] = &[
    ErrorPolicy {
        code: "NOT_FOUND",
        category: AppErrorCategory::NotFound,
        http_status: 404,
        display_prefix: Some("not found: "),
    },
    ErrorPolicy {
        code: "VALIDATION_ERROR",
        category: AppErrorCategory::InvalidInput,
        http_status: 400,
        display_prefix: Some("validation: "),
    },
    ErrorPolicy {
        code: "PARSE_ERROR",
        category: AppErrorCategory::InvalidInput,
        http_status: 422,
        display_prefix: Some("parse: "),
    },
    ErrorPolicy {
        code: "INVALID_PATH",
        category: AppErrorCategory::InvalidInput,
        http_status: 400,
        display_prefix: Some("invalid path: "),
    },
    ErrorPolicy {
        code: "BAD_REQUEST",
        category: AppErrorCategory::InvalidInput,
        http_status: 400,
        display_prefix: Some("bad request: "),
    },
    ErrorPolicy {
        code: "CONFLICT",
        category: AppErrorCategory::Conflict,
        http_status: 409,
        display_prefix: Some("conflict: "),
    },
    ErrorPolicy {
        code: codes::SQL_ERROR,
        category: AppErrorCategory::InvalidInput,
        http_status: 422,
        display_prefix: Some("sql engine: "),
    },
    ErrorPolicy {
        code: "INTERNAL_ERROR",
        category: AppErrorCategory::Internal,
        http_status: 500,
        display_prefix: None,
    },
    ErrorPolicy {
        code: codes::SINGLETON_NOT_FOUND,
        category: AppErrorCategory::NotFound,
        http_status: 404,
        display_prefix: None,
    },
    ErrorPolicy {
        code: codes::UNIQUE_VIOLATION,
        category: AppErrorCategory::Conflict,
        http_status: 409,
        display_prefix: None,
    },
    ErrorPolicy {
        code: codes::SINGLETON_VIOLATION,
        category: AppErrorCategory::Conflict,
        http_status: 409,
        display_prefix: None,
    },
    ErrorPolicy {
        code: codes::REFERENCES_VIOLATION,
        category: AppErrorCategory::Conflict,
        http_status: 409,
        display_prefix: None,
    },
    ErrorPolicy {
        code: codes::CASCADE_CYCLE,
        category: AppErrorCategory::Conflict,
        http_status: 409,
        display_prefix: None,
    },
    ErrorPolicy {
        code: codes::SCHEMA_DESTRUCTIVE_BLOCKED,
        category: AppErrorCategory::Conflict,
        http_status: 409,
        display_prefix: None,
    },
    ErrorPolicy {
        code: codes::NOT_NULL_VIOLATION,
        category: AppErrorCategory::InvalidInput,
        http_status: 422,
        display_prefix: None,
    },
    ErrorPolicy {
        code: codes::UNKNOWN_FIELD,
        category: AppErrorCategory::InvalidInput,
        http_status: 422,
        display_prefix: None,
    },
    ErrorPolicy {
        code: codes::TYPE_NOT_REGISTERED,
        category: AppErrorCategory::InvalidInput,
        http_status: 422,
        display_prefix: None,
    },
    ErrorPolicy {
        code: codes::TRANSACTION_NOT_SUPPORTED,
        category: AppErrorCategory::InvalidInput,
        http_status: 422,
        display_prefix: None,
    },
    ErrorPolicy {
        code: codes::INVALID_ZONE,
        category: AppErrorCategory::InvalidInput,
        http_status: 422,
        display_prefix: None,
    },
    // Explicitly registered as Internal: a code we know about and chose to
    // treat as internal, unlike an unregistered code that merely falls back.
    ErrorPolicy {
        code: codes::SCHEMA_APPLY_PARTIAL,
        category: AppErrorCategory::Internal,
        http_status: 500,
        display_prefix: None,
    },
];

fn policy_for(code: &str) -> Option<&'static ErrorPolicy> {
    ERROR_POLICY.iter().find(|p| p.code == code)
}

/// The code each `DoogatError` variant carries. Variants with no dedicated
/// code are internal errors, so an unhandled variant can never panic.
fn code_for(err: &DoogatError) -> &'static str {
    match err {
        DoogatError::NotFound(_) => "NOT_FOUND",
        DoogatError::Validation(_) => "VALIDATION_ERROR",
        DoogatError::Parse(_) => "PARSE_ERROR",
        DoogatError::InvalidPath(_) => "INVALID_PATH",
        DoogatError::BadRequest(_) => "BAD_REQUEST",
        DoogatError::Conflict(_) => "CONFLICT",
        DoogatError::SqlEngine(_) => codes::SQL_ERROR,
        DoogatError::Structured { code, .. } => code,
        _ => "INTERNAL_ERROR",
    }
}

/// The structured-context entry a code promotes to `AppError.field`, read
/// from the real context rather than any fixed column name.
fn field_for(code: &str, details: &[(String, AppErrorDetail)]) -> Option<String> {
    let entry = |key: &str| details.iter().find(|(k, _)| k == key).map(|(_, v)| v);
    match code {
        codes::NOT_NULL_VIOLATION => match entry("column") {
            Some(AppErrorDetail::String(s)) => Some(s.clone()),
            _ => None,
        },
        codes::UNIQUE_VIOLATION => match entry("columns") {
            Some(AppErrorDetail::List(cols)) if cols.len() == 1 => Some(cols[0].clone()),
            _ => None,
        },
        _ => None,
    }
}

/// `DoogatError` is not `Clone`, so classification lives on the borrowed
/// impl; the owned one delegates to it. Category comes from [`ERROR_POLICY`].
impl From<&DoogatError> for AppError {
    fn from(err: &DoogatError) -> Self {
        let code = code_for(err);
        let details: Vec<(String, AppErrorDetail)> = match err {
            DoogatError::Structured { context, .. } => context
                .iter()
                .map(|(k, v)| (k.clone(), AppErrorDetail::from(v.clone())))
                .collect(),
            _ => vec![],
        };
        AppError {
            code,
            message: err.to_string(),
            category: policy_for(code).map_or(AppErrorCategory::Internal, |p| p.category.clone()),
            field: field_for(code, &details),
            details,
        }
    }
}

impl From<DoogatError> for AppError {
    fn from(err: DoogatError) -> Self {
        AppError::from(&err)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiVariant {
    NotFound,
    Validation,
    Conflict,
    Internal,
}

impl From<AppErrorCategory> for FfiVariant {
    fn from(c: AppErrorCategory) -> Self {
        match c {
            AppErrorCategory::NotFound => FfiVariant::NotFound,
            AppErrorCategory::InvalidInput => FfiVariant::Validation,
            AppErrorCategory::Conflict => FfiVariant::Conflict,
            AppErrorCategory::Internal => FfiVariant::Internal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorRendering {
    pub code: &'static str,
    pub message: String,
    pub http_status: u16,
    pub category: AppErrorCategory,
    pub field: Option<String>,
    pub details: Vec<(String, AppErrorDetail)>,
    pub ffi_variant: FfiVariant,
}

/// The single code -> (status, redaction, ffi-hint) policy. Every transport
/// adapter (GraphQL, REST, NoSQL HTTP; PgWire/FFI in later work) derives its
/// output from this function and must not re-decide redaction or status.
/// Status and prefix come from the code's [`ERROR_POLICY`] row; the category
/// is the one the caller's `AppError` already carries.
///
/// AppError.message is DoogatError::to_string(), which for every simple
/// variant carries a thiserror Display prefix ("sql engine: ", "not found: ",
/// "validation: ", "invalid path: ", "conflict: ", "bad request: ",
/// "parse: " - ddb-core/src/error.rs's #[error(...)] attributes). Legacy
/// classify never carried this prefix (it matches DoogatError variants
/// directly and clones the inner String), so this is the one place that
/// strips it before any transport sees the message.
///
/// Internal-category errors are logged here via `tracing::error!` with the
/// unredacted message before redaction — the single logging point for every
/// transport (legacy `classify`'s per-variant match used to log on REST/NoSQL
/// directly; that is now centralized here instead of duplicated per caller).
pub fn error_rendering(app: AppError) -> ErrorRendering {
    let AppError { code, message, category, field, details } = app;
    let policy = policy_for(code);
    let message = match policy.and_then(|p| p.display_prefix) {
        Some(p) => message.strip_prefix(p).unwrap_or(&message).to_string(),
        None => message,
    };
    let is_internal = matches!(category, AppErrorCategory::Internal);
    let http_status: u16 = policy.map_or(
        match category {
            AppErrorCategory::NotFound => 404,
            AppErrorCategory::Conflict => 409,
            AppErrorCategory::Internal => 500,
            AppErrorCategory::InvalidInput => 422,
        },
        |p| p.http_status,
    );
    let ffi_variant = FfiVariant::from(category.clone());
    if is_internal {
        tracing::error!(code = %code, message = %message, "internal error");
        ErrorRendering {
            code,
            message: "internal error".to_string(),
            http_status,
            category,
            field: None,
            details: vec![],
            ffi_variant,
        }
    } else {
        ErrorRendering { code, message, http_status, category, field, details, ffi_variant }
    }
}

#[cfg(test)]
mod tests;
