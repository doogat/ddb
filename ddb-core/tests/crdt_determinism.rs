//! Determinism + two-node swap-convergence tests for the CRDT resolver (PRD 00165).

use ddb_core::crdt_resolver::{merge_body, merge_frontmatter};
use ddb_core::types::{ConflictFile, ResolvedFile};

/// Helper: extract the `title:` line from a YAML string.
fn title_line(yaml: &str) -> Option<&str> {
    yaml.lines().find(|l| l.starts_with("title:"))
}

// ── Test 1: re-resolving same conflict is byte-identical ─────────────

#[test]
fn re_resolving_same_conflict_is_byte_identical() {
    // Full three-zone doogat with a frontmatter scalar conflict.
    let ancestor = "---\ntitle: Original\n---\nAncestor body.\n---\n- source:: Ancestor";
    let ours = "---\ntitle: Ours\n---\nOurs body.\n---\n- source:: Ancestor";
    let theirs = "---\ntitle: Theirs\n---\nOurs body.\n---\n- source:: Ancestor";

    let make_conflict = || {
        vec![ConflictFile {
            path: "ddb/20260226120000.md".into(),
            ancestor: Some(ancestor.into()),
            ours: ours.into(),
            theirs: theirs.into(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }]
    };

    let r1: Vec<ResolvedFile> = ddb_core::crdt_resolver::resolve_conflicts(make_conflict(), None)
        .expect("first resolve should succeed");
    let r2: Vec<ResolvedFile> = ddb_core::crdt_resolver::resolve_conflicts(make_conflict(), None)
        .expect("second resolve should succeed");

    assert_eq!(r1.len(), 1);
    assert_eq!(r2.len(), 1);

    // Content must be identical across both resolutions
    assert_eq!(
        r1[0].content, r2[0].content,
        "resolved content must be byte-identical on re-resolution"
    );

    // CRDT state bytes must also be identical (fails with random actor IDs)
    assert_eq!(
        r1[0].fm_crdt_bytes, r2[0].fm_crdt_bytes,
        "fm_crdt_bytes must be byte-identical on re-resolution"
    );
}

// ── Test 2: swapped roles converge on same scalar winner ─────────────

#[test]
fn swapped_roles_converge_on_same_scalar_winner() {
    // Asymmetric edits: X changes ONE key, Y changes TWO keys.
    let ancestor = "title: Base";
    let x = "title: FromX";
    let y = "title: FromY\nauthor: Y";

    // Node A: ours=X, theirs=Y
    let (a, _) = merge_frontmatter(ancestor, x, y).expect("merge A should succeed");
    // Node B: ours=Y, theirs=X (roles swapped)
    let (b, _) = merge_frontmatter(ancestor, y, x).expect("merge B should succeed");

    let title_a = title_line(&a)
        .expect("result A must contain a title line");
    let title_b = title_line(&b)
        .expect("result B must contain a title line");

    assert_eq!(
        title_a, title_b,
        "swapped-role merges must pick the same scalar winner:\nA={}\nB={}",
        title_a, title_b
    );

    // Winner must be one of the two candidates (sanity check)
    let winner = title_a.trim();
    assert!(
        winner == "title: FromX" || winner == "title: FromY",
        "winner must be FromX or FromY, got: {}",
        winner
    );
}

// ── Test 3: swapped roles converge byte-identical for list fields ────

#[test]
fn swapped_roles_converge_byte_identical_for_list_fields() {
    let ancestor = "tags:\n  - shared";
    let x = "tags:\n  - shared\n  - x";
    let y = "tags:\n  - shared\n  - y";

    // Node A: ours=X, theirs=Y
    let (a, _) = merge_frontmatter(ancestor, x, y).expect("merge A should succeed");
    // Node B: ours=Y, theirs=X (roles swapped)
    let (b, _) = merge_frontmatter(ancestor, y, x).expect("merge B should succeed");

    assert_eq!(
        a, b,
        "swapped-role list merges must produce byte-identical YAML:\nA={}\nB={}",
        a, b
    );
}

// ── Test 4: swapped roles converge on same body interleave ───────────

#[test]
fn swapped_roles_converge_on_same_body_interleave() {
    let ancestor = "Line one.\nLine two.\nLine three.\n";
    let x = "Line one X.\nLine two.\nLine three.\n";
    let y = "Line one.\nLine two Y.\nLine three.\n";

    // Node A: ours=X, theirs=Y
    let a = merge_body(ancestor, x, y).expect("body merge A should succeed");
    // Node B: ours=Y, theirs=X (roles swapped)
    let b = merge_body(ancestor, y, x).expect("body merge B should succeed");

    assert_eq!(
        a, b,
        "swapped-role body merges must produce identical interleave:\nA={}\nB={}",
        a, b
    );
}

// ── Test 5: identical frontmatter + different body resolves stably ───

#[test]
fn identical_ours_and_theirs_frontmatter_resolves_stably() {
    let ancestor = "---\ntitle: Same\n---\nAncestor body.";
    let ours = "---\ntitle: Same\n---\nBody ours";
    let theirs = "---\ntitle: Same\n---\nBody theirs";

    let make_conflict = || {
        vec![ConflictFile {
            path: "ddb/20260226120000.md".into(),
            ancestor: Some(ancestor.into()),
            ours: ours.into(),
            theirs: theirs.into(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        }]
    };

    let r1: Vec<ResolvedFile> = ddb_core::crdt_resolver::resolve_conflicts(make_conflict(), None)
        .expect("first resolve should not panic");
    let r2: Vec<ResolvedFile> = ddb_core::crdt_resolver::resolve_conflicts(make_conflict(), None)
        .expect("second resolve should not panic");

    assert_eq!(r1.len(), 1);
    assert_eq!(r2.len(), 1);

    // Both runs must produce identical content and CRDT bytes
    assert_eq!(
        r1[0].content, r2[0].content,
        "identical-fm resolve must be stable across runs"
    );
    assert_eq!(
        r1[0].fm_crdt_bytes, r2[0].fm_crdt_bytes,
        "identical-fm CRDT bytes must be stable across runs"
    );
}

// ── Test 6: empty-ancestor swap convergence (concurrent create) ──────

#[test]
fn empty_ancestor_swap_converges() {
    // Concurrent same-key CREATE with no ancestor (empty string).
    let x = "title: CreatedX";
    let y = "title: CreatedY";

    // Node A: ours=X, theirs=Y
    let (a, _) = merge_frontmatter("", x, y).expect("merge A should succeed");
    // Node B: ours=Y, theirs=X (roles swapped)
    let (b, _) = merge_frontmatter("", y, x).expect("merge B should succeed");

    let title_a = title_line(&a)
        .expect("result A must contain a title line");
    let title_b = title_line(&b)
        .expect("result B must contain a title line");

    assert_eq!(
        title_a, title_b,
        "empty-ancestor swap must converge on same title:\nA={}\nB={}",
        title_a, title_b
    );
}

// Test 7: concurrent SAME-POSITION body inserts converge identically.
// PRD 00165 metric #3: two inserts competing at the SAME offset must
// interleave deterministically (actor-ordered), symmetric under role swap.
// Distinct from Test 4, which edits different lines and never forces a
// same-position tie-break.
#[test]
fn swapped_roles_converge_on_same_position_body_insert() {
    let ancestor = "shared line\n";
    let x = "X-side shared line\n"; // inserts "X-side " at offset 0
    let y = "Y-side shared line\n"; // inserts "Y-side " at the SAME offset 0

    let a = merge_body(ancestor, x, y).expect("body merge A should succeed");
    let b = merge_body(ancestor, y, x).expect("body merge B should succeed");

    assert_eq!(
        a, b,
        "swapped-role SAME-POSITION body inserts must interleave identically:\nA={}\nB={}",
        a, b
    );
}
