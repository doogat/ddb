use crate::common::DdbTestRepo;

#[test]
fn insert_folder_typedef_creates_subdirectory_path() {
    let repo = DdbTestRepo::init();

    // Create typedef via SQL
    repo.ddb()
        .args(["query", "CREATE TABLE widget (color TEXT)"])
        .assert()
        .success();

    // Manually add folder: true to the typedef doogat
    let typedef_dir = repo.path().join("ddb/_typedef");
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
    let added = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["add", "-A"])
        .output()
        .unwrap();
    assert!(added.status.success(), "fixture git add failed: {added:?}");
    let committed = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["commit", "-m", "add folder to widget"])
        .output()
        .unwrap();
    assert!(
        committed.status.success(),
        "fixture git commit failed: {committed:?}"
    );

    // Reindex to pick up folder setting
    repo.ddb().arg("reindex").assert().success();

    // INSERT a widget via SQL
    let out = repo
        .ddb()
        .args(["query", "INSERT INTO widget (color) VALUES ('red')"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "INSERT failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Verify the file is at ddb/widget/{id}.md
    let expected_path = repo.path().join(format!("ddb/widget/{id}.md"));
    assert!(
        expected_path.is_file(),
        "expected file at {}",
        expected_path.display()
    );
}

#[test]
fn insert_no_folder_typedef_stays_flat() {
    let repo = DdbTestRepo::init();

    // Create typedef WITHOUT folder
    repo.ddb()
        .args(["query", "CREATE TABLE gadget (size TEXT)"])
        .assert()
        .success();

    // INSERT — should stay flat
    let out = repo
        .ddb()
        .args(["query", "INSERT INTO gadget (size) VALUES ('large')"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "INSERT failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Verify the file is at ddb/{id}.md (flat)
    let flat_path = repo.path().join(format!("ddb/{id}.md"));
    let folder_path = repo.path().join(format!("ddb/gadget/{id}.md"));
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

#[test]
fn cli_create_respects_folder_typedef() {
    let repo = DdbTestRepo::init();

    // Create typedef with folder: true via SQL + manual edit
    repo.ddb()
        .args(["query", "CREATE TABLE pet (species TEXT)"])
        .assert()
        .success();

    let typedef_dir = repo.path().join("ddb/_typedef");
    let typedef_file = std::fs::read_dir(&typedef_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| {
            let content = std::fs::read_to_string(e.path()).unwrap_or_default();
            content.contains("title: pet")
        })
        .expect("pet typedef not found");

    let content = std::fs::read_to_string(typedef_file.path()).unwrap();
    let updated = content.replace("type: _typedef", "type: _typedef\nfolder: true");
    std::fs::write(typedef_file.path(), &updated).unwrap();

    std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["add", "-A"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["commit", "-m", "add folder to pet"])
        .output()
        .unwrap();
    repo.ddb().arg("reindex").assert().success();

    // CLI create with --type pet
    let out = repo
        .ddb()
        .args(["create", "--title", "Buddy", "--type", "pet"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Should be in ddb/pet/{id}.md
    let folder_path = repo.path().join(format!("ddb/pet/{id}.md"));
    assert!(
        folder_path.exists(),
        "CLI create should put typed doogat in pet/ subdirectory: {}",
        folder_path.display()
    );
}
