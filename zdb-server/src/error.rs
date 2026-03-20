use async_graphql::ServerError;
use zdb_core::error::ZettelError;

/// Classify a ZettelError for external exposure.
///
/// Returns `(error_code, user_safe_message)`. Internal errors are redacted
/// to a generic message and the original is logged via tracing.
pub fn classify(e: &ZettelError) -> (&'static str, String) {
    match e {
        // Safe to expose — user-actionable
        ZettelError::NotFound(m) => ("NOT_FOUND", m.clone()),
        ZettelError::Validation(m) => ("VALIDATION_ERROR", m.clone()),
        ZettelError::InvalidPath(m) => ("INVALID_PATH", m.clone()),
        // SQL errors — redact raw details
        ZettelError::SqlEngine(_) => {
            tracing::error!(%e, "internal error");
            ("SQL_ERROR", "query failed".into())
        }
        // All other variants are internal — redact completely.
        // Explicit matches so the compiler forces a decision on new variants.
        ZettelError::Git(_)
        | ZettelError::Yaml(_)
        | ZettelError::Sql(_)
        | ZettelError::Automerge(_)
        | ZettelError::Io(_)
        | ZettelError::Toml(_)
        | ZettelError::Parse(_)
        | ZettelError::VersionMismatch { .. }
        | ZettelError::Redb(_) => {
            tracing::error!(%e, "internal error");
            ("INTERNAL_ERROR", "internal error".into())
        }
    }
}

/// Convert ZettelError to ServerError for use in dynamic schema resolvers.
pub fn to_server_error(e: ZettelError) -> ServerError {
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
        let (code, msg) = classify(&ZettelError::NotFound("zettel not found".into()));
        assert_eq!(code, "NOT_FOUND");
        assert_eq!(msg, "zettel not found");
    }

    #[test]
    fn validation_passes_through() {
        let (code, msg) = classify(&ZettelError::Validation("title required".into()));
        assert_eq!(code, "VALIDATION_ERROR");
        assert_eq!(msg, "title required");
    }

    #[test]
    fn invalid_path_passes_through() {
        let (code, msg) = classify(&ZettelError::InvalidPath("../escape".into()));
        assert_eq!(code, "INVALID_PATH");
        assert_eq!(msg, "../escape");
    }

    #[test]
    fn sql_engine_redacted_to_query_failed() {
        let (code, msg) =
            classify(&ZettelError::SqlEngine("near SELCT: syntax error".into()));
        assert_eq!(code, "SQL_ERROR");
        assert_eq!(msg, "query failed");
    }

    #[test]
    fn git_error_redacted() {
        let (code, msg) = classify(&ZettelError::Git("object not found".into()));
        assert_eq!(code, "INTERNAL_ERROR");
        assert_eq!(msg, "internal error");
    }

    #[test]
    fn io_error_redacted() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "secret path");
        let (code, msg) = classify(&ZettelError::Io(io_err));
        assert_eq!(code, "INTERNAL_ERROR");
        assert_eq!(msg, "internal error");
    }

    #[test]
    fn yaml_error_redacted() {
        let (code, msg) = classify(&ZettelError::Yaml("bad yaml at line 3".into()));
        assert_eq!(code, "INTERNAL_ERROR");
        assert_eq!(msg, "internal error");
    }

    #[test]
    fn sql_error_redacted() {
        let (code, msg) = classify(&ZettelError::Sql("table schema leaked".into()));
        assert_eq!(code, "INTERNAL_ERROR");
        assert_eq!(msg, "internal error");
    }

    #[test]
    fn redb_error_redacted() {
        let (code, msg) = classify(&ZettelError::Redb("corrupt table".into()));
        assert_eq!(code, "INTERNAL_ERROR");
        assert_eq!(msg, "internal error");
    }

    #[test]
    fn version_mismatch_redacted() {
        let (code, msg) = classify(&ZettelError::VersionMismatch { repo: 99, driver: 1 });
        assert_eq!(code, "INTERNAL_ERROR");
        assert_eq!(msg, "internal error");
    }

    #[test]
    fn to_server_error_uses_classification() {
        let err = to_server_error(ZettelError::Git("repo corrupt".into()));
        assert_eq!(err.message, "internal error");

        let err = to_server_error(ZettelError::NotFound("missing".into()));
        assert_eq!(err.message, "missing");
    }

    #[test]
    fn graphql_sql_error_sanitized() {
        let err = to_server_error(ZettelError::SqlEngine(
            "near \"SELCT\": syntax error at position 0".into(),
        ));
        assert_eq!(err.message, "query failed");
        assert!(!err.message.contains("SELCT"));
        assert!(!err.message.contains("syntax error"));
    }

    #[test]
    fn graphql_internal_error_no_details() {
        let err = to_server_error(ZettelError::Git(
            "/Users/secret/.zdb/objects/ab/cdef1234".into(),
        ));
        assert_eq!(err.message, "internal error");
        assert!(!err.message.contains("secret"));
        assert!(!err.message.contains(".zdb"));
    }

    #[test]
    fn graphql_not_found_descriptive() {
        let err = to_server_error(ZettelError::NotFound("zettel 20260319120000 not found".into()));
        assert_eq!(err.message, "zettel 20260319120000 not found");
    }
}
