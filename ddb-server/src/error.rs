use async_graphql::ServerError;
use ddb_core::error::DoogatError;

/// Classify a DoogatError for external exposure.
///
/// Returns `(error_code, user_safe_message)`. Internal errors are redacted
/// to a generic message and the original is logged via tracing.
pub fn classify(e: &DoogatError) -> (&'static str, String) {
    match e {
        // Safe to expose — user-actionable
        DoogatError::NotFound(m) => ("NOT_FOUND", m.clone()),
        DoogatError::Validation(m) => ("VALIDATION_ERROR", m.clone()),
        DoogatError::InvalidPath(m) => ("INVALID_PATH", m.clone()),
        DoogatError::Conflict(m) => ("CONFLICT", m.clone()),
        DoogatError::BadRequest(m) => ("BAD_REQUEST", m.clone()),
        // SQL errors — redact raw details
        DoogatError::SqlEngine(_) => {
            tracing::error!(%e, "internal error");
            ("SQL_ERROR", "query failed".into())
        }
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

/// Convert DoogatError to ServerError for use in dynamic schema resolvers.
pub fn to_server_error(e: DoogatError) -> ServerError {
    let (code, message) = classify(&e);
    let mut err = ServerError::new(message, None);
    err.extensions = Some({
        let mut map = async_graphql::ErrorExtensionValues::default();
        map.set("code", code);
        map
    });
    err
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn sql_engine_redacted_to_query_failed() {
        let (code, msg) =
            classify(&DoogatError::SqlEngine("near SELCT: syntax error".into()));
        assert_eq!(code, "SQL_ERROR");
        assert_eq!(msg, "query failed");
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
    fn graphql_sql_error_sanitized() {
        let err = to_server_error(DoogatError::SqlEngine(
            "near \"SELCT\": syntax error at position 0".into(),
        ));
        assert_eq!(err.message, "query failed");
        assert!(!err.message.contains("SELCT"));
        assert!(!err.message.contains("syntax error"));
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
}
