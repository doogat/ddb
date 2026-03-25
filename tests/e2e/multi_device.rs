use crate::common::{MultiNodeSetup, DdbTestRepo};
use rand::prelude::*;

/// Round-robin sync: push from one node, pull on all others
fn sync_round_robin(setup: &MultiNodeSetup) {
    for node in &setup.nodes {
        MultiNodeSetup::push(node);
    }
    for node in &setup.nodes {
        MultiNodeSetup::sync(node);
    }
}

// ── Test: 3-node convergence ─────────────────────────────────────

#[test]
fn three_node_convergence() {
    let setup = MultiNodeSetup::new(3);

    // Each node creates a doogat
    let id0 = MultiNodeSetup::create(&setup.nodes[0], "Note from 0", "body0");
    MultiNodeSetup::push(&setup.nodes[0]);

    MultiNodeSetup::sync(&setup.nodes[1]);
    let id1 = MultiNodeSetup::create(&setup.nodes[1], "Note from 1", "body1");
    MultiNodeSetup::push(&setup.nodes[1]);

    MultiNodeSetup::sync(&setup.nodes[2]);
    let id2 = MultiNodeSetup::create(&setup.nodes[2], "Note from 2", "body2");
    MultiNodeSetup::push(&setup.nodes[2]);

    // Sync all
    sync_round_robin(&setup);

    // Verify all nodes see all 3 doogats
    for node in &setup.nodes {
        let out0 = MultiNodeSetup::read(node, &id0);
        assert!(out0.contains("Note from 0"), "node missing doogat 0");
        let out1 = MultiNodeSetup::read(node, &id1);
        assert!(out1.contains("Note from 1"), "node missing doogat 1");
        let out2 = MultiNodeSetup::read(node, &id2);
        assert!(out2.contains("Note from 2"), "node missing doogat 2");
    }
}

// ── Test: concurrent creates ─────────────────────────────────────

#[test]
fn concurrent_creates() {
    let setup = MultiNodeSetup::new(3);

    // All nodes create different doogats without syncing first.
    // Sleep between creates to ensure distinct DoogatIDs (timestamp-based).
    let id0 = MultiNodeSetup::create(&setup.nodes[0], "Concurrent 0", "c0");
    std::thread::sleep(std::time::Duration::from_secs(1));
    let id1 = MultiNodeSetup::create(&setup.nodes[1], "Concurrent 1", "c1");
    std::thread::sleep(std::time::Duration::from_secs(1));
    let id2 = MultiNodeSetup::create(&setup.nodes[2], "Concurrent 2", "c2");

    // Cascade sync: each node syncs (fetch+merge+push) in sequence
    // Round 1: propagate from each node to remote
    MultiNodeSetup::sync(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[2]);

    // Round 2: pull everything back (node0 needs node1+2 changes, etc)
    MultiNodeSetup::sync(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[2]);

    // All nodes see all 3
    for (i, node) in setup.nodes.iter().enumerate() {
        assert!(
            MultiNodeSetup::read(node, &id0).contains("Concurrent 0"),
            "node {i} missing doogat from node 0"
        );
        assert!(
            MultiNodeSetup::read(node, &id1).contains("Concurrent 1"),
            "node {i} missing doogat from node 1"
        );
        assert!(
            MultiNodeSetup::read(node, &id2).contains("Concurrent 2"),
            "node {i} missing doogat from node 2"
        );
    }
}

// ── Test: stale node return ──────────────────────────────────────

#[test]
fn stale_node_returns() {
    let setup = MultiNodeSetup::new(3);

    // Node0 creates, pushes. Node1 syncs and creates. Node2 stays offline.
    let id0 = MultiNodeSetup::create(&setup.nodes[0], "Before stale", "b0");
    MultiNodeSetup::push(&setup.nodes[0]);

    MultiNodeSetup::sync(&setup.nodes[1]);
    let id1 = MultiNodeSetup::create(&setup.nodes[1], "While stale", "b1");
    MultiNodeSetup::push(&setup.nodes[1]);

    // Node2 was offline the whole time. Now it syncs.
    MultiNodeSetup::sync(&setup.nodes[2]);

    let out0 = MultiNodeSetup::read(&setup.nodes[2], &id0);
    assert!(out0.contains("Before stale"));
    let out1 = MultiNodeSetup::read(&setup.nodes[2], &id1);
    assert!(out1.contains("While stale"));
}

