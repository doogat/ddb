use tokio::sync::broadcast;

use crate::events;

/// Convert a broadcast::Receiver into a Stream that skips lag errors and ends on close.
pub(crate) fn event_stream(
    rx: broadcast::Receiver<events::DoogatEvent>,
) -> impl futures_util::Stream<Item = async_graphql::Result<events::DoogatEvent>> {
    futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(event) => return Some((Ok(event), rx)),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("subscription lagged, skipped {n} events");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
}

// -- Argument validation helpers --

const MAX_LIMIT: i64 = 10_000;
const VALID_FORMATS: &[&str] = &["array", "objects"];

/// Validate that a limit value is non-negative and within bounds.
pub(super) fn check_limit(n: i64) -> async_graphql::Result<i64> {
    if n < 0 {
        return Err(async_graphql::Error::new("limit must be non-negative"));
    }
    if n > MAX_LIMIT {
        return Err(async_graphql::Error::new(format!(
            "limit must not exceed {MAX_LIMIT}"
        )));
    }
    Ok(n)
}

/// Validate that an offset value is non-negative.
pub(super) fn check_offset(n: i64) -> async_graphql::Result<i64> {
    if n < 0 {
        return Err(async_graphql::Error::new("offset must be non-negative"));
    }
    Ok(n)
}

/// Validate that a format string is in the known set.
pub(super) fn check_format(s: &str) -> async_graphql::Result<()> {
    if !VALID_FORMATS.contains(&s) {
        return Err(async_graphql::Error::new(format!(
            "invalid format '{}'; allowed: {}",
            s,
            VALID_FORMATS.join(", ")
        )));
    }
    Ok(())
}

/// Extract and validate an optional non-negative `limit` argument.
pub(crate) fn validate_limit(
    ctx: &async_graphql::dynamic::ResolverContext,
) -> async_graphql::Result<Option<i64>> {
    match ctx.args.get("limit") {
        Some(v) => {
            let n = v
                .i64()
                .map_err(|_| async_graphql::Error::new("limit must be an integer"))?;
            check_limit(n)?;
            Ok(Some(n))
        }
        None => Ok(None),
    }
}

/// Extract and validate an optional non-negative `offset` argument.
pub(crate) fn validate_offset(
    ctx: &async_graphql::dynamic::ResolverContext,
) -> async_graphql::Result<Option<i64>> {
    match ctx.args.get("offset") {
        Some(v) => {
            let n = v
                .i64()
                .map_err(|_| async_graphql::Error::new("offset must be an integer"))?;
            check_offset(n)?;
            Ok(Some(n))
        }
        None => Ok(None),
    }
}

/// Extract and validate an optional `format` argument against the known set.
pub(crate) fn validate_format<'a>(
    ctx: &'a async_graphql::dynamic::ResolverContext,
) -> async_graphql::Result<&'a str> {
    match ctx.args.get("format") {
        Some(v) => {
            let s = v
                .string()
                .map_err(|_| async_graphql::Error::new("format must be a string"))?;
            check_format(s)?;
            Ok(s)
        }
        None => Ok("array"),
    }
}
