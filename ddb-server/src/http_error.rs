use axum::http::StatusCode;
use axum::Json;
use ddb_core::app_contract::AppError;
use ddb_core::error::DoogatError;

#[derive(serde::Serialize)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
}

pub fn http_error_response(e: DoogatError) -> (StatusCode, Json<ErrorBody>) {
    let rendering = ddb_core::app_contract::error_rendering(AppError::from(e));
    let status = StatusCode::from_u16(rendering.http_status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorBody { error: rendering.code.into(), message: rendering.message }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddb_core::error::{codes, ErrorValue};

    fn structured(code: &'static str, message: &str) -> DoogatError {
        DoogatError::Structured {
            code,
            message: message.into(),
            context: vec![("table".into(), ErrorValue::String("t".into()))],
        }
    }

    fn respond(e: DoogatError) -> (StatusCode, String, String) {
        let (status, Json(body)) = http_error_response(e);
        (status, body.error, body.message)
    }

    #[test]
    fn sql_error_renders_422_with_message_preserved() {
        let (status, code, msg) = respond(DoogatError::SqlEngine("near SELCT: syntax error".into()));
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(code, "SQL_ERROR");
        assert_eq!(msg, "near SELCT: syntax error");
    }

    #[test]
    fn schema_apply_partial_renders_500_with_message_redacted() {
        let (status, code, msg) = respond(structured(
            codes::SCHEMA_APPLY_PARTIAL,
            "applied [CREATE TABLE a]; failed at CREATE TABLE b: disk full",
        ));
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(code, "SCHEMA_APPLY_PARTIAL");
        assert_eq!(msg, "internal error");
    }

    #[test]
    fn conflict_category_structured_codes_render_409() {
        for code in [
            codes::UNIQUE_VIOLATION,
            codes::SINGLETON_VIOLATION,
            codes::REFERENCES_VIOLATION,
            codes::CASCADE_CYCLE,
            codes::SCHEMA_DESTRUCTIVE_BLOCKED,
        ] {
            let (status, rendered_code, msg) = respond(structured(code, "detail text"));
            assert_eq!(status, StatusCode::CONFLICT, "status for {code}");
            assert_eq!(rendered_code, code);
            assert_eq!(msg, "detail text", "message for {code}");
        }
    }

    #[test]
    fn singleton_not_found_renders_404() {
        let (status, code, msg) = respond(structured(codes::SINGLETON_NOT_FOUND, "no singleton"));
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(code, "SINGLETON_NOT_FOUND");
        assert_eq!(msg, "no singleton");
    }

    #[test]
    fn validation_error_renders_400_with_unprefixed_message() {
        let (status, code, msg) = respond(DoogatError::Validation("title required".into()));
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(code, "VALIDATION_ERROR");
        assert_eq!(msg, "title required");
    }

    #[test]
    fn not_found_renders_404_with_unprefixed_message() {
        let (status, code, msg) = respond(DoogatError::NotFound("doogat 42".into()));
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(code, "NOT_FOUND");
        assert_eq!(msg, "doogat 42");
    }

    #[test]
    fn conflict_renders_409() {
        let (status, code, _) = respond(DoogatError::Conflict("stale".into()));
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(code, "CONFLICT");
    }

    #[test]
    fn internal_variant_renders_500_redacted() {
        let (status, code, msg) = respond(DoogatError::Git("/secret/path".into()));
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(code, "INTERNAL_ERROR");
        assert_eq!(msg, "internal error");
    }
}