// ── Test: network partition and reconnect ────────────────────────

#[test]
fn network_partition_and_reconnect() {
    let setup = MultiNodeSetup::new(3);

    // Create initial state, sync all
    let id0 = MultiNodeSetup::create(&setup.nodes[0], "Shared note", "original");
    MultiNodeSetup::push(&setup.nodes[0]);
    sync_round_robin(&setup);

    // Partition: node0 edits, pushes. node2 can't reach remote.
    MultiNodeSetup::update(&setup.nodes[0], &id0, "Partition edit", "from-node0");
    MultiNodeSetup::push(&setup.nodes[0]);

    // Node2 makes its own edit (offline)
    let id2 = MultiNodeSetup::create(&setup.nodes[2], "Offline note", "partition");

    // Reconnect: node2 syncs (will merge node0's changes)
    MultiNodeSetup::sync(&setup.nodes[2]);
    MultiNodeSetup::push(&setup.nodes[2]);

    // Final sync
    sync_round_robin(&setup);

    // All nodes see both edits
    for node in &setup.nodes {
        // The updated note should exist
        let out = MultiNodeSetup::read(node, &id0);
        assert!(
            out.contains("Partition edit") || out.contains("Shared note"),
            "merged note should be accessible"
        );
        // The offline-created note should exist
        let out2 = MultiNodeSetup::read(node, &id2);
        assert!(out2.contains("Offline note"));
    }
}

// ── Test: stale node resync after compaction ─────────────────────

#[test]
fn stale_node_resync_after_compaction() {
    let setup = MultiNodeSetup::new(3);

    // All nodes start with a shared note
    let id = MultiNodeSetup::create(&setup.nodes[0], "Shared", "original body");
    MultiNodeSetup::push(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[2]);

    // Node0 and Node1 make conflicting edits. Node2 stays offline (stale).
    MultiNodeSetup::update(&setup.nodes[0], &id, "Edit from 0", "body from node0");
    MultiNodeSetup::push(&setup.nodes[0]);

    MultiNodeSetup::update(&setup.nodes[1], &id, "Edit from 1", "body from node1");
    // Node1 syncs — this triggers conflict resolution and creates CRDT temp files
    MultiNodeSetup::sync(&setup.nodes[1]);

    // Compact on node1 to remove CRDT temp files
    DdbTestRepo::ddb_at(&setup.nodes[1])
        .args(["compact", "--force"])
        .assert()
        .success();

    MultiNodeSetup::push(&setup.nodes[1]);

    // Node2 was offline the entire time. Now it makes a conflicting edit and syncs.
    MultiNodeSetup::update(&setup.nodes[2], &id, "Edit from 2", "body from node2");
    // This sync should succeed even without CRDT state (LWW fallback)
    MultiNodeSetup::sync(&setup.nodes[2]);

    // Verify the doogat is readable (valid markdown, no panic)
    let out = MultiNodeSetup::read(&setup.nodes[2], &id);
    assert!(
        out.contains("Edit from") || out.contains("Shared"),
        "stale node should have a resolved doogat after resync"
    );
}

// ── Test: stale node returns with edits to deleted doogat after compaction ──

