//! Unit tests for the GraphQL error conversions.
//!
//! Split out of `error.rs` to keep that file under the 800-line limit;
//! the tests are unchanged.

use super::*;
use async_graphql::Value;
use ddb_core::app_contract::AppErrorCategory;
use ddb_core::error::ErrorValue;

fn ext(err: &ServerError, key: &str) -> Value {
    let exts = err.extensions.as_ref().expect("extensions should be set");
    exts.get(key).cloned().unwrap_or(Value::Null)
}

#[test]
fn not_found_passes_through() {
    let (code, msg) = classify(&DoogatError::NotFound("doogat not found".into()));
    assert_eq!(code, "NOT_FOUND");
    assert_eq!(msg, "doogat not found");
}

#[test]
fn validation_passes_through() {
    let (code, msg) = classify(&DoogatError::Validation("title required".into()));
    assert_eq!(code, "VALIDATION_ERROR");
    assert_eq!(msg, "title required");
}

#[test]
fn invalid_path_passes_through() {
    let (code, msg) = classify(&DoogatError::InvalidPath("../escape".into()));
    assert_eq!(code, "INVALID_PATH");
    assert_eq!(msg, "../escape");
}

#[test]
fn sql_engine_passes_through() {
    let (code, msg) = classify(&DoogatError::SqlEngine("near SELCT: syntax error".into()));
    assert_eq!(code, "SQL_ERROR");
    assert_eq!(msg, "near SELCT: syntax error");
}

#[test]
fn git_error_redacted() {
    let (code, msg) = classify(&DoogatError::Git("object not found".into()));
    assert_eq!(code, "INTERNAL_ERROR");
    assert_eq!(msg, "internal error");
}

#[test]
fn io_error_redacted() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "secret path");
    let (code, msg) = classify(&DoogatError::Io(io_err));
    assert_eq!(code, "INTERNAL_ERROR");
    assert_eq!(msg, "internal error");
}

#[test]
fn yaml_error_redacted() {
    let (code, msg) = classify(&DoogatError::Yaml("bad yaml at line 3".into()));
    assert_eq!(code, "INTERNAL_ERROR");
    assert_eq!(msg, "internal error");
}

#[test]
fn sql_error_redacted() {
    let (code, msg) = classify(&DoogatError::Sql("table schema leaked".into()));
    assert_eq!(code, "INTERNAL_ERROR");
    assert_eq!(msg, "internal error");
}

#[test]
fn redb_error_redacted() {
    let (code, msg) = classify(&DoogatError::Redb("corrupt table".into()));
    assert_eq!(code, "INTERNAL_ERROR");
    assert_eq!(msg, "internal error");
}

#[test]
fn conflict_passes_through() {
    let (code, msg) = classify(&DoogatError::Conflict(
        "merge conflict in ddb/123.md".into(),
    ));
    assert_eq!(code, "CONFLICT");
    assert_eq!(msg, "merge conflict in ddb/123.md");
}

#[test]
fn bad_request_passes_through() {
    let (code, msg) = classify(&DoogatError::BadRequest("missing title".into()));
    assert_eq!(code, "BAD_REQUEST");
    assert_eq!(msg, "missing title");
}

#[test]
fn sync_error_redacted() {
    let (code, msg) = classify(&DoogatError::Sync("remote fetch failed".into()));
    assert_eq!(code, "INTERNAL_ERROR");
    assert_eq!(msg, "internal error");
}

#[test]
fn index_error_redacted() {
    let (code, msg) = classify(&DoogatError::Index("FTS5 corrupt".into()));
    assert_eq!(code, "INTERNAL_ERROR");
    assert_eq!(msg, "internal error");
}

#[test]
fn version_mismatch_redacted() {
    let (code, msg) = classify(&DoogatError::VersionMismatch {
        repo: 99,
        driver: 1,
    });
    assert_eq!(code, "INTERNAL_ERROR");
    assert_eq!(msg, "internal error");
}

