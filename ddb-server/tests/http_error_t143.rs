use axum::http::StatusCode;
use ddb_core::error::{codes, DoogatError};
use ddb_server::http_error::http_error_response;

// ── simple variants ──────────────────────────────────────────────────────────

#[test]
fn not_found_maps_to_404() {
    let (status, body) = http_error_response(DoogatError::NotFound("x".into()));
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body.0.error, "NOT_FOUND");
}

#[test]
fn validation_maps_to_400() {
    let (status, body) = http_error_response(DoogatError::Validation("x".into()));
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body.0.error, "VALIDATION_ERROR");
}

#[test]
fn invalid_path_maps_to_400() {
    let (status, body) = http_error_response(DoogatError::InvalidPath("x".into()));
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body.0.error, "INVALID_PATH");
}

#[test]
fn bad_request_maps_to_400() {
    let (status, body) = http_error_response(DoogatError::BadRequest("x".into()));
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body.0.error, "BAD_REQUEST");
}

#[test]
fn conflict_maps_to_409() {
    let (status, body) = http_error_response(DoogatError::Conflict("x".into()));
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body.0.error, "CONFLICT");
}

#[test]
fn sql_engine_maps_to_422() {
    let (status, body) = http_error_response(DoogatError::SqlEngine("x".into()));
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body.0.error, "SQL_ERROR");
}

// ── Structured variants ──────────────────────────────────────────────────────

#[test]
fn unique_violation_structured_maps_to_409() {
    let e = DoogatError::Structured {
        code: codes::UNIQUE_VIOLATION,
        message: "x".into(),
        context: vec![],
    };
    let (status, body) = http_error_response(e);
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body.0.error, "UNIQUE_VIOLATION");
}

#[test]
fn singleton_violation_structured_maps_to_409() {
    let e = DoogatError::Structured {
        code: codes::SINGLETON_VIOLATION,
        message: "x".into(),
        context: vec![],
    };
    let (status, body) = http_error_response(e);
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body.0.error, "SINGLETON_VIOLATION");
}

#[test]
fn references_violation_structured_maps_to_409() {
    let e = DoogatError::Structured {
        code: codes::REFERENCES_VIOLATION,
        message: "x".into(),
        context: vec![],
    };
    let (status, body) = http_error_response(e);
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body.0.error, "REFERENCES_VIOLATION");
}

#[test]
fn cascade_cycle_structured_maps_to_409() {
    let e = DoogatError::Structured {
        code: codes::CASCADE_CYCLE,
        message: "x".into(),
        context: vec![],
    };
    let (status, body) = http_error_response(e);
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body.0.error, "CASCADE_CYCLE");
}

#[test]
fn singleton_not_found_structured_maps_to_404() {
    let e = DoogatError::Structured {
        code: codes::SINGLETON_NOT_FOUND,
        message: "x".into(),
        context: vec![],
    };
    let (status, body) = http_error_response(e);
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body.0.error, "SINGLETON_NOT_FOUND");
}

#[test]
fn not_null_violation_structured_maps_to_422() {
    let e = DoogatError::Structured {
        code: codes::NOT_NULL_VIOLATION,
        message: "x".into(),
        context: vec![],
    };
    let (status, body) = http_error_response(e);
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body.0.error, "NOT_NULL_VIOLATION");
}

#[test]
fn unknown_field_structured_maps_to_422() {
    let e = DoogatError::Structured {
        code: codes::UNKNOWN_FIELD,
        message: "x".into(),
        context: vec![],
    };
    let (status, body) = http_error_response(e);
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body.0.error, "UNKNOWN_FIELD");
}

#[test]
fn type_not_registered_structured_maps_to_422() {
    let e = DoogatError::Structured {
        code: codes::TYPE_NOT_REGISTERED,
        message: "x".into(),
        context: vec![],
    };
    let (status, body) = http_error_response(e);
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body.0.error, "TYPE_NOT_REGISTERED");
}

#[test]
fn unknown_structured_code_maps_to_422_with_code_passthrough() {
    let e = DoogatError::Structured {
        code: "SOME_UNKNOWN_CODE",
        message: "x".into(),
        context: vec![],
    };
    let (status, body) = http_error_response(e);
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body.0.error, "SOME_UNKNOWN_CODE");
}

// ── PRD 00161 schema-apply structured codes (Contract C) ──────────────────────

#[test]
fn schema_destructive_blocked_structured_maps_to_409() {
    let e = DoogatError::Structured {
        code: codes::SCHEMA_DESTRUCTIVE_BLOCKED,
        message: "x".into(),
        context: vec![],
    };
    let (status, body) = http_error_response(e);
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body.0.error, "SCHEMA_DESTRUCTIVE_BLOCKED");
}

#[test]
fn schema_apply_partial_structured_maps_to_500() {
    // REST passes the SCHEMA_APPLY_PARTIAL message through verbatim: the classify
    // transport path does NOT redact structured-code messages (unlike the GraphQL
    // app_contract path, which redacts Internal-category errors to "internal
    // error"). Pin both the 500 status AND the message passthrough so the
    // documented REST contract (detailed roll-back summary, not "internal error")
    // cannot silently drift, and so the GraphQL/REST divergence stays visible.
    let detail = "schema apply failed after 1 of 2 operations and rolled back: create table failed";
    let e = DoogatError::Structured {
        code: codes::SCHEMA_APPLY_PARTIAL,
        message: detail.into(),
        context: vec![],
    };
    let (status, body) = http_error_response(e);
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body.0.error, "SCHEMA_APPLY_PARTIAL");
    assert_eq!(body.0.message, detail);
}

// ── internal variants → 500 ──────────────────────────────────────────────────

#[test]
fn internal_variant_maps_to_500() {
    let e = DoogatError::Io(std::io::Error::other("x"));
    let (status, body) = http_error_response(e);
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body.0.error, "INTERNAL_ERROR");
}
