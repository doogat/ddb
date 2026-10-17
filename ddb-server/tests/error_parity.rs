//! Cross-transport error-parity conformance tests.
//!
//! Each sample declares its expected code, user-visible message, HTTP status
//! and `FfiVariant` as literals written here, then drives the same error
//! through all four public rendering entry points: legacy GraphQL
//! (`to_graphql_error`), `AppError` GraphQL (`to_graphql_error_from_app`),
//! REST (`http_error_response`) and NoSQL (`nosql_error`, whose real axum
//! response is parsed). Redacted samples additionally pin that the GraphQL
//! extensions expose `code` and nothing else, so a context leak fails here.

use async_graphql::Value;
use axum::http::StatusCode;
use ddb_core::app_contract::{error_rendering, AppError, FfiVariant};
use ddb_core::error::{codes, DoogatError, ErrorValue};
use ddb_server::error::{to_graphql_error, to_graphql_error_from_app};
use ddb_server::http_error::http_error_response;
use ddb_server::nosql_api::nosql_error;

/// Literal expectations for one sample. Nothing in here is computed by the
/// code under test.
struct Expect {
    code: &'static str,
    message: &'static str,
    status: StatusCode,
    ffi: FfiVariant,
    /// `true` for samples whose details must never reach a client: the GraphQL
    /// extensions must then carry `code` alone (no context keys, no `field`).
    only_code_extensions: bool,
}

fn structured(code: &'static str, message: &str) -> DoogatError {
    DoogatError::Structured {
        code,
        message: message.into(),
        context: vec![("table".into(), ErrorValue::String("t".into()))],
    }
}

fn assert_graphql(err: async_graphql::Error, expect: &Expect, path: &str) {
    assert_eq!(
        err.message, expect.message,
        "{path} message for {}",
        expect.code
    );
    let exts = err
        .extensions
        .as_ref()
        .unwrap_or_else(|| panic!("{path} sets extensions for {}", expect.code));
    assert_eq!(
        exts.get("code").cloned(),
        Some(Value::String(expect.code.into())),
        "{path} extensions.code for {}",
        expect.code
    );
    if expect.only_code_extensions {
        let json = serde_json::to_value(exts).expect("extensions serialize");
        let keys: Vec<&String> = json
            .as_object()
            .expect("extensions are a JSON object")
            .keys()
            .collect();
        assert_eq!(
            keys,
            vec!["code"],
            "{path} must expose only `code` for redacted {}",
            expect.code
        );
    }
}

fn assert_rest(err: DoogatError, expect: &Expect) {
    let (status, body) = http_error_response(err);
    assert_eq!(status, expect.status, "REST status for {}", expect.code);
    assert_eq!(body.0.error, expect.code, "REST code for {}", expect.code);
    assert_eq!(
        body.0.message, expect.message,
        "REST message for {}",
        expect.code
    );
}

