use crate::common::DdbTestRepo;
use predicates::prelude::*;

/// Write `yaml` to `<name>.yaml` inside the repo dir and return its absolute path.
fn write_schema(repo: &DdbTestRepo, name: &str, yaml: &str) -> String {
    let path = repo.path().join(format!("{name}.yaml"));
    std::fs::write(&path, yaml).unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn dry_run_prints_plan_and_creates_nothing() {
    // `schema apply --dry-run` for a brand-new type prints a single create_type
    // op (kind + table + destructive flag + rendered CREATE TABLE SQL) and
    // mutates nothing: the table is NOT queryable afterward.
    let repo = DdbTestRepo::init();
    let yaml = "\
types:
  - name: dryrunwidget
    columns:
      - name: drylabel
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
";
    let schema = write_schema(&repo, "dryrunwidget", yaml);

    repo.ddb()
        .args(["schema", "apply", &schema, "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("create_type"))
        .stdout(predicate::str::contains("dryrunwidget"))
        .stdout(predicate::str::contains("CREATE TABLE"))
        // The rendered CREATE TABLE SQL must NAME the declared column, not just
        // print the bare `CREATE TABLE` keyword. A hardcoded banner that never
        // reads the schema cannot satisfy this.
        .stdout(predicate::str::contains("drylabel"));

    // Nothing was created: querying the declared type fails.
    repo.ddb()
        .args(["query", "SELECT drylabel FROM dryrunwidget"])
        .assert()
        .failure();
}

#[test]
fn diff_matches_dry_run_and_creates_nothing() {
    // `schema diff <file>` is equivalent to `schema apply <file> --dry-run`:
    // same plan content, no mutation. The two stdout streams must be
    // BYTE-IDENTICAL (modulo a single trailing newline), forcing one shared
    // plan-rendering code path rather than two independently hardcoded banners.
    let repo = DdbTestRepo::init();
    let yaml = "\
types:
  - name: diffgadget
    columns:
      - name: difftag
        data_type: TEXT
        zone: body
";
    let schema = write_schema(&repo, "diffgadget", yaml);

    let a = repo.ddb().args(["schema", "diff", &schema]).output().unwrap();
    let b = repo
        .ddb()
        .args(["schema", "apply", &schema, "--dry-run"])
        .output()
        .unwrap();
    assert!(a.status.success() && b.status.success());
    let sa = String::from_utf8_lossy(&a.stdout);
    let sb = String::from_utf8_lossy(&b.stdout);

    // Sanity: the shared output is a real create_type plan naming the column.
    assert!(sa.contains("create_type"));
    assert!(sa.contains("diffgadget"));
    assert!(sa.contains("CREATE TABLE"));
    assert!(sa.contains("difftag"));

    assert_eq!(
        sa.trim_end(),
        sb.trim_end(),
        "diff must be byte-identical to apply --dry-run"
    );

    // diff mutated nothing: the type is still absent.
    repo.ddb()
        .args(["query", "SELECT difftag FROM diffgadget"])
        .assert()
        .failure();
}

#[test]
fn apply_creates_declared_type() {
    // A real apply (no flags) creates the declared type as a real, queryable
    // table. Cross-check: the column is selectable afterward.
    let repo = DdbTestRepo::init();
    let yaml = "\
types:
  - name: realsprocket
    columns:
      - name: reallabel
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
";
    let schema = write_schema(&repo, "realsprocket", yaml);

    repo.ddb()
        .args(["schema", "apply", &schema])
        .assert()
        .success();

    // The type now exists and the declared column is queryable.
    repo.ddb()
        .args(["query", "SELECT reallabel FROM realsprocket"])
        .assert()
        .success();
}

#[test]
fn add_column_drift_dry_run_shows_add_not_create() {
    // The strongest discriminator: a type already exists with ONE column; the
    // desired schema declares a SECOND column. A real diff must report an
    // `add_column` op (ALTER TABLE ... ADD COLUMN) and MUST NOT report
    // `create_type` (the type is not new). A fake that always prints
    // `create_type` dies here. Dry-run mutates nothing.
    let repo = DdbTestRepo::init();

    // Step 1: create the type for real with a single column.
    let one_col = "\
types:
  - name: driftvalve
    columns:
      - name: basecol
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
";
    let one_schema = write_schema(&repo, "driftvalve_one", one_col);
    repo.ddb()
        .args(["schema", "apply", &one_schema])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "SELECT basecol FROM driftvalve"])
        .assert()
        .success();

    // Step 2: desired schema adds a second column; dry-run the drift.
    let two_col = "\
types:
  - name: driftvalve
    columns:
      - name: basecol
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
      - name: extracol
        data_type: TEXT
        zone: body
";
    let two_schema = write_schema(&repo, "driftvalve_two", two_col);

    repo.ddb()
        .args(["schema", "apply", &two_schema, "--dry-run"])
        .assert()
        .success()
        // The op is add_column and it names the new column.
        .stdout(predicate::str::contains("add_column"))
        .stdout(predicate::str::contains("extracol"))
        // The rendered SQL is an ALTER TABLE ... ADD COLUMN, not a CREATE TABLE.
        .stdout(predicate::str::contains("ALTER TABLE"))
        .stdout(predicate::str::contains("ADD COLUMN"))
        // The type already exists: a real diff must NOT claim create_type.
        .stdout(predicate::str::contains("create_type").not());

    // Dry-run mutated nothing: the new column was NOT actually added.
    repo.ddb()
        .args(["query", "SELECT extracol FROM driftvalve"])
        .assert()
        .failure();
}

#[test]
fn reapply_is_idempotent_noop() {
    // Re-running the same apply once converged exits 0 and reports nothing to
    // do (empty plan), and the type remains queryable.
    let repo = DdbTestRepo::init();
    let yaml = "\
types:
  - name: idempotentcog
    columns:
      - name: idemlabel
        data_type: TEXT
        zone: body
";
    let schema = write_schema(&repo, "idempotentcog", yaml);

    repo.ddb()
        .args(["schema", "apply", &schema])
        .assert()
        .success();

    // Second apply: no operations to perform.
    repo.ddb()
        .args(["schema", "apply", &schema])
        .assert()
        .success();

    // A diff after convergence shows no create/add/drop op for this table.
    repo.ddb()
        .args(["schema", "diff", &schema])
        .assert()
        .success()
        .stdout(predicate::str::contains("create_type").not())
        .stdout(predicate::str::contains("add_column").not())
        .stdout(predicate::str::contains("drop_column").not());

    // Still queryable.
    repo.ddb()
        .args(["query", "SELECT idemlabel FROM idempotentcog"])
        .assert()
        .success();
}

#[test]
fn destructive_drop_blocked_without_allow_flag() {
    // Live type has label+note; desired declares only label, so the plan would
    // DROP note (destructive). Without --allow-destructive the apply is
    // refused (non-zero, stderr mentions destructive) and the live schema is
    // UNCHANGED: `note` is still selectable.
    let repo = DdbTestRepo::init();

    let full = "\
types:
  - name: blockedflange
    columns:
      - name: keeplabel
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
      - name: dropnote
        data_type: TEXT
        zone: body
";
    let full_schema = write_schema(&repo, "blockedflange_full", full);
    repo.ddb()
        .args(["schema", "apply", &full_schema])
        .assert()
        .success();
    // Both columns present after the initial create.
    repo.ddb()
        .args(["query", "SELECT dropnote FROM blockedflange"])
        .assert()
        .success();

    let reduced = "\
types:
  - name: blockedflange
    columns:
      - name: keeplabel
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
";
    let reduced_schema = write_schema(&repo, "blockedflange_reduced", reduced);

    // The plan for the reduced schema must surface a real drop_column op that
    // NAMES the column being dropped (ALTER TABLE ... DROP COLUMN dropnote). A
    // create-only fake never emits drop_column, so it dies here.
    repo.ddb()
        .args(["schema", "diff", &reduced_schema])
        .assert()
        .success()
        .stdout(predicate::str::contains("drop_column"))
        .stdout(predicate::str::contains("dropnote"));

    // Apply without the flag is refused, and names the destructive block.
    repo.ddb()
        .args(["schema", "apply", &reduced_schema])
        .assert()
        .failure()
        .stderr(predicate::str::contains("destructive"));

    // The drop did NOT happen: `dropnote` is still selectable.
    repo.ddb()
        .args(["query", "SELECT dropnote FROM blockedflange"])
        .assert()
        .success();
}

#[test]
fn destructive_drop_allowed_with_flag() {
    // Same destructive scenario, but with --allow-destructive the apply
    // proceeds (exit 0) and the column is actually dropped: `note` is no longer
    // selectable, while the kept column still is.
    let repo = DdbTestRepo::init();

    let full = "\
types:
  - name: allowedbracket
    columns:
      - name: stablelabel
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
      - name: goingnote
        data_type: TEXT
        zone: body
";
    let full_schema = write_schema(&repo, "allowedbracket_full", full);
    repo.ddb()
        .args(["schema", "apply", &full_schema])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "SELECT goingnote FROM allowedbracket"])
        .assert()
        .success();

    let reduced = "\
types:
  - name: allowedbracket
    columns:
      - name: stablelabel
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
";
    let reduced_schema = write_schema(&repo, "allowedbracket_reduced", reduced);

    repo.ddb()
        .args(["schema", "apply", &reduced_schema, "--allow-destructive"])
        .assert()
        .success();

    // The drop happened: `goingnote` is gone, `stablelabel` remains.
    repo.ddb()
        .args(["query", "SELECT goingnote FROM allowedbracket"])
        .assert()
        .failure();
    repo.ddb()
        .args(["query", "SELECT stablelabel FROM allowedbracket"])
        .assert()
        .success();
}
