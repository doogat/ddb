//! Repo-aware ID minting and poison-file reindex warnings.
//!
//! §62 (PRD 00164, unified repo-aware ID minting): the batch (multi-row
//! INSERT) and single (`ddb create`) mint paths agree on "id taken", so
//! back-to-back mints in the same wall-clock second yield distinct,
//! non-colliding ids. No sleep between the two mints is deliberate — that's
//! exactly the same-second workaround PRD 00164 removed.
//!
//! §63 (PRD 00169, poison-file reindex resilience): a doogat planted by a
//! raw git commit (bypassing the `ddb` CLI's own validation) with
//! unparseable frontmatter triggers a `REINDEX_SKIPPED_FILES` warning that
//! names the skipped path. The warning surfaces on the FIRST GraphQL
//! mutation that calls `ensure_fresh()` after the poison commit; a second
//! mutation would see nothing stale and return no warnings.

use crate::common::{DdbTestRepo, ServerGuard};
use predicates::prelude::*;
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

/// A far-future id, so the poison file can never collide with a minted one.
const POISON_PATH: &str = "ddb/29990101000000.md";

/// Run a raw `git` command in `repo`, asserting it succeeded.
fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("git failed to run");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// True if `s` is exactly 14 ASCII digits (the `ddb` id shape).
fn is_14_digit_id(s: &str) -> bool {
    s.len() == 14 && s.chars().all(|c| c.is_ascii_digit())
}

/// PRD 00164 removed the stale same-second mint workaround: the batch
/// (multi-row INSERT) and single (`ddb create`) mint paths must not collide
/// even with zero delay between them. Deliberately no `sleep` between the
/// two mints below — that gap is exactly what this test proves unnecessary.
#[test]
fn integration_62_id_minting_no_same_second_collision() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE int164 (label TEXT)"])
        .assert()
        .success();

    let batch_assert = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO int164 (label) VALUES ('a'), ('b'), ('c')",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(","));
    let batch_stdout = String::from_utf8_lossy(&batch_assert.get_output().stdout)
        .trim()
        .to_owned();

    let single_assert = repo
        .ddb()
        .args([
            "create",
            "--type",
            "int164",
            "--title",
            "single 164",
            "--set",
            "label=d",
        ])
        .assert()
        .success();
    let single_stdout = String::from_utf8_lossy(&single_assert.get_output().stdout)
        .trim()
        .to_owned();

    let batch_ids: Vec<&str> = batch_stdout.split(',').collect();
    assert_eq!(
        batch_ids.len(),
        3,
        "expected 3 comma-separated ids from the batch insert: {batch_stdout:?}"
    );
    for id in &batch_ids {
        assert!(
            is_14_digit_id(id),
            "expected a 14-digit numeric id, got {id:?} in {batch_stdout:?}"
        );
    }
    assert!(
        is_14_digit_id(&single_stdout),
        "expected a single 14-digit numeric id, got {single_stdout:?}"
    );

    let mut all_ids: Vec<String> = batch_ids.into_iter().map(str::to_owned).collect();
    all_ids.push(single_stdout);
    let unique: HashSet<String> = all_ids.into_iter().collect();
    assert_eq!(
        unique.len(),
        4,
        "batch and single mint paths must yield 4 distinct ids (no same-second collision): {unique:?}"
    );
}

/// A poison file committed by raw git must surface a named
/// `REINDEX_SKIPPED_FILES` warning on the first mutation after the poison
/// commit — never silently, and never blocking the mutation itself.
///
/// Ordering is load-bearing: the server starts (and records HEAD) BEFORE the
/// poison file is planted, and the assertion targets the very first mutation
/// after the poison commit. A warm-up call before this assertion would
/// consume the warning and turn the check into a false pass.
#[test]
fn integration_63_poison_file_graphql_warning() {
    let repo = DdbTestRepo::init();
    // Seed one ordinary doogat so the index is non-empty and HEAD exists
    // BEFORE the server starts.
    repo.ddb()
        .args(["create", "--title", "seed", "--body", "seed body"])
        .assert()
        .success();

    let server = ServerGuard::start(&repo);

    // Plant the poison file directly via raw git, bypassing the `ddb` CLI's
    // own validation, AFTER the server has already started and recorded HEAD.
    std::fs::write(
        repo.path().join(POISON_PATH),
        "---\n: invalid yaml [\n---\nbody\n",
    )
    .expect("failed to write poison file");
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-m", "add poison doogat"]);

    // First mutation since server start / since the poison commit — this is
    // where the warning fires.
    let result = server
        .graphql(r#"mutation { createDoogat(input: { title: "after poison" }) { id title } }"#);

    assert!(
        result.get("errors").is_none(),
        "mutation should succeed despite the poison file elsewhere in the repo: {result}"
    );
    assert!(
        result.get("data").is_some(),
        "mutation response must contain data: {result}"
    );

    let warnings = result["extensions"]["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("expected extensions.warnings to be an array: {result}"));
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one warning on the first mutation after the poison commit: {result}"
    );
    assert_eq!(
        warnings[0]["code"].as_str(),
        Some("REINDEX_SKIPPED_FILES"),
        "unexpected warning code: {result}"
    );
    let message = warnings[0]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected warning message to be a string: {result}"));
    assert!(
        message.contains("29990101000000"),
        "the skipped file must be named in the warning message: {message:?}"
    );
}
