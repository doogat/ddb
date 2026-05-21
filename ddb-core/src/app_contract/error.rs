use crate::error::DoogatError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppErrorCategory {
    NotFound,
    InvalidInput,
    Conflict,
    Internal,
}

#[derive(Debug, Clone)]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub category: AppErrorCategory,
    pub field: Option<String>,
}

impl From<DoogatError> for AppError {
    fn from(err: DoogatError) -> Self {
        let message = err.to_string();
        match err {
            DoogatError::NotFound(_) => AppError {
                code: "NOT_FOUND".into(),
                message,
                category: AppErrorCategory::NotFound,
                field: None,
            },
            DoogatError::Validation(_) => AppError {
                code: "VALIDATION_ERROR".into(),
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
            },
            DoogatError::Parse(_) => AppError {
                code: "PARSE_ERROR".into(),
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
            },
            DoogatError::InvalidPath(_) => AppError {
                code: "INVALID_PATH".into(),
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
            },
            DoogatError::BadRequest(_) => AppError {
                code: "BAD_REQUEST".into(),
                message,
                category: AppErrorCategory::InvalidInput,
                field: None,
            },
            DoogatError::Conflict(_) => AppError {
                code: "CONFLICT".into(),
                message,
                category: AppErrorCategory::Conflict,
                field: None,
            },
            DoogatError::Structured {
                code,
                message: sc_message,
                ..
            } => {
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
                    code: code.into(),
                    message: sc_message,
                    category,
                    field: None,
                }
            }
            _ => AppError {
                code: "INTERNAL_ERROR".into(),
                message,
                category: AppErrorCategory::Internal,
                field: None,
            },
        }
    }
}
