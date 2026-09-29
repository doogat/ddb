//! Bundle-import payload ref namespaces through the `ddb` CLI (PRD 00203, FT-7).
//! Twin: `ddb-core/src/ffi/tests.rs` (`bundle_import_ffi_*`); keep edits paired.
//!
//! Golden workflow, under `fetch.prune` false and true: a real failing import A
//! keeps its fetched refs; node 2 repairs the collision locally so A's stale
//! master would now merge; a masterless bundle B fails without merging A; an
//! independent import C succeeds and removes only its own refs; the repaired
//! retry of A cleans only A's namespace; a final reimport is a no-op.
//! Git and `ddb_core` calls only build fixtures and observe results; every
//! operation under test is a `ddb` CLI invocation.

use std::path::{Path, PathBuf};
use std::process::Output;

use ddb_core::error::DoogatError;
use ddb_core::git_ops::GitRepo;
use ddb_core::hlc::Hlc;
use tempfile::TempDir;

use crate::common::{DdbTestRepo, MultiNodeSetup};

const BUNDLE_NS: &str = "refs/remotes/bundle/";
const COLLISION: &str = "ddb/20260302000000.md";
/// A's losing side of the add/add collision: no frontmatter, so its id cannot
/// be rewritten and the merge must fail.
const LOSER: &str = "Just a plain body with no frontmatter block at all.\n";
const A_EXTRA_ID: &str = "20260303000000";
const SIDECAR: &str = "ddb/20260401000000.md";
const C_ID: &str = "20260305000000";

/// Sorted `(refname, oid)` pairs.
type Refs = Vec<(String, String)>;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("git output must be UTF-8")
}

