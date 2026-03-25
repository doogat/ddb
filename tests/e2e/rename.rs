use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn rename_moves_file_and_rewrites_backlinks() {
    let repo = DdbTestRepo::init();

    // Create target doogat B
    let b_out = repo
        .ddb()
        .args(["create", "--title", "Target", "--body", "I am B."])
        .output()
        .unwrap();
    let b_id = String::from_utf8_lossy(&b_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create doogat A linking to B via bare ID
    let a_out = repo
        .ddb()
        .args([
            "create",
            "--title",
            "Linker A",
            "--body",
            &format!("See [[{b_id}|Target]]."),
        ])
        .output()
        .unwrap();
    let a_id = String::from_utf8_lossy(&a_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create doogat C also linking to B
    repo.ddb()
        .args([
            "create",
            "--title",
            "Linker C",
            "--body",
            &format!("Also see [[{b_id}]]."),
        ])
        .assert()
        .success();

    // Reindex so wikilinks are in _ddb_links
    // (create command doesn't extract wikilinks from body)
    repo.ddb().arg("reindex").assert().success();

    // Rename B to a subfolder
    let new_path = format!("ddb/contact/{b_id}.md");
    repo.ddb()
        .args(["rename", &b_id, &new_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 backlinks updated"));

    // Verify B is at new path
    assert!(repo.path().join(&new_path).exists());
    assert!(!repo.path().join(format!("ddb/{b_id}.md")).exists());

    // Verify A's backlink was rewritten
    let a_content = repo.ddb().args(["read", &a_id]).output().unwrap();
    let a_text = String::from_utf8_lossy(&a_content.stdout);
    let new_target = format!("ddb/contact/{b_id}");
    assert!(
        a_text.contains(&format!("[[{new_target}|Target]]")),
        "expected rewritten link in A, got: {a_text}"
    );
}

#[test]
fn rename_no_backlinks() {
    let repo = DdbTestRepo::init();

    let out = repo
        .ddb()
        .args([
            "create",
            "--title",
            "Lonely",
            "--body",
            "No one links here.",
        ])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    let new_path = format!("ddb/contact/{id}.md");
    repo.ddb()
        .args(["rename", &id, &new_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 backlinks updated"));

    assert!(repo.path().join(&new_path).exists());
}

#[test]
fn rename_rewrites_markdown_and_embed_links() {
    let repo = DdbTestRepo::init();

    // Create target doogat
    let b_out = repo
        .ddb()
        .args(["create", "--title", "Target", "--body", "I am B."])
        .output()
        .unwrap();
    let b_id = String::from_utf8_lossy(&b_out.stdout).trim().to_string();
    let b_path_no_ext = format!("ddb/{b_id}");
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create linker with wikilink + markdown link + embed
    let body =
        format!("Wiki: [[{b_id}]]\nMd: [link]({b_path_no_ext})\nEmbed: ![[{b_path_no_ext}]]");
    let a_out = repo
        .ddb()
        .args(["create", "--title", "Linker", "--body", &body])
        .output()
        .unwrap();
    let a_id = String::from_utf8_lossy(&a_out.stdout).trim().to_string();

    // Reindex so links are in _ddb_links
    repo.ddb().arg("reindex").assert().success();

    // Rename target
    let new_path = format!("ddb/contact/{b_id}.md");
    repo.ddb()
        .args(["rename", &b_id, &new_path])
        .assert()
        .success();

    // Read linker content — all three link types should be rewritten
    let a_content = repo.ddb().args(["read", &a_id]).output().unwrap();
    let a_text = String::from_utf8_lossy(&a_content.stdout);
    let new_target = format!("ddb/contact/{b_id}");

    assert!(
        a_text.contains(&format!("[[{new_target}]]")),
        "wikilink not rewritten: {a_text}"
    );
    assert!(
        a_text.contains(&format!("[link]({new_target})")),
        "markdown link not rewritten: {a_text}"
    );
    assert!(
        a_text.contains(&format!("![[{new_target}]]")),
        "embed not rewritten: {a_text}"
    );
}