#[test]
fn to_server_error_uses_classification() {
    let err = to_server_error(DoogatError::Git("repo corrupt".into()));
    assert_eq!(err.message, "internal error");

    let err = to_server_error(DoogatError::NotFound("missing".into()));
    assert_eq!(err.message, "missing");
}

#[test]
fn graphql_sql_engine_error_descriptive() {
    let err = to_server_error(DoogatError::SqlEngine(
        "CREATE INDEX not supported: indexes on the materialized cache are rebuilt from doogat data on reindex".into(),
    ));
    assert_eq!(
        err.message,
        "CREATE INDEX not supported: indexes on the materialized cache are rebuilt from doogat data on reindex"
    );
}

#[test]
fn graphql_internal_error_no_details() {
    let err = to_server_error(DoogatError::Git(
        "/Users/secret/.ddb/objects/ab/cdef1234".into(),
    ));
    assert_eq!(err.message, "internal error");
    assert!(!err.message.contains("secret"));
    assert!(!err.message.contains(".ddb"));
}

#[test]
fn graphql_not_found_descriptive() {
    let err = to_server_error(DoogatError::NotFound(
        "doogat 20260319120000 not found".into(),
    ));
    assert_eq!(err.message, "doogat 20260319120000 not found");
}

// --- PRD 00129 §6: structured error codes ---

#[test]
fn structured_not_null_violation_emits_code_and_table_column() {
    let err = to_server_error(DoogatError::not_null_violation("link", "url"));
    assert_eq!(err.message, "NOT NULL constraint violated: link.url");
    assert_eq!(
        ext(&err, "code"),
        Value::String("NOT_NULL_VIOLATION".into())
    );
    assert_eq!(ext(&err, "table"), Value::String("link".into()));
    assert_eq!(ext(&err, "column"), Value::String("url".into()));
}

#[test]
fn structured_unknown_field_emits_code_and_unknown_field() {
    let err = to_server_error(DoogatError::unknown_field("link", "bogus"));
    assert_eq!(err.message, "unknown column: link.bogus");
    assert_eq!(ext(&err, "code"), Value::String("UNKNOWN_FIELD".into()));
    assert_eq!(ext(&err, "unknown_field"), Value::String("bogus".into()));
}

#[test]
fn structured_unique_violation_emits_columns_and_values_lists() {
    let err = to_server_error(DoogatError::unique_violation(
        "category-membership",
        ["link", "category"],
        ["20260416120000", "20260416120001"],
    ));
    assert_eq!(ext(&err, "code"), Value::String("UNIQUE_VIOLATION".into()));
    assert_eq!(
        ext(&err, "columns"),
        Value::List(vec![
            Value::String("link".into()),
            Value::String("category".into()),
        ])
    );
    assert_eq!(
        ext(&err, "values"),
        Value::List(vec![
            Value::String("20260416120000".into()),
            Value::String("20260416120001".into()),
        ])
    );
}

#[test]
fn structured_references_violation_emits_referencing_context() {
    let err = to_server_error(DoogatError::references_violation(
        "20260416120000",
        "link",
        "category-membership",
        "20260416130000",
    ));
    assert_eq!(
        ext(&err, "code"),
        Value::String("REFERENCES_VIOLATION".into())
    );
    assert_eq!(
        ext(&err, "referencing_table"),
        Value::String("category-membership".into())
    );
    assert_eq!(
        ext(&err, "referencing_id"),
        Value::String("20260416130000".into())
    );
}

#[test]
fn structured_type_not_registered_emits_type_field() {
    let err = to_server_error(DoogatError::type_not_registered("widget"));
    assert_eq!(err.message, "type \"widget\" is not a registered typedef");
    assert_eq!(
        ext(&err, "code"),
        Value::String("TYPE_NOT_REGISTERED".into())
    );
    assert_eq!(ext(&err, "type"), Value::String("widget".into()));
}