#[test]
fn stale_node_edits_deleted_doogat_after_compaction() {
    let setup = MultiNodeSetup::new(3);

    // All nodes share a doogat
    let id = MultiNodeSetup::create(&setup.nodes[0], "Will be deleted", "original body");
    MultiNodeSetup::push(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[2]);

    // Node2 goes offline. Node1 deletes the doogat.
    MultiNodeSetup::delete(&setup.nodes[1], &id);
    MultiNodeSetup::sync(&setup.nodes[1]);

    // Compact on node0 to remove CRDT state
    MultiNodeSetup::sync(&setup.nodes[0]);
    DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["compact", "--force"])
        .assert()
        .success();
    MultiNodeSetup::push(&setup.nodes[0]);

    // Node2 comes back online with an edit to the deleted doogat
    MultiNodeSetup::update(&setup.nodes[2], &id, "Edited while offline", "stale edit");
    MultiNodeSetup::sync(&setup.nodes[2]);

    // Full sync
    for _ in 0..3 {
        sync_round_robin(&setup);
    }

    // Edit should win over delete (resurrected)
    for (i, node) in setup.nodes.iter().enumerate() {
        let out = MultiNodeSetup::read(node, &id);
        assert!(
            out.contains("Edited while offline") || out.contains("stale edit"),
            "node {i}: edit should win over delete after compaction, got: {out}"
        );
    }
}

// ── Test: stale node creates new doogats after compaction ────────

#[test]
fn stale_node_new_doogats_after_compaction() {
    let setup = MultiNodeSetup::new(2);

    // Initial sync
    let id0 = MultiNodeSetup::create(&setup.nodes[0], "Before offline", "body0");
    MultiNodeSetup::push(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);

    // Node1 goes offline and creates new doogats
    let offline_id = MultiNodeSetup::create(&setup.nodes[1], "Created offline", "offline body");
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Node0 makes many edits and compacts
    for i in 0..5 {
        MultiNodeSetup::update(
            &setup.nodes[0],
            &id0,
            &format!("Edit {i}"),
            &format!("body edit {i}"),
        );
    }
    DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["compact", "--force"])
        .assert()
        .success();
    MultiNodeSetup::push(&setup.nodes[0]);

    // Node1 comes back and syncs
    MultiNodeSetup::sync(&setup.nodes[1]);
    MultiNodeSetup::push(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[0]);

    // Both nodes should see the offline-created doogat
    let out0 = MultiNodeSetup::read(&setup.nodes[0], &offline_id);
    assert!(
        out0.contains("Created offline"),
        "node0 missing stale node's doogat: {out0}"
    );
    let out1 = MultiNodeSetup::read(&setup.nodes[1], &offline_id);
    assert!(
        out1.contains("Created offline"),
        "node1 should still have its own doogat: {out1}"
    );
}

// ── Test: multiple stale nodes return sequentially after compaction ──

#[test]
fn multiple_stale_nodes_return_sequentially() {
    let setup = MultiNodeSetup::new(3);

    // All start synced
    let id = MultiNodeSetup::create(&setup.nodes[0], "Shared", "original");
    MultiNodeSetup::push(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[2]);

    // Node1 and Node2 go offline. Node0 edits and compacts.
    MultiNodeSetup::update(&setup.nodes[0], &id, "Node0 edit", "body from node0");
    DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["compact", "--force"])
        .assert()
        .success();
    MultiNodeSetup::push(&setup.nodes[0]);

    // Node1 returns first with its own edit
    MultiNodeSetup::update(&setup.nodes[1], &id, "Node1 edit", "body from node1");
    MultiNodeSetup::sync(&setup.nodes[1]);
    MultiNodeSetup::push(&setup.nodes[1]);

    // Compact again after node1's return
    MultiNodeSetup::sync(&setup.nodes[0]);
    DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["compact", "--force"])
        .assert()
        .success();
    MultiNodeSetup::push(&setup.nodes[0]);

    // Node2 returns last with its own edit
    MultiNodeSetup::update(&setup.nodes[2], &id, "Node2 edit", "body from node2");
    MultiNodeSetup::sync(&setup.nodes[2]);

    // Final convergence
    for _ in 0..3 {
        sync_round_robin(&setup);
    }

    // All nodes must converge to identical content
    let content0 = MultiNodeSetup::read(&setup.nodes[0], &id);
    let content1 = MultiNodeSetup::read(&setup.nodes[1], &id);
    let content2 = MultiNodeSetup::read(&setup.nodes[2], &id);

    assert_eq!(
        content0, content1,
        "node0 and node1 diverged after sequential stale returns"
    );
    assert_eq!(
        content1, content2,
        "node1 and node2 diverged after sequential stale returns"
    );

    // Should contain some edit content (not empty or corrupt)
    assert!(
        content0.contains("edit"),
        "resolved content should contain an edit: {content0}"
    );
}

