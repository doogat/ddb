use crate::common::{assert_doogat_id, stdout, DdbTestRepo};
use predicates::prelude::*;

#[test]
fn smoke_09_sql_queries() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["create", "--title", "First note (edited)"])
        .assert()
        .success();

    repo.ddb()
        .args(["query", "SELECT id, title FROM doogats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("First note (edited)"));
}

#[test]
fn smoke_11c_ghost_row_recovery() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE smokeghost (name TEXT, UNIQUE(name))"])
        .assert()
        .success()
        .stdout(predicate::str::contains("table smokeghost created"));
    let id = stdout(
        &repo,
        &[
            "query",
            "INSERT INTO smokeghost (title, name) VALUES ('first', 'uq_a')",
        ],
    );
    assert_doogat_id(&id);

    repo.ddb()
        .args([
            "query",
            "INSERT INTO smokeghost (title, name) VALUES ('dup', 'uq_a')",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"))
        .stderr(predicate::str::contains("UNIQUE"));
    repo.ddb()
        .args([
            "query",
            &format!("UPDATE smokeghost SET title = 'recovered' WHERE id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 row(s) affected"));
    repo.ddb()
        .args([
            "query",
            &format!("SELECT title FROM smokeghost WHERE id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("recovered"));
    repo.ddb()
        .args(["query", "DROP TABLE smokeghost CASCADE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dropped"));
}

#[test]
fn smoke_11d_join_smoke_pin() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE smokelink (url TEXT)"])
        .assert()
        .success()
        .stdout(predicate::str::contains("table smokelink created"));
    repo.ddb()
        .args(["query", "CREATE TABLE smokenum (count INTEGER)"])
        .assert()
        .success()
        .stdout(predicate::str::contains("table smokenum created"));
    repo.ddb()
        .args([
            "query",
            "INSERT INTO smokelink (title, url) VALUES ('a', 'https://a.com')",
        ])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "INSERT INTO smokenum (title, count) VALUES ('a', 1)",
        ])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "SELECT l.title, n.count FROM smokelink l JOIN smokenum n ON l.title = n.title",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("a | 1"));
    repo.ddb()
        .args(["query", "DROP TABLE smokelink CASCADE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dropped"));
    repo.ddb()
        .args(["query", "DROP TABLE smokenum CASCADE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dropped"));
}

#[test]
fn smoke_11a_alter_zone_title_template() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE foo (bar TEXT, baz INTEGER)"])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "ALTER TABLE foo SET ZONE frontmatter FOR bar"])
        .assert()
        .success()
        .stdout(predicate::str::contains("zone set to frontmatter"));
    repo.ddb()
        .args(["query", "ALTER TABLE foo SET TITLE TEMPLATE 'my-template'"])
        .assert()
        .success()
        .stdout(predicate::str::contains("title template set"));
    repo.ddb()
        .args(["query", "ALTER TABLE foo DROP TITLE TEMPLATE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("title template dropped"));
}

#[test]
fn smoke_11b_create_table_if_not_exists() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE foo (bar TEXT, baz INTEGER)"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE IF NOT EXISTS foo (bar TEXT, baz INTEGER)",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("already exists"));
    repo.ddb()
        .args(["query", "CREATE TABLE IF NOT EXISTS newifne (x TEXT)"])
        .assert()
        .success()
        .stdout(predicate::str::contains("table newifne created"));
    repo.ddb()
        .args(["query", "CREATE TABLE IF NOT EXISTS newifne (x TEXT)"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already exists"));
}

#[test]
fn smoke_16f_app_building_flow() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE abcategory (name VARCHAR(100), priority ENUM('low','medium','high'))",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("table abcategory created"));
    let category_id = stdout(
        &repo,
        &[
            "query",
            "INSERT INTO abcategory (name, priority) VALUES ('work', 'high')",
        ],
    );
    assert_doogat_id(&category_id);
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE abbookmark (url VARCHAR(2048), description TEXT, abcategory TEXT REFERENCES abcategory)",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("table abbookmark created"));
    repo.ddb()
        .args(["query", "ALTER TABLE abbookmark SET ZONE reference FOR url"])
        .assert()
        .success()
        .stdout(predicate::str::contains("zone set to reference"));
    repo.ddb()
        .args(["query", "ALTER TABLE abbookmark SET TITLE TEMPLATE '{url}'"])
        .assert()
        .success()
        .stdout(predicate::str::contains("title template set"));
    let bookmark1_id = stdout(
        &repo,
        &[
            "query",
            "INSERT INTO abbookmark (title, url, description) VALUES ('Rust Book', 'https://doc.rust-lang.org', 'The official Rust book')",
        ],
    );
    assert_doogat_id(&bookmark1_id);
    let bookmark2_id = stdout(
        &repo,
        &[
            "query",
            "INSERT INTO abbookmark (url, description) VALUES ('https://crates.io', 'Rust package registry')",
        ],
    );
    assert_doogat_id(&bookmark2_id);
    repo.ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO abbookmark_abcategory (abbookmark_id, abcategory_id) VALUES ('{bookmark1_id}', '{category_id}')"
            ),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 row"));
    repo.ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO abbookmark_abcategory (abbookmark_id, abcategory_id) VALUES ('{bookmark2_id}', '{category_id}')"
            ),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 row"));
    repo.ddb()
        .args(["query", "SELECT url FROM abbookmark"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rust-lang"));
    repo.ddb()
        .args(["query", "SELECT COUNT(*) FROM abbookmark_abcategory"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2"));
    repo.ddb()
        .args(["query", "SELECT priority FROM abcategory"])
        .assert()
        .success()
        .stdout(predicate::str::contains("high"));
    repo.ddb()
        .args(["query", "DROP TABLE abbookmark CASCADE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dropped"));
    repo.ddb()
        .args(["query", "DROP TABLE abcategory CASCADE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dropped"));
}

#[test]
fn smoke_19_boolean_consistency() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE booltest (label TEXT, active BOOLEAN)",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("table booltest created"));
    for sql in [
        "INSERT INTO booltest (label, active) VALUES ('on', true)",
        "INSERT INTO booltest (label, active) VALUES ('off', false)",
        "INSERT INTO booltest (label) VALUES ('none')",
    ] {
        repo.ddb().args(["query", sql]).assert().success();
    }
    repo.ddb()
        .args(["query", "SELECT active FROM booltest WHERE active = 1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("true"));
    repo.ddb()
        .args(["query", "SELECT active FROM booltest WHERE active = 0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("false"));
    repo.ddb()
        .args(["query", "SELECT active FROM booltest WHERE label = 'none'"])
        .assert()
        .success()
        .stdout(predicate::str::contains("NULL"));
    repo.ddb()
        .args([
            "query",
            "SELECT label, active FROM booltest WHERE label = 'on'",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("on"))
        .stdout(predicate::str::contains("true"));
    repo.ddb()
        .args(["query", "DROP TABLE booltest CASCADE"])
        .assert()
        .success();
}

#[test]
fn smoke_20_type_table_self_contained() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE foo (bar TEXT, baz INTEGER)"])
        .assert()
        .success();
    let id = stdout(
        &repo,
        &[
            "query",
            "INSERT INTO foo (title, bar, baz) VALUES ('Self-Contained Test', 'val', 1)",
        ],
    );

    repo.ddb()
        .args([
            "query",
            &format!("SELECT id, title, date, updated_at, bar FROM foo WHERE id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id))
        .stdout(predicate::str::contains("Self-Contained Test"));
    repo.ddb()
        .args(["query", "DROP TABLE foo CASCADE"])
        .assert()
        .success();
}

#[test]
fn smoke_27_on_conflict_do_nothing() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE upsert_test (code TEXT, label TEXT)"])
        .assert()
        .success();

    let typedef = std::fs::read_dir(repo.path().join("ddb/_typedef"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| {
            std::fs::read_to_string(entry.path())
                .unwrap_or_default()
                .contains("title: upsert_test")
        })
        .expect("upsert_test typedef not found");
    let content = std::fs::read_to_string(typedef.path()).unwrap();
    let patched = content.replace(
        "type: _typedef",
        "type: _typedef\nunique_together:\n  - - code",
    );
    std::fs::write(typedef.path(), patched).unwrap();
    for args in [
        &["add", "-A"][..],
        &["commit", "-m", "add unique_together"][..],
    ] {
        let status = std::process::Command::new("git")
            .current_dir(repo.path())
            .args(args)
            .status()
            .expect("git failed to run");
        assert!(status.success(), "git {args:?} failed");
    }
    repo.ddb().arg("reindex").assert().success();

    let id1 = stdout(
        &repo,
        &[
            "query",
            "INSERT INTO upsert_test (title, code, label) VALUES ('First', 'ABC', 'original')",
        ],
    );
    assert_doogat_id(&id1);
    let id2 = stdout(
        &repo,
        &[
            "query",
            "INSERT INTO upsert_test (title, code, label) VALUES ('Second', 'ABC', 'duplicate') ON CONFLICT DO NOTHING",
        ],
    );
    assert_eq!(id2, id1);
    let id3 = stdout(
        &repo,
        &[
            "query",
            "INSERT INTO upsert_test (title, code, label) VALUES ('Third', 'DEF', 'new') ON CONFLICT DO NOTHING",
        ],
    );
    assert_ne!(id3, id1);
    repo.ddb()
        .args(["query", "DROP TABLE upsert_test CASCADE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dropped"));
}

#[test]
fn smoke_30_singleton_typedef_crud() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE smoke_singleton (theme TEXT) SINGLETON",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("table smoke_singleton created"));
    let id = stdout(
        &repo,
        &[
            "query",
            "INSERT INTO smoke_singleton (title, theme) VALUES ('cfg', 'dark')",
        ],
    );
    assert_doogat_id(&id);
    repo.ddb()
        .args([
            "query",
            "INSERT INTO smoke_singleton (title, theme) VALUES ('cfg2', 'light')",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"))
        .stderr(predicate::str::contains("SINGLETON constraint"));
    repo.ddb()
        .args([
            "query",
            &format!("UPDATE smoke_singleton SET theme = 'auto' WHERE id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 row(s) affected"));
    repo.ddb()
        .args([
            "query",
            &format!("SELECT theme FROM smoke_singleton WHERE id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("auto"));
    repo.ddb()
        .args(["query", "DROP TABLE smoke_singleton CASCADE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dropped"));
}
