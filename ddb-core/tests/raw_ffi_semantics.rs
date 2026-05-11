//! PRD 00139 cycle-3 task #1: FFI raw-frontmatter semantics.
//!
//! Pins the contract for `DoogatService::create_doogat_raw` and
//! `DoogatService::update_doogat_raw` — the FFI write entry points used by
//! Swift/Kotlin via UniFFI. Both functions take a raw Markdown string and
//! must round-trip arbitrary frontmatter (registered and unregistered
//! keys, registered and unregistered types) while still enforcing
//! SINGLETON and UNIQUE constraints for *registered* typedefs.

use ddb_core::error::{codes, DoogatError, ErrorValue};
use ddb_core::service::DoogatService;

fn structured_code(err: &DoogatError) -> &'static str {
    match err {
        DoogatError::Structured { code, .. } => code,
        other => panic!("expected Structured error, got: {other:?}"),
    }
}

fn structured_context_string(err: &DoogatError, key: &str) -> String {
    let DoogatError::Structured { context, .. } = err else {
        panic!("expected Structured error, got: {err:?}");
    };
    match context.iter().find(|(k, _)| k == key) {
        Some((_, ErrorValue::String(value))) => value.clone(),
        Some((_, other)) => panic!("ctx key `{key}` must be String, got: {other:?}"),
        None => panic!("ctx is missing key `{key}` in {err:?}"),
    }
}

// -- create_doogat_raw --------------------------------------------------

#[test]
fn create_doogat_raw_accepts_unregistered_type() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();

    // Note: `foo` is NOT a registered typedef. Legacy/FFI contract is that
    // raw create stores the doogat as-is with `type: foo` preserved.
    let content = "\
---
id: 20260101120000
title: UnregisteredTypeRow
type: foo
---
Body content for unregistered type.
";

    let id = svc
        .create_doogat_raw(content, "create unregistered type")
        .expect("create_doogat_raw must accept unregistered type");
    assert_eq!(id, "20260101120000");

    let stored = svc
        .read_doogat(&id)
        .expect("read_doogat must find the row we just created");
    assert!(
        stored.contains("type: foo"),
        "stored frontmatter must preserve `type: foo`; got:\n{stored}"
    );
    assert!(
        stored.contains("title: UnregisteredTypeRow"),
        "stored frontmatter must preserve title; got:\n{stored}"
    );
}

#[test]
fn create_doogat_raw_preserves_custom_frontmatter_keys() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();

    let content = "\
---
id: 20260101120100
title: WithCustomKeys
custom_key: some_value
another: 42
---
Body.
";

    let id = svc
        .create_doogat_raw(content, "create with custom keys")
        .expect("create_doogat_raw must accept arbitrary frontmatter keys");

    let stored = svc.read_doogat(&id).expect("read_doogat must succeed");
    assert!(
        stored.contains("custom_key: some_value"),
        "custom_key must round-trip into stored doogat; got:\n{stored}"
    );
    assert!(
        stored.contains("another: 42"),
        "another: 42 must round-trip into stored doogat; got:\n{stored}"
    );
}

#[test]
fn create_doogat_raw_rejects_second_singleton_insert() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();
    svc.execute_sql("CREATE TABLE app_config (theme TEXT) SINGLETON")
        .unwrap();

    let first = "\
---
id: 20260101120200
title: FirstConfig
type: app_config
theme: dark
---
";
    svc.create_doogat_raw(first, "first singleton row")
        .expect("first singleton create must succeed");

    let second = "\
---
id: 20260101120300
title: SecondConfig
type: app_config
theme: light
---
";
    let err = svc
        .create_doogat_raw(second, "second singleton row")
        .expect_err("second create_doogat_raw on SINGLETON typedef must reject");

    assert_eq!(
        structured_code(&err),
        codes::SINGLETON_VIOLATION,
        "expected SINGLETON_VIOLATION, got: {err:?}"
    );
    assert_eq!(structured_context_string(&err, "table"), "app_config");
}

#[test]
fn create_doogat_raw_rejects_unique_violation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();
    svc.execute_sql("CREATE TABLE link (url TEXT UNIQUE)")
        .unwrap();

    let first = "\
---
id: 20260101130000
title: FirstLink
type: link
url: https://example.com/a
---
";
    svc.create_doogat_raw(first, "first link")
        .expect("first link create must succeed");

    let dup = "\
---
id: 20260101130100
title: DupLink
type: link
url: https://example.com/a
---
";
    let err = svc
        .create_doogat_raw(dup, "duplicate link")
        .expect_err("UNIQUE column collision must reject");

    assert_eq!(
        structured_code(&err),
        codes::UNIQUE_VIOLATION,
        "expected UNIQUE_VIOLATION, got: {err:?}"
    );
}