fn ddb_ok(dir: &Path, args: &[&str]) -> String {
    let out = DdbTestRepo::ddb_at(dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "ddb {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    text(&out.stdout)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn import(dir: &Path, bundle: &Path) -> Output {
    DdbTestRepo::ddb_at(dir)
        .args(["bundle", "import"])
        .arg(bundle)
        .output()
        .unwrap()
}

fn export(dir: &Path, bundle: &Path) {
    let out = bundle.to_str().unwrap();
    ddb_ok(dir, &["bundle", "export", "--full", "--output", out]);
}

fn commit(dir: &Path, path: &str, content: &str) {
    GitRepo::open(dir)
        .unwrap()
        .commit_file(path, content, &format!("write {path}"))
        .unwrap();
}

fn head(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

/// HEAD's parent OIDs, sorted.
fn parents(dir: &Path) -> Vec<String> {
    let mut parents: Vec<String> = git(dir, &["rev-parse", "HEAD^@"])
        .lines()
        .map(str::to_string)
        .collect();
    parents.sort();
    parents
}

/// `path` in the committed HEAD tree; `None` only when it is absent.
fn committed(dir: &Path, path: &str) -> Option<String> {
    match GitRepo::open(dir).unwrap().read_file(path) {
        Ok(content) => Some(content),
        Err(DoogatError::NotFound(_)) => None,
        Err(e) => panic!("reading {path} from HEAD failed: {e}"),
    }
}

/// Pin the repo's machine-local HLC so its next commits carry this clock.
fn seed_hlc(dir: &Path, wall_ms: u64, node: &str) {
    let hlc = Hlc {
        wall_ms,
        counter: 0,
        node: node.into(),
    };
    std::fs::write(dir.join(".git/ddb-hlc"), hlc.to_string()).unwrap();
}

fn clone(src: &Path, parent: &Path) -> PathBuf {
    git(parent, &["clone", src.to_str().unwrap(), "repo"]);
    let repo = parent.join("repo");
    git(&repo, &["config", "commit.gpgsign", "false"]);
    repo
}

/// A parentless commit no branch or bundle reaches: a target for foreign refs
/// that a correct import never merges.
fn throwaway_commit(dir: &Path) -> String {
    let args = [
        "-c",
        "user.name=ddb-test",
        "-c",
        "user.email=ddb-test@example.invalid",
        "commit-tree",
        "--no-gpg-sign",
        "-m",
        "test: foreign ref target",
        "HEAD^{tree}",
    ];
    git(dir, &args).trim().to_string()
}

/// Whether `theirs` now merges into HEAD without conflicts (fixture check).
fn mergeable(dir: &Path, theirs: &str) -> bool {
    let repo = GitRepo::open(dir).unwrap();
    let ours = repo.repo.head().unwrap().peel_to_commit().unwrap();
    let theirs = repo.repo.revparse_single(theirs).unwrap();
    let theirs = theirs.peel_to_commit().unwrap();
    !repo
        .repo
        .merge_commits(&ours, &theirs, None)
        .unwrap()
        .has_conflicts()
}

/// Every ref and its OID, sorted. Malformed records fail the test.
fn all_refs(dir: &Path) -> Refs {
    let out = git(dir, &["for-each-ref", "--format=%(objectname) %(refname)"]);
    let mut refs: Refs = out
        .lines()
        .map(|line| {
            let (oid, name) = line
                .split_once(' ')
                .filter(|(oid, name)| {
                    oid.len() == 40
                        && oid.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                        && !name.is_empty()
                })
                .unwrap_or_else(|| panic!("malformed ref record {line:?}"));
            (name.to_string(), oid.to_string())
        })
        .collect();
    refs.sort();
    refs
}

/// Every ref under `refs/remotes/bundle/`.
fn bundle_refs(dir: &Path) -> Refs {
    all_refs(dir)
        .into_iter()
        .filter(|(name, _)| name.starts_with(BUNDLE_NS))
        .collect()
}

/// The single namespace component under `refs/remotes/bundle/` that `after`
/// has and `before` lacks; it must be a 64-hex payload id.
fn new_namespace(before: &[(String, String)], after: &[(String, String)]) -> String {
    let component = |name: &str| {
        name.strip_prefix(BUNDLE_NS)
            .and_then(|rest| rest.split_once('/'))
            .map(|(ns, _)| ns.to_string())
    };
    let old: Vec<String> = before.iter().filter_map(|(n, _)| component(n)).collect();
    let mut fresh: Vec<String> = after
        .iter()
        .filter_map(|(n, _)| component(n))
        .filter(|ns| !old.contains(ns))
        .collect();
    fresh.sort();
    fresh.dedup();
    assert_eq!(
        fresh.len(),
        1,
        "exactly one new bundle namespace expected, got {fresh:?} in {after:?}"
    );
    let id = fresh.remove(0);
    assert!(
        id.len() == 64 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
        "bundle namespace {id:?} must be a 64-hex payload id"
    );
    id
}

struct Fixture {
    _node1: DdbTestRepo,
    scratch: TempDir,
    node2: PathBuf,
    a_bundle: PathBuf,
    a_master: String,
    /// Every bundle ref the failed import A kept.
    a_refs: Refs,
    /// Commit no bundle reaches; the target of every planted foreign ref.
    foreign_target: String,
}

/// Node 1 and its clone node 2 share an ancestor; node 2 sets `fetch.prune`.
fn two_nodes(prune: bool) -> (DdbTestRepo, TempDir, PathBuf) {
    let node1 = DdbTestRepo::init();
    commit(
        node1.path(),
        "ddb/20260301000000.md",
        "---\nid: 20260301000000\ntitle: Ancestor\n---\nShared body\n",
    );
    let scratch = TempDir::new().unwrap();
    let node2 = clone(node1.path(), scratch.path());
    ddb_ok(node1.path(), &["register-node", "Node1"]);
    ddb_ok(&node2, &["register-node", "Node2"]);
    git(&node2, &["config", "fetch.prune", &prune.to_string()]);
    (node1, scratch, node2)
}

/// Both nodes add `COLLISION`; node 1 (theirs) writes the frontmatter-less
/// loser under the strictly lower HLC and also adds A's extra doogat.
fn diverge_on_collision(node1: &Path, node2: &Path) {
    seed_hlc(node1, u64::MAX / 2, "theirsss");
    commit(
        node1,
        &format!("ddb/{A_EXTRA_ID}.md"),
        "---\nid: 20260303000000\ntitle: Imported A\n---\nA body\n",
    );
    commit(node1, COLLISION, LOSER);
    seed_hlc(node2, u64::MAX / 2 + 1_000_000, "oursssss");
    commit(
        node2,
        COLLISION,
        "---\nid: 20260302000000\ntitle: Winner\n---\nWinner body\n",
    );
}

/// The failed import A kept exactly its delivered `master` under its payload
/// namespace, and nothing else under `refs/remotes/bundle/`.
fn kept_a_refs(dir: &Path, a_master: &str) -> Refs {
    let a_refs = bundle_refs(dir);
    let a_id = new_namespace(&[], &a_refs);
    assert_eq!(
        a_refs,
        vec![(format!("{BUNDLE_NS}{a_id}/master"), a_master.to_string())],
        "setup invalid: failed import A must keep exactly its master under its namespace"
    );
    a_refs
}

/// Import A fails on an unresolvable add/add collision; node 2 then repairs
/// the collision locally with A's exact content, without importing A.
fn failed_a_then_repaired(prune: bool) -> Fixture {
    let (node1, scratch, node2) = two_nodes(prune);
    diverge_on_collision(node1.path(), &node2);

    let a_bundle = scratch.path().join("a.bundle.tar");
    export(node1.path(), &a_bundle);
    let a_master = head(node1.path());
    let out = import(&node2, &a_bundle);
    assert!(
        !out.status.success() && text(&out.stderr).contains("bundle merge failed"),
        "setup invalid: import A must fail as a merge failure, got {out:?}"
    );
    let a_refs = kept_a_refs(&node2, &a_master);

    commit(&node2, COLLISION, LOSER);
    assert!(
        mergeable(&node2, &a_master),
        "setup invalid: A's stale master must now merge cleanly"
    );
    let foreign_target = throwaway_commit(&node2);
    Fixture {
        _node1: node1,
        scratch,
        node2,
        a_bundle,
        a_master,
        a_refs,
        foreign_target,
    }
}

/// A's payload namespace id, taken from the refs the failed import kept.
fn a_payload_id(f: &Fixture) -> String {
    new_namespace(&[], &f.a_refs)
}

/// Export a full bundle whose only branch is `sidecar`; returns its head.
fn export_masterless(bundle: &Path) -> String {
    let repo = DdbTestRepo::init();
    commit(
        repo.path(),
        SIDECAR,
        "---\nid: 20260401000000\ntitle: Sidecar\n---\nSidecar body\n",
    );
    ddb_ok(repo.path(), &["register-node", "Node3"]);
    git(repo.path(), &["branch", "-m", "sidecar"]);
    export(repo.path(), bundle);
    head(repo.path())
}

/// Committed data after B: the local repair stands, and neither A's stale
/// master nor B's sidecar landed.
fn assert_b_merged_nothing(f: &Fixture, prune: bool) {
    assert_eq!(
        committed(&f.node2, COLLISION).as_deref(),
        Some(LOSER),
        "prune={prune}: repaired local data must be unchanged"
    );
    assert_eq!(
        committed(&f.node2, &format!("ddb/{A_EXTRA_ID}.md")),
        None,
        "prune={prune}: B must not merge A's stale master"
    );
    assert_eq!(
        committed(&f.node2, SIDECAR),
        None,
        "prune={prune}: B's data must not land without a delivered master"
    );
}

/// Masterless B fails as a merge failure naming its payload and branches,
/// merges nothing (A's stale master included), adds exactly its fetched head
/// and leaves every other ref untouched. Returns B's payload id.
fn masterless_b_fails(f: &Fixture, prune: bool) -> String {
    let b_bundle = f.scratch.path().join("b.bundle.tar");
    let b_head = export_masterless(&b_bundle);
    let refs_before = all_refs(&f.node2);
    let head_before = head(&f.node2);

    let out = import(&f.node2, &b_bundle);
    let stdout = text(&out.stdout);
    assert!(
        !out.status.success() && !stdout.contains("imported:"),
        "prune={prune}: masterless B must fail, never merge A's stale master; stdout: {stdout}"
    );
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("bundle merge failed:"),
        "prune={prune}: B must report a merge failure, got: {stderr}"
    );
    assert_eq!(head(&f.node2), head_before, "prune={prune}: HEAD moved");
    assert_b_merged_nothing(f, prune);

    let refs_after = all_refs(&f.node2);
    let b_id = new_namespace(&refs_before, &refs_after);
    assert!(
        stderr.contains(&b_id) && stderr.contains("sidecar"),
        "prune={prune}: the error must name B's payload id {b_id} and delivered branch: {stderr}"
    );
    let mut expected = refs_before;
    expected.push((format!("{BUNDLE_NS}{b_id}/sidecar"), b_head));
    expected.sort();
    assert_eq!(
        refs_after, expected,
        "prune={prune}: B adds exactly its fetched head; A's kept refs and all others survive"
    );
    b_id
}

/// Legacy flat ref, sibling-prefix of A's namespace and an unrelated remote
/// ref, all at the fixture's throwaway commit that no bundle reaches.
fn plant_foreign_refs(f: &Fixture, a_id: &str) -> Vec<String> {
    let names = vec![
        format!("{BUNDLE_NS}master"),
        format!("{BUNDLE_NS}{a_id}x/master"),
        "refs/remotes/elsewhere/feature".to_string(),
    ];
    for name in &names {
        git(&f.node2, &["update-ref", name, &f.foreign_target]);
    }
    names
}

/// Independent two-branch C, descended from node 2, imports cleanly and
/// removes exactly its own refs.
fn independent_c_succeeds(f: &Fixture, prune: bool) {
    let dir3 = TempDir::new().unwrap();
    let node3 = clone(&f.node2, dir3.path());
    ddb_ok(&node3, &["register-node", "Node4"]);
    commit(
        &node3,
        &format!("ddb/{C_ID}.md"),
        "---\nid: 20260305000000\ntitle: Imported C\n---\nC body\n",
    );
    git(&node3, &["branch", "feature"]);
    let c_bundle = dir3.path().join("c.bundle.tar");
    export(&node3, &c_bundle);
    let c_master = head(&node3);
    let before = all_refs(&f.node2);

    let out = import(&f.node2, &c_bundle);
    assert!(
        out.status.success() && text(&out.stdout).contains("imported: conflicts resolved: 0"),
        "prune={prune}: independent C must import, got {out:?}"
    );
    assert_eq!(
        head(&f.node2),
        c_master,
        "prune={prune}: C must fast-forward"
    );
    assert!(
        MultiNodeSetup::read(&f.node2, C_ID).contains("title: Imported C"),
        "prune={prune}: C's doogat must be readable"
    );
    let expected: Refs = before
        .into_iter()
        .map(|(name, oid)| {
            let oid = if name == "refs/heads/master" {
                c_master.clone()
            } else {
                oid
            };
            (name, oid)
        })
        .collect();
    assert_eq!(
        all_refs(&f.node2),
        expected,
        "prune={prune}: C removes exactly its own refs; A's, B's, legacy, sibling and \
         unrelated refs survive exactly"
    );
}

/// The repaired retry of A lands as one clean merge of exactly the prior HEAD
/// and A's master: A's and C's doogats are readable, B's data stays out.
fn assert_retry_merges_only_a(f: &Fixture, prune: bool) {
    let head_before = head(&f.node2);
    let out = import(&f.node2, &f.a_bundle);
    assert!(
        out.status.success() && text(&out.stdout).contains("imported: conflicts resolved: 0"),
        "prune={prune}: the repaired retry of A must merge cleanly, got {out:?}"
    );
    let mut expected = vec![head_before, f.a_master.clone()];
    expected.sort();
    assert_eq!(
        parents(&f.node2),
        expected,
        "prune={prune}: the retry must merge exactly A's namespace master"
    );
    assert_eq!(committed(&f.node2, COLLISION).as_deref(), Some(LOSER));
    assert_eq!(
        committed(&f.node2, SIDECAR),
        None,
        "prune={prune}: A's retry must not merge B's data"
    );
    assert!(
        MultiNodeSetup::read(&f.node2, A_EXTRA_ID).contains("title: Imported A"),
        "prune={prune}: A's imported doogat must be readable"
    );
    assert!(
        MultiNodeSetup::read(&f.node2, C_ID).contains("title: Imported C"),
        "prune={prune}: C's doogat must survive A's retry"
    );
}

/// Apart from `master`, the retry removed exactly A's namespace from `before`;
/// every `survivors` prefix still has a ref. Returns the refs after the retry.
fn assert_retry_cleans_only_a(
    f: &Fixture,
    before: &[(String, String)],
    a_id: &str,
    survivors: &[String],
    prune: bool,
) -> Refs {
    let a_ns = format!("{BUNDLE_NS}{a_id}/");
    let without_master = |refs: &[(String, String)]| -> Refs {
        refs.iter()
            .filter(|(name, _)| name != "refs/heads/master")
            .cloned()
            .collect()
    };
    let after = all_refs(&f.node2);
    let expected: Refs = without_master(before)
        .into_iter()
        .filter(|(name, _)| !name.starts_with(&a_ns))
        .collect();
    assert_eq!(
        without_master(after.as_slice()),
        expected,
        "prune={prune}: the retry removes exactly A's namespace"
    );
    for name in survivors {
        assert!(
            after.iter().any(|(n, _)| n.starts_with(name.as_str())),
            "prune={prune}: {name} must survive A's retry, got {after:?}"
        );
    }
    after
}

/// Reimporting A reports the existing no-op and moves neither HEAD nor refs.
fn assert_reimport_is_noop(f: &Fixture, after: &[(String, String)], prune: bool) {
    let head_before = head(&f.node2);
    let out = import(&f.node2, &f.a_bundle);
    assert!(
        out.status.success() && text(&out.stdout).contains("imported: conflicts resolved: 0"),
        "prune={prune}: reimporting A must be the existing no-op, got {out:?}"
    );
    assert_eq!(
        head(&f.node2),
        head_before,
        "prune={prune}: reimport moved HEAD"
    );
    assert_eq!(
        all_refs(&f.node2),
        after,
        "prune={prune}: reimport changed refs"
    );
}

#[test]
fn bundle_import_masterless_bundle_never_consumes_failed_bundle_refs() {
    for prune in [false, true] {
        let f = failed_a_then_repaired(prune);
        // The 00168 upgrade leftover: a flat ref at A's stale master.
        git(
            &f.node2,
            &["update-ref", &format!("{BUNDLE_NS}master"), &f.a_master],
        );
        let b_id = masterless_b_fails(&f, prune);
        assert_ne!(
            a_payload_id(&f),
            b_id,
            "prune={prune}: A and B need distinct namespaces"
        );
    }
}

#[test]
fn bundle_import_success_preserves_sibling_and_legacy_refs() {
    for prune in [false, true] {
        let f = failed_a_then_repaired(prune);
        masterless_b_fails(&f, prune);
        plant_foreign_refs(&f, &a_payload_id(&f));
        independent_c_succeeds(&f, prune);
    }
}

#[test]
fn bundle_import_retry_cleans_only_its_payload_namespace() {
    for prune in [false, true] {
        let f = failed_a_then_repaired(prune);
        let b_id = masterless_b_fails(&f, prune);
        let a_id = a_payload_id(&f);
        let mut survivors = plant_foreign_refs(&f, &a_id);
        survivors.push(format!("{BUNDLE_NS}{b_id}/"));
        independent_c_succeeds(&f, prune);

        let before = all_refs(&f.node2);
        assert_retry_merges_only_a(&f, prune);
        let after = assert_retry_cleans_only_a(&f, &before, &a_id, &survivors, prune);
        assert_reimport_is_noop(&f, &after, prune);
    }
}