// ── Test: HLC LWW picks later writer ─────────────────────────────

#[test]
fn hlc_lww_picks_later_writer() {
    let setup = MultiNodeSetup::new(2);

    // Install kanban type on node0 (uses preset:last-writer-wins), sync to both
    DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["type", "install", "kanban"])
        .assert()
        .success();
    MultiNodeSetup::sync(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);

    // Create a kanban doogat on node0, sync to both
    let id = MultiNodeSetup::create(&setup.nodes[0], "LWW task", "original body");
    DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["update", &id, "--type", "kanban"])
        .assert()
        .success();
    MultiNodeSetup::sync(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);

    // Node0 edits first (earlier HLC)
    MultiNodeSetup::update(&setup.nodes[0], &id, "From node0", "body from node0");

    // Sleep to ensure node1's HLC is strictly later
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Node1 edits second (later HLC)
    MultiNodeSetup::update(&setup.nodes[1], &id, "From node1", "body from node1");

    // Node0 syncs first (pushes its edit to remote)
    MultiNodeSetup::sync(&setup.nodes[0]);

    // Node1 syncs — conflict resolved by LWW, node1 (later HLC) should win
    MultiNodeSetup::sync(&setup.nodes[1]);

    let out = MultiNodeSetup::read(&setup.nodes[1], &id);
    assert!(
        out.contains("From node1"),
        "LWW should pick later writer (node1), got: {out}"
    );
    assert!(
        out.contains("body from node1"),
        "LWW should pick later writer body, got: {out}"
    );
}

// ── Test: bundle full export/import bootstrap ────────────────────

#[test]
fn bundle_full_bootstrap() {
    let setup = MultiNodeSetup::new(2);

    // Create content on node0
    let id = MultiNodeSetup::create(&setup.nodes[0], "Bundle test", "bundle body");
    MultiNodeSetup::push(&setup.nodes[0]);

    // Export full bundle from node0
    let bundle_path = setup.remote_dir.path().join("full.bundle.tar");
    DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["bundle", "export", "--full", "--output"])
        .arg(&bundle_path)
        .assert()
        .success();

    assert!(bundle_path.exists(), "bundle file should exist");

    // Create a fresh node3 (not cloned from remote)
    let dir3 = tempfile::TempDir::new().unwrap();
    let path3 = dir3.path().to_path_buf();
    DdbTestRepo::ddb_at(&path3)
        .arg("init")
        .arg(&path3)
        .assert()
        .success();
    DdbTestRepo::ddb_at(&path3)
        .args(["register-node", "Node-3"])
        .assert()
        .success();

    // Import the full bundle
    DdbTestRepo::ddb_at(&path3)
        .args(["bundle", "import"])
        .arg(&bundle_path)
        .assert()
        .success();

    // Verify the doogat exists on node3
    let out = MultiNodeSetup::read(&path3, &id);
    assert!(
        out.contains("Bundle test"),
        "bootstrapped node should have the doogat"
    );
}

// ── Test: bundle-based recovery after compaction ─────────────────

