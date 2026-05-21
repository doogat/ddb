//! Output envelope types for the application contract layer.
//!
//! `AppOutput<T>` wraps a successful result with adapter-neutral warnings.
//! Transports must surface these warnings; they cannot be silently discarded
//! for promised workflows (PRD 00147).

/// A stable, adapter-neutral warning attached to a successful command result.
///
/// `code` is a stable static string suitable for programmatic handling;
/// `message` is a human-readable description that transports may format for
/// end users.
#[derive(Debug, Clone)]
pub struct AppWarning {
    pub code: &'static str,
    pub message: String,
}

/// Adapter-neutral envelope for a successful application command result.
///
/// The `value` field carries the primary result; `warnings` carries
/// best-effort or partial-success signals that transports must forward to
/// callers rather than discard.
#[derive(Debug, Clone)]
pub struct AppOutput<T> {
    pub value: T,
    pub warnings: Vec<AppWarning>,
}
