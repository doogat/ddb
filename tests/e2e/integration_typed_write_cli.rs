//! Ports tests/integration.sh §48 (lines 3563-3621, PRD 00129), §49 (lines
//! 3626-3669, PRD 00133) and §44.L (lines 3674-3693, PRD 00134): typed
//! write blockers (CREATE INDEX no-op/rejection, ON DELETE CASCADE/RESTRICT,
//! cascade cycle detection), CLI typed `create` populating REFERENCES
//! columns and rejecting wrong-type FKs, and CLI `create` populating an
//! auto-generated junction table atomically.

use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn integration_48_typed_write_blockers_and_cascade() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args([
            "query",
            "CREATE TABLE p9_link (title TEXT, url VARCHAR(255))",
        ])
        .assert()
        .success();

    // §3b: CREATE INDEX IF NOT EXISTS is accepted as a no-op so legacy
    // startup migrations keep working.
    let idx_out = repo
        .ddb()
        .args([
            "query",
            "CREATE INDEX IF NOT EXISTS idx_p9_url ON p9_link(url)",
        ])
        .output()
        .unwrap();
    let idx_combined = format!(
        "{}{}",
        String::from_utf8_lossy(&idx_out.stdout),
        String::from_utf8_lossy(&idx_out.stderr)
    );
    assert!(idx_combined.contains("ignored"));

    // Plain CREATE INDEX (no IF NOT EXISTS) still rejects.
    let plain_out = repo
        .ddb()
        .args(["query", "CREATE INDEX idx_plain ON p9_link(url)"])
        .output()
        .unwrap();
    assert!(!plain_out.status.success());
    let plain_combined = format!(
        "{}{}",
        String::from_utf8_lossy(&plain_out.stdout),
        String::from_utf8_lossy(&plain_out.stderr)
    );
    assert!(plain_combined.contains("CREATE INDEX not supported"));

    // §2: ON DELETE CASCADE walks one level.
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE p9_membership (title TEXT, link VARCHAR(255) REFERENCES p9_link(id) ON DELETE CASCADE)",
        ])
        .assert()
        .success();

    let link_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO p9_link (title, url) VALUES ('Parent', 'https://x')",
        ])
        .output()
        .unwrap();
    let link_id = String::from_utf8_lossy(&link_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let mem_out = repo
        .ddb()
        .args([
            "query",
            &format!("INSERT INTO p9_membership (title, link) VALUES ('Child', '{link_id}')"),
        ])
        .output()
        .unwrap();
    let mem_id = String::from_utf8_lossy(&mem_out.stdout).trim().to_string();
    assert!(!mem_id.is_empty());

    repo.ddb().args(["delete", &link_id]).assert().success();

    let after_link = repo
        .ddb()
        .args([
            "query",
            &format!("SELECT id FROM p9_link WHERE id = '{link_id}'"),
        ])
        .output()
        .unwrap();
    let after_link_combined = format!(
        "{}{}",
        String::from_utf8_lossy(&after_link.stdout),
        String::from_utf8_lossy(&after_link.stderr)
    );
    assert!(!after_link_combined.contains(&link_id));

    let after_mem = repo
        .ddb()
        .args([
            "query",
            &format!("SELECT id FROM p9_membership WHERE id = '{mem_id}'"),
        ])
        .output()
        .unwrap();
    let after_mem_combined = format!(
        "{}{}",
        String::from_utf8_lossy(&after_mem.stdout),
        String::from_utf8_lossy(&after_mem.stderr)
    );
    assert!(!after_mem_combined.contains(&mem_id));

    // §2: ON DELETE RESTRICT (default) blocks parent delete.
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE p9_blocker (title TEXT, link VARCHAR(255) NOT NULL REFERENCES p9_link(id))",
        ])
        .assert()
        .success();

    let link2_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO p9_link (title, url) VALUES ('R Parent', 'https://r')",
        ])
        .output()
        .unwrap();
    let link2_id = String::from_utf8_lossy(&link2_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    repo.ddb()
        .args([
            "query",
            &format!("INSERT INTO p9_blocker (title, link) VALUES ('Block', '{link2_id}')"),
        ])
        .assert()
        .success();

    let restrict_out = repo.ddb().args(["delete", &link2_id]).output().unwrap();
    assert!(!restrict_out.status.success());
    let restrict_combined = format!(
        "{}{}",
        String::from_utf8_lossy(&restrict_out.stdout),
        String::from_utf8_lossy(&restrict_out.stderr)
    );
    assert!(restrict_combined.contains("NOT NULL REFERENCES from p9_blocker.link"));

    // §2: cascade cycle detection.
    repo.ddb()
        .args(["query", "CREATE TABLE p9_a (title TEXT)"])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "CREATE TABLE p9_b (title TEXT)"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "ALTER TABLE p9_a ADD COLUMN b VARCHAR(255) REFERENCES p9_b(id) ON DELETE CASCADE",
        ])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "ALTER TABLE p9_b ADD COLUMN a VARCHAR(255) REFERENCES p9_a(id) ON DELETE CASCADE",
        ])
        .assert()
        .success();

    let a_out = repo
        .ddb()
        .args(["query", "INSERT INTO p9_a (title) VALUES ('A')"])
        .output()
        .unwrap();
    let a_id = String::from_utf8_lossy(&a_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let b_out = repo
        .ddb()
        .args(["query", "INSERT INTO p9_b (title) VALUES ('B')"])
        .output()
        .unwrap();
    let b_id = String::from_utf8_lossy(&b_out.stdout).trim().to_string();

    repo.ddb()
        .args([
            "query",
            &format!("UPDATE p9_a SET b = '{b_id}' WHERE id = '{a_id}'"),
        ])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            &format!("UPDATE p9_b SET a = '{a_id}' WHERE id = '{b_id}'"),
        ])
        .assert()
        .success();

    let cycle_out = repo.ddb().args(["delete", &a_id]).output().unwrap();
    assert!(!cycle_out.status.success());
    let cycle_combined = format!(
        "{}{}",
        String::from_utf8_lossy(&cycle_out.stdout),
        String::from_utf8_lossy(&cycle_out.stderr)
    );
    assert!(cycle_combined.contains("cascade delete would form a cycle"));
}

