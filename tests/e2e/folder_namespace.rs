use crate::common::ZdbTestRepo;

#[test]
fn insert_folder_typedef_creates_subdirectory_path() {
    let repo = ZdbTestRepo::init();

    // Create typedef via SQL
    repo.zdb()
        .args(["query", "CREATE TABLE widget (color TEXT)"])
        .assert()
        .success();

    // Manually add folder: true to the typedef zettel
    let typedef_dir = repo.path().join("zettelkasten/_typedef");
    let typedef_file = std::fs::read_dir(&typedef_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| {
            let content = std::fs::read_to_string(e.path()).unwrap_or_default();
            content.contains("title: widget")
        })
        .expect("widget typedef not found");

    let content = std::fs::read_to_string(typedef_file.path()).unwrap();
    let updated = content.replace("type: _typedef", "type: _typedef\nfolder: true");
    std::fs::write(typedef_file.path(), &updated).unwrap();

    // Commit the change
    std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["add", "-A"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["commit", "-m", "add folder to widget"])
        .output()
        .unwrap();

    // Reindex to pick up folder setting
    repo.zdb().arg("reindex").assert().success();

    // INSERT a widget via SQL
    let out = repo
        .zdb()
        .args(["query", "INSERT INTO widget (color) VALUES ('red')"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "INSERT failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Verify the file is at zettelkasten/widget/{id}.md
    let expected_path = repo.path().join(format!("zettelkasten/widget/{id}.md"));
    assert!(
        expected_path.exists(),
        "expected file at {}",
        expected_path.display()
    );
}

#[test]
fn insert_no_folder_typedef_stays_flat() {
    let repo = ZdbTestRepo::init();

    // Create typedef WITHOUT folder
    repo.zdb()
        .args(["query", "CREATE TABLE gadget (size TEXT)"])
        .assert()
        .success();

    // INSERT — should stay flat
    let out = repo
        .zdb()
        .args(["query", "INSERT INTO gadget (size) VALUES ('large')"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "INSERT failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Verify the file is at zettelkasten/{id}.md (flat)
    let flat_path = repo.path().join(format!("zettelkasten/{id}.md"));
    let folder_path = repo.path().join(format!("zettelkasten/gadget/{id}.md"));
    assert!(
        flat_path.exists(),
        "expected flat file at {}",
        flat_path.display()
    );
    assert!(
        !folder_path.exists(),
        "should NOT be in gadget/ subdirectory"
    );
}
