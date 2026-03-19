use async_graphql::ServerError;
use zdb_core::error::ZettelError;

/// Classify a ZettelError for external exposure.
///
/// Returns `(error_code, user_safe_message)`. Internal errors are redacted
/// to a generic message and the original is logged to stderr.
pub fn classify(e: &ZettelError) -> (&'static str, String) {
    match e {
        // Safe to expose — user-actionable
        ZettelError::NotFound(m) => ("NOT_FOUND", m.clone()),
        ZettelError::Validation(m) => ("VALIDATION_ERROR", m.clone()),
        ZettelError::InvalidPath(m) => ("INVALID_PATH", m.clone()),
        // SQL errors — redact raw details
        ZettelError::SqlEngine(_) => {
            eprintln!("[zdb-server] internal error: {e}");
            ("SQL_ERROR", "query failed".into())
        }
        // All other variants are internal — redact completely
        _ => {
            eprintln!("[zdb-server] internal error: {e}");
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
