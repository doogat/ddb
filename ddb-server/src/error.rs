use async_graphql::{Error, ServerError};
use ddb_core::error::{DoogatError, ErrorContext, ErrorValue};

/// Classify a DoogatError for external exposure.
///
/// Returns `(error_code, user_safe_message)`. Internal errors are redacted
/// to a generic message and the original is logged via tracing.
pub fn classify(e: &DoogatError) -> (&'static str, String) {
    match e {
        // Structured errors carry their own code and a user-safe message.
        // PRD 00129 §6.
        DoogatError::Structured { code, message, .. } => (code, message.clone()),
        // Safe to expose — user-actionable
        DoogatError::NotFound(m) => ("NOT_FOUND", m.clone()),
        DoogatError::Validation(m) => ("VALIDATION_ERROR", m.clone()),
        DoogatError::InvalidPath(m) => ("INVALID_PATH", m.clone()),
        DoogatError::Conflict(m) => ("CONFLICT", m.clone()),
        DoogatError::BadRequest(m) => ("BAD_REQUEST", m.clone()),
        // SQL engine errors are user-actionable (unsupported DDL, bad syntax, etc.)
        DoogatError::SqlEngine(m) => ("SQL_ERROR", m.clone()),
        // All other variants are internal — redact completely.
        // Explicit matches so the compiler forces a decision on new variants.
        DoogatError::Git(_)
        | DoogatError::Yaml(_)
        | DoogatError::Sql(_)
        | DoogatError::Automerge(_)
        | DoogatError::Io(_)
        | DoogatError::Toml(_)
        | DoogatError::Parse(_)
        | DoogatError::Sync(_)
        | DoogatError::Index(_)
        | DoogatError::VersionMismatch { .. }
        | DoogatError::Redb(_) => {
            tracing::error!(%e, "internal error");
            ("INTERNAL_ERROR", "internal error".into())
        }
    }
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

/// Copy each `(key, value)` pair from a structured-error context into the
/// GraphQL extensions map, converting `ErrorValue::List` to a JSON array.
fn attach_context(
    map: &mut async_graphql::ErrorExtensionValues,
    context: &ErrorContext,
) {
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

    fn ext(err: &ServerError, key: &str) -> Value {
        let exts = err
            .extensions
            .as_ref()
            .expect("extensions should be set");
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
        let (code, msg) =
            classify(&DoogatError::SqlEngine("near SELCT: syntax error".into()));
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
        let (code, msg) = classify(&DoogatError::Conflict("merge conflict in ddb/123.md".into()));
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
        let (code, msg) = classify(&DoogatError::VersionMismatch { repo: 99, driver: 1 });
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
        let err = to_server_error(DoogatError::NotFound("doogat 20260319120000 not found".into()));
        assert_eq!(err.message, "doogat 20260319120000 not found");
    }

    // --- PRD 00129 §6: structured error codes ---

    #[test]
    fn structured_not_null_violation_emits_code_and_table_column() {
        let err = to_server_error(DoogatError::not_null_violation("link", "url"));
        assert_eq!(err.message, "NOT NULL constraint violated: link.url");
        assert_eq!(ext(&err, "code"), Value::String("NOT_NULL_VIOLATION".into()));
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
        assert_eq!(
            err.message,
            "type \"widget\" is not a registered typedef"
        );
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
}
