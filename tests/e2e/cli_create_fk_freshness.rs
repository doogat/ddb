//! Cross-process FK freshness on `ddb create` (issue #16, PRD 00136).
//!
//! `ddb create --type X --set ref=<id>` must succeed when `<id>` was
//! created by a *previous* `ddb create` invocation in the same shell —
//! without an intermediate `ddb reindex` or any other rebuilding command.
//!
//! The bug: `service::create_doogat_with_extra` did not call
//! `ensure_fresh()` on entry, unlike its sibling methods. The second
//! process's fresh `DoogatService` opened a stale view of the
//! materialized typed table, so the FK validator rejected with
//! `REFERENCES_VIOLATION` even though the parent existed in git and in
//! the global `doogats` index.
//!
//! This test exercises two SEPARATE `assert_cmd` processes (the bug only
//! manifests across the cross-process boundary; an in-process service
//! call would hide it).

use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn create_link_after_category_in_separate_process_succeeds_without_reindex() {
    let repo = DdbTestRepo::init();

    // Define typedefs (matches the issue #16 reproduction shape).
    repo.ddb()
        .args(["query", "CREATE TABLE category (fqn VARCHAR(255))"])
        .assert()
        .success()
        .stdout(predicate::str::contains("table category created"));
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE link (url TEXT, category TEXT REFERENCES category)",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("table link created"));

    // Process 1: create the parent category via `ddb create`.
    let cat_out = repo
        .ddb()
        .args([
            "create",
            "--type",
            "category",
            "--title",
            "Cat A",
            "--set",
            "fqn=test.fqn",
        ])
        .output()
        .expect("ddb create category failed to spawn");
    assert!(
        cat_out.status.success(),
        "ddb create category failed: stdout={} stderr={}",
        String::from_utf8_lossy(&cat_out.stdout),
        String::from_utf8_lossy(&cat_out.stderr),
    );
    let cat_id = String::from_utf8_lossy(&cat_out.stdout).trim().to_string();
    assert!(!cat_id.is_empty(), "category id should not be empty");
    assert!(
        cat_id.len() == 14 && cat_id.chars().all(|c| c.is_ascii_digit()),
        "category id must contain exactly 14 digits: {cat_id:?}"
    );

    // Process 2: create the child link referencing the just-created
    // category. NO intermediate `ddb reindex` — the whole point of #16
    // is that this should work without one.
    let link_out = repo
        .ddb()
        .args([
            "create",
            "--type",
            "link",
            "--title",
            "Link A",
            "--set",
            "url=https://a",
            "--set",
            &format!("category={cat_id}"),
        ])
        .output()
        .expect("ddb create link failed to spawn");

    assert!(
        link_out.status.success(),
        "ddb create link rejected; \
         expected success because category '{cat_id}' was created in a prior \
         ddb create process. stdout={} stderr={}",
        String::from_utf8_lossy(&link_out.stdout),
        String::from_utf8_lossy(&link_out.stderr),
    );
    let link_id = String::from_utf8_lossy(&link_out.stdout).trim().to_string();
    assert!(!link_id.is_empty(), "link id should not be empty");
    assert!(
        link_id.len() == 14 && link_id.chars().all(|c| c.is_ascii_digit()),
        "link id must contain exactly 14 digits: {link_id:?}"
    );
    assert_ne!(link_id, cat_id, "link id should differ from category id");
}
