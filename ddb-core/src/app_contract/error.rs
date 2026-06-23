use crate::error::{codes, DoogatError, ErrorValue};

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
            DoogatError::Structured { code, context, .. } => {
                let category = match code {
                    codes::SINGLETON_NOT_FOUND => AppErrorCategory::NotFound,
                    codes::UNIQUE_VIOLATION
                    | codes::SINGLETON_VIOLATION
                    | codes::REFERENCES_VIOLATION
                    | codes::CASCADE_CYCLE => AppErrorCategory::Conflict,
                    codes::NOT_NULL_VIOLATION
                    | codes::UNKNOWN_FIELD
                    | codes::TYPE_NOT_REGISTERED => AppErrorCategory::InvalidInput,
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

#[cfg(test)]
mod tests {
    // All references below are fully-qualified (`crate::error::...`,
    // `crate::app_contract::...`) because `super::*` would only bring this
    // module's own items into scope, none of which these tests use.

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
        assert_eq!(app.category, crate::app_contract::AppErrorCategory::Conflict);
        assert_eq!(app.message, "plan blocked: risky operations require override");
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
        assert_eq!(app.category, crate::app_contract::AppErrorCategory::Conflict);
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
        assert_eq!(app.category, crate::app_contract::AppErrorCategory::Internal);
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
    fn schema_unsupported_change_usable_as_app_warning_code() {
        let warning = crate::app_contract::AppWarning {
            code: crate::app_contract::SCHEMA_UNSUPPORTED_CHANGE,
            message: "altering column type is not supported".into(),
        };
        assert_eq!(warning.code, "SCHEMA_UNSUPPORTED_CHANGE");
    }
}
