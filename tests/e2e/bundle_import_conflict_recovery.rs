use crate::common::{DdbTestRepo, MultiNodeSetup};
use predicates::prelude::*;

/// Bundle-import conflict recovery: two nodes diverge without git-remote sync,
/// a real conflict is resolved through bundle import, and re-importing the same
/// bundle is a no-op (PRD 00168).
#[test]
fn bundle_import_recovers_real_conflict_and_reimport_is_noop() {
    let setup = MultiNodeSetup::new(2);

    let id = MultiNodeSetup::create(&setup.nodes[0], "Shared", "original body");
    MultiNodeSetup::push(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);

    MultiNodeSetup::update(&setup.nodes[0], &id, "Bundle Laptop Title", "laptop body");

    let bundle_path = setup.remote_dir.path().join("conflict.bundle.tar");
    DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["bundle", "export", "--full", "--output"])
        .arg(&bundle_path)
        .assert()
        .success();

    // Node 1: update the SAME doogat's title to a different value ("Desktop" side).
    // This is the conflicting local write — node 1 has NOT synced/fetched node 0's
    // update, so the two nodes diverge only through the bundle (air-gapped scenario).
    MultiNodeSetup::update(&setup.nodes[1], &id, "Bundle Desktop Title", "desktop body");

    DdbTestRepo::ddb_at(&setup.nodes[1])
        .args(["bundle", "import"])
        .arg(&bundle_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("imported: conflicts resolved: 1"));

    // Read the doogat on node 1: either LWW winner title is acceptable — the point
    // is a real merge landed, not which side won.
    let result = DdbTestRepo::ddb_at(&setup.nodes[1])
        .args(["read", &id])
        .assert()
        .success();
    let output = String::from_utf8_lossy(&result.get_output().stdout);
    let title = output
        .lines()
        .find_map(|line| line.strip_prefix("title: "))
        .expect("merged doogat must contain a title line");
    assert!(
        matches!(title, "Bundle Laptop Title" | "Bundle Desktop Title"),
        "merged title must match one of the conflicting values, got: {title:?}"
    );

    // Re-import the SAME bundle: its commits are already ancestors of local HEAD,
    // so the merge classifies as already-up-to-date and resolves zero (new) conflicts.
    DdbTestRepo::ddb_at(&setup.nodes[1])
        .args(["bundle", "import"])
        .arg(&bundle_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("imported: conflicts resolved: 0"));
}
