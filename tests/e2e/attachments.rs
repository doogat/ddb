use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn attach_and_list() {
    let repo = DdbTestRepo::init();

    // Create a doogat
    let out = repo
        .ddb()
        .args(["create", "--title", "Test Doogat", "--body", "Some body"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Create a temp file to attach
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"hello attachment").unwrap();
    let tmp_with_ext = tmp.path().parent().unwrap().join("test-doc.txt");
    std::fs::copy(tmp.path(), &tmp_with_ext).unwrap();

    // Attach
    repo.ddb()
        .args(["attach", &id, tmp_with_ext.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("attached test-doc.txt"));

    // List
    repo.ddb()
        .args(["attachments", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("test-doc.txt"))
        .stdout(predicate::str::contains("text/plain"));

    // Verify via SQL
    repo.ddb()
        .args([
            "query",
            &format!("SELECT name, mime FROM _ddb_attachments WHERE doogat_id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("test-doc.txt"));

    std::fs::remove_file(&tmp_with_ext).ok();
}

#[test]
fn detach_removes_file() {
    let repo = DdbTestRepo::init();

    let out = repo
        .ddb()
        .args(["create", "--title", "Detach Test", "--body", "body"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"data").unwrap();
    let tmp_path = tmp.path().parent().unwrap().join("remove-me.bin");
    std::fs::copy(tmp.path(), &tmp_path).unwrap();

    repo.ddb()
        .args(["attach", &id, tmp_path.to_str().unwrap()])
        .assert()
        .success();

    // Detach
    repo.ddb()
        .args(["detach", &id, "remove-me.bin"])
        .assert()
        .success()
        .stdout(predicate::str::contains("detached"));

    // List should be empty
    repo.ddb()
        .args(["attachments", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("no attachments"));

    // File should be gone from repo
    let file_path = repo.path().join(format!("reference/{id}/remove-me.bin"));
    assert!(!file_path.exists());

    std::fs::remove_file(&tmp_path).ok();
}

#[test]
fn multiple_attachments() {
    let repo = DdbTestRepo::init();

    let out = repo
        .ddb()
        .args(["create", "--title", "Multi", "--body", "body"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    let dir = tempfile::TempDir::new().unwrap();
    for name in &["a.txt", "b.pdf", "c.png"] {
        let p = dir.path().join(name);
        std::fs::write(&p, format!("content of {name}").as_bytes()).unwrap();
        repo.ddb()
            .args(["attach", &id, p.to_str().unwrap()])
            .assert()
            .success();
    }

    // All three listed
    let out = repo.ddb().args(["attachments", &id]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("a.txt"));
    assert!(stdout.contains("b.pdf"));
    assert!(stdout.contains("c.png"));

    // Detach one
    repo.ddb().args(["detach", &id, "b.pdf"]).assert().success();

    let out = repo.ddb().args(["attachments", &id]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("a.txt"));
    assert!(!stdout.contains("b.pdf"));
    assert!(stdout.contains("c.png"));
}

#[test]
fn reindex_preserves_attachments() {
    let repo = DdbTestRepo::init();

    let out = repo
        .ddb()
        .args(["create", "--title", "Reindex Test", "--body", "body"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"reindex data").unwrap();
    let tmp_path = tmp.path().parent().unwrap().join("reindex.txt");
    std::fs::copy(tmp.path(), &tmp_path).unwrap();

    repo.ddb()
        .args(["attach", &id, tmp_path.to_str().unwrap()])
        .assert()
        .success();

    // Reindex
    repo.ddb().args(["reindex"]).assert().success();

    // Attachment still queryable
    repo.ddb()
        .args([
            "query",
            &format!("SELECT name FROM _ddb_attachments WHERE doogat_id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("reindex.txt"));

    std::fs::remove_file(&tmp_path).ok();
}

#[test]
fn attach_overwrites_same_name() {
    let repo = DdbTestRepo::init();

    let out = repo
        .ddb()
        .args(["create", "--title", "Overwrite", "--body", "body"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join("file.txt");

    std::fs::write(&p, b"v1").unwrap();
    repo.ddb()
        .args(["attach", &id, p.to_str().unwrap()])
        .assert()
        .success();

    std::fs::write(&p, b"v2-longer").unwrap();
    repo.ddb()
        .args(["attach", &id, p.to_str().unwrap()])
        .assert()
        .success();

    // Only one attachment, with updated size
    let out = repo.ddb().args(["attachments", &id]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1);
    assert!(stdout.contains("9 bytes")); // "v2-longer" = 9 bytes
}

#[test]
fn detach_nonexistent_errors() {
    let repo = DdbTestRepo::init();

    let out = repo
        .ddb()
        .args(["create", "--title", "No attach", "--body", "body"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    repo.ddb()
        .args(["detach", &id, "missing.txt"])
        .assert()
        .failure();
}

#[test]
fn attach_nonexistent_doogat_errors() {
    let repo = DdbTestRepo::init();

    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join("file.txt");
    std::fs::write(&p, b"data").unwrap();

    repo.ddb()
        .args(["attach", "99999999999999", p.to_str().unwrap()])
        .assert()
        .failure();
}