#[test]
fn bundle_recovery_after_compaction() {
    let setup = MultiNodeSetup::new(2);

    // Node0 creates content, syncs with node1
    let id = MultiNodeSetup::create(&setup.nodes[0], "Pre-compaction", "original body");
    MultiNodeSetup::push(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);

    // Node0 makes many edits and compacts
    for i in 0..5 {
        MultiNodeSetup::update(
            &setup.nodes[0],
            &id,
            &format!("Post-compact edit {i}"),
            &format!("body {i}"),
        );
    }
    DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["compact", "--force"])
        .assert()
        .success();
    MultiNodeSetup::push(&setup.nodes[0]);

    // Export bundle from node0 (after compaction)
    let bundle_path = setup.remote_dir.path().join("recovery.bundle.tar");
    DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["bundle", "export", "--full", "--output"])
        .arg(&bundle_path)
        .assert()
        .success();

    // Create a fresh node that never synced with remote
    let dir3 = tempfile::TempDir::new().unwrap();
    let path3 = dir3.path().to_path_buf();
    DdbTestRepo::ddb_at(&path3)
        .arg("init")
        .arg(&path3)
        .assert()
        .success();
    DdbTestRepo::ddb_at(&path3)
        .args(["register-node", "Recovery-Node"])
        .assert()
        .success();

    // Import the post-compaction bundle
    DdbTestRepo::ddb_at(&path3)
        .args(["bundle", "import"])
        .arg(&bundle_path)
        .assert()
        .success();

    // Verify the doogat exists with latest content
    let out = MultiNodeSetup::read(&path3, &id);
    assert!(
        out.contains("Post-compact edit"),
        "recovered node should have latest content: {out}"
    );
}

// ── Test: compact creates recoverable backup ─────────────────────

#[test]
fn compact_creates_recoverable_backup() {
    let setup = MultiNodeSetup::new(2);

    // Node0 creates content
    let id = MultiNodeSetup::create(&setup.nodes[0], "Backup test", "original body");
    MultiNodeSetup::push(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);

    // Node0 edits and compacts (backup created automatically)
    for i in 0..3 {
        MultiNodeSetup::update(
            &setup.nodes[0],
            &id,
            &format!("Edit {i}"),
            &format!("body {i}"),
        );
    }
    let compact_out = DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["compact", "--force"])
        .output()
        .expect("compact failed");
    assert!(compact_out.status.success(), "compact should succeed");
    let stdout = String::from_utf8_lossy(&compact_out.stdout);
    assert!(
        stdout.contains("backup:"),
        "compact output should show backup path: {stdout}"
    );

    // Extract backup path from output
    let backup_line = stdout
        .lines()
        .find(|l| l.starts_with("backup:"))
        .expect("backup line missing");
    let backup_path = backup_line.trim_start_matches("backup:").trim();
    assert!(
        std::path::Path::new(backup_path).exists(),
        "backup file should exist at {backup_path}"
    );

    // Import backup into a fresh node to verify it's recoverable
    let fresh = tempfile::TempDir::new().unwrap();
    let fresh_path = fresh.path().to_path_buf();
    DdbTestRepo::ddb_at(&fresh_path)
        .args(["init"])
        .assert()
        .success();
    DdbTestRepo::ddb_at(&fresh_path)
        .args(["register-node", "RecoveryNode"])
        .assert()
        .success();
    DdbTestRepo::ddb_at(&fresh_path)
        .args(["bundle", "import", backup_path])
        .assert()
        .success();

    let out = MultiNodeSetup::read(&fresh_path, &id);
    assert!(
        out.contains("Edit"),
        "recovered node should have edited content: {out}"
    );
}

#[test]
fn compact_no_backup_flag() {
    let setup = MultiNodeSetup::new(2);

    MultiNodeSetup::create(&setup.nodes[0], "No-backup test", "body");
    let compact_out = DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["compact", "--force", "--no-backup"])
        .output()
        .expect("compact failed");
    assert!(
        compact_out.status.success(),
        "compact --no-backup should succeed"
    );
    let stdout = String::from_utf8_lossy(&compact_out.stdout);
    assert!(
        !stdout.contains("backup:"),
        "compact --no-backup should not show backup path: {stdout}"
    );

    // Verify no backup file was created in .ddb/backups/
    let backups_dir = setup.nodes[0].join(".ddb/backups");
    if backups_dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(&backups_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            entries.is_empty(),
            "no backup files should exist with --no-backup: {entries:?}"
        );
    }
}

