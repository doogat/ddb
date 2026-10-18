//! Tier 1 parity tests for the FFI error mapping: the variant a core error
//! surfaces as must be the one the single error policy (PRD 00173) assigns,
//! derived at runtime, not a hand-maintained allowlist.

use crate::app_contract::{error_rendering, AppError, FfiVariant};
use crate::error::{codes, DoogatError};
use crate::ffi::records::{DdbError, DdbErrorContextEntry};

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

/// Derive the expectation from `error_rendering` at runtime, then assert the
/// conversion agrees with it and kept the source error's own message.
fn assert_parity(err: DoogatError) {
    let expected = error_rendering(AppError::from(&err)).ffi_variant;
    let source_msg = err.to_string();
    let converted = DdbError::from(err);
    let (variant, msg) = policy_shape(&converted).unwrap_or_else(|| {
        panic!("{source_msg:?} converted outside the policy variants: {converted:?}")
    });
    assert_eq!(
        variant, expected,
        "{source_msg:?} must take the policy's ffi_variant, got {converted:?}"
    );
    assert_eq!(
        msg, source_msg,
        "msg must stay the source error's own to_string(), got {converted:?}"
    );
}

#[test]
fn simple_error_arms_take_the_ffi_variant_the_policy_assigns() {
    for err in [
        DoogatError::Git("ref update failed".into()),
        DoogatError::Yaml("bad frontmatter".into()),
        DoogatError::Sql("no such table".into()),
        DoogatError::Automerge("load failed".into()),
        DoogatError::Toml("bad config".into()),
        DoogatError::Parse("missing zone".into()),
        DoogatError::NotFound("20260101000000".into()),
        DoogatError::Validation("title required".into()),
        DoogatError::InvalidPath("../escape".into()),
        DoogatError::SqlEngine("boom".into()),
        DoogatError::Conflict("merge conflict".into()),
        DoogatError::Sync("fetch failed".into()),
        DoogatError::Index("rebuild failed".into()),
        DoogatError::BadRequest("missing id".into()),
    ] {
        assert_parity(err);
    }
}

#[test]
fn structured_error_codes_take_the_ffi_variant_the_policy_assigns() {
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

#[test]
fn singleton_not_found_surfaces_as_not_found_not_validation() {
    let converted = DdbError::from(DoogatError::singleton_not_found("app_config"));
    assert!(
        matches!(&converted, DdbError::NotFound { .. }),
        "SINGLETON_NOT_FOUND is a not-found condition, got {converted:?}"
    );
}

#[test]
fn conflict_sync_and_index_no_longer_collapse_to_git_or_sql() {
    for err in [
        DoogatError::Conflict("merge conflict".into()),
        DoogatError::Sync("fetch failed".into()),
        DoogatError::Index("rebuild failed".into()),
    ] {
        let converted = DdbError::from(err);
        assert!(
            !matches!(&converted, DdbError::Git { .. } | DdbError::Sql { .. }),
            "the lossy legacy mapping is back: {converted:?}"
        );
    }
}

#[test]
fn sql_engine_msg_keeps_its_display_prefix_instead_of_the_policy_message() {
    let err = DoogatError::SqlEngine("boom".into());
    let rendering = error_rendering(AppError::from(&err));
    assert_eq!(
        rendering.message, "boom",
        "the policy strips SQL_ERROR's display prefix, so the two messages differ"
    );
    let converted = DdbError::from(err);
    let (variant, msg) = policy_shape(&converted)
        .unwrap_or_else(|| panic!("SqlEngine converted outside the policy: {converted:?}"));
    assert_eq!(
        msg, "sql engine: boom",
        "FFI msg must be the source error's to_string(), not rendering.message"
    );
    assert_eq!(
        variant, rendering.ffi_variant,
        "SqlEngine must follow SQL_ERROR's registered category, got {converted:?}"
    );
}

#[test]
fn structured_conflict_arm_carries_the_policy_code_and_converted_context() {
    let err = DoogatError::unique_violation("link", ["url"], ["https://x"]);
    let code = error_rendering(AppError::from(&err)).code;
    assert_eq!(code, codes::UNIQUE_VIOLATION);
    match DdbError::from(err) {
        DdbError::Conflict {
            code: ffi_code,
            context,
            ..
        } => {
            assert_eq!(ffi_code.as_deref(), Some(code), "code comes from the policy");
            assert!(
                context.contains(&DdbErrorContextEntry {
                    key: "table".into(),
                    value: Some("link".into()),
                    values: Vec::new(),
                }),
                "string context must survive: {context:?}"
            );
            assert!(
                context.contains(&DdbErrorContextEntry {
                    key: "columns".into(),
                    value: None,
                    values: vec!["url".into()],
                }),
                "list context must survive: {context:?}"
            );
        }
        other => panic!("UNIQUE_VIOLATION is a conflict, got {other:?}"),
    }
}

#[test]
fn structured_validation_arm_carries_the_policy_code_and_converted_context() {
    let err = DoogatError::type_not_registered("widget");
    let rendering = error_rendering(AppError::from(&err));
    assert_eq!(rendering.ffi_variant, FfiVariant::Validation);
    match DdbError::from(err) {
        DdbError::Validation { code, context, .. } => {
            assert_eq!(code.as_deref(), Some(rendering.code));
            assert_eq!(
                context,
                vec![DdbErrorContextEntry {
                    key: "type".into(),
                    value: Some("widget".into()),
                    values: Vec::new(),
                }]
            );
        }
        other => panic!("TYPE_NOT_REGISTERED is a validation error, got {other:?}"),
    }
}

#[test]
fn unstructured_validation_arm_carries_the_policy_code_and_no_context() {
    let err = DoogatError::BadRequest("missing id".into());
    let code = error_rendering(AppError::from(&err)).code;
    match DdbError::from(err) {
        DdbError::Validation {
            code: ffi_code,
            context,
            ..
        } => {
            assert_eq!(
                ffi_code.as_deref(),
                Some(code),
                "the policy code is populated even without structured context"
            );
            assert!(context.is_empty(), "no context to carry: {context:?}");
        }
        other => panic!("BAD_REQUEST is a validation error, got {other:?}"),
    }
}

#[test]
fn version_mismatch_bypasses_the_policy_and_keeps_its_own_variant() {
    let converted = DdbError::from(DoogatError::VersionMismatch { repo: 3, driver: 2 });
    match converted {
        DdbError::VersionMismatch { msg } => assert_eq!(
            msg, "repo format v3, driver supports up to v2",
            "the carve-out message is fixed"
        ),
        other => panic!("VersionMismatch keeps its dedicated FFI variant, got {other:?}"),
    }
}
