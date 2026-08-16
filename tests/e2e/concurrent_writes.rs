//! Cross-process write-lock durability (PRD 00162).
//!
//! Every git write critical section holds an exclusive advisory lock on
//! `<repo>/.git/ddb-write.lock` (see `ddb_core::git_ops::write_lock`), so
//! concurrent writers in *separate OS processes* serialize their
//! `stage → write_tree → resolve-parent → commit` sections and cannot drop
//! each other's commits by resolving a stale parent and force-updating `HEAD`.
//!
//! These tests spawn real separate `ddb` processes (not in-process threads)
//! contending on one repo. A pre-lock build loses commits here; the locked
//! build keeps every one. Durability is asserted against the git `HEAD` tree,
//! never the working directory: a dropped commit leaves its orphaned file on
//! disk but absent from `HEAD`, so only the committed-tree view proves the
//! guarantee.

use crate::common::{ddb_bin, DdbTestRepo};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Spawn `ddb --repo <repo> <args...>` as its own process with piped output.
fn spawn_ddb(repo: &Path, args: &[&str]) -> Child {
    Command::new(ddb_bin())
        .arg("--repo")
        .arg(repo)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn ddb process")
}

/// The set of doogat ids (14-digit stems of `ddb/<id>.md`) present in the
/// repo's HEAD tree, read via `git ls-tree` so the assertion binds to the
/// committed tree rather than the working directory.
fn head_doogat_ids(repo: &Path) -> BTreeSet<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["ls-tree", "-r", "--name-only", "HEAD"])
        .output()
        .expect("git ls-tree failed to run");
    assert!(
        out.status.success(),
        "git ls-tree HEAD failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|p| p.strip_prefix("ddb/").and_then(|r| r.strip_suffix(".md")))
        .filter(|stem| stem.len() == 14 && stem.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_owned)
        .collect()
}

/// A single doogat's committed content, read from HEAD (not the work tree).
fn head_content(repo: &Path, id: &str) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["show", &format!("HEAD:ddb/{id}.md")])
        .output()
        .expect("git show failed to run");
    assert!(
        out.status.success(),
        "git show HEAD:ddb/{id}.md failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Wait for a spawned `ddb` process and return its trimmed stdout, asserting a
/// clean exit. `label` names the writer in failure messages.
fn wait_ok(child: Child, label: &str) -> String {
    let out = child.wait_with_output().expect("failed to wait on ddb process");
    assert!(
        out.status.success(),
        "{label} failed (status {}): stdout={:?} stderr={:?}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Eight concurrent `ddb create` processes must not lose a commit: every id
/// they report as created is durable in HEAD, and HEAD holds nothing beyond
/// exactly that set.
///
/// The assertion is framed around the *distinct ids the processes actually
/// minted*, not a bare count of 8. `ddb create` mints a 14-digit
/// second-resolution id and checks only the on-disk `ddb/` directory before
/// the commit lock is taken, so two same-second creates in separate processes
/// can mint the *same* id and land on one `ddb/<id>.md`. That is the
/// still-open id-mint race (PRD 00164), orthogonal to the write lock. Folding
/// colliding ids into the expected set keeps this test measuring *only* the
/// write-lock guarantee: no distinct committed id is ever silently dropped
/// from HEAD. (The deterministic no-lost-update proof lives in the update test
/// below, where target ids are distinct by construction.)
#[test]
fn concurrent_cli_create_processes_lose_no_commit() {
    let repo = DdbTestRepo::init();

    // No warm-up create: PRD 00169's rebuild lock now serializes the
    // destructive cold-start index rebuild across processes, so the concurrent
    // writers can start from a cold index. (The warm-up used to hide that
    // race, which PRD 00162's git write lock does not cover.)
    const N: usize = 8;
    // Spawn all N before waiting on any — that overlap is the contention.
    let children: Vec<Child> = (0..N)
        .map(|k| {
            spawn_ddb(
                repo.path(),
                &[
                    "create",
                    "--title",
                    &format!("concurrent writer {k}"),
                    "--body",
                    &format!("body {k}"),
                ],
            )
        })
        .collect();

    let reported: Vec<String> = children
        .into_iter()
        .enumerate()
        .map(|(k, child)| {
            let id = wait_ok(child, &format!("create process {k}"));
            assert!(!id.is_empty(), "create process {k} printed no id");
            id
        })
        .collect();

    let expected: BTreeSet<String> = reported.iter().cloned().collect();
    let head = head_doogat_ids(repo.path());
    assert_eq!(
        head, expected,
        "HEAD doogats must equal the set of ids the concurrent creators \
         minted; a difference means a cross-process commit was lost or \
         orphaned. reported={reported:?} head={head:?}"
    );
}

/// Eight concurrent `ddb update` processes, each rewriting a *distinct*
/// pre-existing doogat, must all land in HEAD.
///
/// Distinct target ids remove the id-mint race (PRD 00164) entirely, so this
/// is the deterministic regression teeth: a pre-lock build drops at least one
/// update's commit via a stale-parent `HEAD` force-update and leaves that
/// doogat's body unchanged in HEAD, while the locked build keeps every update.
#[test]
fn concurrent_cli_update_processes_lose_no_commit() {
    let repo = DdbTestRepo::init();
    const N: usize = 8;

    // Seed N doogats sequentially so their ids are distinct: each create's
    // on-disk existence check bumps a same-second candidate to the next second.
    let ids: Vec<String> = (0..N)
        .map(|k| {
            let child = spawn_ddb(
                repo.path(),
                &[
                    "create",
                    "--title",
                    &format!("update target {k}"),
                    "--body",
                    "original",
                ],
            );
            wait_ok(child, &format!("seed create {k}"))
        })
        .collect();
    assert_eq!(
        ids.iter().collect::<BTreeSet<_>>().len(),
        N,
        "seed ids must be distinct: {ids:?}"
    );

    // Fire N concurrent updates, one per distinct doogat, spawning all before
    // waiting on any.
    let children: Vec<Child> = ids
        .iter()
        .enumerate()
        .map(|(k, id)| {
            spawn_ddb(
                repo.path(),
                &["update", id, "--body", &format!("updated content {k}")],
            )
        })
        .collect();
    for (k, child) in children.into_iter().enumerate() {
        wait_ok(child, &format!("update process {k}"));
    }

    // Every concurrent update must be durable in HEAD.
    for (k, id) in ids.iter().enumerate() {
        let content = head_content(repo.path(), id);
        let needle = format!("updated content {k}");
        assert!(
            content.contains(&needle),
            "update for doogat {id} (writer {k}) was lost: HEAD lacks {needle:?}\n\
             HEAD content:\n{content}"
        );
    }
}
