use axum::http::StatusCode;
use axum::Json;
use ddb_core::error::DoogatError;

#[derive(serde::Serialize)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
}

pub fn http_error_response(e: DoogatError) -> (StatusCode, Json<ErrorBody>) {
    let status = match &e {
        DoogatError::NotFound(_) => StatusCode::NOT_FOUND,
        DoogatError::Validation(_) | DoogatError::InvalidPath(_) | DoogatError::BadRequest(_) => {
            StatusCode::BAD_REQUEST
        }
        DoogatError::Conflict(_) => StatusCode::CONFLICT,
        DoogatError::SqlEngine(_) => StatusCode::UNPROCESSABLE_ENTITY,
        DoogatError::Structured { code, .. } => match *code {
            ddb_core::error::codes::UNIQUE_VIOLATION
            | ddb_core::error::codes::SINGLETON_VIOLATION
            | ddb_core::error::codes::REFERENCES_VIOLATION
            | ddb_core::error::codes::CASCADE_CYCLE => StatusCode::CONFLICT,
            ddb_core::error::codes::SINGLETON_NOT_FOUND => StatusCode::NOT_FOUND,
            ddb_core::error::codes::NOT_NULL_VIOLATION
            | ddb_core::error::codes::UNKNOWN_FIELD
            | ddb_core::error::codes::TYPE_NOT_REGISTERED => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        },
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };

    let (code, message) = crate::error::classify(&e);
    (
        status,
        Json(ErrorBody {
            error: code.into(),
            message,
        }),
    )
}