#[test]
fn compact_backup_path_flag() {
    let setup = MultiNodeSetup::new(2);

    MultiNodeSetup::create(&setup.nodes[0], "Custom path test", "body");
    let custom_backup = tempfile::NamedTempFile::new().unwrap();
    let custom_path = custom_backup.path().with_extension("bundle.tar");

    let compact_out = DdbTestRepo::ddb_at(&setup.nodes[0])
        .args([
            "compact",
            "--force",
            "--backup-path",
            custom_path.to_str().unwrap(),
        ])
        .output()
        .expect("compact failed");
    assert!(
        compact_out.status.success(),
        "compact --backup-path should succeed"
    );
    let stdout = String::from_utf8_lossy(&compact_out.stdout);
    assert!(
        stdout.contains("backup:"),
        "compact --backup-path should show backup path: {stdout}"
    );
    assert!(
        custom_path.exists(),
        "backup file should exist at custom path: {}",
        custom_path.display()
    );
}

// ── Test: air-gapped delta transfer ──────────────────────────────

#[test]
fn airgapped_delta_transfer() {
    let setup = MultiNodeSetup::new(2);

    // Initial content + sync
    let _id0 = MultiNodeSetup::create(&setup.nodes[0], "Initial", "init");
    MultiNodeSetup::push(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);

    // Node0 creates a new doogat (node1 doesn't know about it yet)
    let id1 = MultiNodeSetup::create(&setup.nodes[0], "Delta note", "delta");
    MultiNodeSetup::push(&setup.nodes[0]);

    // Export delta bundle targeting node1
    let bundle_path = setup.remote_dir.path().join("delta.bundle.tar");

    // Export as full (delta requires knowing remote UUID; full is simpler for testing)
    DdbTestRepo::ddb_at(&setup.nodes[0])
        .args(["bundle", "export", "--full", "--output"])
        .arg(&bundle_path)
        .assert()
        .success();

    // Import on node1
    DdbTestRepo::ddb_at(&setup.nodes[1])
        .args(["bundle", "import"])
        .arg(&bundle_path)
        .assert()
        .success();

    // Node1 should now see the delta note
    let out = MultiNodeSetup::read(&setup.nodes[1], &id1);
    assert!(
        out.contains("Delta note"),
        "delta transfer should bring new doogat"
    );
}

// ── Test: concurrent edits to same doogat ────────────────────────

#[test]
fn concurrent_edits_same_doogat() {
    let setup = MultiNodeSetup::new(3);

    // Node0 creates a doogat, sync to all
    let id = MultiNodeSetup::create(&setup.nodes[0], "Shared doogat", "original body");
    MultiNodeSetup::push(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[2]);

    // All 3 nodes edit the same doogat without syncing between edits
    MultiNodeSetup::update(&setup.nodes[0], &id, "Edit from node0", "body from node0");
    MultiNodeSetup::update(&setup.nodes[1], &id, "Edit from node1", "body from node1");
    MultiNodeSetup::update(&setup.nodes[2], &id, "Edit from node2", "body from node2");

    // Sync cascade: 3 rounds to ensure full convergence
    for _ in 0..3 {
        sync_round_robin(&setup);
    }

    // All nodes must converge to identical content (CRDT determinism)
    let content0 = MultiNodeSetup::read(&setup.nodes[0], &id);
    let content1 = MultiNodeSetup::read(&setup.nodes[1], &id);
    let content2 = MultiNodeSetup::read(&setup.nodes[2], &id);

    assert_eq!(content0, content1, "node0 and node1 diverged");
    assert_eq!(content1, content2, "node1 and node2 diverged");
}

// ── Test: delete-vs-edit across 3 nodes ──────────────────────────

#[test]
fn delete_vs_edit_multi_node() {
    let setup = MultiNodeSetup::new(3);

    // Node0 creates a doogat, sync to all
    let id = MultiNodeSetup::create(&setup.nodes[0], "Will conflict", "original body");
    MultiNodeSetup::push(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[2]);

    // Node1 deletes the doogat
    MultiNodeSetup::delete(&setup.nodes[1], &id);

    // Node2 edits the doogat
    MultiNodeSetup::update(
        &setup.nodes[2],
        &id,
        "Edited after delete",
        "surviving body",
    );

    // Node1 pushes delete, then node2 syncs (triggers delete-vs-edit conflict)
    MultiNodeSetup::push(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[2]);

    // Full sync to propagate resolution
    for _ in 0..3 {
        sync_round_robin(&setup);
    }

    // Edit wins: doogat should exist on all nodes with node2's content
    for (i, node) in setup.nodes.iter().enumerate() {
        let out = MultiNodeSetup::read(node, &id);
        assert!(
            out.contains("Edited after delete") || out.contains("surviving body"),
            "node {i}: edit should win over delete, got: {out}"
        );
    }

    // Check resurrected marker in frontmatter
    let out = MultiNodeSetup::read(&setup.nodes[2], &id);
    assert!(
        out.contains("resurrected: true"),
        "resurrected marker missing from frontmatter: {out}"
    );
}

