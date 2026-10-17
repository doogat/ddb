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

impl From<DoogatError> for AppError {
    fn from(err: DoogatError) -> Self {
        let message = err.to_string();
        match err {
            DoogatError::NotFound(_) => AppError {
                code: "NOT_FOUND",
                message,
                category: AppErrorCategory::NotFound,
                field: None,
                details: vec![],
            },
            DoogatError::Validation(_) => AppError {
                code: "VALIDATION_ERROR",
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
                details: vec![],
            },
            DoogatError::Parse(_) => AppError {
                code: "PARSE_ERROR",
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
                details: vec![],
            },
            DoogatError::InvalidPath(_) => AppError {
                code: "INVALID_PATH",
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
                details: vec![],
            },
            DoogatError::BadRequest(_) => AppError {
                code: "BAD_REQUEST",
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
                details: vec![],
            },
            DoogatError::Conflict(_) => AppError {
                code: "CONFLICT",
                message,
                category: AppErrorCategory::Conflict,
                field: None,
                details: vec![],
            },
            DoogatError::SqlEngine(_) => AppError {
                code: codes::SQL_ERROR,
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
                details: vec![],
            },
            DoogatError::Structured { code, context, .. } => {
                let category = match code {
                    codes::SINGLETON_NOT_FOUND => AppErrorCategory::NotFound,
                    codes::UNIQUE_VIOLATION
                    | codes::SINGLETON_VIOLATION
                    | codes::REFERENCES_VIOLATION
                    | codes::CASCADE_CYCLE
                    | codes::SCHEMA_DESTRUCTIVE_BLOCKED => AppErrorCategory::Conflict,
                    codes::NOT_NULL_VIOLATION
                    | codes::UNKNOWN_FIELD
                    | codes::TYPE_NOT_REGISTERED
                    | codes::TRANSACTION_NOT_SUPPORTED => AppErrorCategory::InvalidInput,
                    codes::SCHEMA_APPLY_PARTIAL => AppErrorCategory::Internal,
                    _ => AppErrorCategory::Internal,
                };
                let details: Vec<(String, AppErrorDetail)> = context
                    .into_iter()
                    .map(|(k, v)| (k, AppErrorDetail::from(v)))
                    .collect();
                let field = match code {
                    codes::NOT_NULL_VIOLATION => details
                        .iter()
                        .find(|(k, _)| k == "column")
                        .and_then(|(_, v)| {
                            if let AppErrorDetail::String(s) = v {
                                Some(s.clone())
                            } else {
                                None
                            }
                        }),
                    codes::UNIQUE_VIOLATION => details
                        .iter()
                        .find(|(k, _)| k == "columns")
                        .and_then(|(_, v)| {
                            if let AppErrorDetail::List(cols) = v {
                                if cols.len() == 1 {
                                    Some(cols[0].clone())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }),
                    _ => None,
                };
                AppError {
                    code,
                    message,
                    category,
                    field,
                    details,
                }
            }
            _ => AppError {
                code: "INTERNAL_ERROR",
                message,
                category: AppErrorCategory::Internal,
                field: None,
                details: vec![],
            },
        }
    }
}

/// Borrowing mirror of `From<DoogatError>`: `DoogatError` is not `Clone`, so
/// classification that must keep the error borrowed clones only the payload
/// fields each arm needs. Keep the arms in lockstep with the by-value impl.
impl From<&DoogatError> for AppError {
    fn from(err: &DoogatError) -> Self {
        let message = err.to_string();
        match err {
            DoogatError::NotFound(_) => AppError {
                code: "NOT_FOUND",
                message,
                category: AppErrorCategory::NotFound,
                field: None,
                details: vec![],
            },
            DoogatError::Validation(_) => AppError {
                code: "VALIDATION_ERROR",
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
                details: vec![],
            },
            DoogatError::Parse(_) => AppError {
                code: "PARSE_ERROR",
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
                details: vec![],
            },
            DoogatError::InvalidPath(_) => AppError {
                code: "INVALID_PATH",
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
                details: vec![],
            },
            DoogatError::BadRequest(_) => AppError {
                code: "BAD_REQUEST",
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
                details: vec![],
            },
            DoogatError::Conflict(_) => AppError {
                code: "CONFLICT",
                message,
                category: AppErrorCategory::Conflict,
                field: None,
                details: vec![],
            },
            DoogatError::SqlEngine(_) => AppError {
                code: codes::SQL_ERROR,
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
                details: vec![],
            },
            DoogatError::Structured { code, context, .. } => {
                let code: &'static str = code;
                let category = match code {
                    codes::SINGLETON_NOT_FOUND => AppErrorCategory::NotFound,
                    codes::UNIQUE_VIOLATION
                    | codes::SINGLETON_VIOLATION
                    | codes::REFERENCES_VIOLATION
                    | codes::CASCADE_CYCLE
                    | codes::SCHEMA_DESTRUCTIVE_BLOCKED => AppErrorCategory::Conflict,
                    codes::NOT_NULL_VIOLATION
                    | codes::UNKNOWN_FIELD
                    | codes::TYPE_NOT_REGISTERED
                    | codes::TRANSACTION_NOT_SUPPORTED => AppErrorCategory::InvalidInput,
                    codes::SCHEMA_APPLY_PARTIAL => AppErrorCategory::Internal,
                    _ => AppErrorCategory::Internal,
                };
                let details: Vec<(String, AppErrorDetail)> = context
                    .clone()
                    .into_iter()
                    .map(|(k, v)| (k, AppErrorDetail::from(v)))
                    .collect();
                let field = match code {
                    codes::NOT_NULL_VIOLATION => details
                        .iter()
                        .find(|(k, _)| k == "column")
                        .and_then(|(_, v)| {
                            if let AppErrorDetail::String(s) = v {
                                Some(s.clone())
                            } else {
                                None
                            }
                        }),
                    codes::UNIQUE_VIOLATION => details
                        .iter()
                        .find(|(k, _)| k == "columns")
                        .and_then(|(_, v)| {
                            if let AppErrorDetail::List(cols) = v {
                                if cols.len() == 1 {
                                    Some(cols[0].clone())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }),
                    _ => None,
                };
                AppError {
                    code,
                    message,
                    category,
                    field,
                    details,
                }
            }
            _ => AppError {
                code: "INTERNAL_ERROR",
                message,
                category: AppErrorCategory::Internal,
                field: None,
                details: vec![],
            },
        }
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
///
/// AppError.message is DoogatError::to_string(), which for every simple
/// variant carries a thiserror Display prefix ("sql engine: ", "not found: ",
/// "validation: ", "invalid path: ", "conflict: ", "bad request: ",
/// "parse: " - ddb-core/src/error.rs's #[error(...)] attributes). Legacy
/// classify never carried this prefix (it matches DoogatError variants
/// directly and clones the inner String), so this is the one place that
/// strips it before any transport sees the message.
pub fn error_rendering(app: AppError) -> ErrorRendering {
    let AppError { code, message, category, field, details } = app;
    let prefix = match code {
        "NOT_FOUND" => Some("not found: "),
        "VALIDATION_ERROR" => Some("validation: "),
        "INVALID_PATH" => Some("invalid path: "),
        "CONFLICT" => Some("conflict: "),
        "BAD_REQUEST" => Some("bad request: "),
        "SQL_ERROR" => Some("sql engine: "),
        "PARSE_ERROR" => Some("parse: "),
        _ => None,
    };
    let message = match prefix {
        Some(p) => message.strip_prefix(p).unwrap_or(&message).to_string(),
        None => message,
    };
    let is_internal = matches!(category, AppErrorCategory::Internal);
    let http_status: u16 = match code {
        "SQL_ERROR" | "NOT_NULL_VIOLATION" | "UNKNOWN_FIELD" | "TYPE_NOT_REGISTERED" => 422,
        "VALIDATION_ERROR" => 400,
        _ => match category {
            AppErrorCategory::NotFound => 404,
            AppErrorCategory::Conflict => 409,
            AppErrorCategory::Internal => 500,
            AppErrorCategory::InvalidInput => 422,
        },
    };
    let ffi_variant = FfiVariant::from(category.clone());
    if is_internal {
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
mod tests {
    // The conversion tests below use fully-qualified paths (`crate::error::...`,
    // `crate::app_contract::...`); the rendering-policy tests import the
    // policy items from this module.
    use super::{error_rendering, AppError, AppErrorCategory, ErrorRendering, FfiVariant};
    use crate::error::{codes, DoogatError, ErrorValue};

    fn render(err: DoogatError) -> ErrorRendering {
        error_rendering(AppError::from(err))
    }

    /// Asserts code, category, status and FFI hint, plus the redaction rule:
    /// Internal renders "internal error" with no field/details; everything
    /// else keeps the (prefix-stripped) message.
    fn assert_rendering(
        err: DoogatError,
        code: &str,
        category: AppErrorCategory,
        http_status: u16,
        ffi_variant: FfiVariant,
        message: &str,
    ) {
        let r = render(err);
        assert_eq!(r.code, code, "code for {code}");
        assert_eq!(r.category, category, "category for {code}");
        assert_eq!(r.http_status, http_status, "status for {code}");
        assert_eq!(r.ffi_variant, ffi_variant, "ffi hint for {code}");
        assert_eq!(r.message, message, "message for {code}");
        if category == AppErrorCategory::Internal {
            assert_eq!(r.field, None, "redacted field for {code}");
            assert!(r.details.is_empty(), "redacted details for {code}");
        }
    }

    fn structured(code: &'static str, message: &str) -> DoogatError {
        DoogatError::Structured {
            code,
            message: message.into(),
            context: vec![("table".into(), ErrorValue::String("t".into()))],
        }
    }

    #[test]
    fn simple_variants_render_with_category_status_ffi_hint_and_stripped_prefix() {
        use AppErrorCategory as C;
        use FfiVariant as F;
        let cases = [
            (DoogatError::NotFound("doc 1".into()), "NOT_FOUND", C::NotFound, 404, F::NotFound),
            (DoogatError::Validation("bad id".into()), "VALIDATION_ERROR", C::InvalidInput, 400, F::Validation),
            (DoogatError::Parse("bad yaml".into()), "PARSE_ERROR", C::InvalidInput, 422, F::Validation),
            (DoogatError::InvalidPath("../x".into()), "INVALID_PATH", C::InvalidInput, 422, F::Validation),
            (DoogatError::BadRequest("no body".into()), "BAD_REQUEST", C::InvalidInput, 422, F::Validation),
            (DoogatError::Conflict("stale".into()), "CONFLICT", C::Conflict, 409, F::Conflict),
            (DoogatError::SqlEngine("no such column: bogus".into()), "SQL_ERROR", C::InvalidInput, 422, F::Validation),
        ];
        let messages = ["doc 1", "bad id", "bad yaml", "../x", "no body", "stale", "no such column: bogus"];
        for ((err, code, cat, status, ffi), msg) in cases.into_iter().zip(messages) {
            assert_rendering(err, code, cat, status, ffi, msg);
        }
    }

    #[test]
    fn structured_codes_render_with_category_status_and_ffi_hint() {
        use AppErrorCategory as C;
        use FfiVariant as F;
        let cases = [
            (codes::SINGLETON_NOT_FOUND, C::NotFound, 404, F::NotFound),
            (codes::UNIQUE_VIOLATION, C::Conflict, 409, F::Conflict),
            (codes::SINGLETON_VIOLATION, C::Conflict, 409, F::Conflict),
            (codes::REFERENCES_VIOLATION, C::Conflict, 409, F::Conflict),
            (codes::CASCADE_CYCLE, C::Conflict, 409, F::Conflict),
            (codes::SCHEMA_DESTRUCTIVE_BLOCKED, C::Conflict, 409, F::Conflict),
            (codes::NOT_NULL_VIOLATION, C::InvalidInput, 422, F::Validation),
            (codes::UNKNOWN_FIELD, C::InvalidInput, 422, F::Validation),
            (codes::TYPE_NOT_REGISTERED, C::InvalidInput, 422, F::Validation),
            (codes::TRANSACTION_NOT_SUPPORTED, C::InvalidInput, 422, F::Validation),
        ];
        for (code, cat, status, ffi) in cases {
            assert_rendering(structured(code, "detail text"), code, cat, status, ffi, "detail text");
        }
    }

    #[test]
    fn non_internal_structured_rendering_keeps_details() {
        let r = render(structured(codes::SCHEMA_DESTRUCTIVE_BLOCKED, "blocked"));
        assert_eq!(
            r.details,
            vec![("table".to_string(), super::AppErrorDetail::String("t".into()))]
        );
    }

    #[test]
    fn not_null_violation_rendering_keeps_field() {
        let r = render(DoogatError::not_null_violation("link", "url"));
        assert_eq!(r.field.as_deref(), Some("url"));
        assert_eq!(r.message, "NOT NULL constraint violated: link.url");
    }

    #[test]
    fn schema_apply_partial_renders_internal_500_redacted() {
        let r = render(structured(codes::SCHEMA_APPLY_PARTIAL, "halted after 2 of 3"));
        assert_eq!(r.code, "SCHEMA_APPLY_PARTIAL");
        assert_eq!(r.category, AppErrorCategory::Internal);
        assert_eq!(r.http_status, 500);
        assert_eq!(r.ffi_variant, FfiVariant::Internal);
        assert_eq!(r.message, "internal error");
        assert_eq!(r.field, None);
        assert!(r.details.is_empty());
    }

    #[test]
    fn internal_variants_render_500_redacted() {
        assert_rendering(
            DoogatError::Git("secret repo path /home/x".into()),
            "INTERNAL_ERROR",
            AppErrorCategory::Internal,
            500,
            FfiVariant::Internal,
            "internal error",
        );
        assert_rendering(
            structured("SOME_UNREGISTERED_CODE", "leaky detail"),
            "SOME_UNREGISTERED_CODE",
            AppErrorCategory::Internal,
            500,
            FfiVariant::Internal,
            "internal error",
        );
    }

    #[test]
    fn only_outer_display_prefix_is_stripped_not_matching_inner_text() {
        let r = render(DoogatError::SqlEngine("sql engine: inner detail".into()));
        assert_eq!(r.message, "sql engine: inner detail");
        let r = render(DoogatError::NotFound("not found: nested".into()));
        assert_eq!(r.message, "not found: nested");
    }

    #[test]
    fn ffi_variant_maps_each_category() {
        assert_eq!(FfiVariant::from(AppErrorCategory::NotFound), FfiVariant::NotFound);
        assert_eq!(FfiVariant::from(AppErrorCategory::InvalidInput), FfiVariant::Validation);
        assert_eq!(FfiVariant::from(AppErrorCategory::Conflict), FfiVariant::Conflict);
        assert_eq!(FfiVariant::from(AppErrorCategory::Internal), FfiVariant::Internal);
    }

    #[test]
    fn borrowed_and_owned_conversions_agree() {
        let make: [fn() -> DoogatError; 4] = [
            || DoogatError::NotFound("doc 1".into()),
            || DoogatError::unique_violation("link", ["url"], ["https://x"]),
            || DoogatError::Git("boom".into()),
            || structured(codes::SCHEMA_APPLY_PARTIAL, "halted"),
        ];
        for f in make {
            let err = f();
            let borrowed = AppError::from(&err);
            assert_eq!(borrowed, AppError::from(err));
        }
    }

    #[test]
    fn schema_apply_partial_code_constant_has_exact_value() {
        assert_eq!(
            crate::error::codes::SCHEMA_APPLY_PARTIAL,
            "SCHEMA_APPLY_PARTIAL"
        );
    }

    #[test]
    fn schema_destructive_blocked_code_constant_has_exact_value() {
        assert_eq!(
            crate::error::codes::SCHEMA_DESTRUCTIVE_BLOCKED,
            "SCHEMA_DESTRUCTIVE_BLOCKED"
        );
    }

    #[test]
    fn schema_unsupported_change_warning_code_constant_has_exact_value() {
        assert_eq!(
            crate::app_contract::SCHEMA_UNSUPPORTED_CHANGE,
            "SCHEMA_UNSUPPORTED_CHANGE"
        );
    }

    #[test]
    fn destructive_blocked_maps_to_conflict_category_with_code_and_message() {
        let err = crate::error::DoogatError::Structured {
            code: crate::error::codes::SCHEMA_DESTRUCTIVE_BLOCKED,
            message: "plan blocked: risky operations require override".into(),
            context: vec![],
        };
        let app: crate::app_contract::AppError = err.into();
        assert_eq!(app.code, "SCHEMA_DESTRUCTIVE_BLOCKED");
        assert_eq!(
            app.category,
            crate::app_contract::AppErrorCategory::Conflict
        );
        assert_eq!(
            app.message,
            "plan blocked: risky operations require override"
        );
    }

    #[test]
    fn destructive_blocked_category_is_code_driven_not_message_driven() {
        // Same code, a second distinct message with none of the
        // "destructive"/"drop"/"rename" theme words. The category must derive
        // from `code` (-> Conflict), never from message prose, so an impl that
        // sniffs the message instead of matching the code is rejected here.
        let err = crate::error::DoogatError::Structured {
            code: crate::error::codes::SCHEMA_DESTRUCTIVE_BLOCKED,
            message: "request requires the override flag".into(),
            context: vec![],
        };
        let app: crate::app_contract::AppError = err.into();
        assert_eq!(
            app.category,
            crate::app_contract::AppErrorCategory::Conflict
        );
        assert_eq!(app.message, "request requires the override flag");
    }

    #[test]
    fn apply_partial_maps_to_internal_category_with_code_and_message() {
        let err = crate::error::DoogatError::Structured {
            code: crate::error::codes::SCHEMA_APPLY_PARTIAL,
            message: "halted after 5 of 9 operations".into(),
            context: vec![],
        };
        let app: crate::app_contract::AppError = err.into();
        assert_eq!(app.code, "SCHEMA_APPLY_PARTIAL");
        assert_eq!(
            app.category,
            crate::app_contract::AppErrorCategory::Internal
        );
        assert_eq!(app.message, "halted after 5 of 9 operations");
    }

    #[test]
    fn destructive_blocked_carries_context_into_details_in_order_with_matching_variants() {
        let err = crate::error::DoogatError::Structured {
            code: crate::error::codes::SCHEMA_DESTRUCTIVE_BLOCKED,
            message: "schema plan contains destructive operations".into(),
            context: vec![
                (
                    "table".into(),
                    crate::error::ErrorValue::String("project".into()),
                ),
                (
                    "operations".into(),
                    crate::error::ErrorValue::List(vec![
                        "DROP COLUMN status".into(),
                        "DROP COLUMN owner".into(),
                    ]),
                ),
            ],
        };
        let app: crate::app_contract::AppError = err.into();
        assert_eq!(
            app.details,
            vec![
                (
                    "table".to_string(),
                    crate::app_contract::AppErrorDetail::String("project".into()),
                ),
                (
                    "operations".to_string(),
                    crate::app_contract::AppErrorDetail::List(vec![
                        "DROP COLUMN status".into(),
                        "DROP COLUMN owner".into(),
                    ]),
                ),
            ]
        );
    }

    #[test]
    fn apply_partial_carries_context_into_details_in_order_with_matching_variants() {
        let err = crate::error::DoogatError::Structured {
            code: crate::error::codes::SCHEMA_APPLY_PARTIAL,
            message: "schema apply completed 2 of 3 operations".into(),
            context: vec![
                (
                    "failed_at".into(),
                    crate::error::ErrorValue::String("ADD COLUMN due_date".into()),
                ),
                (
                    "applied".into(),
                    crate::error::ErrorValue::List(vec![
                        "ADD COLUMN status".into(),
                        "ADD COLUMN owner".into(),
                    ]),
                ),
            ],
        };
        let app: crate::app_contract::AppError = err.into();
        assert_eq!(
            app.details,
            vec![
                (
                    "failed_at".to_string(),
                    crate::app_contract::AppErrorDetail::String("ADD COLUMN due_date".into()),
                ),
                (
                    "applied".to_string(),
                    crate::app_contract::AppErrorDetail::List(vec![
                        "ADD COLUMN status".into(),
                        "ADD COLUMN owner".into(),
                    ]),
                ),
            ]
        );
    }

    #[test]
    fn sql_engine_error_maps_to_sql_error_code_with_invalid_input_category() {
        let err = crate::error::DoogatError::SqlEngine("no such column: bogus".into());
        let app: crate::app_contract::AppError = err.into();
        assert_eq!(app.code, "SQL_ERROR");
        assert_eq!(
            app.category,
            crate::app_contract::AppErrorCategory::InvalidInput
        );
    }

    #[test]
    fn schema_unsupported_change_usable_as_app_warning_code() {
        let warning = crate::app_contract::AppWarning {
            code: crate::app_contract::SCHEMA_UNSUPPORTED_CHANGE,
            message: "altering column type is not supported".into(),
        };
        assert_eq!(warning.code, "SCHEMA_UNSUPPORTED_CHANGE");
    }
}