#[test]
fn structured_cascade_cycle_emits_tables_list() {
    let err = to_server_error(DoogatError::cascade_cycle(["a", "b", "a"]));
    assert_eq!(ext(&err, "code"), Value::String("CASCADE_CYCLE".into()));
    assert_eq!(
        ext(&err, "tables"),
        Value::List(vec![
            Value::String("a".into()),
            Value::String("b".into()),
            Value::String("a".into()),
        ])
    );
}

// PRD 00131: `to_graphql_error` is the load-bearing path because dynamic
// `FieldFuture::new` closures resolve to `Result<_, async_graphql::Error>`,
// and async-graphql's blanket `From<Display>` would silently drop
// `extensions` if a `ServerError` were propagated through `?` instead.
#[test]
fn to_graphql_error_preserves_extensions_on_unique_violation_prd_00131() {
    let err = to_graphql_error(DoogatError::unique_violation(
        "category-membership",
        ["link", "category"],
        ["20260416120000", "20260416120001"],
    ));
    let exts = err.extensions.as_ref().expect("extensions must be set");
    assert_eq!(
        exts.get("code").cloned().unwrap_or(Value::Null),
        Value::String("UNIQUE_VIOLATION".into())
    );
    assert_eq!(
        exts.get("columns").cloned().unwrap_or(Value::Null),
        Value::List(vec![
            Value::String("link".into()),
            Value::String("category".into()),
        ])
    );
}

#[test]
fn to_graphql_error_preserves_extensions_on_not_null_prd_00131() {
    let err = to_graphql_error(DoogatError::not_null_violation("link", "url"));
    let exts = err.extensions.as_ref().expect("extensions must be set");
    assert_eq!(
        exts.get("code").cloned().unwrap_or(Value::Null),
        Value::String("NOT_NULL_VIOLATION".into())
    );
    assert_eq!(
        exts.get("column").cloned().unwrap_or(Value::Null),
        Value::String("url".into())
    );
}

fn ext_err(err: &Error, key: &str) -> Value {
    let exts = err.extensions.as_ref().expect("extensions should be set");
    exts.get(key).cloned().unwrap_or(Value::Null)
}

fn assert_app_err_matches_legacy(
    legacy: DoogatError,
    for_app: DoogatError,
    expected_code: &str,
) {
    let legacy_msg = to_graphql_error(legacy).message;
    let err = to_graphql_error_from_app(AppError::from(for_app));
    assert_eq!(err.message, legacy_msg);
    assert_eq!(ext_err(&err, "code"), Value::String(expected_code.into()));
}

#[test]
fn app_err_safe_variants_match_legacy_message_and_code() {
    assert_app_err_matches_legacy(
        DoogatError::transaction_not_supported("BEGIN"),
        DoogatError::transaction_not_supported("BEGIN"),
        "TRANSACTION_NOT_SUPPORTED",
    );
    assert_app_err_matches_legacy(
        DoogatError::NotFound("doogat 42 not found".into()),
        DoogatError::NotFound("doogat 42 not found".into()),
        "NOT_FOUND",
    );
    assert_app_err_matches_legacy(
        DoogatError::Validation("title required".into()),
        DoogatError::Validation("title required".into()),
        "VALIDATION_ERROR",
    );
    assert_app_err_matches_legacy(
        DoogatError::InvalidPath("../escape".into()),
        DoogatError::InvalidPath("../escape".into()),
        "INVALID_PATH",
    );
    assert_app_err_matches_legacy(
        DoogatError::BadRequest("missing title".into()),
        DoogatError::BadRequest("missing title".into()),
        "BAD_REQUEST",
    );
    assert_app_err_matches_legacy(
        DoogatError::Conflict("merge conflict".into()),
        DoogatError::Conflict("merge conflict".into()),
        "CONFLICT",
    );
}