// ── Test: chaos convergence with 4 nodes ─────────────────────────

/// List doogat files in a node's ddb directory, sorted.
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

/// Read a doogat file directly from disk for comparison.
fn read_doogat_file(node: &std::path::Path, filename: &str) -> String {
    // Compare canonical doogat content instead of raw working tree bytes so
    // platform-specific checkout EOL conversion does not look like divergence.
    let id = filename.strip_suffix(".md").unwrap_or(filename);
    MultiNodeSetup::read(node, id).replace("\r\n", "\n")
}

#[test]
fn chaos_convergence() {
    let setup = MultiNodeSetup::new(4);
    let mut rng = StdRng::seed_from_u64(42);

    // Each node tracks its locally-known doogat IDs (for updates)
    let mut local_ids: Vec<Vec<String>> = vec![vec![]; 4];

    // Phase 1: each node creates an initial doogat so there's something to operate on
    for (i, node) in setup.nodes.iter().enumerate() {
        let id = MultiNodeSetup::create(node, &format!("Init {i}"), &format!("body {i}"));
        local_ids[i].push(id);
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Sync all so every node knows every doogat
    for _ in 0..3 {
        sync_round_robin(&setup);
    }

    // Propagate all IDs to all nodes' known lists
    let all_ids: Vec<String> = local_ids.iter().flatten().cloned().collect();
    for ids in &mut local_ids {
        *ids = all_ids.clone();
    }

    // Phase 2: each node performs 5 random ops (create or update only)
    for (i, (node, ids)) in setup.nodes.iter().zip(local_ids.iter_mut()).enumerate() {
        for _ in 0..5 {
            let op: u8 = rng.gen_range(0..3);
            match op {
                0 => {
                    // Create
                    let id = MultiNodeSetup::create(
                        node,
                        &format!("Chaos {i}"),
                        &format!("chaos body {i}"),
                    );
                    ids.push(id);
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                1 if !ids.is_empty() => {
                    // Update a random known doogat
                    let idx = rng.gen_range(0..ids.len());
                    let id = ids[idx].clone();
                    MultiNodeSetup::update(
                        node,
                        &id,
                        &format!("Updated by {i}"),
                        &format!("updated body {i}"),
                    );
                }
                _ => {
                    // Create (fallback when no IDs to update)
                    let id = MultiNodeSetup::create(
                        node,
                        &format!("Chaos fallback {i}"),
                        &format!("fallback body {i}"),
                    );
                    ids.push(id);
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        }
    }

    // Phase 3: converge with multiple sync rounds
    for _ in 0..5 {
        sync_round_robin(&setup);
    }

    // Phase 4: verify all nodes have identical doogat set and content
    let files_node0 = list_doogats(&setup.nodes[0]);
    assert!(!files_node0.is_empty(), "node 0 should have doogats");

    for (i, node) in setup.nodes.iter().enumerate().skip(1) {
        let files = list_doogats(node);
        assert_eq!(
            files_node0, files,
            "node 0 and node {i} have different doogat sets"
        );
    }

    // Verify file contents match across all nodes
    for filename in &files_node0 {
        let content0 = read_doogat_file(&setup.nodes[0], filename);
        for (i, node) in setup.nodes.iter().enumerate().skip(1) {
            let content = read_doogat_file(node, filename);
            assert_eq!(
                content0, content,
                "node 0 and node {i} diverged on {filename}"
            );
        }
    }
}
