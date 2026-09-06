//! Cross-process write-lock races between `ddb serve` and the `ddb` CLI.
//!
//! The headline deployment shape: a long-running server process and an operator
//! at the CLI write to the same repo at the same time. Both sides' writes must
//! be durable, and durability is asserted against the committed `HEAD` tree via
//! `git show`, never the working directory — a commit dropped by a stale-parent
//! `HEAD` force-update leaves its file on disk but absent from `HEAD`, so a
//! work-tree check would pass on exactly the build this test exists to catch.
//! Both calls reporting success is likewise not enough, for the same reason.
//!
//! Several writers per side, not one: the git critical section the lock protects
//! is sub-millisecond, while a CLI process costs ~100-300ms to boot and a
//! GraphQL call is a full HTTP round trip. A single writer per side therefore
//! never overlaps in practice, and passes just as happily on a build with the
//! advisory write lock deleted. `concurrent_writes.rs` establishes the shape
//! that does force overlap — spawn every writer before awaiting any — so this
//! test fires `WRITERS_PER_SIDE` CLI processes and the same number of GraphQL
//! mutations, each against its own pre-seeded doogat, and requires every one of
//! them in `HEAD` at the end.

use crate::common::{ddb_bin, DdbTestRepo, ServerGuard};
use ddb_core::parser::parse;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Concurrent writers fired on each side (GraphQL and CLI) of the race.
const WRITERS_PER_SIDE: usize = 4;

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

/// Wait for a spawned `ddb` process and return its trimmed stdout, asserting a
/// clean exit. `label` names the writer in failure messages.
fn wait_ok(child: Child, label: &str) -> String {
    let out = child
        .wait_with_output()
        .expect("failed to wait on ddb process");
    assert!(
        out.status.success(),
        "{label} failed (status {}): stdout={:?} stderr={:?}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
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

#[test]
fn integration_61_b_concurrent_cli_and_graphql_write_lock() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Seed one target doogat per writer, sequentially, so every concurrent
    // update below rewrites a doogat nobody else touches.
    let serve_ids: Vec<String> = (0..WRITERS_PER_SIDE)
        .map(|k| {
            let seed = server.graphql(&format!(
                r#"mutation {{ createDoogat(input: {{ title: "WL serve seed {k}" }}) {{ id }} }}"#
            ));
            assert!(
                seed.get("errors").is_none(),
                "GraphQL seed create {k} failed: {seed}"
            );
            let id = seed["data"]["createDoogat"]["id"]
                .as_str()
                .unwrap_or_else(|| panic!("GraphQL seed {k} returned no id: {seed}"))
                .to_string();
            assert!(!id.is_empty(), "GraphQL seed {k} returned no id");
            id
        })
        .collect();

    let cli_ids: Vec<String> = (0..WRITERS_PER_SIDE)
        .map(|k| {
            let id = wait_ok(
                spawn_ddb(
                    repo.path(),
                    &["create", "--title", &format!("WL cli seed {k}")],
                ),
                &format!("CLI seed create {k}"),
            );
            assert!(!id.is_empty(), "CLI seed {k} returned no id");
            id
        })
        .collect();

    let seeded: BTreeSet<&String> = serve_ids.iter().chain(cli_ids.iter()).collect();
    assert_eq!(
        seeded.len(),
        2 * WRITERS_PER_SIDE,
        "each concurrent writer needs its own target doogat, otherwise two writers \
         share a file and a lost commit hides behind last-write-wins: \
         serve={serve_ids:?} cli={cli_ids:?}"
    );

    // Fire every writer before waiting on any: the CLI processes boot while the
    // GraphQL mutations are already in flight, so the sides' git critical
    // sections genuinely overlap instead of running back to back.
    let mutations: Vec<String> = serve_ids
        .iter()
        .enumerate()
        .map(|(k, id)| {
            format!(
                r#"mutation {{ updateDoogat(input: {{ id: "{id}", title: "wl-serve-landed-{k}" }}) {{ id }} }}"#
            )
        })
        .collect();

    let server = &server;
    let serve_responses: Vec<Value> = std::thread::scope(|s| {
        let cli_writers: Vec<Child> = cli_ids
            .iter()
            .enumerate()
            .map(|(k, id)| {
                spawn_ddb(
                    repo.path(),
                    &["update", id, "--body", &format!("wl-cli-landed-{k}")],
                )
            })
            .collect();
        let graphql_writers: Vec<_> = mutations
            .iter()
            .map(|mutation| s.spawn(move || server.graphql(mutation)))
            .collect();

        let responses: Vec<Value> = graphql_writers
            .into_iter()
            .enumerate()
            .map(|(k, writer)| {
                writer
                    .join()
                    .unwrap_or_else(|_| panic!("GraphQL writer {k} panicked"))
            })
            .collect();
        for (k, child) in cli_writers.into_iter().enumerate() {
            wait_ok(child, &format!("concurrent CLI update {k}"));
        }
        responses
    });

    // Every GraphQL write reports success against the doogat it aimed at.
    for (k, response) in serve_responses.iter().enumerate() {
        assert!(
            response.get("errors").is_none(),
            "concurrent GraphQL write {k} failed: {response}"
        );
        let updated_id = response["data"]["updateDoogat"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("concurrent GraphQL write {k} returned no id: {response}"));
        assert_eq!(
            updated_id, serve_ids[k],
            "concurrent GraphQL write {k} must report the doogat it updated"
        );
    }

    // Both sides must be durable in the committed tree, not just the work tree.
    for (k, id) in serve_ids.iter().enumerate() {
        let head = head_content(repo.path(), id);
        let needle = format!("wl-serve-landed-{k}");
        assert!(
            head.contains(&needle),
            "serve write {k} not durable in HEAD for {id}: HEAD lacks {needle:?}\n\
             HEAD content:\n{head}"
        );
        let committed = parse(&head, &format!("ddb/{id}.md"))
            .expect("failed to parse committed GraphQL doogat");
        assert_eq!(
            committed.meta.title.as_deref(),
            Some(needle.as_str()),
            "serve write {k} must update the committed title for {id}"
        );
    }
    for (k, id) in cli_ids.iter().enumerate() {
        let head = head_content(repo.path(), id);
        let needle = format!("wl-cli-landed-{k}");
        assert!(
            head.contains(&needle),
            "CLI write {k} not durable in HEAD for {id}: HEAD lacks {needle:?}\n\
             HEAD content:\n{head}"
        );
        let committed =
            parse(&head, &format!("ddb/{id}.md")).expect("failed to parse committed CLI doogat");
        assert_eq!(
            committed.body.trim(),
            needle,
            "CLI write {k} must update the committed body for {id}"
        );
        assert_eq!(
            committed.meta.title,
            Some(format!("WL cli seed {k}")),
            "CLI body write {k} must preserve the committed title for {id}"
        );
    }
}
