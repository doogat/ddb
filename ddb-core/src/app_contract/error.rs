use crate::error::{codes, DoogatError, ErrorValue};

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
            DoogatError::Structured {
                code,
                context,
                ..
            } => {
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
                                if cols.len() == 1 { Some(cols[0].clone()) } else { None }
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
