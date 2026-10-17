//! Unit tests for the app-contract error policy.
//!
//! Split out of `error.rs` to keep that file under the 800-line limit.

// The conversion tests below use fully-qualified paths (`crate::error::...`,
// `crate::app_contract::...`); the rendering-policy tests import the
// policy items from this module.
use super::{error_rendering, AppError, AppErrorCategory, ErrorRendering, FfiVariant};
use crate::error::{codes, DoogatError, ErrorValue};

fn render(err: DoogatError) -> ErrorRendering {
    error_rendering(AppError::from(err))
}

/// Asserts code, category, status and FFI hint, plus the redaction rule:
/// Internal renders "internal error" with no field/details; everything
/// else keeps the (prefix-stripped) message.
fn assert_rendering(
    err: DoogatError,
    code: &str,
    category: AppErrorCategory,
    http_status: u16,
    ffi_variant: FfiVariant,
    message: &str,
) {
    let r = render(err);
    assert_eq!(r.code, code, "code for {code}");
    assert_eq!(r.category, category, "category for {code}");
    assert_eq!(r.http_status, http_status, "status for {code}");
    assert_eq!(r.ffi_variant, ffi_variant, "ffi hint for {code}");
    assert_eq!(r.message, message, "message for {code}");
    if category == AppErrorCategory::Internal {
        assert_eq!(r.field, None, "redacted field for {code}");
        assert!(r.details.is_empty(), "redacted details for {code}");
    }
}