#[test]
fn integration_49_unify_typed_write_paths() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE tw_category (label VARCHAR(64))"])
        .assert()
        .success();

    let cat1_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO tw_category (title, label) VALUES ('c1', 'alpha')",
        ])
        .output()
        .unwrap();
    let cat1_id = String::from_utf8_lossy(&cat1_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let cat2_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO tw_category (title, label) VALUES ('c2', 'beta')",
        ])
        .output()
        .unwrap();
    let cat2_id = String::from_utf8_lossy(&cat2_out.stdout).trim().to_string();

    // CLI create on a typedef with two REFERENCES columns must populate the
    // reference zone.
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE tw_membership (link VARCHAR(64) REFERENCES tw_category, parent VARCHAR(64) REFERENCES tw_category)",
        ])
        .assert()
        .success();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let mem_out = repo
        .ddb()
        .args([
            "create",
            "--type",
            "tw_membership",
            "--title",
            "M1",
            "--set",
            &format!("link={cat1_id}"),
            "--set",
            &format!("parent={cat2_id}"),
        ])
        .output()
        .unwrap();
    let mem_id = String::from_utf8_lossy(&mem_out.stdout).trim().to_string();
    assert!(!mem_id.is_empty());

    repo.ddb()
        .args(["get", &mem_id])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("link:: [[{cat1_id}]]")))
        .stdout(predicate::str::contains(format!("parent:: [[{cat2_id}]]")));

    // FK pointing at a row of the wrong type must be rejected: the FK check
    // must query the typedef's target table, not just any doogat id.
    repo.ddb()
        .args(["query", "CREATE TABLE tw_link (label VARCHAR(64))"])
        .assert()
        .success();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let tw_link_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO tw_link (title, label) VALUES ('not a category', 'plain')",
        ])
        .output()
        .unwrap();
    let tw_link_id = String::from_utf8_lossy(&tw_link_out.stdout)
        .trim()
        .to_string();
    assert!(!tw_link_id.is_empty());

    let bad_out = repo
        .ddb()
        .args([
            "create",
            "--type",
            "tw_membership",
            "--title",
            "Bogus",
            "--set",
            &format!("link={tw_link_id}"),
            "--set",
            &format!("parent={cat1_id}"),
        ])
        .output()
        .unwrap();
    let bad_combined = format!(
        "{}{}",
        String::from_utf8_lossy(&bad_out.stdout),
        String::from_utf8_lossy(&bad_out.stderr)
    );
    assert!(bad_combined.contains("references non-existent tw_category"));
}

#[test]
fn integration_44_l_cli_create_auto_junction() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE j134l_cat (label VARCHAR(64))"])
        .assert()
        .success();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let cat_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO j134l_cat (title, label) VALUES ('lcat', 'alpha')",
        ])
        .output()
        .unwrap();
    let cat_id = String::from_utf8_lossy(&cat_out.stdout).trim().to_string();
    assert!(!cat_id.is_empty());

    repo.ddb()
        .args([
            "query",
            "CREATE TABLE j134l_bm (url VARCHAR(200), category VARCHAR(64) REFERENCES j134l_cat)",
        ])
        .assert()
        .success();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let bm_out = repo
        .ddb()
        .args([
            "create",
            "--type",
            "j134l_bm",
            "--title",
            "L1",
            "--set",
            "url=https://l.example",
            "--set",
            &format!("category={cat_id}"),
        ])
        .output()
        .unwrap();
    let bm_id = String::from_utf8_lossy(&bm_out.stdout).trim().to_string();
    assert!(!bm_id.is_empty());

    // The auto-generated junction table is named <child-table>_<column-name>.
    repo.ddb()
        .args([
            "query",
            &format!(
                "SELECT bm.id FROM j134l_bm bm JOIN j134l_bm_category j ON j.j134l_bm_id = bm.id WHERE j.category_id = '{cat_id}'"
            ),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(bm_id));
}