#[test]
fn create_doogat_raw_rejects_disallowed_allowed_values() {
    // Batch-end follow-up to PRD 00134 doubt review: raw FFI now mirrors the
    // typed path's allowed_values check so apps cannot quietly land
    // out-of-enum values via the FFI raw surface.
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();
    svc.execute_sql("CREATE TABLE color (label TEXT, shade ENUM('light', 'dark'))")
        .unwrap();

    let bad = "\
---
id: 20260101170000
title: BadShade
type: color
shade: neon
---
";
    let err = svc
        .create_doogat_raw(bad, "bad shade")
        .expect_err("create_doogat_raw must reject disallowed values for registered typedef");
    match &err {
        DoogatError::Validation(msg) => assert!(
            msg.contains("not in allowed values"),
            "expected allowed_values rejection wording, got: {msg}"
        ),
        other => panic!("expected DoogatError::Validation, got: {other:?}"),
    }
}

#[test]
fn create_doogat_raw_rejects_fk_to_nonexistent_target() {
    // Batch-end follow-up to PRD 00134 doubt review: raw FFI now checks
    // REFERENCES targets exist when the declared type is a registered typedef.
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();
    svc.execute_sql("CREATE TABLE category (label VARCHAR(64))")
        .unwrap();
    svc.execute_sql("CREATE TABLE link (target VARCHAR(64) REFERENCES category)")
        .unwrap();

    let bad = "\
---
id: 20260101180000
title: BogusLink
type: link
target: 99999999999999
---
";
    let err = svc
        .create_doogat_raw(bad, "bogus link")
        .expect_err("create_doogat_raw must reject FK to non-existent target");
    match &err {
        DoogatError::Validation(msg) => assert!(
            msg.contains("non-existent"),
            "expected dangling-reference wording, got: {msg}"
        ),
        other => panic!("expected DoogatError::Validation, got: {other:?}"),
    }
}

#[test]
fn update_doogat_raw_rejects_disallowed_allowed_values() {
    // Symmetry guard: update_doogat_raw runs the same field validation as
    // create_doogat_raw so apps cannot bypass allowed_values via update.
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();
    svc.execute_sql("CREATE TABLE color (label TEXT, shade ENUM('light', 'dark'))")
        .unwrap();

    let good = "\
---
id: 20260101190000
title: GoodShade
type: color
shade: light
---
";
    let id = svc
        .create_doogat_raw(good, "good shade")
        .expect("baseline good create must succeed");

    let bad = "\
---
id: 20260101190000
title: GoodShade
type: color
shade: neon
---
";
    let err = svc
        .update_doogat_raw(&id, bad, "bad shade")
        .expect_err("update_doogat_raw must reject disallowed values for registered typedef");
    match &err {
        DoogatError::Validation(msg) => assert!(
            msg.contains("not in allowed values"),
            "expected allowed_values rejection wording, got: {msg}"
        ),
        other => panic!("expected DoogatError::Validation, got: {other:?}"),
    }
}

// -- update_doogat_raw --------------------------------------------------

#[test]
fn update_doogat_raw_preserves_date_field() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();

    let original = "\
---
id: 20260101140000
title: WithDate
date: 2026-01-15
---
Original body.
";
    let id = svc
        .create_doogat_raw(original, "create with date")
        .expect("create with date must succeed");

    let updated = "\
---
id: 20260101140000
title: WithDate
date: 2026-05-15
---
Updated body.
";
    svc.update_doogat_raw(&id, updated, "bump date")
        .expect("update_doogat_raw must accept new date");

    let stored = svc.read_doogat(&id).expect("read after update");
    assert!(
        stored.contains("date: 2026-05-15"),
        "update_doogat_raw must replace the date field; got:\n{stored}"
    );
    assert!(
        !stored.contains("date: 2026-01-15"),
        "stale date must not survive; got:\n{stored}"
    );
}

#[test]
fn update_doogat_raw_preserves_custom_frontmatter_keys() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();

    let original = "\
---
id: 20260101150000
title: CustomKeysDoogat
custom_key: original
---
Body.
";
    let id = svc
        .create_doogat_raw(original, "create with custom_key")
        .expect("create must succeed");

    let updated = "\
---
id: 20260101150000
title: CustomKeysDoogat
custom_key: foo
---
Body.
";
    svc.update_doogat_raw(&id, updated, "change custom_key")
        .expect("update_doogat_raw must accept replacement");

    let stored = svc.read_doogat(&id).expect("read after update");
    assert!(
        stored.contains("custom_key: foo"),
        "custom_key must reflect the new value; got:\n{stored}"
    );
}

#[test]
fn update_doogat_raw_clears_title_when_omitted_from_new_content() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();

    let original = "\
---
id: 20260101160000
title: Original
---
Body.
";
    let id = svc
        .create_doogat_raw(original, "create with title")
        .expect("create must succeed");

    // Replacement has NO title in frontmatter. Raw-replace semantics:
    // the new content fully replaces the old, so the title must clear.
    let updated = "\
---
id: 20260101160000
---
Body without title.
";
    svc.update_doogat_raw(&id, updated, "drop title")
        .expect("update_doogat_raw must accept title-less replacement");

    let stored = svc.read_doogat(&id).expect("read after update");
    assert!(
        !stored.contains("title: Original"),
        "old title must not survive a raw replacement that omits it; got:\n{stored}"
    );
    assert!(
        !stored.contains("title:"),
        "no `title:` key should remain after raw replacement omits it; got:\n{stored}"
    );
}