/// A tuple-variant constructor paired with the code it must render as.
type CodedCtor = (fn(String) -> DoogatError, &'static str);

fn structured(code: &'static str, message: &str) -> DoogatError {
    DoogatError::Structured {
        code,
        message: message.into(),
        context: vec![("table".into(), ErrorValue::String("t".into()))],
    }
}

#[test]
fn simple_variants_render_with_category_status_ffi_hint_and_stripped_prefix() {
    use AppErrorCategory as C;
    use FfiVariant as F;
    let cases = [
        (DoogatError::NotFound("doc 1".into()), "NOT_FOUND", C::NotFound, 404, F::NotFound),
        (DoogatError::Validation("bad id".into()), "VALIDATION_ERROR", C::InvalidInput, 400, F::Validation),
        (DoogatError::Parse("bad yaml".into()), "PARSE_ERROR", C::InvalidInput, 422, F::Validation),
        (DoogatError::InvalidPath("../x".into()), "INVALID_PATH", C::InvalidInput, 400, F::Validation),
        (DoogatError::BadRequest("no body".into()), "BAD_REQUEST", C::InvalidInput, 400, F::Validation),
        (DoogatError::Conflict("stale".into()), "CONFLICT", C::Conflict, 409, F::Conflict),
        (DoogatError::SqlEngine("no such column: bogus".into()), "SQL_ERROR", C::InvalidInput, 422, F::Validation),
    ];
    let messages = ["doc 1", "bad id", "bad yaml", "../x", "no body", "stale", "no such column: bogus"];
    for ((err, code, cat, status, ffi), msg) in cases.into_iter().zip(messages) {
        assert_rendering(err, code, cat, status, ffi, msg);
    }
    // Same rule on generated payloads: the rendered message is exactly the
    // payload the caller supplied, for any payload - not a fixed set of
    // Display strings.
    let ctors: [CodedCtor; 7] = [
        (DoogatError::NotFound, "NOT_FOUND"),
        (DoogatError::Validation, "VALIDATION_ERROR"),
        (DoogatError::Parse, "PARSE_ERROR"),
        (DoogatError::InvalidPath, "INVALID_PATH"),
        (DoogatError::BadRequest, "BAD_REQUEST"),
        (DoogatError::Conflict, "CONFLICT"),
        (DoogatError::SqlEngine, "SQL_ERROR"),
    ];
    for (i, (ctor, code)) in ctors.into_iter().enumerate() {
        let payload = format!("payload-{i}");
        let r = render(ctor(payload.clone()));
        assert_eq!(r.code, code, "code for generated {code}");
        assert_eq!(r.message, payload, "stripped message for {code}");
    }
}

#[test]
fn structured_codes_render_with_category_status_and_ffi_hint() {
    use AppErrorCategory as C;
    use FfiVariant as F;
    let cases = [
        (codes::SINGLETON_NOT_FOUND, C::NotFound, 404, F::NotFound),
        (codes::UNIQUE_VIOLATION, C::Conflict, 409, F::Conflict),
        (codes::SINGLETON_VIOLATION, C::Conflict, 409, F::Conflict),
        (codes::REFERENCES_VIOLATION, C::Conflict, 409, F::Conflict),
        (codes::CASCADE_CYCLE, C::Conflict, 409, F::Conflict),
        (codes::SCHEMA_DESTRUCTIVE_BLOCKED, C::Conflict, 409, F::Conflict),
        (codes::NOT_NULL_VIOLATION, C::InvalidInput, 422, F::Validation),
        (codes::UNKNOWN_FIELD, C::InvalidInput, 422, F::Validation),
        (codes::TYPE_NOT_REGISTERED, C::InvalidInput, 422, F::Validation),
        (codes::TRANSACTION_NOT_SUPPORTED, C::InvalidInput, 422, F::Validation),
    ];
    for (code, cat, status, ffi) in cases {
        assert_rendering(structured(code, "detail text"), code, cat, status, ffi, "detail text");
        // No non-internal code may drop its context on the way out.
        assert_eq!(
            render(structured(code, "detail text")).details,
            vec![("table".to_string(), super::AppErrorDetail::String("t".into()))],
            "details for {code}"
        );
    }
}

#[test]
fn non_internal_structured_rendering_keeps_details() {
    let r = render(structured(codes::SCHEMA_DESTRUCTIVE_BLOCKED, "blocked"));
    assert_eq!(
        r.details,
        vec![("table".to_string(), super::AppErrorDetail::String("t".into()))]
    );
}

#[test]
fn not_null_violation_rendering_keeps_field() {
    let r = render(DoogatError::not_null_violation("link", "url"));
    assert_eq!(r.field.as_deref(), Some("url"));
    assert_eq!(r.message, "NOT NULL constraint violated: link.url");
}

#[test]
fn schema_apply_partial_renders_internal_500_redacted() {
    let r = render(structured(codes::SCHEMA_APPLY_PARTIAL, "halted after 2 of 3"));
    assert_eq!(r.code, "SCHEMA_APPLY_PARTIAL");
    assert_eq!(r.category, AppErrorCategory::Internal);
    assert_eq!(r.http_status, 500);
    assert_eq!(r.ffi_variant, FfiVariant::Internal);
    assert_eq!(r.message, "internal error");
    assert_eq!(r.field, None);
    assert!(r.details.is_empty());
}

#[test]
fn internal_variants_render_500_redacted() {
    let expect_internal = |err: DoogatError| {
        assert_rendering(
            err,
            "INTERNAL_ERROR",
            AppErrorCategory::Internal,
            500,
            FfiVariant::Internal,
            "internal error",
        );
    };
    let internal: [fn() -> DoogatError; 9] = [
        || DoogatError::Git("secret repo path /home/x".into()),
        || DoogatError::Io(std::io::Error::other("disk gone")),
        || DoogatError::Yaml("bad anchor".into()),
        || DoogatError::Sql("database is locked".into()),
        || DoogatError::Automerge("bad change".into()),
        || DoogatError::Toml("bad table".into()),
        || DoogatError::Sync("peer refused".into()),
        || DoogatError::Index("index corrupt".into()),
        || DoogatError::VersionMismatch { repo: 3, driver: 2 },
    ];
    for f in internal {
        expect_internal(f());
    }
    #[cfg(feature = "nosql")]
    expect_internal(DoogatError::Redb("transaction aborted".into()));
    assert_rendering(
        structured("SOME_UNREGISTERED_CODE", "leaky detail"),
        "SOME_UNREGISTERED_CODE",
        AppErrorCategory::Internal,
        500,
        FfiVariant::Internal,
        "internal error",
    );
}

#[test]
fn only_outer_display_prefix_is_stripped_not_matching_inner_text() {
    let r = render(DoogatError::SqlEngine("sql engine: inner detail".into()));
    assert_eq!(r.message, "sql engine: inner detail");
    let r = render(DoogatError::NotFound("not found: nested".into()));
    assert_eq!(r.message, "not found: nested");
    // Generated payloads that repeat the outer prefix: exactly one prefix
    // is removed, whatever the payload text is.
    let ctors: [CodedCtor; 3] = [
        (DoogatError::SqlEngine, "sql engine: "),
        (DoogatError::NotFound, "not found: "),
        (DoogatError::Parse, "parse: "),
    ];
    for (i, (ctor, prefix)) in ctors.into_iter().enumerate() {
        let payload = format!("{prefix}inner-{i}");
        assert_eq!(render(ctor(payload.clone())).message, payload, "payload {i}");
    }
}

#[test]
fn ffi_variant_maps_each_category() {
    assert_eq!(FfiVariant::from(AppErrorCategory::NotFound), FfiVariant::NotFound);
    assert_eq!(FfiVariant::from(AppErrorCategory::InvalidInput), FfiVariant::Validation);
    assert_eq!(FfiVariant::from(AppErrorCategory::Conflict), FfiVariant::Conflict);
    assert_eq!(FfiVariant::from(AppErrorCategory::Internal), FfiVariant::Internal);
}

#[test]
fn every_code_constant_is_registered_in_error_policy() {
    for code in codes::ALL {
        assert!(
            super::policy_for(code).is_some(),
            "code {code} is missing from ERROR_POLICY; an unregistered code renders as a redacted 500"
        );
    }
}

#[test]
fn error_policy_has_no_duplicate_codes() {
    for (i, row) in super::ERROR_POLICY.iter().enumerate() {
        assert!(
            super::ERROR_POLICY[..i].iter().all(|p| p.code != row.code),
            "duplicate ERROR_POLICY row for code {}",
            row.code
        );
    }
}

#[test]
fn borrowed_and_owned_conversions_agree() {
    let make: [fn() -> DoogatError; 4] = [
        || DoogatError::NotFound("doc 1".into()),
        || DoogatError::unique_violation("link", ["url"], ["https://x"]),
        || DoogatError::Git("boom".into()),
        || structured(codes::SCHEMA_APPLY_PARTIAL, "halted"),
    ];
    for f in make {
        let err = f();
        let borrowed = AppError::from(&err);
        assert_eq!(borrowed, AppError::from(err));
    }
}

#[test]
fn schema_apply_partial_code_constant_has_exact_value() {
    assert_eq!(
        crate::error::codes::SCHEMA_APPLY_PARTIAL,
        "SCHEMA_APPLY_PARTIAL"
    );
}

#[test]
fn schema_destructive_blocked_code_constant_has_exact_value() {
    assert_eq!(
        crate::error::codes::SCHEMA_DESTRUCTIVE_BLOCKED,
        "SCHEMA_DESTRUCTIVE_BLOCKED"
    );
}

#[test]
fn schema_unsupported_change_warning_code_constant_has_exact_value() {
    assert_eq!(
        crate::app_contract::SCHEMA_UNSUPPORTED_CHANGE,
        "SCHEMA_UNSUPPORTED_CHANGE"
    );
}

#[test]
fn destructive_blocked_maps_to_conflict_category_with_code_and_message() {
    let err = crate::error::DoogatError::Structured {
        code: crate::error::codes::SCHEMA_DESTRUCTIVE_BLOCKED,
        message: "plan blocked: risky operations require override".into(),
        context: vec![],
    };
    let app: crate::app_contract::AppError = err.into();
    assert_eq!(app.code, "SCHEMA_DESTRUCTIVE_BLOCKED");
    assert_eq!(
        app.category,
        crate::app_contract::AppErrorCategory::Conflict
    );
    assert_eq!(
        app.message,
        "plan blocked: risky operations require override"
    );
}

#[test]
fn destructive_blocked_category_is_code_driven_not_message_driven() {
    // Same code, a second distinct message with none of the
    // "destructive"/"drop"/"rename" theme words. The category must derive
    // from `code` (-> Conflict), never from message prose, so an impl that
    // sniffs the message instead of matching the code is rejected here.
    let err = crate::error::DoogatError::Structured {
        code: crate::error::codes::SCHEMA_DESTRUCTIVE_BLOCKED,
        message: "request requires the override flag".into(),
        context: vec![],
    };
    let app: crate::app_contract::AppError = err.into();
    assert_eq!(
        app.category,
        crate::app_contract::AppErrorCategory::Conflict
    );
    assert_eq!(app.message, "request requires the override flag");
}

#[test]
fn apply_partial_maps_to_internal_category_with_code_and_message() {
    let err = crate::error::DoogatError::Structured {
        code: crate::error::codes::SCHEMA_APPLY_PARTIAL,
        message: "halted after 5 of 9 operations".into(),
        context: vec![],
    };
    let app: crate::app_contract::AppError = err.into();
    assert_eq!(app.code, "SCHEMA_APPLY_PARTIAL");
    assert_eq!(
        app.category,
        crate::app_contract::AppErrorCategory::Internal
    );
    assert_eq!(app.message, "halted after 5 of 9 operations");
}

#[test]
fn destructive_blocked_carries_context_into_details_in_order_with_matching_variants() {
    let err = crate::error::DoogatError::Structured {
        code: crate::error::codes::SCHEMA_DESTRUCTIVE_BLOCKED,
        message: "schema plan contains destructive operations".into(),
        context: vec![
            (
                "table".into(),
                crate::error::ErrorValue::String("project".into()),
            ),
            (
                "operations".into(),
                crate::error::ErrorValue::List(vec![
                    "DROP COLUMN status".into(),
                    "DROP COLUMN owner".into(),
                ]),
            ),
        ],
    };
    let app: crate::app_contract::AppError = err.into();
    assert_eq!(
        app.details,
        vec![
            (
                "table".to_string(),
                crate::app_contract::AppErrorDetail::String("project".into()),
            ),
            (
                "operations".to_string(),
                crate::app_contract::AppErrorDetail::List(vec![
                    "DROP COLUMN status".into(),
                    "DROP COLUMN owner".into(),
                ]),
            ),
        ]
    );
}

#[test]
fn apply_partial_carries_context_into_details_in_order_with_matching_variants() {
    let err = crate::error::DoogatError::Structured {
        code: crate::error::codes::SCHEMA_APPLY_PARTIAL,
        message: "schema apply completed 2 of 3 operations".into(),
        context: vec![
            (
                "failed_at".into(),
                crate::error::ErrorValue::String("ADD COLUMN due_date".into()),
            ),
            (
                "applied".into(),
                crate::error::ErrorValue::List(vec![
                    "ADD COLUMN status".into(),
                    "ADD COLUMN owner".into(),
                ]),
            ),
        ],
    };
    let app: crate::app_contract::AppError = err.into();
    assert_eq!(
        app.details,
        vec![
            (
                "failed_at".to_string(),
                crate::app_contract::AppErrorDetail::String("ADD COLUMN due_date".into()),
            ),
            (
                "applied".to_string(),
                crate::app_contract::AppErrorDetail::List(vec![
                    "ADD COLUMN status".into(),
                    "ADD COLUMN owner".into(),
                ]),
            ),
        ]
    );
}

#[test]
fn sql_engine_error_maps_to_sql_error_code_with_invalid_input_category() {
    let err = crate::error::DoogatError::SqlEngine("no such column: bogus".into());
    let app: crate::app_contract::AppError = err.into();
    assert_eq!(app.code, "SQL_ERROR");
    assert_eq!(
        app.category,
        crate::app_contract::AppErrorCategory::InvalidInput
    );
}

#[test]
fn schema_unsupported_change_usable_as_app_warning_code() {
    let warning = crate::app_contract::AppWarning {
        code: crate::app_contract::SCHEMA_UNSUPPORTED_CHANGE,
        message: "altering column type is not supported".into(),
    };
    assert_eq!(warning.code, "SCHEMA_UNSUPPORTED_CHANGE");
}

#[test]
fn invalid_zone_code_constant_has_exact_value() {
    assert_eq!(crate::error::codes::INVALID_ZONE, "INVALID_ZONE");
}

/// INVALID_ZONE is a registered, user-actionable structured code: it must
/// classify as InvalidInput / 422 / Validation and keep its own message and
/// context, not fall through to the Internal + "internal error" redaction.
#[test]
fn invalid_zone_renders_invalid_input_422_validation_keeping_message_and_details() {
    let err = DoogatError::Structured {
        code: codes::INVALID_ZONE,
        message: "unknown zone: footer".into(),
        context: vec![
            ("zone".into(), ErrorValue::String("footer".into())),
            (
                "allowed".into(),
                ErrorValue::List(vec![
                    "frontmatter".into(),
                    "body".into(),
                    "sections".into(),
                ]),
            ),
        ],
    };
    let r = render(err);
    assert_eq!(r.code, "INVALID_ZONE");
    assert_eq!(r.category, AppErrorCategory::InvalidInput);
    assert_eq!(r.http_status, 422);
    assert_eq!(r.ffi_variant, FfiVariant::Validation);
    assert_eq!(r.message, "unknown zone: footer");
    assert_eq!(r.field, None);
    assert_eq!(
        r.details,
        vec![
            (
                "zone".to_string(),
                super::AppErrorDetail::String("footer".into())
            ),
            (
                "allowed".to_string(),
                super::AppErrorDetail::List(vec![
                    "frontmatter".into(),
                    "body".into(),
                    "sections".into(),
                ])
            ),
        ]
    );
}

/// An unregistered structured code stays Internal / 500 / "internal error",
/// and the redaction also drops context that would otherwise populate
/// `field` (the "column"/"columns" keys) - nothing leaks on this path.
#[test]
fn unregistered_structured_code_redacts_message_field_and_details() {
    let err = DoogatError::Structured {
        code: "SOME_UNREGISTERED_CODE",
        message: "secret repo path /home/x leaked".into(),
        context: vec![
            ("column".into(), ErrorValue::String("url".into())),
            ("columns".into(), ErrorValue::List(vec!["url".into()])),
        ],
    };
    let r = render(err);
    assert_eq!(r.code, "SOME_UNREGISTERED_CODE");
    assert_eq!(r.category, AppErrorCategory::Internal);
    assert_eq!(r.http_status, 500);
    assert_eq!(r.ffi_variant, FfiVariant::Internal);
    assert_eq!(r.message, "internal error");
    assert_eq!(r.field, None);
    assert!(r.details.is_empty());

    // A second, unrelated unregistered code redacts the same way: the rule
    // is the Internal category, not one known code string.
    let other = render(structured("ANOTHER_UNKNOWN_CODE", "also secret"));
    assert_eq!(other.category, AppErrorCategory::Internal);
    assert_eq!(other.http_status, 500);
    assert_eq!(other.message, "internal error");
    assert!(other.details.is_empty());

    // Internal category arriving on any code at all still redacts.
    let hand_built = error_rendering(AppError {
        code: "ANY_OTHER",
        message: "secret repo path /home/x".into(),
        category: AppErrorCategory::Internal,
        field: Some("url".into()),
        details: vec![("table".into(), super::AppErrorDetail::String("t".into()))],
    });
    assert_eq!(hand_built.message, "internal error");
    assert_eq!(hand_built.http_status, 500);
    assert_eq!(hand_built.ffi_variant, FfiVariant::Internal);
    assert_eq!(hand_built.field, None);
    assert!(hand_built.details.is_empty());
}

/// Status, category and FFI hint come from `code` alone. Each probe renders
/// twice with prose that points at a *different* category, and both
/// renderings must land on the code's own row of the policy table.
#[test]
fn rendering_is_code_driven_not_message_driven() {
    use AppErrorCategory as C;
    use FfiVariant as F;
    let probes = [
        (codes::INVALID_ZONE, C::InvalidInput, 422, F::Validation),
        (codes::UNIQUE_VIOLATION, C::Conflict, 409, F::Conflict),
        (codes::SINGLETON_NOT_FOUND, C::NotFound, 404, F::NotFound),
        (codes::SCHEMA_APPLY_PARTIAL, C::Internal, 500, F::Internal),
    ];
    for (code, category, http_status, ffi_variant) in probes {
        for message in ["not found: nothing here", "conflict while validating input"] {
            let r = render(structured(code, message));
            assert_eq!(r.category, category, "category for {code} / {message}");
            assert_eq!(r.http_status, http_status, "status for {code} / {message}");
            assert_eq!(r.ffi_variant, ffi_variant, "ffi hint for {code} / {message}");
        }
    }
    // An AppError that already carries a category keeps it: rendering reads
    // the `category` field instead of re-deriving one from `code`.
    let r = error_rendering(AppError {
        code: "SOME_NEW_CODE",
        message: "no row for that id".into(),
        category: C::NotFound,
        field: Some("id".into()),
        details: vec![("table".into(), super::AppErrorDetail::String("t".into()))],
    });
    assert_eq!(r.category, C::NotFound);
    assert_eq!(r.http_status, 404);
    assert_eq!(r.ffi_variant, F::NotFound);
    assert_eq!(r.message, "no row for that id");
    assert_eq!(r.field.as_deref(), Some("id"));
}

/// Rendering an Internal error emits exactly one ERROR event carrying the
/// unredacted message (the single logging point), and rendering a
/// non-internal error logs nothing.
#[test]
fn internal_rendering_logs_unredacted_message_once_and_non_internal_logs_nothing() {
    struct SharedBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for SharedBuf {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn capture(err: DoogatError) -> String {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&buf);
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || SharedBuf(std::sync::Arc::clone(&sink)))
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            // Other tests may have cached "never" interest for this callsite.
            tracing::callsite::rebuild_interest_cache();
            render(err);
        });
        let bytes: Vec<u8> = buf.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    let logged = capture(structured(codes::SCHEMA_APPLY_PARTIAL, "halted after 2 of 3"));
    assert_eq!(logged.matches("ERROR").count(), 1, "one error event: {logged}");
    assert!(
        logged.contains("halted after 2 of 3"),
        "unredacted message logged: {logged}"
    );

    let quiet = capture(structured(codes::INVALID_ZONE, "unknown zone: footer"));
    assert!(!quiet.contains("ERROR"), "no error event: {quiet}");
    assert!(!quiet.contains("unknown zone"), "nothing logged: {quiet}");
}

#[test]
fn remote_timeout_code_is_registered_in_all() {
    assert_eq!(codes::REMOTE_TIMEOUT, "REMOTE_TIMEOUT");
    assert!(codes::ALL.contains(&codes::REMOTE_TIMEOUT));
}

/// A stalled remote is a retryable conflict-class failure surfaced as a
/// gateway timeout, with an operator-safe message (no URL/credentials).
#[test]
fn remote_timeout_renders_conflict_504_keeping_message() {
    let err = DoogatError::remote_timeout("fetch", std::time::Duration::from_secs(30));
    let r = render(err);
    assert_eq!(r.code, "REMOTE_TIMEOUT");
    assert_eq!(r.category, AppErrorCategory::Conflict);
    assert_eq!(r.http_status, 504);
    assert_eq!(r.ffi_variant, FfiVariant::Conflict);
    assert_eq!(r.message, "remote fetch timed out after 30s");
    assert_eq!(
        r.details,
        vec![(
            "operation".to_string(),
            super::AppErrorDetail::String("fetch".into())
        )]
    );
}
