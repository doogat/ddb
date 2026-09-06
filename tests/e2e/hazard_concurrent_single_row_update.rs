//! Hazard H5: two concurrent `ddb update <id> --set` calls on the SAME doogat
//! must not be whole-file last-writer-wins.
//!
//! Evidence (static read, never executed): `update_doogat_parsed` reads the
//! stored file from the HEAD tree (`ddb-core/src/service/update.rs:37-39` via
//! `git_ops/read.rs:25-39`), mutates the parsed copy, and only then calls
//! `commit_file` (`update.rs:103-104`). The repo write lock lives inside
//! `GitRepo::commit_file` (`git_ops/mod.rs:324-326`), so the read-modify-write
//! is NOT atomic across processes and there is no expected-OID check. Two
//! processes can both read base B, one commits `a=<n>` on B, the other commits
//! `b=<n>` on B, and the second commit silently drops the first field.
//!
//! `integration_write_lock_races.rs` deliberately gives every writer its own
//! doogat, so it cannot catch this. This test races two writers on ONE doogat
//! for `ROUNDS` rounds and asserts the SAFE behavior: every writer that exited
//! 0 has its field in HEAD (a loud non-zero refusal is acceptable; a silent
//! loss is not). Failure means H5 is real: a successful update vanished.

use crate::common::{ddb_bin, DdbTestRepo};
use ddb_core::parser::parse;
use ddb_core::types::Value;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};

/// Rounds of the two-writer race; each round spawns both before waiting.
const ROUNDS: usize = 6;

/// Spawn `ddb --repo <repo> update <id> --set <pair>` as its own process.
fn spawn_update(repo: &Path, id: &str, pair: &str) -> Child {
    Command::new(ddb_bin())
        .arg("--repo")
        .arg(repo)
        .args(["update", id, "--set", pair])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn ddb update process")
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

/// A refusal must be loud: a non-zero exit with nothing on stderr is as
/// silent as a lost write.
fn assert_loud_if_refused(round: usize, label: &str, out: &Output) {
    if !out.status.success() {
        assert!(
            !out.stderr.is_empty(),
            "round {round}: writer {label} exited {} with empty stderr - a refusal must be loud",
            out.status
        );
    }
}

#[test]
#[ignore = "fast-track FT-5: hazard H5 confirmed 2026-09-06 (concurrent single-row updates are whole-file last-writer-wins, 6 of 6 rounds); un-ignore with the fix, see dev/local/plans/fast-track-2026-09-06.md"]
fn concurrent_field_updates_on_one_doogat_keep_both_fields_or_refuse_loudly() {
    let repo = DdbTestRepo::init();
    let seed = repo
        .ddb()
        .args(["create", "--title", "H5 shared row"])
        .output()
        .unwrap();
    assert!(
        seed.status.success(),
        "seed create failed: {}",
        String::from_utf8_lossy(&seed.stderr)
    );
    let id = String::from_utf8_lossy(&seed.stdout).trim().to_string();
    assert!(!id.is_empty(), "seed create returned no id");

    let mut lost: Vec<String> = Vec::new();
    for round in 1..=ROUNDS {
        let a_val = format!("alpha-{round}");
        let b_val = format!("beta-{round}");

        // Fire both writers before waiting on either: that is the race.
        let child_a = spawn_update(repo.path(), &id, &format!("a={a_val}"));
        let child_b = spawn_update(repo.path(), &id, &format!("b={b_val}"));
        let out_a = child_a
            .wait_with_output()
            .expect("failed to wait on writer a");
        let out_b = child_b
            .wait_with_output()
            .expect("failed to wait on writer b");

        assert_loud_if_refused(round, "a", &out_a);
        assert_loud_if_refused(round, "b", &out_b);
        assert!(
            out_a.status.success() || out_b.status.success(),
            "round {round}: both writers refused, the round exercised nothing: \
             a stderr={:?} b stderr={:?}",
            String::from_utf8_lossy(&out_a.stderr),
            String::from_utf8_lossy(&out_b.stderr)
        );

        // Durability is judged against the committed HEAD tree, never the
        // work tree or the exit codes alone.
        let head = head_content(repo.path(), &id);
        let committed =
            parse(&head, &format!("ddb/{id}.md")).expect("failed to parse committed doogat");
        for (label, out, expected) in [("a", &out_a, &a_val), ("b", &out_b, &b_val)] {
            if !out.status.success() {
                // Loud refusal: this writer's field is not expected this round.
                continue;
            }
            let actual = committed.meta.extra.get(label);
            if actual != Some(&Value::String(expected.clone())) {
                lost.push(format!(
                    "round {round}: writer {label} exited 0 but HEAD holds {label}={actual:?} \
                     (expected {expected:?}); committed extra={:?}",
                    committed.meta.extra
                ));
            }
        }
    }

    assert!(
        lost.is_empty(),
        "H5 fired: a successful `ddb update --set` vanished from HEAD, so a concurrent \
         single-row update is whole-file last-writer-wins (read outside the write lock, \
         no expected-OID check). Losing rounds:\n{}",
        lost.join("\n")
    );
}