#[test]
fn app_err_sql_engine_renders_sql_error_with_stripped_prefix_not_redacted() {
    let err = to_graphql_error_from_app(AppError::from(DoogatError::SqlEngine(
        "no such column: bogus".into(),
    )));
    assert_eq!(ext_err(&err, "code"), Value::String("SQL_ERROR".into()));
    assert_eq!(err.message, "no such column: bogus");
    assert!(!err.message.contains("sql engine: "));
}

#[test]
fn app_err_internal_variants_redact_to_internal_error() {
    for (app, label) in [
        (
            AppError::from(DoogatError::Git("/secret/path".into())),
            "Git",
        ),
        (
            AppError::from(DoogatError::Io(std::io::Error::other("filesystem broke"))),
            "Io",
        ),
        (
            AppError::from(DoogatError::Yaml("bad yaml at line 3".into())),
            "Yaml",
        ),
    ] {
        let err = to_graphql_error_from_app(app);
        assert_eq!(
            err.message, "internal error",
            "{label}: message should be redacted"
        );
        assert_eq!(
            ext_err(&err, "code"),
            Value::String("INTERNAL_ERROR".into()),
            "{label}: code should be INTERNAL_ERROR"
        );
    }
}

#[test]
fn app_err_internal_category_exposes_only_code_extension() {
    let app = AppError {
        code: "INTERNAL_ERROR",
        message: "/Users/secret/.ddb/objects/ab/cdef".into(),
        category: AppErrorCategory::Internal,
        field: Some("path".into()),
        details: vec![(
            "path".into(),
            AppErrorDetail::String("/Users/secret/.ddb/objects/ab/cdef".into()),
        )],
    };
    let err = to_graphql_error_from_app(app);
    let exts = err.extensions.as_ref().expect("extensions should be set");
    assert_eq!(err.message, "internal error");
    assert_eq!(
        exts.get("code").cloned().unwrap_or(Value::Null),
        Value::String("INTERNAL_ERROR".into())
    );
    assert!(exts.get("field").is_none());
    assert!(exts.get("path").is_none());
}

#[test]
fn app_err_structured_unique_violation_preserves_code_and_lists() {
    let app = AppError::from(DoogatError::unique_violation(
        "category-membership",
        ["link", "category"],
        ["20260416120000", "20260416120001"],
    ));
    let err = to_graphql_error_from_app(app);
    assert_eq!(
        ext_err(&err, "code"),
        Value::String("UNIQUE_VIOLATION".into())
    );
    assert_eq!(
        ext_err(&err, "columns"),
        Value::List(vec![
            Value::String("link".into()),
            Value::String("category".into()),
        ])
    );
    assert_eq!(
        ext_err(&err, "values"),
        Value::List(vec![
            Value::String("20260416120000".into()),
            Value::String("20260416120001".into()),
        ])
    );
}

#[test]
fn app_err_structured_not_null_violation_preserves_code_and_column() {
    let app = AppError::from(DoogatError::not_null_violation("link", "url"));
    let err = to_graphql_error_from_app(app);
    assert_eq!(
        ext_err(&err, "code"),
        Value::String("NOT_NULL_VIOLATION".into())
    );
    assert_eq!(ext_err(&err, "table"), Value::String("link".into()));
    assert_eq!(ext_err(&err, "column"), Value::String("url".into()));
}