/// Drives the NoSQL transport's own entry point. `nosql_error` delegates to the
/// REST helper, so this leg pins the wiring and the serialized body shape, not
/// an independent NoSQL policy: a NoSQL adapter that stopped delegating, or
/// serialized a different body, fails here.
async fn assert_nosql(err: DoogatError, expect: &Expect) {
    let resp = nosql_error(err);
    assert_eq!(
        resp.status(),
        expect.status,
        "NoSQL status for {}",
        expect.code
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("NoSQL error body collects");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("NoSQL error body is JSON");
    assert_eq!(
        json["error"].as_str(),
        Some(expect.code),
        "NoSQL code for {}",
        expect.code
    );
    assert_eq!(
        json["message"].as_str(),
        Some(expect.message),
        "NoSQL message for {}",
        expect.code
    );
}

/// Checks every transport against `expect`'s literals.
async fn assert_parity(make: impl Fn() -> DoogatError, expect: Expect) {
    assert_graphql(to_graphql_error(make()), &expect, "legacy GraphQL");
    assert_graphql(
        to_graphql_error_from_app(AppError::from(make())),
        &expect,
        "AppError GraphQL",
    );
    assert_rest(make(), &expect);
    assert_nosql(make(), &expect).await;
    assert_eq!(
        error_rendering(AppError::from(make())).ffi_variant,
        expect.ffi,
        "FfiVariant for {}",
        expect.code
    );
}

#[tokio::test]
async fn validation_renders_400_validation_everywhere() {
    assert_parity(
        || DoogatError::Validation("title required".into()),
        Expect {
            code: "VALIDATION_ERROR",
            message: "title required",
            status: StatusCode::BAD_REQUEST,
            ffi: FfiVariant::Validation,
            only_code_extensions: true,
        },
    )
    .await;
}

#[tokio::test]
async fn invalid_path_renders_400_validation_everywhere() {
    assert_parity(
        || DoogatError::InvalidPath("../escape".into()),
        Expect {
            code: "INVALID_PATH",
            message: "../escape",
            status: StatusCode::BAD_REQUEST,
            ffi: FfiVariant::Validation,
            only_code_extensions: true,
        },
    )
    .await;
}

#[tokio::test]
async fn bad_request_renders_400_validation_everywhere() {
    assert_parity(
        || DoogatError::BadRequest("missing title".into()),
        Expect {
            code: "BAD_REQUEST",
            message: "missing title",
            status: StatusCode::BAD_REQUEST,
            ffi: FfiVariant::Validation,
            only_code_extensions: true,
        },
    )
    .await;
}

#[tokio::test]
async fn parse_error_renders_422_validation_everywhere() {
    assert_parity(
        || DoogatError::Parse("bad yaml at line 3".into()),
        Expect {
            code: "PARSE_ERROR",
            message: "bad yaml at line 3",
            status: StatusCode::UNPROCESSABLE_ENTITY,
            ffi: FfiVariant::Validation,
            only_code_extensions: true,
        },
    )
    .await;
}

#[tokio::test]
async fn not_found_renders_404_not_found_everywhere() {
    assert_parity(
        || DoogatError::NotFound("doogat 42 not found".into()),
        Expect {
            code: "NOT_FOUND",
            message: "doogat 42 not found",
            status: StatusCode::NOT_FOUND,
            ffi: FfiVariant::NotFound,
            only_code_extensions: true,
        },
    )
    .await;
}

#[tokio::test]
async fn conflict_renders_409_conflict_everywhere() {
    assert_parity(
        || DoogatError::Conflict("merge conflict in ddb/123.md".into()),
        Expect {
            code: "CONFLICT",
            message: "merge conflict in ddb/123.md",
            status: StatusCode::CONFLICT,
            ffi: FfiVariant::Conflict,
            only_code_extensions: true,
        },
    )
    .await;
}

#[tokio::test]
async fn sql_engine_error_renders_422_validation_everywhere() {
    assert_parity(
        || DoogatError::SqlEngine("near SELCT: syntax error".into()),
        Expect {
            code: "SQL_ERROR",
            message: "near SELCT: syntax error",
            status: StatusCode::UNPROCESSABLE_ENTITY,
            ffi: FfiVariant::Validation,
            only_code_extensions: true,
        },
    )
    .await;
}

#[tokio::test]
async fn git_error_is_redacted_to_500_internal_everywhere() {
    assert_parity(
        || DoogatError::Git("/secret/path".into()),
        Expect {
            code: "INTERNAL_ERROR",
            message: "internal error",
            status: StatusCode::INTERNAL_SERVER_ERROR,
            ffi: FfiVariant::Internal,
            only_code_extensions: true,
        },
    )
    .await;
}

#[tokio::test]
async fn schema_apply_partial_is_redacted_to_500_internal_everywhere() {
    assert_parity(
        || {
            structured(
                codes::SCHEMA_APPLY_PARTIAL,
                "applied [CREATE TABLE a]; failed at CREATE TABLE b: disk full",
            )
        },
        Expect {
            code: codes::SCHEMA_APPLY_PARTIAL,
            message: "internal error",
            status: StatusCode::INTERNAL_SERVER_ERROR,
            ffi: FfiVariant::Internal,
            only_code_extensions: true,
        },
    )
    .await;
}

#[tokio::test]
async fn schema_destructive_blocked_renders_409_conflict_everywhere() {
    assert_parity(
        || {
            structured(
                codes::SCHEMA_DESTRUCTIVE_BLOCKED,
                "plan contains destructive ops",
            )
        },
        Expect {
            code: codes::SCHEMA_DESTRUCTIVE_BLOCKED,
            message: "plan contains destructive ops",
            status: StatusCode::CONFLICT,
            ffi: FfiVariant::Conflict,
            only_code_extensions: false,
        },
    )
    .await;
}

#[tokio::test]
async fn unique_violation_renders_409_conflict_everywhere() {
    assert_parity(
        || structured(codes::UNIQUE_VIOLATION, "duplicate key"),
        Expect {
            code: codes::UNIQUE_VIOLATION,
            message: "duplicate key",
            status: StatusCode::CONFLICT,
            ffi: FfiVariant::Conflict,
            only_code_extensions: false,
        },
    )
    .await;
}

#[tokio::test]
async fn singleton_not_found_renders_404_not_found_everywhere() {
    assert_parity(
        || structured(codes::SINGLETON_NOT_FOUND, "no singleton"),
        Expect {
            code: codes::SINGLETON_NOT_FOUND,
            message: "no singleton",
            status: StatusCode::NOT_FOUND,
            ffi: FfiVariant::NotFound,
            only_code_extensions: false,
        },
    )
    .await;
}

#[tokio::test]
async fn transaction_not_supported_renders_422_validation_everywhere() {
    assert_parity(
        || {
            structured(
                codes::TRANSACTION_NOT_SUPPORTED,
                "BEGIN is not supported over this transport",
            )
        },
        Expect {
            code: codes::TRANSACTION_NOT_SUPPORTED,
            message: "BEGIN is not supported over this transport",
            status: StatusCode::UNPROCESSABLE_ENTITY,
            ffi: FfiVariant::Validation,
            only_code_extensions: false,
        },
    )
    .await;
}

#[tokio::test]
async fn invalid_zone_renders_422_validation_everywhere() {
    assert_parity(
        || structured(codes::INVALID_ZONE, "unknown zone `middle`"),
        Expect {
            code: codes::INVALID_ZONE,
            message: "unknown zone `middle`",
            status: StatusCode::UNPROCESSABLE_ENTITY,
            ffi: FfiVariant::Validation,
            only_code_extensions: false,
        },
    )
    .await;
}

#[tokio::test]
async fn unregistered_code_keeps_its_code_but_is_redacted_everywhere() {
    assert_parity(
        || structured("SOME_UNREGISTERED_CODE", "leaky /secret/detail"),
        Expect {
            code: "SOME_UNREGISTERED_CODE",
            message: "internal error",
            status: StatusCode::INTERNAL_SERVER_ERROR,
            ffi: FfiVariant::Internal,
            only_code_extensions: true,
        },
    )
    .await;
}
