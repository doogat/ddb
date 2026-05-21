use crate::error::DoogatError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppErrorCategory {
    NotFound,
    InvalidInput,
    Conflict,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub code: &'static str,
    pub message: String,
    pub category: AppErrorCategory,
    pub field: Option<String>,
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
            },
            DoogatError::Validation(_) => AppError {
                code: "VALIDATION_ERROR",
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
            },
            DoogatError::Parse(_) => AppError {
                code: "PARSE_ERROR",
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
            },
            DoogatError::InvalidPath(_) => AppError {
                code: "INVALID_PATH",
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
            },
            DoogatError::BadRequest(_) => AppError {
                code: "BAD_REQUEST",
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
            },
            DoogatError::Conflict(_) => AppError {
                code: "CONFLICT",
                message,
                category: AppErrorCategory::Conflict,
                field: None,
            },
            DoogatError::Structured { code, .. } => {
                let category = match code {
                    "SINGLETON_NOT_FOUND" => AppErrorCategory::NotFound,
                    "UNIQUE_VIOLATION"
                    | "SINGLETON_VIOLATION"
                    | "REFERENCES_VIOLATION"
                    | "CASCADE_CYCLE" => AppErrorCategory::Conflict,
                    "NOT_NULL_VIOLATION" | "UNKNOWN_FIELD" | "TYPE_NOT_REGISTERED" => {
                        AppErrorCategory::InvalidInput
                    }
                    _ => AppErrorCategory::Internal,
                };
                AppError {
                    code,
                    message,
                    category,
                    field: None,
                }
            }
            _ => AppError {
                code: "INTERNAL_ERROR",
                message,
                category: AppErrorCategory::Internal,
                field: None,
            },
        }
    }
}