// PRD 00161 (Contract D): `SCHEMA_APPLY_PARTIAL` is the `Internal` app-error
// category, so the GraphQL conversion MUST redact its message to "internal
// error" and expose only the stable `code` — never the partial-apply
// `applied_ops` / `failed_op` context (which is available on CLI/FFI only).
// This pins the documented redaction contract so it cannot silently drift
// into leaking partial-apply internals over the network surface.
#[test]
fn app_err_schema_apply_partial_redacts_message_and_drops_partial_context() {
    let app = AppError {
        code: ddb_core::error::codes::SCHEMA_APPLY_PARTIAL,
        message: "applied [CREATE TABLE a]; failed at CREATE TABLE b: disk full".into(),
        category: AppErrorCategory::Internal,
        field: Some("failed_op".into()),
        details: vec![
            (
                "applied_ops".into(),
                AppErrorDetail::List(vec!["CREATE TABLE a".into()]),
            ),
            (
                "failed_op".into(),
                AppErrorDetail::String("CREATE TABLE b".into()),
            ),
        ],
    };
    let err = to_graphql_error_from_app(app);
    let exts = err.extensions.as_ref().expect("extensions should be set");

    // Message redacted — no raw failure detail leaks to the network surface.
    assert_eq!(err.message, "internal error");
    assert!(!err.message.contains("CREATE TABLE"));
    assert!(!err.message.contains("disk full"));

    // Stable code is still exposed so clients can branch on it.
    assert_eq!(
        exts.get("code").cloned().unwrap_or(Value::Null),
        Value::String("SCHEMA_APPLY_PARTIAL".into())
    );

    // Partial-apply context must NOT leak: no field, no applied_ops, no failed_op.
    assert!(exts.get("field").is_none(), "field must not leak");
    assert!(
        exts.get("applied_ops").is_none(),
        "applied_ops must not leak"
    );
    assert!(exts.get("failed_op").is_none(), "failed_op must not leak");
}

// --- Legacy GraphQL path (`to_graphql_error` / `to_server_error`) must make
// no redaction decision of its own: its output has to agree with the shared
// policy, including dropping the structured context the policy discarded.

/// Every code string in the (private) `ERROR_POLICY` table of
/// `ddb-core/src/app_contract/error.rs`, listed here so the legacy path can
/// be checked against the shared policy across the whole registered code
/// space instead of at a handful of points.
const ALL_POLICY_CODES: &[&str] = &[
    "NOT_FOUND", "VALIDATION_ERROR", "PARSE_ERROR", "INVALID_PATH",
    "BAD_REQUEST", "CONFLICT", "SQL_ERROR", "INTERNAL_ERROR",
    "SINGLETON_NOT_FOUND", "UNIQUE_VIOLATION", "SINGLETON_VIOLATION",
    "REFERENCES_VIOLATION", "CASCADE_CYCLE", "SCHEMA_DESTRUCTIVE_BLOCKED",
    "NOT_NULL_VIOLATION", "UNKNOWN_FIELD", "TYPE_NOT_REGISTERED",
    "TRANSACTION_NOT_SUPPORTED", "INVALID_ZONE", "SCHEMA_APPLY_PARTIAL",
];

/// The policy rows whose category is `Internal`: redaction must hold for
/// each of them, not just for one familiar code string.
const INTERNAL_POLICY_CODES: &[&str] = &["INTERNAL_ERROR", "SCHEMA_APPLY_PARTIAL"];

/// Structured fixture whose message and context keys both look alarming,
/// so redaction cannot be distinguished from message-substring or
/// context-key sniffing on this fixture alone.
fn alarming_structured(code: &'static str) -> DoogatError {
    DoogatError::Structured {
        code,
        message: "applied [CREATE TABLE a]; failed at CREATE TABLE b: /Users/secret/x".into(),
        context: vec![
            (
                "applied_ops".into(),
                ErrorValue::List(vec!["CREATE TABLE a".into()]),
            ),
            (
                "failed_op".into(),
                ErrorValue::String("CREATE TABLE b".into()),
            ),
        ],
    }
}

/// Structured fixture with a benign message and benign context keys: an
/// implementation that sniffs redaction from the message text or from
/// `applied_ops`/`failed_op` key names leaks on this one.
fn structured_with_benign_context(code: &'static str) -> DoogatError {
    DoogatError::Structured {
        code,
        message: "ok".into(),
        context: vec![
            ("note".into(), ErrorValue::String("x".into())),
            (
                "items".into(),
                ErrorValue::List(vec!["a".into(), "b".into()]),
            ),
        ],
    }
}

