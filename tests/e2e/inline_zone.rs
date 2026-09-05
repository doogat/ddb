use crate::common::DdbTestRepo;
use predicates::prelude::*;

/// PRD 00160: an inline `ZONE frontmatter` declaration on a CREATE TABLE column
/// overrides the implicit (body) zone so the column materializes into the
/// doogat's YAML frontmatter (proven via `ddb read`), and the declared zone
/// survives a `ddb reindex` round-trip.
#[test]
fn inline_zone_frontmatter_materializes_and_survives_reindex() {
    let repo = DdbTestRepo::init();

    // `summary TEXT` defaults to the body zone; `ZONE frontmatter` overrides it.
    // `body_notes TEXT` keeps the default body zone as a control column.
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE note (summary TEXT ZONE frontmatter, body_notes TEXT)",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("table note created"));

    let out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO note (summary, body_notes) VALUES ('quick recap', 'long form notes')",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "insert failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // `summary` lands in frontmatter as a `summary: <value>` YAML key (a body-zone
    // column would not produce such a key). The body control value appears in the body.
    repo.ddb()
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("summary: quick recap"))
        .stdout(predicate::str::contains("long form notes"));

    // Round-trip: reindex rebuilds the typedef from git, so the declared zone
    // persists and a row inserted after reindex still materializes `summary`
    // into frontmatter.
    repo.ddb().arg("reindex").assert().success();

    let out2 = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO note (summary, body_notes) VALUES ('after reindex', 'more body')",
        ])
        .output()
        .unwrap();
    assert!(
        out2.status.success(),
        "post-reindex insert failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let id2 = String::from_utf8_lossy(&out2.stdout).trim().to_string();

    repo.ddb()
        .args(["read", &id2])
        .assert()
        .success()
        .stdout(predicate::str::contains("summary: after reindex"));
    repo.ddb()
        .args(["query", "DROP TABLE note CASCADE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dropped"));
}

/// PRD 00160 (blind review, Critical): two `CREATE TABLE` statements in a single
/// `ddb query` batch must BOTH have their inline `ZONE` applied. Pre-fix, only
/// the first table's column list was stripped, so the second table's `ZONE`
/// token reached the parser and failed the whole batch. Each table's zoned
/// column must materialize into frontmatter independently.
#[test]
fn inline_zone_multi_statement_batch_applies_to_every_table() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args([
            "query",
            "CREATE TABLE alpha (a_sum TEXT ZONE frontmatter); CREATE TABLE beta (b_sum TEXT ZONE frontmatter)",
        ])
        .assert()
        .success();

    // Both tables exist; each table's zoned column lands in frontmatter on insert.
    for (table, col, val) in [
        ("alpha", "a_sum", "alpha recap"),
        ("beta", "b_sum", "beta recap"),
    ] {
        let insert = format!("INSERT INTO {table} ({col}) VALUES ('{val}')");
        let out = repo.ddb().args(["query", &insert]).output().unwrap();
        assert!(
            out.status.success(),
            "insert into {table} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let expected = format!("{col}: {val}");
        repo.ddb()
            .args(["read", &id])
            .assert()
            .success()
            .stdout(predicate::str::contains(expected));
    }
}
