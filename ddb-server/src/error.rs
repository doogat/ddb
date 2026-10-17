use async_graphql::{Error, ServerError};
use ddb_core::app_contract::{AppError, AppErrorDetail};
use ddb_core::error::DoogatError;

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
/// Delegates to [`to_graphql_error_from_app`], so message, `field` and
/// context all come from the shared `error_rendering` policy and this path
/// makes no redaction decision of its own.
///
/// Returning `Error` (not `ServerError`) is load-bearing: dynamic-schema
/// `FieldFuture::new` closures resolve to `Result<_, async_graphql::Error>`
/// and rely on `?` propagating the value-with-extensions directly. Going
/// through `ServerError` here would invoke async-graphql's blanket
/// `impl<T: Display> From<T> for Error` (PRD 00131), which builds a fresh
/// `Error` from `to_string()` and silently drops `extensions`.
pub fn to_graphql_error(e: DoogatError) -> Error {
    to_graphql_error_from_app(AppError::from(e))
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

#[cfg(test)]
mod tests;