fn internal_structured_schema_apply_partial() -> DoogatError {
    alarming_structured(ddb_core::error::codes::SCHEMA_APPLY_PARTIAL)
}

/// The only extensions a redacted error may carry: the code, nothing else.
fn only_code_extensions(code: &str) -> async_graphql::ErrorExtensionValues {
    let mut map = async_graphql::ErrorExtensionValues::default();
    map.set("code", code);
    map
}

#[test]
fn legacy_graphql_error_on_internal_structured_code_exposes_only_code() {
    for &code in INTERNAL_POLICY_CODES {
        for fixture in [alarming_structured(code), structured_with_benign_context(code)] {
            let err = to_graphql_error(fixture);
            assert_eq!(err.message, "internal error", "{code}: message");
            assert_eq!(
                err.extensions.as_ref(),
                Some(&only_code_extensions(code)),
                "{code}: extensions must carry the code and nothing else"
            );
        }
    }
}

#[test]
fn legacy_graphql_error_on_unregistered_structured_code_exposes_only_code() {
    for code in ["NOT_A_CODE", "SCHEMA_APPLY_PENDING", "UNREGISTERED_CODE"] {
        for fixture in [alarming_structured(code), structured_with_benign_context(code)] {
            let err = to_graphql_error(fixture);
            assert_eq!(err.message, "internal error", "{code}: message");
            assert!(!err.message.contains("secret"), "{code}: path leak");
            assert_eq!(
                err.extensions.as_ref(),
                Some(&only_code_extensions(code)),
                "{code}: extensions must carry the code and nothing else"
            );
        }
    }
}

#[test]
fn legacy_graphql_error_on_non_internal_code_keeps_context_and_adds_field() {
    let err = to_graphql_error(DoogatError::not_null_violation("link", "url"));

    assert_eq!(err.message, "NOT NULL constraint violated: link.url");
    assert_eq!(
        ext_err(&err, "code"),
        Value::String("NOT_NULL_VIOLATION".into())
    );
    assert_eq!(ext_err(&err, "table"), Value::String("link".into()));
    assert_eq!(ext_err(&err, "column"), Value::String("url".into()));
    assert_eq!(ext_err(&err, "field"), Value::String("url".into()));
}

#[test]
fn legacy_server_error_redacts_internal_structured_code_like_graphql_error() {
    let err = to_server_error(internal_structured_schema_apply_partial());

    assert_eq!(err.message, "internal error");
    assert_eq!(
        ext(&err, "code"),
        Value::String("SCHEMA_APPLY_PARTIAL".into())
    );
    {
        let exts = err.extensions.as_ref().expect("extensions should be set");
        assert!(exts.get("field").is_none(), "field must not leak");
        assert!(
            exts.get("applied_ops").is_none(),
            "applied_ops must not leak"
        );
        assert!(exts.get("failed_op").is_none(), "failed_op must not leak");
    }
    assert_eq!(
        err.extensions,
        to_graphql_error(internal_structured_schema_apply_partial()).extensions,
        "to_server_error must expose the same extensions as to_graphql_error"
    );
}

fn assert_legacy_and_app_paths_agree(make: impl Fn() -> DoogatError, label: &str) {
    let legacy = to_graphql_error(make());
    let from_app = to_graphql_error_from_app(AppError::from(make()));
    assert_eq!(
        legacy.message, from_app.message,
        "{label}: message must agree"
    );
    assert_eq!(
        legacy.extensions, from_app.extensions,
        "{label}: extensions must agree"
    );
}

#[test]
fn legacy_and_app_graphql_paths_agree_on_message_and_extensions() {
    assert_legacy_and_app_paths_agree(
        || DoogatError::not_null_violation("link", "url"),
        "non-internal structured",
    );
    assert_legacy_and_app_paths_agree(
        internal_structured_schema_apply_partial,
        "internal structured",
    );
    assert_legacy_and_app_paths_agree(
        || DoogatError::SqlEngine("no such column: bogus".into()),
        "simple variant",
    );
}

