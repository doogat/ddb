//! Poison-file reindex resilience (PRD 00169).
//!
//! A malformed doogat reaches a repo the way sync and hand edits deliver it:
//! committed by raw git, bypassing the `ddb` CLI's own validation. One such
//! file must never fail the whole batch — a lenient `ddb reindex` skips it,
//! names it, and still indexes every valid doogat around it. `--strict` is the
//! opt-in that restores the old hard failure for callers that must not proceed
//! on a partial index.

use crate::common::DdbTestRepo;
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

/// A repo holding two valid doogats plus a committed poison file whose
/// frontmatter is unparsable YAML. Returns the valid ids.
fn repo_with_poison_file() -> (DdbTestRepo, Vec<String>) {
    let repo = DdbTestRepo::init();
    let ids: Vec<String> = (0..2)
        .map(|k| {
            let out = repo
                .ddb()
                .args([
                    "create",
                    "--title",
                    &format!("valid {k}"),
                    "--body",
                    "content",
                ])
                .output()
                .expect("ddb create failed to run");
            assert!(
                out.status.success(),
                "seed create {k} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        })
        .collect();

    std::fs::write(
        repo.path().join(POISON_PATH),
        "---\n: invalid yaml [\n---\nbody\n",
    )
    .expect("failed to write poison file");
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-m", "add poison doogat"]);

    (repo, ids)
}

/// A lenient `ddb reindex` must skip the unparsable file rather than abort:
/// it exits 0, names the skipped path, and reports every valid doogat as
/// indexed. Pre-00169 one such file failed the whole command.
#[test]
fn lenient_reindex_skips_poison_file_and_indexes_the_rest() {
    let (repo, ids) = repo_with_poison_file();

    let out = repo
        .ddb()
        .args(["reindex"])
        .output()
        .expect("ddb reindex failed to run");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

    assert!(
        out.status.success(),
        "lenient reindex must not fail on a poison file (status {}): \
         stdout={stdout:?} stderr={stderr:?}",
        out.status
    );
    assert!(
        stdout.contains(&format!("indexed {} doogats", ids.len())),
        "reindex must report both valid doogats as indexed: stdout={stdout:?}"
    );
    assert!(
        stderr.contains(POISON_PATH),
        "the skip must name the poison path so it is never silent: stderr={stderr:?}"
    );

    // The valid doogats stay queryable around the skipped file.
    let query = repo
        .ddb()
        .args(["query", "SELECT id FROM doogats"])
        .output()
        .expect("ddb query failed to run");
    let rows = String::from_utf8_lossy(&query.stdout).into_owned();
    for id in &ids {
        assert!(
            rows.contains(id.as_str()),
            "valid doogat {id} must remain queryable after the skip: rows={rows:?}"
        );
    }
}

/// `--strict` is the opt-in hard failure: over the same corpus the command must
/// exit non-zero and its error must name the offending path.
///
/// The assertion targets the `error:` line specifically, not the path anywhere
/// in stderr: the lenient skip logs that same path as a warning, so a
/// `--strict` that regressed to lenient would still print it and a bare
/// `contains` would pass. Only the error line separates an abort from a skip.
/// (Same false-pass shape that was fixed in `tests/smoke.sh` section 32.)
#[test]
fn strict_reindex_fails_and_names_the_poison_file() {
    let (repo, _ids) = repo_with_poison_file();

    let out = repo
        .ddb()
        .args(["reindex", "--strict"])
        .output()
        .expect("ddb reindex --strict failed to run");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

    assert!(
        !out.status.success(),
        "strict reindex must fail on a poison file: stderr={stderr:?}"
    );
    let abort = stderr
        .lines()
        .find(|line| line.starts_with("error:"))
        .unwrap_or_else(|| panic!("strict reindex printed no error line: stderr={stderr:?}"));
    assert!(
        abort.contains(POISON_PATH),
        "the strict abort must name the offending path: error line={abort:?}"
    );
}
