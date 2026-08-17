//! Ports tests/integration.sh §29 (lines 3215-3223): `update-bin --help`
//! output and `update-bin --rollback` with no backup present.

use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn integration_29_update_bin_help_and_rollback() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["update-bin", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Update ddb"))
        .stdout(predicate::str::contains("--rollback"));

    // Rollback with no backup should fail gracefully.
    let out = repo
        .ddb()
        .args(["update-bin", "--rollback"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("no backup"));
}
