use crate::common::ZdbTestRepo;
use predicates::prelude::*;

#[test]
fn rename_moves_file_and_rewrites_backlinks() {
    let repo = ZdbTestRepo::init();

    // Create target zettel B
    let b_out = repo
        .zdb()
        .args(["create", "--title", "Target", "--body", "I am B."])
        .output()
        .unwrap();
    let b_id = String::from_utf8_lossy(&b_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create zettel A linking to B via bare ID
    let a_out = repo
        .zdb()
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

    // Create zettel C also linking to B
    repo.zdb()
        .args([
            "create",
            "--title",
            "Linker C",
            "--body",
            &format!("Also see [[{b_id}]]."),
        ])
        .assert()
        .success();

    // Reindex so wikilinks are in _zdb_links
    // (create command doesn't extract wikilinks from body)
    repo.zdb().arg("reindex").assert().success();

    // Rename B to a subfolder
    let new_path = format!("zettelkasten/contact/{b_id}.md");
    repo.zdb()
        .args(["rename", &b_id, &new_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 backlinks updated"));

    // Verify B is at new path
    assert!(repo.path().join(&new_path).exists());
    assert!(!repo.path().join(format!("zettelkasten/{b_id}.md")).exists());

    // Verify A's backlink was rewritten
    let a_content = repo.zdb().args(["read", &a_id]).output().unwrap();
    let a_text = String::from_utf8_lossy(&a_content.stdout);
    let new_target = format!("zettelkasten/contact/{b_id}");
    assert!(
        a_text.contains(&format!("[[{new_target}|Target]]")),
        "expected rewritten link in A, got: {a_text}"
    );
}

#[test]
fn rename_no_backlinks() {
    let repo = ZdbTestRepo::init();

    let out = repo
        .zdb()
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

    let new_path = format!("zettelkasten/contact/{id}.md");
    repo.zdb()
        .args(["rename", &id, &new_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 backlinks updated"));

    assert!(repo.path().join(&new_path).exists());
}

#[test]
fn rename_rewrites_markdown_and_embed_links() {
    let repo = ZdbTestRepo::init();

    // Create target zettel
    let b_out = repo
        .zdb()
        .args(["create", "--title", "Target", "--body", "I am B."])
        .output()
        .unwrap();
    let b_id = String::from_utf8_lossy(&b_out.stdout).trim().to_string();
    let b_path_no_ext = format!("zettelkasten/{b_id}");
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create linker with wikilink + markdown link + embed
    let body =
        format!("Wiki: [[{b_id}]]\nMd: [link]({b_path_no_ext})\nEmbed: ![[{b_path_no_ext}]]");
    let a_out = repo
        .zdb()
        .args(["create", "--title", "Linker", "--body", &body])
        .output()
        .unwrap();
    let a_id = String::from_utf8_lossy(&a_out.stdout).trim().to_string();

    // Reindex so links are in _zdb_links
    repo.zdb().arg("reindex").assert().success();

    // Rename target
    let new_path = format!("zettelkasten/contact/{b_id}.md");
    repo.zdb()
        .args(["rename", &b_id, &new_path])
        .assert()
        .success();

    // Read linker content — all three link types should be rewritten
    let a_content = repo.zdb().args(["read", &a_id]).output().unwrap();
    let a_text = String::from_utf8_lossy(&a_content.stdout);
    let new_target = format!("zettelkasten/contact/{b_id}");

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
