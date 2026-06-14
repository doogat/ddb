//! CLI `ddb create --type <unregistered>` regression (PRD 00155; nightly
//! `full-validation` red since 2026-05-29 at `tests/smoke.sh:33`).
//!
//! PRD 00147 rerouted CLI create through the app facade, whose
//! `batch_create_with_message` contract rejects an unregistered `doogat_type`
//! with `TYPE_NOT_REGISTERED` (GraphQL `createDoogat` parity). That silently
//! replaced the released (<= v0.2.5) CLI contract, where an unregistered
//! `--type` produced a base-only doogat (PRD 00129 §T3 / PRD 00133). PRD 00155
//! restores it via an explicit `UnregisteredTypePolicy::BaseOnly` on the CLI
//! create command; the lenient path now surfaces an `UNREGISTERED_TYPE_BASE_ONLY`
//! warning on stderr (the released path was silent).
//!
//! This is the exact `tests/smoke.sh:33` shape: `ddb create --type project`
//! on a fresh repo with no `ddb type install project`.

use crate::common::DdbTestRepo;

#[test]
fn cli_create_unregistered_type_creates_base_doogat_with_warning() {
    let repo = DdbTestRepo::init();

    // smoke.sh:33 shape: unregistered `project` type on a fresh repo.
    let out = repo
        .ddb()
        .args([
            "create",
            "--title",
            "Project Alpha",
            "--type",
            "project",
            "--tags",
            "active",
            "--body",
            "A project doogat",
        ])
        .output()
        .expect("ddb create failed to spawn");

    assert!(
        out.status.success(),
        "ddb create --type project (unregistered) must exit 0 (base-only \
         create; the <= v0.2.5 contract PRD 00155 restores). stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // stdout is the new doogat's 14-digit id.
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(
        id.len(),
        14,
        "stdout should be a 14-digit doogat id, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_digit()),
        "doogat id should be all digits, got {id:?}"
    );

    // The lenient base-only path surfaces the warning on stderr; the released
    // path was silent, so PRD 00155 adds the `UNREGISTERED_TYPE_BASE_ONLY` code.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("UNREGISTERED_TYPE_BASE_ONLY"),
        "stderr must carry the UNREGISTERED_TYPE_BASE_ONLY warning, got: {stderr}"
    );
}
