use crate::common::{MultiNodeSetup, DdbTestRepo};
use predicates::prelude::*;

/// Helper: directly commit a doogat file to a node's repo (bypasses `ddb create`
/// so we can force a specific ID).
fn commit_doogat(node: &std::path::Path, id: &str, title: &str, body: &str) {
    let content = format!(
        "---\nid: {id}\ntitle: {title}\ndate: 2026-01-01\n---\n{body}\n"
    );
    let path = format!("ddb/{id}.md");
    // Use git directly to commit the file
    let full_path = node.join(&path);
    std::fs::create_dir_all(full_path.parent().unwrap()).unwrap();
    std::fs::write(&full_path, &content).unwrap();
    std::process::Command::new("git")
        .current_dir(node)
        .args(["add", &path])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(node)
        .args(["commit", "-m", &format!("add doogat {id}")])
        .output()
        .unwrap();
}

/// List all doogat files (excluding _typedef) in a node's ddb directory.
fn list_doogats(node: &std::path::Path) -> Vec<String> {
    let zk_dir = node.join("ddb");
    let mut files: Vec<String> = std::fs::read_dir(&zk_dir)
        .unwrap()
        .filter_map(|e| {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") && !name.starts_with('_') {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    files.sort();
    files
}

/// Read a doogat file directly from disk.
fn read_doogat_raw(node: &std::path::Path, filename: &str) -> String {
    let path = node.join("ddb").join(filename);
    std::fs::read_to_string(path).unwrap()
}

// ── Test: add-add collision — both doogats survive ──────────────────

#[test]
fn add_add_collision_both_doogats_survive() {
    let setup = MultiNodeSetup::new(2);

    let colliding_id = "20260101120000";

    // Node 0 creates a doogat with the colliding ID via direct file commit
    commit_doogat(&setup.nodes[0], colliding_id, "From Node 0", "Body from node zero");
    MultiNodeSetup::push(&setup.nodes[0]);

    // Node 1 creates a doogat with the SAME ID (before syncing)
    commit_doogat(&setup.nodes[1], colliding_id, "From Node 1", "Body from node one");

    // Node 1 syncs — this should detect the add-add collision
    DdbTestRepo::ddb_at(&setup.nodes[1])
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("collisions reassigned: 1"));

    // After sync, node 1 should have TWO doogat files
    let files = list_doogats(&setup.nodes[1]);
    assert_eq!(
        files.len(),
        2,
        "expected 2 doogats after collision resolution, got: {files:?}"
    );

    // One file should still be at the original ID
    assert!(
        files.contains(&format!("{colliding_id}.md")),
        "winner should keep original path: {files:?}"
    );

    // The other file should have a different ID
    let other_file = files
        .iter()
        .find(|f| *f != &format!("{colliding_id}.md"))
        .expect("should have a second doogat file");
    let new_id = other_file.trim_end_matches(".md");
    assert_ne!(new_id, colliding_id, "loser should have a new ID");

    // Both doogats should preserve their original content
    let winner_content = read_doogat_raw(&setup.nodes[1], &format!("{colliding_id}.md"));
    let loser_content = read_doogat_raw(&setup.nodes[1], other_file);

    // One should contain "node zero" content, the other "node one"
    let has_zero = winner_content.contains("Body from node zero")
        || loser_content.contains("Body from node zero");
    let has_one = winner_content.contains("Body from node one")
        || loser_content.contains("Body from node one");

    assert!(has_zero, "node zero's content should survive");
    assert!(has_one, "node one's content should survive");

    // Both should have distinct IDs in their frontmatter
    let winner_has_id = winner_content.contains(&format!("id: {colliding_id}"));
    let loser_has_new_id = loser_content.contains(&format!("id: {new_id}"));
    assert!(
        winner_has_id,
        "winner frontmatter should have original ID: {winner_content}"
    );
    assert!(
        loser_has_new_id,
        "loser frontmatter should have new ID {new_id}: {loser_content}"
    );

    // Push collision resolution back, sync node 0, verify convergence
    MultiNodeSetup::push(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[0]);

    let files_0 = list_doogats(&setup.nodes[0]);
    assert_eq!(
        files_0, files,
        "both nodes should converge to same doogat set"
    );
}

// ── Test: add-add collision — wikilink rewritten ────────────────────

#[test]
fn add_add_collision_link_rewritten() {
    let setup = MultiNodeSetup::new(2);

    let colliding_id = "20260101120000";

    // Node 0 creates the colliding doogat AND a third doogat that links to it
    commit_doogat(
        &setup.nodes[0],
        colliding_id,
        "Target Note",
        "Target body from node zero",
    );

    // Create a linking doogat on node 0 (using a different ID)
    let linker_id = "20260101120100";
    let linker_content = format!(
        "---\nid: {linker_id}\ntitle: Linker Note\ndate: 2026-01-01\n---\nSee [[{colliding_id}]] for details.\n"
    );
    let linker_path = format!("ddb/{linker_id}.md");
    let full_linker = setup.nodes[0].join(&linker_path);
    std::fs::write(&full_linker, &linker_content).unwrap();
    std::process::Command::new("git")
        .current_dir(&setup.nodes[0])
        .args(["add", &linker_path])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(&setup.nodes[0])
        .args(["commit", "-m", "add linker doogat"])
        .output()
        .unwrap();

    MultiNodeSetup::push(&setup.nodes[0]);

    // Node 1 creates a doogat with the SAME colliding ID (before syncing)
    commit_doogat(
        &setup.nodes[1],
        colliding_id,
        "Collider Note",
        "Collider body from node one",
    );

    // Node 1 syncs — collision detected, loser reassigned, links rewritten
    DdbTestRepo::ddb_at(&setup.nodes[1])
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("collisions reassigned: 1"));

    // Find the loser's new ID
    let files = list_doogats(&setup.nodes[1]);
    assert_eq!(files.len(), 3, "should have 3 doogats: {files:?}");

    let new_id_file = files
        .iter()
        .find(|f| {
            *f != &format!("{colliding_id}.md") && *f != &format!("{linker_id}.md")
        })
        .expect("should have a reassigned doogat file");
    let _new_id = new_id_file.trim_end_matches(".md");

    // The linker doogat's wikilink should now point to the new ID
    // (if the loser was "ours"/local, i.e., from node 1, then the linker came
    // from node 0 alongside the winner — the link rewrite only applies when
    // the loser's old ID appeared in other files)
    //
    // Since "theirs" (remote / node 0) wins on tie, the local doogat (node 1)
    // is the loser. The linker from node 0 references the winner's ID, so the
    // linker should remain unchanged — it still references the winning ID.
    //
    // Let's verify the linker still contains a valid link:
    let linker_on_1 = read_doogat_raw(&setup.nodes[1], &format!("{linker_id}.md"));

    // The linker should reference either the original colliding_id (winner) or
    // the new_id (if node 0's doogat was the loser). Since theirs wins on HLC
    // tie, node 0's doogat (theirs/remote) keeps the original ID.
    assert!(
        linker_on_1.contains(&format!("[[{colliding_id}]]")),
        "linker should still reference the winner's ID: {linker_on_1}"
    );

    // Now test the case where the linker references the loser:
    // Create a setup where local doogat links to the colliding ID, and it's the
    // loser — the link should be rewritten.
    let setup2 = MultiNodeSetup::new(2);

    // Node 0 creates a doogat with the colliding ID
    commit_doogat(
        &setup2.nodes[0],
        colliding_id,
        "Remote Note",
        "Remote body",
    );
    MultiNodeSetup::push(&setup2.nodes[0]);

    // Node 1 creates the same-ID doogat AND a linker to it
    commit_doogat(
        &setup2.nodes[1],
        colliding_id,
        "Local Note",
        "Local body",
    );
    let local_linker_id = "20260101120200";
    let local_linker_content = format!(
        "---\nid: {local_linker_id}\ntitle: Local Linker\ndate: 2026-01-01\n---\nRef: [[{colliding_id}]] here.\n"
    );
    let local_linker_path = format!("ddb/{local_linker_id}.md");
    let full_local_linker = setup2.nodes[1].join(&local_linker_path);
    std::fs::write(&full_local_linker, &local_linker_content).unwrap();
    std::process::Command::new("git")
        .current_dir(&setup2.nodes[1])
        .args(["add", &local_linker_path])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(&setup2.nodes[1])
        .args(["commit", "-m", "add local linker"])
        .output()
        .unwrap();

    // Sync — local doogat is the loser (theirs/remote wins on tie)
    DdbTestRepo::ddb_at(&setup2.nodes[1])
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("collisions reassigned: 1"));

    // Find loser's new ID
    let files2 = list_doogats(&setup2.nodes[1]);
    let new_id_file2 = files2
        .iter()
        .find(|f| {
            *f != &format!("{colliding_id}.md") && *f != &format!("{local_linker_id}.md")
        })
        .expect("should have reassigned doogat");
    let new_id2 = new_id_file2.trim_end_matches(".md");

    // The local linker's link should be rewritten to point to the new ID
    let linker_content2 = read_doogat_raw(&setup2.nodes[1], &format!("{local_linker_id}.md"));
    assert!(
        linker_content2.contains(&format!("[[{new_id2}]]")),
        "linker should have wikilink rewritten from {colliding_id} to {new_id2}: {linker_content2}"
    );

    // The original colliding_id link should be gone from the linker
    assert!(
        !linker_content2.contains(&format!("[[{colliding_id}]]")),
        "old wikilink should be rewritten: {linker_content2}"
    );
}
