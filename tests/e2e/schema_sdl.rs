use crate::common::DdbTestRepo;
use predicates::prelude::*;
use std::collections::BTreeSet;

/// Two typedefs in one repo: proves every typedef reaches the SDL.
const TWO_TYPES_YAML: &str = "\
types:
  - name: clisdlvalve
    columns:
      - name: valvelabel
        data_type: VARCHAR(255)
        zone: frontmatter
  - name: clisdlpump
    columns:
      - name: pumpcount
        data_type: INTEGER
        zone: frontmatter
";

/// Near-miss of `valvelabel` (shared prefix) used by a second repo.
const NEAR_MISS_YAML: &str = "\
types:
  - name: clisdlvalvex
    columns:
      - name: valvelabelx
        data_type: VARCHAR(255)
        zone: frontmatter
";

/// Init a repo and apply `yaml` through `ddb schema apply`.
fn repo_with(yaml: &str) -> DdbTestRepo {
    let repo = DdbTestRepo::init();
    let path = repo.path().join("desired.yaml");
    std::fs::write(&path, yaml).unwrap();
    repo.ddb()
        .args(["schema", "apply", path.to_str().unwrap()])
        .assert()
        .success();
    repo
}

/// Whole identifiers, so `valvelabel` never matches `valvelabelx`.
fn identifiers(sdl: &str) -> BTreeSet<String> {
    sdl.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn sdl_stdout(repo: &DdbTestRepo) -> String {
    let out = repo.ddb().args(["schema", "sdl"]).output().unwrap();
    assert!(out.status.success(), "schema sdl failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn sdl_prints_every_typedef_to_stdout_without_a_server() {
    // No `serve`, no token, no port: the command alone yields the SDL.
    let repo = repo_with(TWO_TYPES_YAML);
    let sdl = sdl_stdout(&repo);

    assert!(sdl.contains("type Query"), "missing root Query:\n{sdl}");
    assert!(sdl.contains("type Mutation"), "missing root Mutation:\n{sdl}");
    let ids = identifiers(&sdl);
    assert!(ids.contains("valvelabel"), "missing valvelabel:\n{sdl}");
    assert!(ids.contains("pumpcount"), "missing pumpcount:\n{sdl}");
    assert!(!ids.contains("valvelabelx"), "near-miss field leaked:\n{sdl}");
    let lower = sdl.to_lowercase();
    assert!(lower.contains("clisdlvalve") && lower.contains("clisdlpump"), "missing types:\n{sdl}");
}

#[test]
fn sdl_out_writes_same_content_and_keeps_it_off_stdout() {
    let repo = repo_with(TWO_TYPES_YAML);
    let printed = sdl_stdout(&repo);
    let out_dir = tempfile::tempdir().unwrap();
    let out_path = out_dir.path().join("schema.graphql");

    let out = repo
        .ddb()
        .args(["schema", "sdl", "--out", out_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "schema sdl --out failed: {}", String::from_utf8_lossy(&out.stderr));

    let written = std::fs::read_to_string(&out_path).expect("--out must create the file");
    assert_eq!(
        written.trim_end_matches('\n'),
        printed.trim_end_matches('\n'),
        "--out content must equal the stdout SDL"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("type Query"), "--out must not duplicate SDL on stdout:\n{stdout}");
    assert!(!stdout.contains("valvelabel"), "--out must not duplicate SDL on stdout:\n{stdout}");
}

#[test]
fn sdl_on_repo_without_typedefs_prints_base_sdl() {
    let repo = DdbTestRepo::init();
    let sdl = sdl_stdout(&repo);

    assert!(sdl.contains("type Query"), "missing root Query:\n{sdl}");
    assert!(sdl.contains("type Mutation"), "missing root Mutation:\n{sdl}");
    let ids = identifiers(&sdl);
    for token in ["valvelabel", "pumpcount", "valvelabelx"] {
        assert!(!ids.contains(token), "empty repo SDL exposes `{token}`:\n{sdl}");
    }
}

#[test]
fn sdl_reads_the_repo_flag_not_the_cwd() {
    // Run from repo B's directory while pointing --repo at repo A.
    let a = repo_with(TWO_TYPES_YAML);
    let b = repo_with(NEAR_MISS_YAML);

    let out = a
        .ddb()
        .current_dir(b.path())
        .args(["schema", "sdl"])
        .output()
        .unwrap();
    assert!(out.status.success(), "schema sdl failed: {}", String::from_utf8_lossy(&out.stderr));
    let ids = identifiers(&String::from_utf8_lossy(&out.stdout));
    assert!(ids.contains("valvelabel"), "SDL must come from --repo A");
    assert!(!ids.contains("valvelabelx"), "SDL must not come from cwd repo B");

    let b_ids = identifiers(&sdl_stdout(&b));
    assert!(b_ids.contains("valvelabelx") && !b_ids.contains("valvelabel"));
}

#[test]
fn sdl_out_to_unwritable_path_fails_loudly() {
    let repo = repo_with(TWO_TYPES_YAML);
    let out_dir = tempfile::tempdir().unwrap();
    let bad = out_dir.path().join("no-such-dir").join("schema.graphql");

    repo.ddb()
        .args(["schema", "sdl", "--out", bad.to_str().unwrap()])
        .assert()
        .failure()
        .stdout(predicate::str::contains("type Query").not());
    assert!(!bad.exists(), "a failed --out write must not leave a file");
}

#[test]
fn sdl_outside_a_repo_fails() {
    let not_repo = tempfile::tempdir().unwrap();

    DdbTestRepo::ddb_at(not_repo.path())
        .args(["schema", "sdl"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("type Query").not());
}
