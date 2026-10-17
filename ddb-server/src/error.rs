use async_graphql::{Error, ServerError};
use ddb_core::app_contract::{AppError, AppErrorDetail};
use ddb_core::error::{DoogatError, ErrorContext, ErrorValue};

/// Classify a DoogatError for external exposure.
///
/// Returns `(error_code, user_safe_message)`, derived from the shared
/// `error_rendering` policy. Internal errors are redacted to a generic message.
pub fn classify(e: &DoogatError) -> (&'static str, String) {
    let rendering = ddb_core::app_contract::error_rendering(AppError::from(e));
    (rendering.code, rendering.message)
}

/// Convert a DoogatError to an `async_graphql::Error`, attaching
/// `extensions.code` and any structured per-code context fields
/// (PRD 00129 §6).
///
/// Returning `Error` (not `ServerError`) is load-bearing: dynamic-schema
/// `FieldFuture::new` closures resolve to `Result<_, async_graphql::Error>`
/// and rely on `?` propagating the value-with-extensions directly. Going
/// through `ServerError` here would invoke async-graphql's blanket
/// `impl<T: Display> From<T> for Error` (PRD 00131), which builds a fresh
/// `Error` from `to_string()` and silently drops `extensions`.
pub fn to_graphql_error(e: DoogatError) -> Error {
    let (code, message) = classify(&e);
    let mut err = Error::new(message);
    let mut map = async_graphql::ErrorExtensionValues::default();
    map.set("code", code);
    if let DoogatError::Structured { context, .. } = &e {
        attach_context(&mut map, context);
    }
    err.extensions = Some(map);
    err
}

/// `ServerError` variant of [`to_graphql_error`] for non-resolver call
/// sites (e.g. tests that need a `ServerError` shape). Resolvers must
/// use [`to_graphql_error`] so `extensions` survive the
/// `FieldFuture::new` boundary; see PRD 00131.
pub fn to_server_error(e: DoogatError) -> ServerError {
    let err = to_graphql_error(e);
    ServerError {
        message: err.message,
        source: err.source,
        locations: Vec::new(),
        path: Vec::new(),
        extensions: err.extensions,
    }
}

/// Convert an adapter-neutral `AppError` (the application contract envelope
/// from PRD 00147) into an `async_graphql::Error`.
///
/// Code, message, field and details come from the shared `error_rendering`
/// policy, which strips the Display prefix, redacts internal-category errors
/// to `"internal error"` with no field/details, and logs them centrally
/// (PRD 00129 §6 / PRD 00131 redaction behavior at the AppError boundary).
pub fn to_graphql_error_from_app(app: AppError) -> Error {
    let rendering = ddb_core::app_contract::error_rendering(app);
    let mut err = Error::new(rendering.message);
    let mut map = async_graphql::ErrorExtensionValues::default();
    map.set("code", rendering.code);
    if let Some(field) = rendering.field {
        map.set("field", field);
    }
    for (key, val) in rendering.details {
        let value = match val {
            AppErrorDetail::String(s) => async_graphql::Value::String(s),
            AppErrorDetail::List(items) => async_graphql::Value::List(
                items
                    .into_iter()
                    .map(async_graphql::Value::String)
                    .collect(),
            ),
        };
        map.set(key, value);
    }
    err.extensions = Some(map);
    err
}

/// Copy each `(key, value)` pair from a structured-error context into the
/// GraphQL extensions map, converting `ErrorValue::List` to a JSON array.
fn attach_context(map: &mut async_graphql::ErrorExtensionValues, context: &ErrorContext) {
    use async_graphql::Value;
    for (key, val) in context {
        let value = match val {
            ErrorValue::String(s) => Value::String(s.clone()),
            ErrorValue::List(items) => {
                Value::List(items.iter().map(|s| Value::String(s.clone())).collect())
            }
        };
        map.set(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_graphql::Value;
    use ddb_core::app_contract::AppErrorCategory;

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

    fn internal_structured_schema_apply_partial() -> DoogatError {
        DoogatError::Structured {
            code: ddb_core::error::codes::SCHEMA_APPLY_PARTIAL,
            message: "applied [CREATE TABLE a]; failed at CREATE TABLE b: disk full".into(),
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

    fn unregistered_structured_code() -> DoogatError {
        DoogatError::Structured {
            code: "NOT_A_CODE",
            message: "/Users/secret/.ddb/objects/ab/cdef is unreadable".into(),
            context: vec![
                (
                    "failed_op".into(),
                    ErrorValue::String("/Users/secret/.ddb/objects/ab/cdef".into()),
                ),
                (
                    "applied_ops".into(),
                    ErrorValue::List(vec!["read blob ab/cdef".into()]),
                ),
            ],
        }
    }

    #[test]
    fn legacy_graphql_error_on_internal_structured_code_exposes_only_code() {
        let err = to_graphql_error(internal_structured_schema_apply_partial());
        let exts = err.extensions.as_ref().expect("extensions should be set");

        assert_eq!(err.message, "internal error");
        assert_eq!(
            exts.get("code").cloned().unwrap_or(Value::Null),
            Value::String("SCHEMA_APPLY_PARTIAL".into())
        );
        assert!(exts.get("field").is_none(), "field must not leak");
        assert!(
            exts.get("applied_ops").is_none(),
            "applied_ops must not leak"
        );
        assert!(exts.get("failed_op").is_none(), "failed_op must not leak");
    }

    #[test]
    fn legacy_graphql_error_on_unregistered_structured_code_exposes_only_code() {
        let err = to_graphql_error(unregistered_structured_code());
        let exts = err.extensions.as_ref().expect("extensions should be set");

        assert_eq!(err.message, "internal error");
        assert!(!err.message.contains("secret"));
        assert_eq!(
            exts.get("code").cloned().unwrap_or(Value::Null),
            Value::String("NOT_A_CODE".into())
        );
        assert!(exts.get("field").is_none(), "field must not leak");
        assert!(
            exts.get("applied_ops").is_none(),
            "applied_ops must not leak"
        );
        assert!(exts.get("failed_op").is_none(), "failed_op must not leak");
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
}
