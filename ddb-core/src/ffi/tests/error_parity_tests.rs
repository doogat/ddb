//! Tier 1 parity tests for the FFI error mapping: the variant a core error
//! surfaces as must be the one the single error policy (PRD 00173) assigns,
//! derived at runtime, not a hand-maintained allowlist.
//!
//! Every message these tests feed in carries a runtime nonce, so a conversion
//! that keys on literal message strings (or on a `Display` prefix) cannot
//! reproduce the expectations.

use crate::app_contract::{error_rendering, AppError, FfiVariant};
use crate::error::{codes, DoogatError, ErrorValue};
use crate::ffi::records::{DdbError, DdbErrorContextEntry};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A value no implementation can hardcode: the wall clock in nanoseconds plus
/// a per-call counter, so two probes never share a message either.
fn nonce() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    format!("{nanos}-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// A message built at run time, so it cannot appear in any lookup table.
fn probe_msg(label: &str) -> String {
    format!("{label} {}", nonce())
}

/// Every `DoogatError` arm constructible in-crate outside the two carve-outs
/// (`VersionMismatch`, and `Redb` which needs the `nosql` feature).
fn simple_arms() -> Vec<DoogatError> {
    vec![
        DoogatError::Git(probe_msg("ref update failed")),
        DoogatError::Yaml(probe_msg("bad frontmatter")),
        DoogatError::Sql(probe_msg("no such table")),
        DoogatError::Automerge(probe_msg("load failed")),
        DoogatError::Io(std::io::Error::other(probe_msg("disk gone"))),
        DoogatError::Toml(probe_msg("bad config")),
        DoogatError::Parse(probe_msg("missing zone")),
        DoogatError::NotFound(probe_msg("20260101000000")),
        DoogatError::Validation(probe_msg("title required")),
        DoogatError::InvalidPath(probe_msg("../escape")),
        DoogatError::SqlEngine(probe_msg("boom")),
        DoogatError::Conflict(probe_msg("merge conflict")),
        DoogatError::Sync(probe_msg("fetch failed")),
        DoogatError::Index(probe_msg("rebuild failed")),
        DoogatError::BadRequest(probe_msg("missing id")),
    ]
}

/// A `Structured` error for `code` whose message and context are built at run
/// time, so neither can be recovered from a hardcoded table.
fn structured_probe(code: &'static str) -> DoogatError {
    DoogatError::Structured {
        code,
        message: probe_msg("structured"),
        context: vec![("probe".into(), ErrorValue::str(nonce()))],
    }
}

/// One `Structured` error per registered code, so no subset of `codes::ALL`
/// can be classified by an allowlist with a default.
fn structured_arms() -> Vec<DoogatError> {
    codes::ALL.iter().copied().map(structured_probe).collect()
}

/// The policy-visible shape of a converted FFI error: the `FfiVariant` this
/// variant stands for plus the message it carries. `None` for any variant the
/// policy cannot produce (`Git`, `Sql`, `SqlEngine`, ...), which is itself a
/// parity failure.
fn policy_shape(err: &DdbError) -> Option<(FfiVariant, &str)> {
    match err {
        DdbError::NotFound { msg } => Some((FfiVariant::NotFound, msg.as_str())),
        DdbError::Conflict { msg, .. } => Some((FfiVariant::Conflict, msg.as_str())),
        DdbError::Validation { msg, .. } => Some((FfiVariant::Validation, msg.as_str())),
        DdbError::Io { msg } => Some((FfiVariant::Internal, msg.as_str())),
        _ => None,
    }
}

/// The `code` and `context` a converted error carries; `None` for the two
/// variants (`NotFound`, `Io`) that have no such fields.
fn carried(err: &DdbError) -> Option<(Option<&str>, &[DdbErrorContextEntry])> {
    match err {
        DdbError::Conflict { code, context, .. } | DdbError::Validation { code, context, .. } => {
            Some((code.as_deref(), context.as_slice()))
        }
        _ => None,
    }
}

fn str_entry(key: &str, value: &str) -> DdbErrorContextEntry {
    DdbErrorContextEntry {
        key: key.into(),
        value: Some(value.into()),
        values: Vec::new(),
    }
}

fn list_entry(key: &str, values: &[&str]) -> DdbErrorContextEntry {
    DdbErrorContextEntry {
        key: key.into(),
        value: None,
        values: values.iter().map(|v| (*v).to_string()).collect(),
    }
}

/// Derive every expectation from `error_rendering` at run time: the variant,
/// the policy `code` on the arms that carry one, the source error's own
/// message, and an empty context for a source that has none.
fn assert_parity(err: DoogatError) {
    let expected = error_rendering(AppError::from(&err));
    let source_msg = err.to_string();
    let structured = matches!(err, DoogatError::Structured { .. });
    let converted = DdbError::from(err);
    let (variant, msg) = policy_shape(&converted).unwrap_or_else(|| {
        panic!("{source_msg:?} converted outside the policy variants: {converted:?}")
    });
    assert_eq!(
        variant, expected.ffi_variant,
        "{source_msg:?} must take the policy's ffi_variant, got {converted:?}"
    );
    assert_eq!(
        msg, source_msg,
        "msg must stay the source error's own to_string(), got {converted:?}"
    );
    if let Some((code, context)) = carried(&converted) {
        assert_eq!(
            code,
            Some(expected.code),
            "{source_msg:?} must carry the policy's code, got {converted:?}"
        );
        if !structured {
            assert!(
                context.is_empty(),
                "an error with no structured context must carry none, got {converted:?}"
            );
        }
    }
}

/// Assert the converted error carries exactly `code` and exactly `context` -
/// no fabricated, dropped or leaked entry.
fn assert_carried(converted: &DdbError, code: &str, context: &[DdbErrorContextEntry]) {
    let (got_code, got_context) =
        carried(converted).unwrap_or_else(|| panic!("{converted:?} carries no code or context"));
    assert_eq!(
        got_code,
        Some(code),
        "code must come from the policy, got {converted:?}"
    );
    assert_eq!(
        got_context, context,
        "context must be exactly the converted source context, got {converted:?}"
    );
}

#[test]
fn simple_error_arms_take_the_ffi_variant_the_policy_assigns() {
    for err in simple_arms() {
        assert_parity(err);
    }
}

/// `msg` is uniformly `err.to_string()`, which for the `Io` arm keeps
/// `DoogatError`'s own `"io: "` prefix rather than the inner error's text.
#[test]
fn io_error_follows_the_policy_and_keeps_its_own_io_prefix() {
    let err = DoogatError::Io(std::io::Error::other("disk gone"));
    let expected = error_rendering(AppError::from(&err)).ffi_variant;
    let converted = DdbError::from(err);
    let (variant, msg) = policy_shape(&converted)
        .unwrap_or_else(|| panic!("Io converted outside the policy: {converted:?}"));
    assert_eq!(
        variant, expected,
        "the Io arm is policy-derived like every other non-carve-out arm, got {converted:?}"
    );
    assert_eq!(
        msg, "io: disk gone",
        "the uniform err.to_string() rule keeps DoogatError's io: prefix, got {converted:?}"
    );
}

#[test]
fn structured_error_codes_take_the_ffi_variant_the_policy_assigns() {
    for err in structured_arms() {
        assert_parity(err);
    }
    for err in [
        DoogatError::singleton_not_found("app_config"),
        DoogatError::unique_violation("link", ["url"], ["https://x"]),
        DoogatError::references_violation("link", "link", "membership", "20260101000000"),
        DoogatError::cascade_cycle(["a", "b", "a"]),
        DoogatError::singleton_violation("app_config", "20260101000000"),
    ] {
        assert_parity(err);
    }
}

/// No `DoogatError` may reach a variant that predates the policy: those arms
/// are unreachable now, so a parallel classification table cannot hide there.
#[test]
fn the_conversion_never_produces_a_legacy_pre_policy_variant() {
    for err in simple_arms().into_iter().chain(structured_arms()) {
        let source_msg = err.to_string();
        let converted = DdbError::from(err);
        assert!(
            !matches!(
                &converted,
                DdbError::Git { .. }
                    | DdbError::Yaml { .. }
                    | DdbError::Sql { .. }
                    | DdbError::Automerge { .. }
                    | DdbError::Parse { .. }
                    | DdbError::Config { .. }
                    | DdbError::SqlEngine { .. }
            ),
            "{source_msg:?} reached a legacy pre-policy variant: {converted:?}"
        );
    }
}

#[test]
fn singleton_not_found_surfaces_as_not_found_not_validation() {
    let converted = DdbError::from(DoogatError::singleton_not_found("app_config"));
    assert!(
        matches!(&converted, DdbError::NotFound { .. }),
        "SINGLETON_NOT_FOUND is a not-found condition, got {converted:?}"
    );
}

#[test]
fn sql_engine_msg_keeps_its_display_prefix_instead_of_the_policy_message() {
    let err = DoogatError::SqlEngine(probe_msg("boom"));
    let rendering = error_rendering(AppError::from(&err));
    let source_msg = err.to_string();
    assert_eq!(
        format!("sql engine: {}", rendering.message),
        source_msg,
        "the policy strips SQL_ERROR's display prefix, so the two messages differ"
    );
    let converted = DdbError::from(err);
    let (variant, msg) = policy_shape(&converted)
        .unwrap_or_else(|| panic!("SqlEngine converted outside the policy: {converted:?}"));
    assert_eq!(
        msg, source_msg,
        "FFI msg must be the source error's to_string(), not rendering.message"
    );
    assert_eq!(
        variant, rendering.ffi_variant,
        "SqlEngine must follow SQL_ERROR's registered category, got {converted:?}"
    );
}

#[test]
fn structured_conflict_arm_carries_the_policy_code_and_converted_context() {
    let err = DoogatError::unique_violation("t-9f3", ["c-1", "c-2"], ["v-1", "v-2"]);
    let code = error_rendering(AppError::from(&err)).code;
    assert_eq!(code, codes::UNIQUE_VIOLATION);
    let converted = DdbError::from(err);
    assert!(
        matches!(&converted, DdbError::Conflict { .. }),
        "UNIQUE_VIOLATION is a conflict, got {converted:?}"
    );
    assert_carried(
        &converted,
        code,
        &[
            str_entry("table", "t-9f3"),
            list_entry("columns", &["c-1", "c-2"]),
            list_entry("values", &["v-1", "v-2"]),
        ],
    );
}

#[test]
fn structured_validation_arm_carries_the_policy_code_and_converted_context() {
    let err = DoogatError::type_not_registered("widget");
    let rendering = error_rendering(AppError::from(&err));
    assert_eq!(rendering.ffi_variant, FfiVariant::Validation);
    let converted = DdbError::from(err);
    assert_carried(&converted, rendering.code, &[str_entry("type", "widget")]);

    let err = DoogatError::not_null_violation("link", "url");
    let rendering = error_rendering(AppError::from(&err));
    assert_eq!(rendering.ffi_variant, FfiVariant::Validation);
    let converted = DdbError::from(err);
    assert_carried(
        &converted,
        rendering.code,
        &[str_entry("table", "link"), str_entry("column", "url")],
    );
}

/// Context that the message does not contain must still arrive intact, so it
/// cannot have been scraped out of the message text.
#[test]
fn validation_context_survives_when_it_is_not_recoverable_from_the_message() {
    let secret = nonce();
    let err = DoogatError::Structured {
        code: codes::UNKNOWN_FIELD,
        message: probe_msg("unknown column"),
        context: vec![
            ("hidden".into(), ErrorValue::str(secret.clone())),
            ("choices".into(), ErrorValue::list(["k-1", "k-2"])),
        ],
    };
    let rendering = error_rendering(AppError::from(&err));
    assert_eq!(rendering.ffi_variant, FfiVariant::Validation);
    let converted = DdbError::from(err);
    assert_carried(
        &converted,
        rendering.code,
        &[
            str_entry("hidden", &secret),
            list_entry("choices", &["k-1", "k-2"]),
        ],
    );
}

#[test]
fn version_mismatch_bypasses_the_policy_and_keeps_its_own_variant() {
    for (repo, driver) in [(3u32, 2u32), (9, 1)] {
        let err = DoogatError::VersionMismatch { repo, driver };
        let expected = err
            .to_string()
            .strip_prefix("version mismatch: ")
            .expect("VersionMismatch Display carries its prefix")
            .to_string();
        match DdbError::from(err) {
            DdbError::VersionMismatch { msg } => {
                assert_eq!(
                    msg, expected,
                    "the carve-out msg is the source error's own wording for repo={repo} driver={driver}"
                );
                assert_eq!(msg, format!("repo format v{repo}, driver supports up to v{driver}"));
            }
            other => panic!("VersionMismatch keeps its dedicated FFI variant, got {other:?}"),
        }
    }
}