/// Both GraphQL conversions on one error value: the app path reads it by
/// reference first, the legacy path consumes it, so a single fixture (not
/// two separately built ones) is compared.
fn assert_paths_agree(err: DoogatError) {
    let label = err.to_string();
    let from_app = to_graphql_error_from_app(AppError::from(&err));
    let legacy = to_graphql_error(err);
    assert_eq!(
        legacy.message, from_app.message,
        "{label}: message must agree"
    );
    assert_eq!(
        legacy.extensions, from_app.extensions,
        "{label}: extensions must agree"
    );
}

#[test]
fn legacy_and_app_graphql_paths_agree_on_every_policy_code() {
    for &code in ALL_POLICY_CODES {
        assert_paths_agree(structured_with_benign_context(code));
        assert_paths_agree(alarming_structured(code));
    }
}

#[test]
fn legacy_and_app_graphql_paths_agree_on_every_structured_constructor() {
    for err in [
        DoogatError::unique_violation("m", ["a", "b"], ["1", "2"]),
        DoogatError::unique_violation("post", ["slug"], ["x"]),
        DoogatError::references_violation("a", "b", "c", "d"),
        DoogatError::dangling_reference("a", "b", "c", "d"),
        DoogatError::not_null_violation("link", "url"),
        DoogatError::not_null_violation("post", "slug"),
        DoogatError::unknown_field("link", "bogus"),
        DoogatError::type_not_registered("widget"),
        DoogatError::cascade_cycle(["a", "b", "a"]),
        DoogatError::singleton_violation("app_config", "20260510120000"),
        DoogatError::singleton_not_found("app_config"),
        DoogatError::transaction_not_supported("BEGIN"),
    ] {
        assert_paths_agree(err);
    }
}

#[test]
fn legacy_graphql_error_exposes_field_hint_only_where_the_policy_has_one() {
    for (err, expected_field) in [
        (DoogatError::not_null_violation("link", "url"), Some("url")),
        (DoogatError::not_null_violation("post", "slug"), Some("slug")),
        (
            DoogatError::unique_violation("post", ["slug"], ["x"]),
            Some("slug"),
        ),
        (
            DoogatError::unique_violation("m", ["a", "b"], ["1", "2"]),
            None,
        ),
        (DoogatError::unknown_field("link", "bogus"), None),
        (DoogatError::type_not_registered("widget"), None),
        (DoogatError::references_violation("a", "b", "c", "d"), None),
    ] {
        let label = err.to_string();
        let err = to_graphql_error(err);
        let exts = err.extensions.as_ref().expect("extensions should be set");
        match expected_field {
            Some(field) => assert_eq!(
                exts.get("field").cloned().unwrap_or(Value::Null),
                Value::String(field.into()),
                "{label}: field hint"
            ),
            None => assert!(
                exts.get("field").is_none(),
                "{label}: policy gives no field hint, so none may be invented"
            ),
        }
    }
}

#[test]
fn legacy_graphql_error_round_trips_string_and_ordered_list_context() {
    let err = to_graphql_error(DoogatError::Structured {
        code: ddb_core::error::codes::UNIQUE_VIOLATION,
        message: "duplicate row".into(),
        context: vec![
            ("table".into(), ErrorValue::String("link".into())),
            (
                "columns".into(),
                ErrorValue::List(vec!["c1".into(), "c2".into(), "c3".into()]),
            ),
        ],
    });

    assert_eq!(err.message, "duplicate row");
    assert_eq!(ext_err(&err, "table"), Value::String("link".into()));
    assert_eq!(
        ext_err(&err, "columns"),
        Value::List(vec![
            Value::String("c1".into()),
            Value::String("c2".into()),
            Value::String("c3".into()),
        ])
    );
}
