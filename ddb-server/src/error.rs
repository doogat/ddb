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
mod tests;
