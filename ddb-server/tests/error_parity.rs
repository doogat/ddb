//! Cross-transport error-parity conformance tests (PRD 00173 follow-up).
//!
//! Drives a representative error-code set through all three rendering entry
//! points (`classify` for the legacy GraphQL path, `to_graphql_error_from_app`
//! for the `AppError`-based GraphQL path, and `http_error_response` for REST)
//! and asserts they agree on code, redaction and the documented per-code HTTP
//! status. Also checks that `error_rendering` always resolves to one of the
//! four `FfiVariant` values (the NoSQL/FFI-side contract).

use async_graphql::Value;
use axum::http::StatusCode;
use ddb_core::app_contract::{error_rendering, AppError, FfiVariant};
use ddb_core::error::{codes, DoogatError, ErrorValue};
use ddb_server::error::{classify, to_graphql_error_from_app};
use ddb_server::http_error::http_error_response;

fn structured(code: &'static str, message: &str) -> DoogatError {
    DoogatError::Structured {
        code,
        message: message.into(),
        context: vec![("table".into(), ErrorValue::String("t".into()))],
    }
}

fn graphql_code(err: &async_graphql::Error) -> Value {
    let exts = err.extensions.as_ref().expect("extensions should be set");
    exts.get("code").cloned().unwrap_or(Value::Null)
}

/// Asserts that a `DoogatError` sample renders identically (code + redaction)
/// across GraphQL (`classify`, `to_graphql_error_from_app`) and REST
/// (`http_error_response`), and that the REST status matches the documented
/// per-code status.
fn assert_parity(make: impl Fn() -> DoogatError, expected_status: StatusCode) {
    let (classify_code, classify_msg) = classify(&make());

    let graphql_err = to_graphql_error_from_app(AppError::from(make()));
    let graphql_code_val = graphql_code(&graphql_err);
    assert_eq!(
        graphql_code_val,
        Value::String(classify_code.to_string()),
        "GraphQL code must match classify's code for {classify_code}"
    );
    assert_eq!(
        graphql_err.message, classify_msg,
        "GraphQL message (redaction) must match classify's message for {classify_code}"
    );

    let (status, body) = http_error_response(make());
    assert_eq!(
        status, expected_status,
        "REST status mismatch for {classify_code}"
    );
    assert_eq!(
        body.0.error, classify_code,
        "REST code must match classify's code for {classify_code}"
    );
    assert_eq!(
        body.0.message, classify_msg,
        "REST message (redaction) must match classify's message for {classify_code}"
    );

    // NoSQL/FFI-side contract: error_rendering always resolves to one of the
    // four documented FfiVariant values.
    let rendering = error_rendering(AppError::from(make()));
    match rendering.ffi_variant {
        FfiVariant::NotFound
        | FfiVariant::Validation
        | FfiVariant::Conflict
        | FfiVariant::Internal => {}
    }
}

#[test]
fn validation_parity_across_transports() {
    assert_parity(
        || DoogatError::Validation("title required".into()),
        StatusCode::BAD_REQUEST,
    );
}

#[test]
fn not_found_parity_across_transports() {
    assert_parity(
        || DoogatError::NotFound("doogat 42 not found".into()),
        StatusCode::NOT_FOUND,
    );
}

#[test]
fn conflict_parity_across_transports() {
    assert_parity(
        || DoogatError::Conflict("merge conflict in ddb/123.md".into()),
        StatusCode::CONFLICT,
    );
}

#[test]
fn internal_parity_across_transports() {
    assert_parity(
        || DoogatError::Git("/secret/path".into()),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
}

#[test]
fn sql_error_parity_across_transports() {
    assert_parity(
        || DoogatError::SqlEngine("near SELCT: syntax error".into()),
        StatusCode::UNPROCESSABLE_ENTITY,
    );
}

#[test]
fn schema_apply_partial_parity_across_transports() {
    assert_parity(
        || {
            structured(
                codes::SCHEMA_APPLY_PARTIAL,
                "applied [CREATE TABLE a]; failed at CREATE TABLE b: disk full",
            )
        },
        StatusCode::INTERNAL_SERVER_ERROR,
    );
}

#[test]
fn schema_destructive_blocked_parity_across_transports() {
    assert_parity(
        || {
            structured(
                codes::SCHEMA_DESTRUCTIVE_BLOCKED,
                "plan contains destructive ops",
            )
        },
        StatusCode::CONFLICT,
    );
}

// UNIQUE_VIOLATION and SINGLETON_NOT_FOUND pin that the category-driven
// `error_rendering` rewrite still reproduces the documented 409/404 status
// that today's REST surface already relies on.

#[test]
fn unique_violation_parity_across_transports() {
    assert_parity(
        || structured(codes::UNIQUE_VIOLATION, "duplicate key"),
        StatusCode::CONFLICT,
    );
}

#[test]
fn singleton_not_found_parity_across_transports() {
    assert_parity(
        || structured(codes::SINGLETON_NOT_FOUND, "no singleton"),
        StatusCode::NOT_FOUND,
    );
}
