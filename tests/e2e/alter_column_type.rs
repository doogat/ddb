use crate::common::DdbTestRepo;
use predicates::prelude::*;

// PRD 00128: preserve the VARCHAR(255) -> TEXT regression and integration
// section 47's VARCHAR(32) -> VARCHAR(100) -> TEXT sequence in fresh processes.

#[test]
fn widens_varchar_to_text_and_accepts_longer_values() {
    let repo = DdbTestRepo::init();

    for (table, limit) in [("link", 255), ("ac_link", 32)] {
        repo.ddb()
            .args([
                "query",
                &format!("CREATE TABLE {table} (url VARCHAR({limit}))"),
            ])
            .assert()
            .success();

        let boundary = "a".repeat(limit);
        let inserted = repo
            .ddb()
            .args([
                "query",
                &format!("INSERT INTO {table} (title, url) VALUES ('boundary', '{boundary}')"),
            ])
            .assert()
            .success();
        assert!(!String::from_utf8_lossy(&inserted.get_output().stdout)
            .trim()
            .is_empty());
        let boundary_id = String::from_utf8_lossy(&inserted.get_output().stdout)
            .trim()
            .to_owned();
        assert!(
            boundary_id.len() == 14 && boundary_id.bytes().all(|byte| byte.is_ascii_digit()),
            "boundary INSERT must return a 14-digit id: {boundary_id:?}"
        );

        let too_long = "b".repeat(if limit == 32 { 80 } else { 2000 });
        repo.ddb()
            .args([
                "query",
                &format!("INSERT INTO {table} (title, url) VALUES ('toolong', '{too_long}')"),
            ])
            .assert()
            .failure()
            .stderr(predicate::str::contains("exceeds limit"));

        if limit == 32 {
            repo.ddb()
                .args([
                    "query",
                    "ALTER TABLE ac_link ALTER COLUMN url TYPE VARCHAR(100)",
                ])
                .assert()
                .success();
            let inserted = repo
                .ddb()
                .args([
                    "query",
                    &format!("INSERT INTO ac_link (title, url) VALUES ('now-ok', '{too_long}')"),
                ])
                .assert()
                .success();
            assert!(!String::from_utf8_lossy(&inserted.get_output().stdout)
                .trim()
                .is_empty());
            repo.ddb()
                .args([
                    "query",
                    "ALTER TABLE ac_link ALTER COLUMN url TYPE VARCHAR(5)",
                ])
                .assert()
                .failure()
                .stderr(predicate::str::contains("cannot narrow"))
                .stderr(predicate::str::contains("2 existing rows exceed limit"));
        }

        repo.ddb()
            .args([
                "query",
                &format!("ALTER TABLE {table} ALTER COLUMN url TYPE TEXT"),
            ])
            .assert()
            .success();

        let url_2000 = "b".repeat(2000);
        let inserted = repo
            .ddb()
            .args([
                "query",
                &format!("INSERT INTO {table} (title, url) VALUES ('text-row', '{url_2000}')"),
            ])
            .assert()
            .success();
        assert!(!String::from_utf8_lossy(&inserted.get_output().stdout)
            .trim()
            .is_empty());
        let widened_id = String::from_utf8_lossy(&inserted.get_output().stdout)
            .trim()
            .to_owned();
        assert!(
            widened_id.len() == 14 && widened_id.bytes().all(|byte| byte.is_ascii_digit()),
            "post-widen INSERT must return a 14-digit id: {widened_id:?}"
        );
        repo.ddb()
            .args(["query", &format!("SELECT id FROM {table}")])
            .assert()
            .success()
            .stdout(predicate::str::contains(&widened_id));

        let url_another = "c".repeat(1500);
        repo.ddb()
            .args([
                "query",
                &format!("INSERT INTO {table} (title, url) VALUES ('persisted', '{url_another}')"),
            ])
            .assert()
            .success();
    }
}

#[test]
fn narrowing_rejects_with_row_count_message() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE narrow (body VARCHAR(100))"])
        .assert()
        .success();

    let long = "x".repeat(80);
    repo.ddb()
        .args([
            "query",
            &format!("INSERT INTO narrow (title, body) VALUES ('t', '{long}')"),
        ])
        .assert()
        .success();

    repo.ddb()
        .args([
            "query",
            "ALTER TABLE narrow ALTER COLUMN body TYPE VARCHAR(20)",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot narrow"))
        .stderr(predicate::str::contains("1 existing rows"));
}
