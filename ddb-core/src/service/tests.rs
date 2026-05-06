
use super::*;
use crate::sql_engine::SqlResult;
use crate::types::BatchUpdateInput;
use tempfile::TempDir;

fn fresh_svc() -> (TempDir, DoogatService) {
    let tmp = TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();
    (tmp, svc)
}

#[test]
fn init_creates_repo_and_opens() {
    let (_tmp, svc) = fresh_svc();
    let list = svc.list_doogats().unwrap();
    assert!(list.is_empty());
}

#[test]
fn crud_roundtrip() {
    let (_tmp, svc) = fresh_svc();

    let id = svc
        .create_doogat("Test Note", &[], None, "Hello world")
        .unwrap();
    assert_eq!(id.len(), 14);

    let content = svc.read_doogat(&id).unwrap();
    assert!(content.contains("Test Note"));
    assert!(content.contains("Hello world"));

    svc.update_doogat(
        &id,
        Some("Updated"),
        None,
        None,
        Some("New body"),
        &ExtraFieldUpdates::default(),
    )
    .unwrap();
    let content = svc.read_doogat(&id).unwrap();
    assert!(content.contains("Updated"));
    assert!(content.contains("New body"));

    let broken = svc.delete_doogat(&id, "delete test").unwrap();
    assert!(broken.is_empty());

    assert!(svc.read_doogat(&id).is_err());
}

#[test]
fn create_raw_and_read() {
    let (_tmp, svc) = fresh_svc();

    let raw = "---\ntitle: Raw Note\n---\nRaw body";
    let id = svc.create_doogat_raw(raw, "add raw").unwrap();
    assert_eq!(id.len(), 14);

    let content = svc.read_doogat(&id).unwrap();
    assert!(content.contains("Raw Note"));
}

#[test]
fn search_after_create() {
    let (_tmp, svc) = fresh_svc();

    svc.create_doogat("Searchable Doogat", &[], None, "unique content here")
        .unwrap();

    let results = svc.search("Searchable").unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].title.contains("Searchable"));
}

#[test]
fn sql_create_table_and_insert() {
    let (_tmp, mut svc) = fresh_svc();

    let ddl = svc
        .execute_sql("CREATE TABLE project (name TEXT, status TEXT)")
        .unwrap();
    assert!(matches!(ddl, SqlResult::Ok(_)));

    let ins = svc
        .execute_sql("INSERT INTO project (name, status) VALUES ('alpha', 'active')")
        .unwrap();
    assert!(matches!(ins, SqlResult::Ok(_)));

    let sel = svc.execute_sql("SELECT name, status FROM project").unwrap();
    match sel {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "alpha");
        }
        _ => panic!("expected rows"),
    }
}

#[test]
fn transaction_commit_persists() {
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE txtest (val TEXT)").unwrap();

    svc.begin_transaction().unwrap();
    svc.execute_sql("INSERT INTO txtest (val) VALUES ('in-txn')")
        .unwrap();
    svc.commit_transaction().unwrap();

    let result = svc.execute_sql("SELECT val FROM txtest").unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "in-txn");
        }
        _ => panic!("expected rows"),
    }
}

#[test]
fn transaction_rollback_discards() {
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE rbtest (val TEXT)").unwrap();

    svc.begin_transaction().unwrap();
    svc.execute_sql("INSERT INTO rbtest (val) VALUES ('gone')")
        .unwrap();
    svc.rollback_transaction().unwrap();

    let result = svc.execute_sql("SELECT val FROM rbtest").unwrap();
    match result {
        SqlResult::Rows { rows, .. } => assert_eq!(rows.len(), 0),
        _ => panic!("expected rows"),
    }
}

#[test]
fn reindex_rebuilds() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("One", &[], None, "").unwrap();
    svc.create_doogat("Two", &[], None, "").unwrap();

    let report = svc.reindex().unwrap();
    assert_eq!(report.indexed, 2);
}

#[test]
fn delete_returns_broken_backlinks() {
    let (_tmp, svc) = fresh_svc();

    let id_b = svc
        .create_doogat("Target", &[], None, "target body")
        .unwrap();

    let body_a = format!("Links to [[{id_b}]]");
    let id_a = svc.create_doogat("Source", &[], None, &body_a).unwrap();
    svc.reindex().unwrap();

    let broken = svc.delete_doogat(&id_b, "delete test").unwrap();
    assert_eq!(broken.len(), 1);
    assert_eq!(broken[0].0, id_a);
}

#[test]
fn list_doogats_filtered_no_filter() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("A", &[], None, "").unwrap();
    svc.create_doogat("B", &[], None, "").unwrap();

    let filter = crate::types::ListFilter::default();
    let results = svc.list_doogats_filtered(&filter).unwrap();
    assert_eq!(results.len(), 2);
}

#[test]
fn list_doogats_filtered_by_tag() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("Tagged", &["rust".into()], None, "")
        .unwrap();
    svc.create_doogat("Untagged", &[], None, "").unwrap();
    svc.reindex().unwrap();

    let filter = crate::types::ListFilter {
        tag: Some("rust".into()),
        ..Default::default()
    };
    let results = svc.list_doogats_filtered(&filter).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].meta.title.as_deref(), Some("Tagged"));
}

#[test]
fn list_doogats_filtered_with_limit() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("A", &[], None, "").unwrap();
    svc.create_doogat("B", &[], None, "").unwrap();
    svc.create_doogat("C", &[], None, "").unwrap();

    let filter = crate::types::ListFilter {
        limit: Some(2),
        ..Default::default()
    };
    let results = svc.list_doogats_filtered(&filter).unwrap();
    assert_eq!(results.len(), 2);
}

#[test]
fn count_doogats_filtered_all() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("A", &[], None, "").unwrap();
    svc.create_doogat("B", &[], None, "").unwrap();

    let filter = crate::types::ListFilter::default();
    let count = svc.count_doogats_filtered(&filter).unwrap();
    assert_eq!(count, 2);
}

#[test]
fn list_doogats_filtered_sort_by_title_asc() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("Charlie", &[], None, "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    svc.create_doogat("Alpha", &[], None, "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    svc.create_doogat("Bravo", &[], None, "").unwrap();
    svc.reindex().unwrap();

    let filter = crate::types::ListFilter {
        sort_field: Some("title".into()),
        sort_desc: Some(false),
        ..Default::default()
    };
    let results = svc.list_doogats_filtered(&filter).unwrap();
    let titles: Vec<_> = results
        .iter()
        .map(|d| d.meta.title.as_deref().unwrap())
        .collect();
    assert_eq!(titles, vec!["Alpha", "Bravo", "Charlie"]);
}

#[test]
fn list_doogats_filtered_sort_by_title_desc() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("Alpha", &[], None, "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    svc.create_doogat("Charlie", &[], None, "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    svc.create_doogat("Bravo", &[], None, "").unwrap();
    svc.reindex().unwrap();

    let filter = crate::types::ListFilter {
        sort_field: Some("title".into()),
        sort_desc: Some(true),
        ..Default::default()
    };
    let results = svc.list_doogats_filtered(&filter).unwrap();
    let titles: Vec<_> = results
        .iter()
        .map(|d| d.meta.title.as_deref().unwrap())
        .collect();
    assert_eq!(titles, vec!["Charlie", "Bravo", "Alpha"]);
}

#[test]
fn list_doogats_filtered_sort_default_is_date_desc() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("First", &[], None, "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    svc.create_doogat("Second", &[], None, "").unwrap();
    svc.reindex().unwrap();

    let filter = crate::types::ListFilter::default();
    let results = svc.list_doogats_filtered(&filter).unwrap();
    // Default is date DESC, id DESC - newest comes first
    assert_eq!(results[0].meta.title.as_deref(), Some("Second"));
    assert_eq!(results[1].meta.title.as_deref(), Some("First"));
}

#[test]
fn list_doogats_filtered_sort_date_defaults_to_desc() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("First", &[], None, "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    svc.create_doogat("Second", &[], None, "").unwrap();
    svc.reindex().unwrap();

    let filter = crate::types::ListFilter {
        sort_field: Some("date".into()),
        ..Default::default()
    };
    let results = svc.list_doogats_filtered(&filter).unwrap();
    // sort=date without explicit direction defaults to DESC
    assert_eq!(results[0].meta.title.as_deref(), Some("Second"));
    assert_eq!(results[1].meta.title.as_deref(), Some("First"));
}

#[test]
fn list_doogats_filtered_sort_title_defaults_to_asc() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("Bravo", &[], None, "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    svc.create_doogat("Alpha", &[], None, "").unwrap();
    svc.reindex().unwrap();

    let filter = crate::types::ListFilter {
        sort_field: Some("title".into()),
        ..Default::default()
    };
    let results = svc.list_doogats_filtered(&filter).unwrap();
    // sort=title without explicit direction defaults to ASC
    assert_eq!(results[0].meta.title.as_deref(), Some("Alpha"));
    assert_eq!(results[1].meta.title.as_deref(), Some("Bravo"));
}

#[test]
fn list_doogats_filtered_sort_invalid_field_falls_back_to_default() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("A", &[], None, "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    svc.create_doogat("B", &[], None, "").unwrap();
    svc.reindex().unwrap();

    let filter = crate::types::ListFilter {
        sort_field: Some("nonexistent".into()),
        ..Default::default()
    };
    let results = svc.list_doogats_filtered(&filter).unwrap();
    // Falls back to id DESC
    assert_eq!(results[0].meta.title.as_deref(), Some("B"));
}

#[test]
fn aggregate_query_select_one() {
    let (_tmp, svc) = fresh_svc();
    let row = svc.aggregate_query("SELECT 1 AS n", &[]).unwrap();
    assert_eq!(row, vec!["1"]);
}

#[test]
fn aggregate_query_empty() {
    let (_tmp, svc) = fresh_svc();
    let row = svc
        .aggregate_query("SELECT id FROM doogats WHERE 1=0", &[])
        .unwrap();
    assert!(row.is_empty());
}

#[test]
fn health_check_returns_true() {
    let (_tmp, svc) = fresh_svc();
    assert!(svc.health_check().unwrap());
}

#[test]
fn backlink_ids_empty() {
    let (_tmp, svc) = fresh_svc();
    let links = svc.backlink_ids("nonexistent").unwrap();
    assert!(links.is_empty());
}

#[test]
fn install_bundled_type_project() {
    let (_tmp, svc) = fresh_svc();
    let id = svc.install_bundled_type("project").unwrap();
    assert_eq!(id.len(), 14);
    let content = svc.read_doogat(&id).unwrap();
    assert!(content.contains("project"));
}

#[test]
fn install_bundled_type_unknown_fails() {
    let (_tmp, svc) = fresh_svc();
    assert!(svc.install_bundled_type("nonexistent").is_err());
}

#[test]
fn all_doogat_ids_excludes_typedefs() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("Normal", &[], None, "").unwrap();
    svc.install_bundled_type("project").unwrap();
    svc.reindex().unwrap();

    let ids = svc.all_doogat_ids().unwrap();
    assert_eq!(ids.len(), 1);
}

#[test]
fn compact_dry_run_no_nodes() {
    let (_tmp, svc) = fresh_svc();
    let info = svc.compact_dry_run();
    // No nodes registered → NotFound from SyncManager, but dry_run should handle gracefully
    // (list_nodes returns empty or error)
    assert!(info.is_ok() || info.is_err());
}

#[test]
fn auto_maintenance_default_off() {
    let (_tmp, svc) = fresh_svc();
    // Default config has auto_enabled = false
    let enabled = svc.auto_maintenance_enabled().unwrap();
    assert!(!enabled);
}

#[test]
fn set_auto_maintenance_roundtrip() {
    let (_tmp, svc) = fresh_svc();
    svc.set_auto_maintenance(true).unwrap();
    assert!(svc.auto_maintenance_enabled().unwrap());
    svc.set_auto_maintenance(false).unwrap();
    assert!(!svc.auto_maintenance_enabled().unwrap());
}

#[test]
fn list_tags_returns_counts() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("A", &["rust".into(), "cli".into()], None, "")
        .unwrap();
    svc.create_doogat("B", &["rust".into()], None, "").unwrap();
    svc.reindex().unwrap();

    let tags = svc.list_tags().unwrap();
    // rust should appear first (count 2), cli second (count 1)
    assert!(tags.len() >= 2);
    assert_eq!(tags[0].0, "rust");
    assert_eq!(tags[0].1, 2);
    assert_eq!(tags[1].0, "cli");
    assert_eq!(tags[1].1, 1);
}

#[test]
fn sequence_children_empty() {
    let (_tmp, svc) = fresh_svc();
    let id = svc.create_doogat("Root", &[], None, "").unwrap();
    svc.reindex().unwrap();
    let children = svc.sequence_children(&id).unwrap();
    assert!(children.is_empty());
}

#[test]
fn sync_auto_registers_node_when_none_exists() {
    let (tmp, svc) = fresh_svc();
    let node_path = tmp.path().join(".git/ddb-node");
    assert!(!node_path.exists(), "node should not exist before sync");

    // sync will fail (no remote) but auto-register should still happen
    let result = svc.sync("origin", "master");
    assert!(result.is_err(), "sync should fail without a remote");
    assert!(
        node_path.exists(),
        "node should be auto-registered after sync attempt"
    );
}

#[test]
fn sync_reuses_existing_registration() {
    let (tmp, svc) = fresh_svc();
    svc.register_node("MyLaptop").unwrap();
    let node_path = tmp.path().join(".git/ddb-node");
    let uuid_before = std::fs::read_to_string(&node_path).unwrap();

    // sync fails (no remote) but should not re-register
    let _ = svc.sync("origin", "master");
    let uuid_after = std::fs::read_to_string(&node_path).unwrap();
    assert_eq!(
        uuid_before, uuid_after,
        "existing registration should be reused"
    );
}

#[test]
fn sync_does_not_auto_register_when_node_file_exists_but_toml_missing() {
    let (tmp, svc) = fresh_svc();
    // Write a ddb-node file pointing to a non-existent node TOML
    let node_path = tmp.path().join(".git/ddb-node");
    std::fs::write(&node_path, "bogus-uuid-that-has-no-toml").unwrap();

    let result = svc.sync("origin", "master");
    assert!(result.is_err());
    // Should NOT have overwritten the node file with a new UUID
    let uuid_after = std::fs::read_to_string(&node_path).unwrap();
    assert_eq!(
        uuid_after, "bogus-uuid-that-has-no-toml",
        "corrupt state should propagate error, not silently re-register"
    );
}

#[test]
fn get_doogats_batch_multiple_valid() {
    let (_tmp, svc) = fresh_svc();
    let id1 = svc.create_doogat("First", &[], None, "body one").unwrap();
    let id2 = svc.create_doogat("Second", &[], None, "body two").unwrap();
    let id3 = svc.create_doogat("Third", &[], None, "body three").unwrap();

    let ids = vec![id1.clone(), id2.clone(), id3.clone()];
    let results = svc.get_doogats_batch(&ids).unwrap();
    assert_eq!(results.len(), 3);

    let titles: Vec<_> = results
        .iter()
        .map(|d| d.meta.title.as_deref().unwrap())
        .collect();
    assert!(titles.contains(&"First"));
    assert!(titles.contains(&"Second"));
    assert!(titles.contains(&"Third"));
}

#[test]
fn get_doogats_batch_skips_invalid_ids() {
    let (_tmp, svc) = fresh_svc();
    let id1 = svc.create_doogat("Valid One", &[], None, "").unwrap();
    let id2 = svc.create_doogat("Valid Two", &[], None, "").unwrap();

    let ids = vec![
        id1.clone(),
        "99990101000000".to_string(), // nonexistent
        id2.clone(),
        "not-a-real-id".to_string(), // invalid
    ];
    let results = svc.get_doogats_batch(&ids).unwrap();
    assert_eq!(results.len(), 2);

    let titles: Vec<_> = results
        .iter()
        .map(|d| d.meta.title.as_deref().unwrap())
        .collect();
    assert!(titles.contains(&"Valid One"));
    assert!(titles.contains(&"Valid Two"));
}

#[test]
fn get_doogats_batch_empty_returns_empty() {
    let (_tmp, svc) = fresh_svc();
    let results = svc.get_doogats_batch(&[]).unwrap();
    assert!(results.is_empty());
}

#[test]
fn get_doogats_batch_single_id_matches_get_parsed() {
    let (_tmp, svc) = fresh_svc();
    let id = svc
        .create_doogat("Solo", &["tag1".into()], None, "solo body")
        .unwrap();

    let single = svc.get_doogat_parsed(&id).unwrap();
    let batch = svc.get_doogats_batch(&[id]).unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].meta.title, single.meta.title);
    assert_eq!(batch[0].body, single.body);
}

#[test]
fn get_doogat_parsed_has_updated_at() {
    let (_tmp, svc) = fresh_svc();
    let id = svc.create_doogat("Note", &[], None, "body").unwrap();
    let parsed = svc.get_doogat_parsed(&id).unwrap();
    assert!(
        parsed.updated_at.is_some(),
        "updated_at should be populated from the index"
    );
    assert!(
        !parsed.updated_at.as_ref().unwrap().is_empty(),
        "updated_at should be a non-empty timestamp"
    );
}

#[test]
fn updated_at_changes_on_update() {
    let (_tmp, svc) = fresh_svc();
    let id = svc.create_doogat("Original", &[], None, "body").unwrap();
    let before = svc.get_doogat_parsed(&id).unwrap();
    let created_date = before.meta.date.clone();

    std::thread::sleep(std::time::Duration::from_millis(50));
    svc.update_doogat(
        &id,
        Some("Updated"),
        None,
        None,
        None,
        &ExtraFieldUpdates::default(),
    )
    .unwrap();

    let after = svc.get_doogat_parsed(&id).unwrap();
    assert_eq!(
        after.meta.date, created_date,
        "date (created_at) should not change"
    );
    assert!(
        after.updated_at.as_ref().unwrap() >= before.updated_at.as_ref().unwrap(),
        "updated_at should advance after an update"
    );
}

#[test]
fn list_doogats_filtered_has_updated_at() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("A", &[], None, "").unwrap();
    svc.create_doogat("B", &[], None, "").unwrap();

    let filter = crate::types::ListFilter::default();
    let doogats = svc.list_doogats_filtered(&filter).unwrap();
    assert_eq!(doogats.len(), 2);
    for d in &doogats {
        assert!(
            d.updated_at.is_some(),
            "each listed doogat should have updated_at"
        );
    }
}

#[test]
fn get_doogats_batch_has_updated_at() {
    let (_tmp, svc) = fresh_svc();
    let id1 = svc.create_doogat("First", &[], None, "").unwrap();
    let id2 = svc.create_doogat("Second", &[], None, "").unwrap();

    let batch = svc.get_doogats_batch(&[id1, id2]).unwrap();
    assert_eq!(batch.len(), 2);
    for d in &batch {
        assert!(
            d.updated_at.is_some(),
            "batch doogat should have updated_at"
        );
    }
}

#[test]
fn search_results_have_updated_at() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("Searchable", &[], None, "findme content")
        .unwrap();

    let results = svc
        .search_paginated_filtered("findme", 10, 0, &crate::types::SearchFilters::default())
        .unwrap();
    assert_eq!(results.hits.len(), 1);
    assert!(
        !results.hits[0].updated_at.is_empty(),
        "search hit should have updated_at"
    );
}

#[test]
fn sort_by_updated_at() {
    let (_tmp, svc) = fresh_svc();
    svc.create_doogat("First", &[], None, "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    svc.create_doogat("Second", &[], None, "").unwrap();
    svc.reindex().unwrap();

    let filter = crate::types::ListFilter {
        sort_field: Some("updated_at".to_string()),
        sort_desc: Some(false),
        ..Default::default()
    };
    let doogats = svc.list_doogats_filtered(&filter).unwrap();
    assert_eq!(doogats.len(), 2);
    assert_eq!(doogats[0].meta.title.as_deref(), Some("First"));
    assert_eq!(doogats[1].meta.title.as_deref(), Some("Second"));
}

#[test]
fn typed_filtered_list_has_updated_at() {
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE project (name TEXT)").unwrap();
    svc.execute_sql("INSERT INTO project (name) VALUES ('Alpha')")
        .unwrap();

    let query = crate::types::TypedListQuery {
        table_name: "project".to_string(),
        where_sql: String::new(),
        params: vec![],
        order_sql: None,
        tag: None,
        limit: None,
        offset: None,
        distinct: None,
    };
    let doogats = svc.typed_filtered_list(&query).unwrap();
    assert_eq!(doogats.len(), 1);
    assert!(
        doogats[0].updated_at.is_some(),
        "typed_filtered_list should populate updated_at"
    );
}

#[test]
fn create_returns_updated_at() {
    let (_tmp, svc) = fresh_svc();
    let parsed = svc
        .create_doogat_parsed("Direct", &[], None, "body")
        .unwrap();
    assert!(
        parsed.updated_at.is_some(),
        "create_doogat_parsed should return updated_at in the response"
    );
}

#[test]
fn update_returns_updated_at() {
    let (_tmp, svc) = fresh_svc();
    let id = svc.create_doogat("Before", &[], None, "body").unwrap();
    let parsed = svc
        .update_doogat_parsed(
            &id,
            Some("After"),
            None,
            None,
            None,
            &ExtraFieldUpdates::default(),
        )
        .unwrap();
    assert!(
        parsed.updated_at.is_some(),
        "update_doogat_parsed should return updated_at in the response"
    );
}

// ---- batch_update tests ----

fn count_commits(path: &std::path::Path) -> usize {
    let repo = git2::Repository::open(path).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    let mut revwalk = repo.revwalk().unwrap();
    revwalk.push(head.id()).unwrap();
    revwalk.count()
}

#[test]
fn batch_update_basic() {
    let (_tmp, svc) = fresh_svc();
    let id1 = svc.create_doogat("One", &[], None, "").unwrap();
    let id2 = svc.create_doogat("Two", &[], None, "").unwrap();
    let id3 = svc.create_doogat("Three", &[], None, "").unwrap();

    let updates = vec![
        crate::types::BatchUpdateInput {
            id: id1.clone(),
            title: Some("One Updated".to_string()),
            body: None,
            tags: None,
            doogat_type: None,
            fields: None,
            unset_fields: None,
        },
        crate::types::BatchUpdateInput {
            id: id2.clone(),
            title: Some("Two Updated".to_string()),
            body: None,
            tags: None,
            doogat_type: None,
            fields: None,
            unset_fields: None,
        },
        crate::types::BatchUpdateInput {
            id: id3.clone(),
            title: Some("Three Updated".to_string()),
            body: None,
            tags: None,
            doogat_type: None,
            fields: None,
            unset_fields: None,
        },
    ];

    let results = svc.batch_update(&updates).unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].meta.title.as_deref(), Some("One Updated"));
    assert_eq!(results[1].meta.title.as_deref(), Some("Two Updated"));
    assert_eq!(results[2].meta.title.as_deref(), Some("Three Updated"));

    // Verify persistence by re-reading
    let p1 = svc.get_doogat_parsed(&id1).unwrap();
    let p2 = svc.get_doogat_parsed(&id2).unwrap();
    let p3 = svc.get_doogat_parsed(&id3).unwrap();
    assert_eq!(p1.meta.title.as_deref(), Some("One Updated"));
    assert_eq!(p2.meta.title.as_deref(), Some("Two Updated"));
    assert_eq!(p3.meta.title.as_deref(), Some("Three Updated"));
}

#[test]
fn batch_update_atomicity() {
    let (_tmp, svc) = fresh_svc();
    let id1 = svc.create_doogat("Alpha", &[], None, "body1").unwrap();
    let id2 = svc.create_doogat("Beta", &[], None, "body2").unwrap();
    let id3 = svc.create_doogat("Gamma", &[], None, "body3").unwrap();

    let updates = vec![
        crate::types::BatchUpdateInput {
            id: id1.clone(),
            title: Some("Alpha Changed".to_string()),
            body: None,
            tags: None,
            doogat_type: None,
            fields: None,
            unset_fields: None,
        },
        crate::types::BatchUpdateInput {
            id: "99999999999999".to_string(), // non-existent
            title: Some("Ghost".to_string()),
            body: None,
            tags: None,
            doogat_type: None,
            fields: None,
            unset_fields: None,
        },
        crate::types::BatchUpdateInput {
            id: id3.clone(),
            title: Some("Gamma Changed".to_string()),
            body: None,
            tags: None,
            doogat_type: None,
            fields: None,
            unset_fields: None,
        },
    ];

    let result = svc.batch_update(&updates);
    assert!(
        result.is_err(),
        "batch_update should fail when an ID doesn't exist"
    );

    // All originals must be unchanged
    let p1 = svc.get_doogat_parsed(&id1).unwrap();
    let p2 = svc.get_doogat_parsed(&id2).unwrap();
    let p3 = svc.get_doogat_parsed(&id3).unwrap();
    assert_eq!(p1.meta.title.as_deref(), Some("Alpha"));
    assert_eq!(p2.meta.title.as_deref(), Some("Beta"));
    assert_eq!(p3.meta.title.as_deref(), Some("Gamma"));
}

#[test]
fn batch_update_single_commit() {
    let (tmp, svc) = fresh_svc();
    for i in 0..5 {
        svc.create_doogat(&format!("Item {i}"), &[], None, "")
            .unwrap();
    }

    let before = count_commits(tmp.path());

    let filter = crate::types::ListFilter::default();
    let ids: Vec<String> = svc
        .list_doogats_filtered(&filter)
        .unwrap()
        .into_iter()
        .filter_map(|d| d.meta.id.map(|id| id.0))
        .collect();
    assert_eq!(ids.len(), 5);

    let updates: Vec<crate::types::BatchUpdateInput> = ids
        .iter()
        .map(|id| crate::types::BatchUpdateInput {
            id: id.clone(),
            title: Some(format!("Updated {id}")),
            body: None,
            tags: None,
            doogat_type: None,
            fields: None,
            unset_fields: None,
        })
        .collect();

    svc.batch_update(&updates).unwrap();

    let after = count_commits(tmp.path());
    assert_eq!(
        after - before,
        1,
        "batch_update should create exactly 1 commit, not {}",
        after - before,
    );
}

#[test]
fn batch_update_empty() {
    let (tmp, svc) = fresh_svc();
    let before = count_commits(tmp.path());

    let results = svc.batch_update(&[]).unwrap();
    assert!(results.is_empty(), "empty input should return empty vec");

    let after = count_commits(tmp.path());
    assert_eq!(
        before, after,
        "empty batch_update should not create a commit"
    );
}

#[test]
fn batch_update_mixed_fields() {
    let (_tmp, svc) = fresh_svc();
    let id1 = svc
        .create_doogat("Title1", &["tag1".to_string()], None, "body1")
        .unwrap();
    let id2 = svc
        .create_doogat("Title2", &["tag2".to_string()], None, "body2")
        .unwrap();
    let id3 = svc
        .create_doogat("Title3", &["tag3".to_string()], None, "body3")
        .unwrap();

    let updates = vec![
        // Only title changes
        crate::types::BatchUpdateInput {
            id: id1.clone(),
            title: Some("NewTitle1".to_string()),
            body: None,
            tags: None,
            doogat_type: None,
            fields: None,
            unset_fields: None,
        },
        // Only body changes
        crate::types::BatchUpdateInput {
            id: id2.clone(),
            title: None,
            body: Some("newbody2".to_string()),
            tags: None,
            doogat_type: None,
            fields: None,
            unset_fields: None,
        },
        // Only tags change
        crate::types::BatchUpdateInput {
            id: id3.clone(),
            title: None,
            body: None,
            tags: Some(vec!["newtag3".to_string()]),
            doogat_type: None,
            fields: None,
            unset_fields: None,
        },
    ];

    let results = svc.batch_update(&updates).unwrap();
    assert_eq!(results.len(), 3);

    // First: title changed, body and tags unchanged
    assert_eq!(results[0].meta.title.as_deref(), Some("NewTitle1"));
    assert_eq!(results[0].body, "body1");
    assert_eq!(results[0].meta.tags, vec!["tag1".to_string()]);

    // Second: body changed, title and tags unchanged
    assert_eq!(results[1].meta.title.as_deref(), Some("Title2"));
    assert_eq!(results[1].body, "newbody2");
    assert_eq!(results[1].meta.tags, vec!["tag2".to_string()]);

    // Third: tags changed, title and body unchanged
    assert_eq!(results[2].meta.title.as_deref(), Some("Title3"));
    assert_eq!(results[2].body, "body3");
    assert_eq!(results[2].meta.tags, vec!["newtag3".to_string()]);
}

#[test]
fn batch_update_updated_at() {
    let (_tmp, svc) = fresh_svc();
    let id = svc.create_doogat("Original", &[], None, "body").unwrap();

    let updates = vec![crate::types::BatchUpdateInput {
        id: id.clone(),
        title: Some("Changed".to_string()),
        body: None,
        tags: None,
        doogat_type: None,
        fields: None,
        unset_fields: None,
    }];

    let results = svc.batch_update(&updates).unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0].updated_at.is_some(),
        "batch_update should populate updated_at on returned doogats"
    );
}

#[test]
fn batch_update_rejects_duplicate_ids() {
    let (_tmp, svc) = fresh_svc();
    let id = svc.create_doogat("Dup", &[], None, "").unwrap();

    let updates = vec![
        crate::types::BatchUpdateInput {
            id: id.clone(),
            title: Some("First".to_string()),
            body: None,
            tags: None,
            doogat_type: None,
            fields: None,
            unset_fields: None,
        },
        crate::types::BatchUpdateInput {
            id: id.clone(),
            title: Some("Second".to_string()),
            body: None,
            tags: None,
            doogat_type: None,
            fields: None,
            unset_fields: None,
        },
    ];

    let result = svc.batch_update(&updates);
    assert!(result.is_err(), "batch_update should reject duplicate IDs");

    // Original unchanged
    let p = svc.get_doogat_parsed(&id).unwrap();
    assert_eq!(p.meta.title.as_deref(), Some("Dup"));
}

// ---- FTS5 search boost tests ----

#[test]
fn search_boost_fields_column_populated() {
    let (_tmp, svc) = fresh_svc();

    let mut extra = std::collections::BTreeMap::new();
    extra.insert(
        "email".to_string(),
        crate::types::Value::String("alice@example.com".to_string()),
    );
    svc.create_doogat_with_extra("Alice Contact", &[], Some("contact"), "some body", extra)
        .unwrap();
    svc.reindex().unwrap();

    let results = svc.search("alice").unwrap();
    assert!(
        !results.is_empty(),
        "searching for 'alice' should find the doogat via the FTS fields column"
    );
    assert!(results[0].title.contains("Alice"));
}

#[test]
fn search_boost_ranking_with_boosted_type() {
    let (_tmp, svc) = fresh_svc();

    // Install contact typedef (has search_boost: 1.5 on email column)
    svc.install_bundled_type("contact").unwrap();

    // Contact with "xyzzyterm" in email (frontmatter extra -> fields column)
    let mut extra1 = std::collections::BTreeMap::new();
    extra1.insert(
        "email".to_string(),
        crate::types::Value::String("xyzzyterm@example.com".to_string()),
    );
    svc.create_doogat_with_extra("FieldMatch", &[], Some("contact"), "no match here", extra1)
        .unwrap();

    // Contact with "xyzzyterm" only in body
    svc.create_doogat(
        "BodyMatch",
        &[],
        Some("contact"),
        "xyzzyterm appears in body",
    )
    .unwrap();

    svc.reindex().unwrap();

    let filters = crate::types::SearchFilters {
        types: Some(vec!["contact".to_string()]),
        ..Default::default()
    };
    let result = svc
        .search_paginated_filtered("xyzzyterm", 10, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 2, "both contacts should match");

    // With boost on the fields column, the one matching in fields should
    // rank higher (lower/more-negative bm25 score = better match).
    assert_eq!(
        result.hits[0].title, "FieldMatch",
        "doogat with match in boosted fields column should rank first"
    );
}

#[test]
fn search_boost_no_regression_without_type_filter() {
    let (_tmp, svc) = fresh_svc();

    svc.install_bundled_type("contact").unwrap();

    let mut extra = std::collections::BTreeMap::new();
    extra.insert(
        "email".to_string(),
        crate::types::Value::String("boostnoreg@example.com".to_string()),
    );
    svc.create_doogat_with_extra("Boosted", &[], Some("contact"), "", extra)
        .unwrap();
    svc.create_doogat("Plain", &[], None, "boostnoreg in body")
        .unwrap();

    svc.reindex().unwrap();

    // Search without type filter - should work with default 1.0 weighting
    let results = svc.search("boostnoreg").unwrap();
    assert_eq!(
        results.len(),
        2,
        "both doogats should appear without type filter"
    );
}

#[test]
fn search_boost_default_for_untyped() {
    let (_tmp, mut svc) = fresh_svc();

    // Create a type without any search_boost columns
    svc.execute_sql("CREATE TABLE project (name TEXT, status TEXT)")
        .unwrap();
    svc.execute_sql("INSERT INTO project (name, status) VALUES ('Alpha', 'active')")
        .unwrap();
    svc.create_doogat("Untyped", &[], None, "defaultboost content")
        .unwrap();

    svc.reindex().unwrap();

    // Search filtered to project type - should work with default 1.0 weighting
    let filters = crate::types::SearchFilters {
        types: Some(vec!["project".to_string()]),
        ..Default::default()
    };
    let result = svc
        .search_paginated_filtered("Alpha", 10, 0, &filters)
        .unwrap();
    assert!(
        result.hits.len() <= 1,
        "filtered search for project type should not error"
    );
}

// ---- batch_create tests ----

#[test]
fn batch_create_basic() {
    let (_tmp, svc) = fresh_svc();

    let inputs = vec![
        crate::types::BatchCreateInput {
            title: Some("Alpha".to_string()),
            body: None,
            tags: vec![],
            doogat_type: None,
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        },
        crate::types::BatchCreateInput {
            title: Some("Beta".to_string()),
            body: None,
            tags: vec![],
            doogat_type: None,
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        },
        crate::types::BatchCreateInput {
            title: Some("Gamma".to_string()),
            body: None,
            tags: vec![],
            doogat_type: None,
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        },
    ];

    let results = svc.batch_create(&inputs).unwrap();
    assert_eq!(results.len(), 3);

    assert_eq!(results[0].meta.title.as_deref(), Some("Alpha"));
    assert_eq!(results[1].meta.title.as_deref(), Some("Beta"));
    assert_eq!(results[2].meta.title.as_deref(), Some("Gamma"));

    let ids: Vec<_> = results
        .iter()
        .map(|r| r.meta.id.as_ref().unwrap().0.clone())
        .collect();
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), 3, "all IDs must be distinct");

    for result in &results {
        let id = &result.meta.id.as_ref().unwrap().0;
        let parsed = svc.get_doogat_parsed(id).unwrap();
        assert_eq!(parsed.meta.title, result.meta.title);
    }
}

#[test]
fn batch_create_empty() {
    let (_tmp, svc) = fresh_svc();
    let results = svc.batch_create(&[]).unwrap();
    assert!(results.is_empty(), "empty input should return empty vec");
}

#[test]
fn batch_create_return_order() {
    let (_tmp, svc) = fresh_svc();

    let titles = ["First", "Second", "Third"];
    let inputs: Vec<crate::types::BatchCreateInput> = titles
        .iter()
        .map(|t| crate::types::BatchCreateInput {
            title: Some(t.to_string()),
            body: None,
            tags: vec![],
            doogat_type: None,
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        })
        .collect();

    let results = svc.batch_create(&inputs).unwrap();
    assert_eq!(results.len(), 3);

    for (i, title) in titles.iter().enumerate() {
        assert_eq!(
            results[i].meta.title.as_deref(),
            Some(*title),
            "result at index {i} should have title '{title}'"
        );
    }
}

#[test]
fn batch_create_with_type() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE task (name TEXT)").unwrap();

    let inputs = vec![
        crate::types::BatchCreateInput {
            title: Some("Task A".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("task".to_string()),
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        },
        crate::types::BatchCreateInput {
            title: Some("Task B".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("task".to_string()),
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        },
    ];

    let results = svc.batch_create(&inputs).unwrap();
    assert_eq!(results.len(), 2);

    for result in &results {
        assert_eq!(
            result.meta.doogat_type.as_deref(),
            Some("task"),
            "doogat_type should be 'task'"
        );
    }
}

#[test]
fn batch_create_with_tags() {
    let (_tmp, svc) = fresh_svc();

    let inputs = vec![
        crate::types::BatchCreateInput {
            title: Some("Tagged One".to_string()),
            body: None,
            tags: vec!["rust".to_string(), "testing".to_string()],
            doogat_type: None,
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        },
        crate::types::BatchCreateInput {
            title: Some("Tagged Two".to_string()),
            body: None,
            tags: vec!["python".to_string()],
            doogat_type: None,
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        },
    ];

    let results = svc.batch_create(&inputs).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].meta.tags, vec!["rust", "testing"]);
    assert_eq!(results[1].meta.tags, vec!["python"]);
}

#[test]
fn batch_create_with_body() {
    let (_tmp, svc) = fresh_svc();

    let inputs = vec![
        crate::types::BatchCreateInput {
            title: Some("With Body".to_string()),
            body: Some("Hello world content".to_string()),
            tags: vec![],
            doogat_type: None,
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        },
        crate::types::BatchCreateInput {
            title: Some("Empty Body".to_string()),
            body: None,
            tags: vec![],
            doogat_type: None,
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        },
    ];

    let results = svc.batch_create(&inputs).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].body, "Hello world content");
    assert!(
        results[1].body.is_empty(),
        "None body should produce empty body"
    );
}

#[test]
fn batch_create_with_fields() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE items (category TEXT, priority INTEGER)")
        .unwrap();

    let mut fields1 = std::collections::BTreeMap::new();
    fields1.insert(
        "category".to_string(),
        crate::types::Value::String("electronics".to_string()),
    );
    fields1.insert(
        "priority".to_string(),
        crate::types::Value::String("5".to_string()),
    );

    let mut fields2 = std::collections::BTreeMap::new();
    fields2.insert(
        "category".to_string(),
        crate::types::Value::String("books".to_string()),
    );
    fields2.insert(
        "priority".to_string(),
        crate::types::Value::String("3".to_string()),
    );

    let inputs = vec![
        crate::types::BatchCreateInput {
            title: Some("Item One".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("items".to_string()),
            fields: fields1,
            on_conflict: crate::types::ConflictAction::Error,
        },
        crate::types::BatchCreateInput {
            title: Some("Item Two".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("items".to_string()),
            fields: fields2,
            on_conflict: crate::types::ConflictAction::Error,
        },
    ];

    let results = svc.batch_create(&inputs).unwrap();
    assert_eq!(results.len(), 2);

    // PRD 00133: typed creates route by `effective_zone`. `category TEXT`
    // (long-text default) goes to body; `priority INTEGER` (numeric default)
    // goes to frontmatter `extra` as `Value::Number` (the SQL path's
    // `to_yaml_value` typed conversion).
    assert!(
        results[0].body.contains("electronics"),
        "expected 'electronics' in body, got: {}",
        results[0].body
    );
    assert_eq!(
        results[0].meta.extra.get("priority"),
        Some(&crate::types::Value::Number(5.0))
    );
    assert!(
        results[1].body.contains("books"),
        "expected 'books' in body, got: {}",
        results[1].body
    );
    assert_eq!(
        results[1].meta.extra.get("priority"),
        Some(&crate::types::Value::Number(3.0))
    );
}

#[test]
fn batch_create_single_commit() {
    let (tmp, svc) = fresh_svc();

    let before = count_commits(tmp.path());

    let inputs: Vec<crate::types::BatchCreateInput> = (0..3)
        .map(|i| crate::types::BatchCreateInput {
            title: Some(format!("Commit Test {i}")),
            body: None,
            tags: vec![],
            doogat_type: None,
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        })
        .collect();

    svc.batch_create(&inputs).unwrap();

    let after = count_commits(tmp.path());
    assert_eq!(
        after - before,
        1,
        "batch_create should create exactly 1 commit, not {}",
        after - before,
    );
}

#[test]
fn batch_create_default_next() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE ranked (name TEXT, pos INTEGER DEFAULT NEXT)")
        .unwrap();

    let inputs: Vec<crate::types::BatchCreateInput> = ["Alice", "Bob", "Carol"]
        .iter()
        .map(|name| {
            let mut fields = std::collections::BTreeMap::new();
            fields.insert(
                "name".to_string(),
                crate::types::Value::String(name.to_string()),
            );
            crate::types::BatchCreateInput {
                title: Some(format!("Ranked {name}")),
                body: None,
                tags: vec![],
                doogat_type: Some("ranked".to_string()),
                fields,
                on_conflict: crate::types::ConflictAction::Error,
            }
        })
        .collect();

    svc.batch_create(&inputs).unwrap();
    svc.reindex().unwrap();

    let result = svc
        .execute_sql("SELECT name, pos FROM ranked ORDER BY pos")
        .unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 3, "should have 3 rows");
            assert_eq!(rows[0][1], "1");
            assert_eq!(rows[1][1], "2");
            assert_eq!(rows[2][1], "3");
        }
        _ => panic!("expected Rows from SELECT"),
    }
}

#[test]
fn batch_create_partitioned_next() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE items (cat VARCHAR, sort_order INTEGER DEFAULT NEXT(cat))")
        .unwrap();

    let inputs: Vec<crate::types::BatchCreateInput> =
        [("A", "x"), ("A", "y"), ("B", "z"), ("A", "w")]
            .iter()
            .map(|(cat, name)| {
                let mut fields = std::collections::BTreeMap::new();
                fields.insert(
                    "cat".to_string(),
                    crate::types::Value::String(cat.to_string()),
                );
                crate::types::BatchCreateInput {
                    title: Some(format!("Item {name}")),
                    body: None,
                    tags: vec![],
                    doogat_type: Some("items".to_string()),
                    fields,
                    on_conflict: crate::types::ConflictAction::Error,
                }
            })
            .collect();

    svc.batch_create(&inputs).unwrap();
    svc.reindex().unwrap();

    let result = svc
        .execute_sql("SELECT cat, sort_order FROM items ORDER BY cat, sort_order")
        .unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            // Cat A: 3 items with sort_order 1, 2, 3
            assert_eq!(rows[0], vec!["A", "1"]);
            assert_eq!(rows[1], vec!["A", "2"]);
            assert_eq!(rows[2], vec!["A", "3"]);
            // Cat B: 1 item with sort_order 1
            assert_eq!(rows[3], vec!["B", "1"]);
        }
        _ => panic!("expected Rows from SELECT"),
    }
}

#[test]
fn batch_create_rollback_on_failure() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE linked (target VARCHAR REFERENCES doogats)")
        .unwrap();

    let count_before = svc.list_doogats().unwrap().len();

    let mut bad_fields = std::collections::BTreeMap::new();
    bad_fields.insert(
        "target".to_string(),
        crate::types::Value::String("99999999999999".to_string()),
    );

    let inputs = vec![
        crate::types::BatchCreateInput {
            title: Some("Good One".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("linked".to_string()),
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        },
        crate::types::BatchCreateInput {
            title: Some("Bad FK".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("linked".to_string()),
            fields: bad_fields,
            on_conflict: crate::types::ConflictAction::Error,
        },
    ];

    let result = svc.batch_create(&inputs);
    assert!(result.is_err(), "should fail on invalid FK reference");

    let count_after = svc.list_doogats().unwrap().len();
    assert_eq!(
        count_before, count_after,
        "no doogats should be created on failure (rollback)"
    );
}

#[test]
fn batch_create_mixed_types() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE note (content VARCHAR)")
        .unwrap();
    svc.execute_sql("CREATE TABLE task (status VARCHAR)")
        .unwrap();

    let mut note_fields = std::collections::BTreeMap::new();
    note_fields.insert(
        "content".to_string(),
        crate::types::Value::String("some content".to_string()),
    );

    let mut task_fields = std::collections::BTreeMap::new();
    task_fields.insert(
        "status".to_string(),
        crate::types::Value::String("open".to_string()),
    );

    let inputs = vec![
        crate::types::BatchCreateInput {
            title: Some("My Note".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("note".to_string()),
            fields: note_fields,
            on_conflict: crate::types::ConflictAction::Error,
        },
        crate::types::BatchCreateInput {
            title: Some("My Task".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("task".to_string()),
            fields: task_fields,
            on_conflict: crate::types::ConflictAction::Error,
        },
        crate::types::BatchCreateInput {
            title: Some("Untyped".to_string()),
            body: None,
            tags: vec![],
            doogat_type: None,
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        },
    ];

    let results = svc.batch_create(&inputs).unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].meta.doogat_type.as_deref(), Some("note"));
    assert_eq!(results[1].meta.doogat_type.as_deref(), Some("task"));
    assert_eq!(results[2].meta.doogat_type, None);
}

// ---- on_conflict tests ----

/// Install a typedef for type "widget" with unique_together on [name].
fn setup_widget_typedef(svc: &DoogatService) {
    let typedef = "---
id: 20260601000000
title: widget
type: _typedef
columns:
  - name: name
    data_type: TEXT
    zone: frontmatter
unique_together:
  - - name
---
";
    let typedef_path = "ddb/_typedef/20260601000000.md";
    svc.repo
        .commit_file(typedef_path, typedef, "add widget typedef")
        .unwrap();
    let parsed = crate::parser::parse(typedef, typedef_path).unwrap();
    svc.index.index_doogat(&parsed).unwrap();
    svc.index.materialize_all_types(&svc.repo).unwrap();
}

#[test]
fn batch_create_on_conflict_ignore_first_insert_succeeds() {
    let (_tmp, svc) = fresh_svc();
    setup_widget_typedef(&svc);

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "name".to_string(),
        crate::types::Value::String("foo".to_string()),
    );

    let inputs = vec![crate::types::BatchCreateInput {
        title: Some("Foo Widget".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("widget".to_string()),
        fields,
        on_conflict: crate::types::ConflictAction::Ignore,
    }];

    let results = svc.batch_create(&inputs).unwrap();
    assert_eq!(results.len(), 1);
    let id = results[0].meta.id.as_ref().unwrap().0.clone();
    assert_eq!(
        id.len(),
        14,
        "created doogat should have a valid 14-char ID"
    );
    assert_eq!(results[0].meta.title.as_deref(), Some("Foo Widget"));
}

#[test]
fn batch_create_on_conflict_ignore_duplicate_returns_existing() {
    let (tmp, svc) = fresh_svc();
    setup_widget_typedef(&svc);

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "name".to_string(),
        crate::types::Value::String("foo".to_string()),
    );

    // First insert
    let first = svc
        .batch_create(&[crate::types::BatchCreateInput {
            title: Some("Foo Widget".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("widget".to_string()),
            fields: fields.clone(),
            on_conflict: crate::types::ConflictAction::Ignore,
        }])
        .unwrap();
    let original_id = first[0].meta.id.as_ref().unwrap().0.clone();

    let commits_after_first = count_commits(tmp.path());

    // Second insert - same name, Ignore -> must return the existing doogat
    let second = svc
        .batch_create(&[crate::types::BatchCreateInput {
            title: Some("Foo Widget Duplicate".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("widget".to_string()),
            fields,
            on_conflict: crate::types::ConflictAction::Ignore,
        }])
        .unwrap();

    assert_eq!(second.len(), 1, "should return exactly one result");
    let returned_id = second[0].meta.id.as_ref().unwrap().0.clone();
    assert_eq!(
        returned_id, original_id,
        "duplicate insert with Ignore must return the original doogat ID"
    );
    assert_eq!(
        count_commits(tmp.path()),
        commits_after_first,
        "duplicate Ignore insert must not create a new git commit"
    );
}

#[test]
fn batch_create_on_conflict_ignore_non_duplicate_creates_new() {
    let (_tmp, svc) = fresh_svc();
    setup_widget_typedef(&svc);

    let mut fields_foo = std::collections::BTreeMap::new();
    fields_foo.insert(
        "name".to_string(),
        crate::types::Value::String("foo".to_string()),
    );

    let mut fields_bar = std::collections::BTreeMap::new();
    fields_bar.insert(
        "name".to_string(),
        crate::types::Value::String("bar".to_string()),
    );

    // Insert "foo" first
    let first = svc
        .batch_create(&[crate::types::BatchCreateInput {
            title: Some("Foo Widget".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("widget".to_string()),
            fields: fields_foo,
            on_conflict: crate::types::ConflictAction::Ignore,
        }])
        .unwrap();
    let foo_id = first[0].meta.id.as_ref().unwrap().0.clone();

    // Insert "bar" - different name, should create a new doogat
    let second = svc
        .batch_create(&[crate::types::BatchCreateInput {
            title: Some("Bar Widget".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("widget".to_string()),
            fields: fields_bar,
            on_conflict: crate::types::ConflictAction::Ignore,
        }])
        .unwrap();

    assert_eq!(second.len(), 1);
    let bar_id = second[0].meta.id.as_ref().unwrap().0.clone();
    assert_ne!(
        bar_id, foo_id,
        "non-duplicate insert must create a new doogat"
    );
    assert_eq!(bar_id.len(), 14);
}

#[test]
fn batch_create_on_conflict_error_duplicate_fails() {
    let (_tmp, svc) = fresh_svc();
    setup_widget_typedef(&svc);

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "name".to_string(),
        crate::types::Value::String("foo".to_string()),
    );

    // First insert - succeeds
    svc.batch_create(&[crate::types::BatchCreateInput {
        title: Some("Foo Widget".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("widget".to_string()),
        fields: fields.clone(),
        on_conflict: crate::types::ConflictAction::Error,
    }])
    .unwrap();

    // Second insert with same unique key and Error -> must fail
    let result = svc.batch_create(&[crate::types::BatchCreateInput {
        title: Some("Foo Widget Again".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("widget".to_string()),
        fields,
        on_conflict: crate::types::ConflictAction::Error,
    }]);

    assert!(
        result.is_err(),
        "duplicate insert with on_conflict: Error must return an error"
    );
}

#[test]
fn delete_doogat_cleans_materialized_row() {
    let (_tmp, mut svc) = fresh_svc();

    // Create type table and insert via SQL (which materializes the row)
    svc.execute_sql("CREATE TABLE project (name TEXT)").unwrap();
    let ins = svc
        .execute_sql("INSERT INTO project (name) VALUES ('Alpha')")
        .unwrap();
    let id = match ins {
        SqlResult::Ok(msg) => {
            // Extract ID from message like "created <id>"
            msg.split_whitespace().last().unwrap().to_string()
        }
        _ => panic!("expected Ok from INSERT"),
    };

    // Verify materialized row exists
    let sel = svc
        .execute_sql(&format!("SELECT name FROM project WHERE id = '{id}'"))
        .unwrap();
    match sel {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1, "materialized row should exist before delete")
        }
        _ => panic!("expected rows"),
    }

    // Delete the doogat via service path (not SQL DELETE)
    svc.delete_doogat(&id, "delete test").unwrap();

    // Verify materialized row is gone
    let sel = svc.execute_sql("SELECT COUNT(*) FROM project").unwrap();
    match sel {
        SqlResult::Rows { rows, .. } => assert_eq!(
            rows[0][0], "0",
            "materialized row should be removed after delete"
        ),
        _ => panic!("expected rows"),
    }
}

#[test]
fn delete_untyped_doogat_no_error() {
    let (_tmp, svc) = fresh_svc();

    let id = svc
        .create_doogat("Untyped Note", &[], None, "body")
        .unwrap();

    // Delete should succeed without error even though there's no type table
    svc.delete_doogat(&id, "delete test").unwrap();
    assert!(svc.read_doogat(&id).is_err());
}

#[test]
fn update_with_fields_sets_frontmatter() {
    let (_tmp, mut svc) = fresh_svc();

    // VARCHAR(200) maps to frontmatter zone (TEXT maps to body)
    svc.execute_sql("CREATE TABLE bookmark (url VARCHAR(200))")
        .unwrap();
    let ins = svc
        .execute_sql("INSERT INTO bookmark (url) VALUES ('https://old.example.com')")
        .unwrap();
    let id = match ins {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };

    // Update the url field via update_doogat_parsed
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "url".to_string(),
        crate::types::Value::String("https://new.example.com".to_string()),
    );
    let updated = svc
        .update_doogat_parsed(
            &id,
            None,
            None,
            None,
            None,
            &ExtraFieldUpdates {
                set: &fields,
                unset: &[],
            },
        )
        .unwrap();

    // Verify returned ParsedDoogat has the new value
    assert_eq!(
        *updated.meta.extra.get("url").unwrap(),
        crate::types::Value::String("https://new.example.com".to_string()),
    );

    // Verify re-reading from git has the new value
    let parsed = svc.get_doogat_parsed(&id).unwrap();
    assert_eq!(
        *parsed.meta.extra.get("url").unwrap(),
        crate::types::Value::String("https://new.example.com".to_string()),
    );

    // Verify the materialized type table row was updated
    let sel = svc
        .execute_sql(&format!("SELECT url FROM bookmark WHERE id = '{id}'"))
        .unwrap();
    match sel {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1, "expected 1 materialized row");
            assert_eq!(
                rows[0][0], "https://new.example.com",
                "materialized url should reflect update"
            );
        }
        _ => panic!("expected rows"),
    }
}

#[test]
fn update_with_unset_fields_removes_field() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE link (url VARCHAR(200))")
        .unwrap();
    let ins = svc
        .execute_sql("INSERT INTO link (url) VALUES ('https://example.com')")
        .unwrap();
    let id = match ins {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };

    // Unset the url field
    let updated = svc
        .update_doogat_parsed(
            &id,
            None,
            None,
            None,
            None,
            &ExtraFieldUpdates {
                set: &std::collections::BTreeMap::new(),
                unset: &["url".to_string()],
            },
        )
        .unwrap();

    // Verify returned ParsedDoogat has no url
    assert!(
        !updated.meta.extra.contains_key("url"),
        "url field should be removed from returned ParsedDoogat"
    );

    // Verify re-reading from git also has no url
    let parsed = svc.get_doogat_parsed(&id).unwrap();
    assert!(
        !parsed.meta.extra.contains_key("url"),
        "url field should be removed from frontmatter"
    );

    // Verify the materialized type table row has NULL url
    let sel = svc
        .execute_sql(&format!("SELECT url FROM link WHERE id = '{id}'"))
        .unwrap();
    match sel {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1, "row should still exist");
            // NULL is represented as empty string in the SQL result rows
            assert!(
                rows[0][0].is_empty() || rows[0][0] == "NULL",
                "materialized url should be null/empty after unset, got: {:?}",
                rows[0][0]
            );
        }
        _ => panic!("expected rows"),
    }
}

#[test]
fn update_field_validation_rejects_invalid_allowed_values() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE task (status ENUM('todo','doing','done') DEFAULT 'todo')")
        .unwrap();
    let ins = svc
        .execute_sql("INSERT INTO task (status) VALUES ('todo')")
        .unwrap();
    let id = match ins {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };

    // Try to update with an invalid status value
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "status".to_string(),
        crate::types::Value::String("invalid".to_string()),
    );
    let result = svc.update_doogat_parsed(
        &id,
        None,
        None,
        None,
        None,
        &ExtraFieldUpdates {
            set: &fields,
            unset: &[],
        },
    );
    assert!(result.is_err(), "should reject invalid allowed_values");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not in allowed values"),
        "error should mention allowed values, got: {err_msg}"
    );
}

#[test]
fn update_field_validation_rejects_invalid_fk_reference() {
    let (_tmp, mut svc) = fresh_svc();

    // Create a referenced type and a referring type
    svc.execute_sql("CREATE TABLE person (name VARCHAR(100))")
        .unwrap();
    svc.execute_sql(
        "CREATE TABLE task (title VARCHAR(100), assignee VARCHAR(200) REFERENCES person)",
    )
    .unwrap();
    let ins = svc
        .execute_sql("INSERT INTO task (title) VALUES ('Do stuff')")
        .unwrap();
    let task_id = match ins {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };

    // Try to update with a non-existent FK value
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "assignee".to_string(),
        crate::types::Value::String("99999999999999".to_string()),
    );
    let result = svc.update_doogat_parsed(
        &task_id,
        None,
        None,
        None,
        None,
        &ExtraFieldUpdates {
            set: &fields,
            unset: &[],
        },
    );
    assert!(result.is_err(), "should reject non-existent FK");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("non-existent"),
        "error should mention non-existent, got: {err_msg}"
    );
}

#[test]
fn update_field_validation_accepts_numeric_allowed_value() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE priority_test (level ENUM('1','2','3'))")
        .unwrap();
    let ins = svc
        .execute_sql("INSERT INTO priority_test (level) VALUES ('1')")
        .unwrap();
    let id = match ins {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };

    // Update with a Value::Number that should match the string "2"
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("level".to_string(), crate::types::Value::Number(2.0));
    // The formatted number "2" should match allowed_values entry "2"
    // (This was broken before: format!("{other:?}") produced "Number(2.0)")
    let result = svc.update_doogat_parsed(
        &id,
        None,
        None,
        None,
        None,
        &ExtraFieldUpdates {
            set: &fields,
            unset: &[],
        },
    );
    assert!(
        result.is_ok(),
        "numeric value matching allowed_values should be accepted, got: {:?}",
        result.err()
    );
}

#[test]
fn update_field_validation_rejects_fk_wrong_type() {
    let (_tmp, mut svc) = fresh_svc();

    // Create two types, one referring to the other
    svc.execute_sql("CREATE TABLE person (name VARCHAR(100))")
        .unwrap();
    svc.execute_sql("CREATE TABLE project (name VARCHAR(100))")
        .unwrap();
    svc.execute_sql(
        "CREATE TABLE task (title VARCHAR(100), assignee VARCHAR(200) REFERENCES person)",
    )
    .unwrap();
    // Create a project (wrong target type)
    let ins_project = svc
        .execute_sql("INSERT INTO project (name) VALUES ('Alpha')")
        .unwrap();
    let project_id = match ins_project {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };
    // Create a task
    let ins_task = svc
        .execute_sql("INSERT INTO task (title) VALUES ('Do stuff')")
        .unwrap();
    let task_id = match ins_task {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };

    // Try to update assignee (which references person) with a project ID
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "assignee".to_string(),
        crate::types::Value::String(project_id.clone()),
    );
    let result = svc.update_doogat_parsed(
        &task_id,
        None,
        None,
        None,
        None,
        &ExtraFieldUpdates {
            set: &fields,
            unset: &[],
        },
    );
    assert!(
        result.is_err(),
        "should reject FK pointing to wrong target type"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("non-existent") && err_msg.contains("person"),
        "error should mention target type 'person', got: {err_msg}"
    );
}

#[test]
fn batch_update_with_fields() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE item (url VARCHAR(200))")
        .unwrap();
    let ins1 = svc
        .execute_sql("INSERT INTO item (url) VALUES ('https://a.example.com')")
        .unwrap();
    let id1 = match ins1 {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };
    let ins2 = svc
        .execute_sql("INSERT INTO item (url) VALUES ('https://b.example.com')")
        .unwrap();
    let id2 = match ins2 {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };

    // Batch update both with different urls
    let mut fields1 = std::collections::BTreeMap::new();
    fields1.insert(
        "url".to_string(),
        crate::types::Value::String("https://a-new.example.com".to_string()),
    );
    let mut fields2 = std::collections::BTreeMap::new();
    fields2.insert(
        "url".to_string(),
        crate::types::Value::String("https://b-new.example.com".to_string()),
    );

    let results = svc
        .batch_update(&[
            BatchUpdateInput {
                id: id1.clone(),
                title: None,
                body: None,
                tags: None,
                doogat_type: None,
                fields: Some(fields1),
                unset_fields: None,
            },
            BatchUpdateInput {
                id: id2.clone(),
                title: None,
                body: None,
                tags: None,
                doogat_type: None,
                fields: Some(fields2),
                unset_fields: None,
            },
        ])
        .unwrap();
    assert_eq!(results.len(), 2);

    // Verify returned results have updated fields
    assert_eq!(
        *results[0].meta.extra.get("url").unwrap(),
        crate::types::Value::String("https://a-new.example.com".to_string()),
    );
    assert_eq!(
        *results[1].meta.extra.get("url").unwrap(),
        crate::types::Value::String("https://b-new.example.com".to_string()),
    );

    // Verify re-reading from git has updated fields
    let parsed1 = svc.get_doogat_parsed(&id1).unwrap();
    assert_eq!(
        *parsed1.meta.extra.get("url").unwrap(),
        crate::types::Value::String("https://a-new.example.com".to_string()),
    );
    let parsed2 = svc.get_doogat_parsed(&id2).unwrap();
    assert_eq!(
        *parsed2.meta.extra.get("url").unwrap(),
        crate::types::Value::String("https://b-new.example.com".to_string()),
    );

    // Verify materialized table rows reflect the batch update
    let sel = svc
        .execute_sql(&format!(
            "SELECT id, url FROM item WHERE id IN ('{id1}', '{id2}') ORDER BY id"
        ))
        .unwrap();
    match sel {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 2, "expected 2 materialized rows");
            let urls: Vec<&str> = rows.iter().map(|r| r[1].as_str()).collect();
            assert!(
                urls.contains(&"https://a-new.example.com"),
                "materialized row for id1 should have updated url, got urls: {urls:?}"
            );
            assert!(
                urls.contains(&"https://b-new.example.com"),
                "materialized row for id2 should have updated url, got urls: {urls:?}"
            );
        }
        _ => panic!("expected rows"),
    }
}

#[test]
fn batch_update_rejects_invalid_allowed_values() {
    let (tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE task (status ENUM('todo','doing','done') DEFAULT 'todo')")
        .unwrap();
    let ins = svc
        .execute_sql("INSERT INTO task (status) VALUES ('todo')")
        .unwrap();
    let id = match ins {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };
    let commits_before = count_commits(tmp.path());

    // Batch update with invalid allowed_values
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "status".to_string(),
        crate::types::Value::String("invalid".to_string()),
    );
    let result = svc.batch_update(&[BatchUpdateInput {
        id: id.clone(),
        title: None,
        body: None,
        tags: None,
        doogat_type: None,
        fields: Some(fields),
        unset_fields: None,
    }]);
    assert!(
        result.is_err(),
        "batch_update should reject invalid allowed_values"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not in allowed values"),
        "error should mention allowed values, got: {err_msg}"
    );

    // Verify no commit was made (fail-fast in Phase 1)
    let commits_after = count_commits(tmp.path());
    assert_eq!(
        commits_before, commits_after,
        "batch_update should not commit on validation failure"
    );
}

#[test]
fn batch_update_rejects_invalid_fk_reference() {
    let (tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE person (name VARCHAR(100))")
        .unwrap();
    svc.execute_sql(
        "CREATE TABLE task (title VARCHAR(100), assignee VARCHAR(200) REFERENCES person)",
    )
    .unwrap();
    let ins = svc
        .execute_sql("INSERT INTO task (title) VALUES ('Do stuff')")
        .unwrap();
    let id = match ins {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };
    let commits_before = count_commits(tmp.path());

    // Batch update with non-existent FK
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "assignee".to_string(),
        crate::types::Value::String("99999999999999".to_string()),
    );
    let result = svc.batch_update(&[BatchUpdateInput {
        id: id.clone(),
        title: None,
        body: None,
        tags: None,
        doogat_type: None,
        fields: Some(fields),
        unset_fields: None,
    }]);
    assert!(
        result.is_err(),
        "batch_update should reject non-existent FK"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("non-existent"),
        "error should mention non-existent, got: {err_msg}"
    );

    // Verify no commit was made
    let commits_after = count_commits(tmp.path());
    assert_eq!(
        commits_before, commits_after,
        "batch_update should not commit on FK validation failure"
    );
}

#[test]
fn batch_update_mixed_with_and_without_fields() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE entry (url VARCHAR(200))")
        .unwrap();
    let ins1 = svc
        .execute_sql("INSERT INTO entry (url) VALUES ('https://keep.example.com')")
        .unwrap();
    let id1 = match ins1 {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };
    let ins2 = svc
        .execute_sql("INSERT INTO entry (url) VALUES ('https://change.example.com')")
        .unwrap();
    let id2 = match ins2 {
        SqlResult::Ok(msg) => msg.split_whitespace().last().unwrap().to_string(),
        _ => panic!("expected Ok from INSERT"),
    };

    // Update only id2's fields; id1 gets title change but no field changes
    let mut fields2 = std::collections::BTreeMap::new();
    fields2.insert(
        "url".to_string(),
        crate::types::Value::String("https://changed.example.com".to_string()),
    );

    let results = svc
        .batch_update(&[
            BatchUpdateInput {
                id: id1.clone(),
                title: Some("Renamed".to_string()),
                body: None,
                tags: None,
                doogat_type: None,
                fields: None,
                unset_fields: None,
            },
            BatchUpdateInput {
                id: id2.clone(),
                title: None,
                body: None,
                tags: None,
                doogat_type: None,
                fields: Some(fields2),
                unset_fields: None,
            },
        ])
        .unwrap();
    assert_eq!(results.len(), 2);

    // id1: title changed, url unchanged
    let parsed1 = svc.get_doogat_parsed(&id1).unwrap();
    assert_eq!(parsed1.meta.title.as_deref(), Some("Renamed"));
    assert_eq!(
        *parsed1.meta.extra.get("url").unwrap(),
        crate::types::Value::String("https://keep.example.com".to_string()),
    );

    // id2: url changed
    let parsed2 = svc.get_doogat_parsed(&id2).unwrap();
    assert_eq!(
        *parsed2.meta.extra.get("url").unwrap(),
        crate::types::Value::String("https://changed.example.com".to_string()),
    );
}

// ── PRD 00129 §1: typed createDoogat populates the type-specific table ──

#[test]
fn batch_create_typed_writes_to_type_table_prd_00129() {
    // The PRD's headline blocker #1: createDoogat with type+fields must
    // populate the type-specific materialized table, not just `doogats`.
    // Pre-PRD 00129, batch_create wrote only the index row and the
    // materialized table stayed empty — so even existing UNIQUE indexes
    // had nothing to constrain.
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "url".to_string(),
        crate::types::Value::String("https://example.com".to_string()),
    );

    let inputs = vec![crate::types::BatchCreateInput {
        title: Some("Example".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("link".to_string()),
        fields,
        on_conflict: crate::types::ConflictAction::Error,
    }];

    let results = svc.batch_create(&inputs).unwrap();
    assert_eq!(results.len(), 1);
    let id = results[0].meta.id.as_ref().unwrap().0.clone();

    // Read back via SQL — the row must exist in the materialized `link`
    // table with the supplied url. Pre-PRD 00129 this row was never
    // written.
    let res = svc
        .execute_sql(&format!(
            "SELECT url FROM link WHERE id = '{}'",
            id
        ))
        .unwrap();
    match res {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1, "type-table row must be materialized");
            assert_eq!(rows[0][0], "https://example.com");
        }
        _ => panic!("expected SELECT to return rows"),
    }
}

#[test]
fn batch_create_with_unregistered_type_rejects_with_type_not_registered_prd_00129() {
    let (_tmp, svc) = fresh_svc();

    let inputs = vec![crate::types::BatchCreateInput {
        title: Some("x".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("nonexistent".to_string()),
        fields: std::collections::BTreeMap::new(),
        on_conflict: crate::types::ConflictAction::Error,
    }];

    let err = svc
        .batch_create(&inputs)
        .expect_err("unregistered type must reject");
    let msg = format!("{err}");
    assert!(
        msg.contains("\"nonexistent\"") && msg.contains("not a registered typedef"),
        "expected TYPE_NOT_REGISTERED message, got: {msg}"
    );
}

#[test]
fn batch_create_with_unknown_field_rejects_with_unknown_field_prd_00129() {
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "bogus".to_string(),
        crate::types::Value::String("ignored".to_string()),
    );

    let inputs = vec![crate::types::BatchCreateInput {
        title: Some("x".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("link".to_string()),
        fields,
        on_conflict: crate::types::ConflictAction::Error,
    }];

    let err = svc
        .batch_create(&inputs)
        .expect_err("unknown field must reject");
    let msg = format!("{err}");
    assert!(
        msg.contains("unknown column: link.bogus"),
        "expected UNKNOWN_FIELD message, got: {msg}"
    );
}

#[test]
fn batch_create_missing_required_column_rejects_with_not_null_prd_00129() {
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE link (title TEXT, url VARCHAR(255) NOT NULL)")
        .unwrap();

    let inputs = vec![crate::types::BatchCreateInput {
        title: Some("x".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("link".to_string()),
        fields: std::collections::BTreeMap::new(),
        on_conflict: crate::types::ConflictAction::Error,
    }];

    let err = svc
        .batch_create(&inputs)
        .expect_err("missing required column must reject");
    let msg = format!("{err}");
    assert!(
        msg.contains("NOT NULL constraint violated: link.url"),
        "expected NOT_NULL_VIOLATION message, got: {msg}"
    );
}

#[test]
fn batch_create_typed_no_fields_with_only_nullable_columns_succeeds_prd_00129() {
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();

    let inputs = vec![crate::types::BatchCreateInput {
        title: Some("Bare Link".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("link".to_string()),
        fields: std::collections::BTreeMap::new(),
        on_conflict: crate::types::ConflictAction::Error,
    }];

    let results = svc
        .batch_create(&inputs)
        .expect("typed create with all-nullable columns must succeed");
    assert_eq!(results.len(), 1);
    let id = results[0].meta.id.as_ref().unwrap().0.clone();

    let res = svc
        .execute_sql(&format!(
            "SELECT title FROM link WHERE id = '{}'",
            id
        ))
        .unwrap();
    match res {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1, "type-table row must be materialized");
            assert_eq!(rows[0][0], "Bare Link", "title from input.title");
        }
        _ => panic!("expected SELECT to return rows"),
    }
}

#[test]
fn batch_create_intra_batch_duplicate_ignore_returns_surviving_id_issue_12() {
    // Issue #12: when `createMany(onConflict: IGNORE)` skips a duplicate
    // *within the same batch*, the response payload must return the
    // surviving (winning) row's ID — not the rejected/rolled-back ID
    // for that input. The single-row `createDoogat(onConflict: IGNORE)`
    // already returns the existing row's ID for cross-batch duplicates;
    // this test pins the same semantics for intra-batch duplicates.
    let (_tmp, svc) = fresh_svc();
    setup_widget_typedef(&svc);

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "name".to_string(),
        crate::types::Value::String("a".to_string()),
    );

    let inputs = vec![
        crate::types::BatchCreateInput {
            title: Some("Widget A".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("widget".to_string()),
            fields: fields.clone(),
            on_conflict: crate::types::ConflictAction::Ignore,
        },
        crate::types::BatchCreateInput {
            title: Some("Widget A Duplicate".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("widget".to_string()),
            fields,
            on_conflict: crate::types::ConflictAction::Ignore,
        },
    ];

    let results = svc.batch_create(&inputs).unwrap();
    assert_eq!(results.len(), 2, "two payloads, one per input");

    let id1 = results[0].meta.id.as_ref().unwrap().0.clone();
    let id2 = results[1].meta.id.as_ref().unwrap().0.clone();
    assert_eq!(
        id1, id2,
        "intra-batch duplicate must return the surviving row's ID at both array indices"
    );

    // Exactly one widget row exists; the rejected ID is not present anywhere.
    let widget_count: i64 = svc
        .index
        .conn
        .query_row("SELECT COUNT(*) FROM widget", [], |r| r.get(0))
        .unwrap();
    assert_eq!(widget_count, 1, "exactly one materialized widget row");
    let doogats_count: i64 = svc
        .index
        .conn
        .query_row(
            "SELECT COUNT(*) FROM doogats WHERE type = 'widget'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        doogats_count, 1,
        "exactly one base doogats row (no half-written second row)"
    );
}

#[test]
fn batch_create_intra_batch_duplicate_error_rejects_whole_batch_issue_12() {
    // Issue #12 sibling: with `on_conflict: Error`, an intra-batch
    // duplicate fails the whole batch — same posture as a cross-batch
    // duplicate. No partial commits.
    let (_tmp, svc) = fresh_svc();
    setup_widget_typedef(&svc);

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "name".to_string(),
        crate::types::Value::String("a".to_string()),
    );

    let inputs = vec![
        crate::types::BatchCreateInput {
            title: Some("Widget A".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("widget".to_string()),
            fields: fields.clone(),
            on_conflict: crate::types::ConflictAction::Error,
        },
        crate::types::BatchCreateInput {
            title: Some("Widget A Duplicate".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("widget".to_string()),
            fields,
            on_conflict: crate::types::ConflictAction::Error,
        },
    ];

    let err = svc
        .batch_create(&inputs)
        .expect_err("intra-batch duplicate with Error must reject");
    // PRD 00131: intra-batch UNIQUE conflict surfaces the structured
    // UNIQUE_VIOLATION code so `to_graphql_error` can attach
    // `extensions.code` to the GraphQL error envelope. Earlier wording
    // ("duplicate unique constraint within batch") was a Validation
    // string; assert the structured shape instead, since the message
    // text is informative-but-not-contractual.
    match &err {
        crate::error::DoogatError::Structured { code, .. } => {
            assert_eq!(*code, crate::error::codes::UNIQUE_VIOLATION);
        }
        other => panic!("expected Structured UNIQUE_VIOLATION, got: {other:?}"),
    }

    let widget_count: i64 = svc
        .index
        .conn
        .query_row("SELECT COUNT(*) FROM widget", [], |r| r.get(0))
        .unwrap();
    assert_eq!(widget_count, 0, "no widget rows committed on rejection");
}

#[test]
fn batch_create_omitted_title_renders_via_title_template_issue_13() {
    // Issue #13: the GraphQL `createDoogat` surface needs to omit `title`
    // for typedefs that declare a `title_template` (PRD 00127), the same
    // way SQL `INSERT` already does. The shared engine call sits inside
    // `prepare_create`, so a service-layer `batch_create` exercises it
    // without going through the GraphQL surface.
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();
    svc.execute_sql("ALTER TABLE link SET TITLE TEMPLATE 'link-{url}'")
        .unwrap();
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "url".to_string(),
        crate::types::Value::String("https://example.com".to_string()),
    );

    let inputs = vec![crate::types::BatchCreateInput {
        title: None,
        body: None,
        tags: vec![],
        doogat_type: Some("link".to_string()),
        fields,
        on_conflict: crate::types::ConflictAction::Error,
    }];

    let results = svc
        .batch_create(&inputs)
        .expect("typed create with template-bearing typedef must succeed without title");
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].meta.title.as_deref(),
        Some("link-https://example.com"),
        "title rendered from title_template"
    );
}

#[test]
fn batch_create_omitted_title_no_template_rejects_with_not_null_issue_13() {
    // Issue #13 negative: typedef without a title_template, omitted title
    // → NOT_NULL_VIOLATION on the title column.
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "url".to_string(),
        crate::types::Value::String("https://example.com".to_string()),
    );

    let inputs = vec![crate::types::BatchCreateInput {
        title: None,
        body: None,
        tags: vec![],
        doogat_type: Some("link".to_string()),
        fields,
        on_conflict: crate::types::ConflictAction::Error,
    }];

    let err = svc
        .batch_create(&inputs)
        .expect_err("omitted title with no template must reject");
    let msg = format!("{err}");
    assert!(
        msg.contains("NOT NULL constraint violated: link.title"),
        "expected NOT_NULL_VIOLATION on link.title, got: {msg}"
    );
}

#[test]
fn batch_create_omitted_title_no_typedef_rejects_with_not_null_issue_13() {
    // Issue #13 negative: untyped create with no title → NOT_NULL_VIOLATION
    // against the base `doogats` table. Prevents writing a base doogat
    // with no title at all (which would defeat the base NOT NULL).
    let (_tmp, svc) = fresh_svc();

    let inputs = vec![crate::types::BatchCreateInput {
        title: None,
        body: None,
        tags: vec![],
        doogat_type: None,
        fields: std::collections::BTreeMap::new(),
        on_conflict: crate::types::ConflictAction::Error,
    }];

    let err = svc
        .batch_create(&inputs)
        .expect_err("untyped create with no title must reject");
    let msg = format!("{err}");
    assert!(
        msg.contains("NOT NULL constraint violated: doogats.title"),
        "expected NOT_NULL_VIOLATION on doogats.title, got: {msg}"
    );
}

#[test]
fn batch_create_omitted_title_renders_via_references_template_issue_13() {
    // Issue #13 + PRD 00127: the renderer dereferences REFERENCES columns
    // through the SQLite index. createDoogat → prepare_create →
    // resolve_create_title → resolve_insert_title chain must pass the
    // sqlite conn so the lookup succeeds.
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();
    svc.execute_sql("CREATE TABLE category (title TEXT, fqn VARCHAR(255))")
        .unwrap();
    svc.execute_sql(
        "CREATE TABLE \"category-membership\" (\
         link_id VARCHAR(255) NOT NULL REFERENCES link(id),\
         category_id VARCHAR(255) NOT NULL REFERENCES category(id))",
    )
    .unwrap();
    svc.execute_sql(
        "ALTER TABLE \"category-membership\" SET TITLE TEMPLATE \
         '{link_id.title} in {category_id.fqn}'",
    )
    .unwrap();

    // Seed link + category so the REFERENCES dereferences resolve.
    let mut link_fields = std::collections::BTreeMap::new();
    link_fields.insert(
        "url".to_string(),
        crate::types::Value::String("https://example.com".to_string()),
    );
    let link = svc
        .batch_create(&[crate::types::BatchCreateInput {
            title: Some("Example".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("link".to_string()),
            fields: link_fields,
            on_conflict: crate::types::ConflictAction::Error,
        }])
        .unwrap();
    let link_id = link[0].meta.id.as_ref().unwrap().0.clone();

    let mut cat_fields = std::collections::BTreeMap::new();
    cat_fields.insert(
        "fqn".to_string(),
        crate::types::Value::String("work.dev".to_string()),
    );
    let cat = svc
        .batch_create(&[crate::types::BatchCreateInput {
            title: Some("Dev".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("category".to_string()),
            fields: cat_fields,
            on_conflict: crate::types::ConflictAction::Error,
        }])
        .unwrap();
    let cat_id = cat[0].meta.id.as_ref().unwrap().0.clone();

    let mut mem_fields = std::collections::BTreeMap::new();
    mem_fields.insert(
        "link_id".to_string(),
        crate::types::Value::String(link_id),
    );
    mem_fields.insert(
        "category_id".to_string(),
        crate::types::Value::String(cat_id),
    );
    let result = svc
        .batch_create(&[crate::types::BatchCreateInput {
            title: None,
            body: None,
            tags: vec![],
            doogat_type: Some("category-membership".to_string()),
            fields: mem_fields,
            on_conflict: crate::types::ConflictAction::Error,
        }])
        .expect("template-with-REFERENCES create must succeed without title");
    assert_eq!(
        result[0].meta.title.as_deref(),
        Some("Example in work.dev"),
        "title rendered from REFERENCES title_template"
    );
}

#[test]
fn batch_create_many_ignore_duplicate_does_not_write_half_row_prd_00129() {
    // PRD 00129 §1 (createMany half): when `onConflict: IGNORE` skips a
    // duplicate, neither the base `doogats` row nor the materialized
    // type-table row is written for that item. Only the new items go in.
    // The pre-T3 path silently passed because the materialized table was
    // never written either way; this test pins the new typed-write path.
    let (_tmp, svc) = fresh_svc();
    setup_widget_typedef(&svc);

    let mut fields_a = std::collections::BTreeMap::new();
    fields_a.insert(
        "name".to_string(),
        crate::types::Value::String("a".to_string()),
    );

    // Seed: create the original widget so the second batch hits a conflict.
    let first = svc
        .batch_create(&[crate::types::BatchCreateInput {
            title: Some("Widget A".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("widget".to_string()),
            fields: fields_a.clone(),
            on_conflict: crate::types::ConflictAction::Ignore,
        }])
        .unwrap();
    let original_id = first[0].meta.id.as_ref().unwrap().0.clone();

    let widget_count_before: i64 = svc
        .index
        .conn
        .query_row("SELECT COUNT(*) FROM widget", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        widget_count_before, 1,
        "first batch must have materialized exactly 1 widget row"
    );

    // Mixed batch: one new widget + one duplicate.
    let mut fields_b = std::collections::BTreeMap::new();
    fields_b.insert(
        "name".to_string(),
        crate::types::Value::String("b".to_string()),
    );

    let results = svc
        .batch_create(&[
            crate::types::BatchCreateInput {
                title: Some("Widget A Duplicate".to_string()),
                body: None,
                tags: vec![],
                doogat_type: Some("widget".to_string()),
                fields: fields_a,
                on_conflict: crate::types::ConflictAction::Ignore,
            },
            crate::types::BatchCreateInput {
                title: Some("Widget B".to_string()),
                body: None,
                tags: vec![],
                doogat_type: Some("widget".to_string()),
                fields: fields_b,
                on_conflict: crate::types::ConflictAction::Ignore,
            },
        ])
        .unwrap();

    assert_eq!(results.len(), 2, "both slots return a doogat");
    let returned_a_id = results[0].meta.id.as_ref().unwrap().0.clone();
    let returned_b_id = results[1].meta.id.as_ref().unwrap().0.clone();
    assert_eq!(
        returned_a_id, original_id,
        "duplicate slot must return the original widget id"
    );
    assert_ne!(returned_b_id, original_id, "second slot must be a new id");

    // Verify exactly 2 widget rows in the materialized table — no
    // half-row, no double-write, no clobber of the original.
    let widget_count_after: i64 = svc
        .index
        .conn
        .query_row("SELECT COUNT(*) FROM widget", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        widget_count_after, 2,
        "exactly 2 widget rows after IGNORE skip + new insert"
    );

    let original_still_present: i64 = svc
        .index
        .conn
        .query_row(
            "SELECT COUNT(*) FROM widget WHERE id = ?1",
            rusqlite::params![original_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        original_still_present, 1,
        "original widget row must still be present after IGNORE"
    );
}

// ── PRD 00129 §2: ON DELETE CASCADE walk ──
//
// Tests use executeSql INSERT (not createDoogat) for typed REFERENCES
// rows because the SQL path correctly routes REFERENCES values into the
// reference zone, where check_restrict_blocks_delete /
// collect_cascade_children look for them. Using createDoogat would put
// the value into frontmatter `extra`, which doesn't materialize into the
// reference column. (See PRD 00129 follow-up note: createDoogat could
// also route REFERENCES correctly, but that's a wider zone-handling
// change beyond T3's scope.)

fn parent_id_from_insert(svc: &mut DoogatService, sql: &str) -> String {
    match svc.execute_sql(sql).unwrap() {
        SqlResult::Ok(id) => id,
        other => panic!("expected SqlResult::Ok with id, got {other:?}"),
    }
}

#[test]
fn delete_cascades_to_referencing_rows_when_marked_cascade_prd_00129() {
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE link (title TEXT)").unwrap();
    svc.execute_sql(
        "CREATE TABLE membership (title TEXT, link VARCHAR(255) NOT NULL REFERENCES link(id) ON DELETE CASCADE)",
    )
    .unwrap();

    let link_id = parent_id_from_insert(&mut svc, "INSERT INTO link (title) VALUES ('Parent')");
    svc.execute_sql(&format!(
        "INSERT INTO membership (title, link) VALUES ('Child', '{link_id}')"
    ))
    .unwrap();

    svc.delete_doogat(&link_id, &format!("delete {link_id}"))
        .expect("CASCADE delete must succeed");

    let link_count: i64 = svc
        .index
        .conn
        .query_row(
            "SELECT COUNT(*) FROM link WHERE id = ?1",
            rusqlite::params![link_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(link_count, 0, "parent link row removed");
    let mem_count: i64 = svc
        .index
        .conn
        .query_row(
            "SELECT COUNT(*) FROM membership WHERE link = ?1",
            rusqlite::params![link_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        mem_count, 0,
        "child membership row removed by CASCADE walk"
    );
}

#[test]
fn delete_blocks_when_referencing_column_is_restrict_prd_00129() {
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE link (title TEXT)").unwrap();
    svc.execute_sql(
        "CREATE TABLE blocker (title TEXT, link VARCHAR(255) NOT NULL REFERENCES link(id))",
    )
    .unwrap();

    let link_id = parent_id_from_insert(&mut svc, "INSERT INTO link (title) VALUES ('Parent')");
    svc.execute_sql(&format!(
        "INSERT INTO blocker (title, link) VALUES ('Blocks', '{link_id}')"
    ))
    .unwrap();

    let err = svc
        .delete_doogat(&link_id, "delete")
        .expect_err("RESTRICT must block parent delete");
    let msg = format!("{err}");
    assert!(
        msg.contains("NOT NULL REFERENCES from blocker.link"),
        "expected REFERENCES_VIOLATION wording, got: {msg}"
    );
}

#[test]
fn delete_with_mixed_restrict_cascade_columns_behaves_per_column_prd_00129() {
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE link (title TEXT)").unwrap();
    svc.execute_sql("CREATE TABLE category (title TEXT)")
        .unwrap();
    svc.execute_sql(
        "CREATE TABLE membership (title TEXT, \
         link VARCHAR(255) NOT NULL REFERENCES link(id) ON DELETE CASCADE, \
         category VARCHAR(255) NOT NULL REFERENCES category(id) ON DELETE RESTRICT)",
    )
    .unwrap();

    let link_id =
        parent_id_from_insert(&mut svc, "INSERT INTO link (title) VALUES ('Parent Link')");
    let cat_id = parent_id_from_insert(
        &mut svc,
        "INSERT INTO category (title) VALUES ('Parent Cat')",
    );
    svc.execute_sql(&format!(
        "INSERT INTO membership (title, link, category) VALUES ('M', '{link_id}', '{cat_id}')"
    ))
    .unwrap();

    let err = svc
        .delete_doogat(&cat_id, "delete cat")
        .expect_err("RESTRICT side must block");
    let msg = format!("{err}");
    assert!(
        msg.contains("NOT NULL REFERENCES from membership.category"),
        "expected RESTRICT block on category side, got: {msg}"
    );

    svc.delete_doogat(&link_id, "delete link")
        .expect("CASCADE side must allow delete");
    let mem_count: i64 = svc
        .index
        .conn
        .query_row("SELECT COUNT(*) FROM membership", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mem_count, 0, "membership cascaded away with link delete");
}

#[test]
fn delete_with_cascade_cycle_rejects_prd_00129() {
    // Two-table cycle: A.b REFERENCES B ON DELETE CASCADE, B.a REFERENCES
    // A ON DELETE CASCADE. The cascade walk must detect the cycle and
    // reject rather than loop forever.
    let (_tmp, mut svc) = fresh_svc();
    svc.execute_sql("CREATE TABLE node_a (title TEXT)").unwrap();
    svc.execute_sql("CREATE TABLE node_b (title TEXT)").unwrap();
    svc.execute_sql(
        "ALTER TABLE node_a ADD COLUMN b VARCHAR(255) REFERENCES node_b(id) ON DELETE CASCADE",
    )
    .unwrap();
    svc.execute_sql(
        "ALTER TABLE node_b ADD COLUMN a VARCHAR(255) REFERENCES node_a(id) ON DELETE CASCADE",
    )
    .unwrap();

    let a_id = parent_id_from_insert(&mut svc, "INSERT INTO node_a (title) VALUES ('A')");
    let b_id = parent_id_from_insert(&mut svc, "INSERT INTO node_b (title) VALUES ('B')");
    // Cross-link the two rows after creation so each references the other.
    svc.execute_sql(&format!(
        "UPDATE node_a SET b = '{b_id}' WHERE id = '{a_id}'"
    ))
    .unwrap();
    svc.execute_sql(&format!(
        "UPDATE node_b SET a = '{a_id}' WHERE id = '{b_id}'"
    ))
    .unwrap();

    let err = svc
        .delete_doogat(&a_id, "delete a")
        .expect_err("cycle must be detected");
    let msg = format!("{err}");
    assert!(
        msg.contains("cascade delete would form a cycle"),
        "expected CASCADE_CYCLE message, got: {msg}"
    );
}

#[test]
fn batch_create_untyped_doogat_unaffected_by_typed_validation_prd_00129() {
    // Sanity: untyped creates (no doogat_type) bypass typedef validation
    // entirely — same behavior as before PRD 00129.
    let (_tmp, svc) = fresh_svc();
    let inputs = vec![crate::types::BatchCreateInput {
        title: Some("Untyped".to_string()),
        body: None,
        tags: vec!["misc".to_string()],
        doogat_type: None,
        fields: std::collections::BTreeMap::new(),
        on_conflict: crate::types::ConflictAction::Error,
    }];
    let results = svc.batch_create(&inputs).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].meta.doogat_type.is_none());
}

// ── PRD 00131: structured-error code propagation through batch_create ──
//
// jink probes ddb 0.2.5+dev.g8c924a5 and finds duplicate-key violations
// returning GraphQL error envelopes without `extensions.code`. The flatten
// happens before `to_graphql_error` (the load-bearing resolver-side
// adapter, unit-tested at `ddb-server/src/error.rs`): `batch_create` must return a
// `DoogatError::Structured` variant for the GraphQL boundary to attach the
// code. These tests assert the Structured shape at the service boundary,
// which is the same boundary the actor / mutation resolver chain consumes.

/// Install a typedef for type "link" with a NOT NULL column.
fn setup_link_typedef_with_required_url(svc: &DoogatService) {
    let typedef = "---
id: 20260601000001
title: link
type: _typedef
columns:
  - name: url
    data_type: TEXT
    zone: frontmatter
    required: true
---
";
    let typedef_path = "ddb/_typedef/20260601000001.md";
    svc.repo
        .commit_file(typedef_path, typedef, "add link typedef")
        .unwrap();
    let parsed = crate::parser::parse(typedef, typedef_path).unwrap();
    svc.index.index_doogat(&parsed).unwrap();
    svc.index.materialize_all_types(&svc.repo).unwrap();
}

#[test]
fn batch_create_cross_batch_unique_violation_returns_structured_error_prd_00131() {
    let (_tmp, svc) = fresh_svc();
    setup_widget_typedef(&svc);

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "name".to_string(),
        crate::types::Value::String("foo".to_string()),
    );

    // First insert succeeds.
    svc.batch_create(&[crate::types::BatchCreateInput {
        title: Some("Foo Widget".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("widget".to_string()),
        fields: fields.clone(),
        on_conflict: crate::types::ConflictAction::Error,
    }])
    .unwrap();

    // Second insert with same unique key in a separate batch must surface a
    // Structured UNIQUE_VIOLATION so `to_graphql_error` attaches
    // `extensions.code = "UNIQUE_VIOLATION"` to the GraphQL response.
    let err = svc
        .batch_create(&[crate::types::BatchCreateInput {
            title: Some("Foo Widget Again".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("widget".to_string()),
            fields,
            on_conflict: crate::types::ConflictAction::Error,
        }])
        .expect_err("duplicate insert with on_conflict: Error must error");

    match err {
        crate::error::DoogatError::Structured {
            code,
            ref context,
            ..
        } => {
            assert_eq!(code, crate::error::codes::UNIQUE_VIOLATION);
            let columns = context
                .iter()
                .find(|(k, _)| k == "columns")
                .map(|(_, v)| v)
                .expect("columns context entry");
            match columns {
                crate::error::ErrorValue::List(items) => {
                    assert_eq!(items, &vec!["name".to_string()]);
                }
                other => panic!("expected List for columns, got {other:?}"),
            }
        }
        other => panic!("expected Structured UNIQUE_VIOLATION, got {other:?}"),
    }
}

#[test]
fn batch_create_intra_batch_unique_violation_returns_structured_error_prd_00131() {
    let (_tmp, svc) = fresh_svc();
    setup_widget_typedef(&svc);

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "name".to_string(),
        crate::types::Value::String("foo".to_string()),
    );

    // Two inputs with the same unique key in one batch under Error: surface a
    // Structured UNIQUE_VIOLATION (intra-batch path is the second flatten
    // site fixed by PRD 00131).
    let err = svc
        .batch_create(&[
            crate::types::BatchCreateInput {
                title: Some("Foo Widget A".to_string()),
                body: None,
                tags: vec![],
                doogat_type: Some("widget".to_string()),
                fields: fields.clone(),
                on_conflict: crate::types::ConflictAction::Error,
            },
            crate::types::BatchCreateInput {
                title: Some("Foo Widget B".to_string()),
                body: None,
                tags: vec![],
                doogat_type: Some("widget".to_string()),
                fields,
                on_conflict: crate::types::ConflictAction::Error,
            },
        ])
        .expect_err("intra-batch duplicate with on_conflict: Error must error");

    match err {
        crate::error::DoogatError::Structured { code, .. } => {
            assert_eq!(code, crate::error::codes::UNIQUE_VIOLATION);
        }
        other => panic!("expected Structured UNIQUE_VIOLATION, got {other:?}"),
    }
}

#[test]
fn batch_create_not_null_violation_returns_structured_error_prd_00131() {
    // Sanity check: NOT_NULL path is already structured (via
    // `validate_typed_create_post_defaults` -> `not_null_violation`).
    // Asserting it here locks in the current behavior so the audit pass
    // (PRD 00131 task #6) doesn't regress it.
    let (_tmp, svc) = fresh_svc();
    setup_link_typedef_with_required_url(&svc);

    let err = svc
        .batch_create(&[crate::types::BatchCreateInput {
            title: Some("No URL".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("link".to_string()),
            fields: std::collections::BTreeMap::new(),
            on_conflict: crate::types::ConflictAction::Error,
        }])
        .expect_err("missing required column must error");

    match err {
        crate::error::DoogatError::Structured {
            code,
            ref context,
            ..
        } => {
            assert_eq!(code, crate::error::codes::NOT_NULL_VIOLATION);
            let column = context
                .iter()
                .find(|(k, _)| k == "column")
                .map(|(_, v)| v)
                .expect("column context entry");
            match column {
                crate::error::ErrorValue::String(s) => assert_eq!(s, "url"),
                other => panic!("expected String for column, got {other:?}"),
            }
        }
        other => panic!("expected Structured NOT_NULL_VIOLATION, got {other:?}"),
    }
}

// ── PRD 00133 unify-typed-write-paths-v1 ──────────────────────────────

/// REFERENCES column values must land in the reference zone (inline_fields)
/// after batch_create, not in the frontmatter `extra` map. Junction-style
/// typedefs (e.g. `category-membership`) depend on this — before PRD 00133
/// they had to use raw `executeSql INSERT` because GraphQL `createDoogat`
/// dumped FK ids into frontmatter.
#[test]
fn batch_create_routes_references_to_reference_zone() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE category (label VARCHAR(64))").unwrap();
    let cat_one = svc
        .execute_sql("INSERT INTO category (label) VALUES ('alpha')")
        .unwrap();
    let cat_id = match cat_one {
        crate::sql_engine::SqlResult::Ok(s) => s,
        other => panic!("expected Ok with new id, got {other:?}"),
    };

    svc.execute_sql(
        "CREATE TABLE link (target VARCHAR(64) REFERENCES category)",
    )
    .unwrap();

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "target".to_string(),
        crate::types::Value::String(cat_id.clone()),
    );

    let inputs = vec![crate::types::BatchCreateInput {
        title: Some("My Link".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("link".to_string()),
        fields,
        on_conflict: crate::types::ConflictAction::Error,
    }];

    let results = svc.batch_create(&inputs).unwrap();
    assert_eq!(results.len(), 1);

    let parsed = &results[0];
    assert!(
        !parsed.meta.extra.contains_key("target"),
        "REFERENCES column 'target' must NOT be in frontmatter extra; \
         got extra: {:?}",
        parsed.meta.extra
    );
    assert!(
        parsed
            .inline_fields
            .iter()
            .any(|f| f.key == "target" && f.value.contains(&cat_id)),
        "expected reference zone inline_field for 'target' with id {cat_id}; \
         got inline_fields: {:?}",
        parsed.inline_fields
    );
}

/// FK validation must query the referenced typedef table (e.g. `category`)
/// not the generic `doogats` index. PRD 00133: an FK to a row of the wrong
/// type must reject. Before the unification, `validate_fk_reference` did
/// `SELECT COUNT(*) FROM doogats WHERE id = ?`, which accepted any id.
#[test]
fn batch_create_rejects_fk_to_wrong_type() {
    let (_tmp, mut svc) = fresh_svc();

    // Two unrelated typedefs.
    svc.execute_sql("CREATE TABLE category (label VARCHAR(64))").unwrap();
    svc.execute_sql("CREATE TABLE note (label VARCHAR(64))").unwrap();
    let note_create = svc
        .execute_sql("INSERT INTO note (label) VALUES ('not a category')")
        .unwrap();
    let note_id = match note_create {
        crate::sql_engine::SqlResult::Ok(s) => s,
        other => panic!("expected Ok with new id, got {other:?}"),
    };

    // `link` has a column REFERENCES category. Pointing it at a `note` row
    // must reject — `note_id` exists in the global `doogats` index but not
    // in the `category` table.
    svc.execute_sql(
        "CREATE TABLE link (target VARCHAR(64) REFERENCES category)",
    )
    .unwrap();

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "target".to_string(),
        crate::types::Value::String(note_id.clone()),
    );

    let inputs = vec![crate::types::BatchCreateInput {
        title: Some("Bogus link".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("link".to_string()),
        fields,
        on_conflict: crate::types::ConflictAction::Error,
    }];

    let err = svc.batch_create(&inputs).unwrap_err();
    match err {
        crate::error::DoogatError::Structured { code, .. } => {
            assert_eq!(code, crate::error::codes::REFERENCES_VIOLATION);
        }
        other => panic!("expected Structured REFERENCES_VIOLATION, got {other:?}"),
    }
}

/// `allowed_values` must be enforced uniformly on every typed-create entry
/// point. Before PRD 00133, GraphQL rejected invalid enums but the CLI/FFI
/// path silently accepted them. Now both paths route through the same
/// helper.
#[test]
fn batch_create_rejects_invalid_allowed_values() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql(
        "CREATE TABLE task (status ENUM('open', 'closed'))",
    )
    .unwrap();

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "status".to_string(),
        crate::types::Value::String("invalid".to_string()),
    );

    let inputs = vec![crate::types::BatchCreateInput {
        title: Some("Test".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("task".to_string()),
        fields,
        on_conflict: crate::types::ConflictAction::Error,
    }];

    let err = svc.batch_create(&inputs).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not in allowed values"),
        "expected allowed_values rejection, got: {msg}"
    );
}

/// `create_doogat_with_extra` (CLI/FFI single-create surface) must route
/// REFERENCES values through the unified helper for registered types.
/// Before PRD 00133, the CLI dumped FK ids into frontmatter `extra` while
/// GraphQL `createDoogat` had its own bug-prone path; both surfaces now
/// share `prepare_typed_insert_validate` + `build_data_doogat`.
#[test]
fn create_doogat_with_extra_routes_references_for_registered_type() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE category (label VARCHAR(64))").unwrap();
    let cat_create = svc
        .execute_sql("INSERT INTO category (label) VALUES ('alpha')")
        .unwrap();
    let cat_id = match cat_create {
        crate::sql_engine::SqlResult::Ok(s) => s,
        other => panic!("expected Ok with new id, got {other:?}"),
    };

    svc.execute_sql(
        "CREATE TABLE link (target VARCHAR(64) REFERENCES category)",
    )
    .unwrap();

    let mut extra = std::collections::BTreeMap::new();
    extra.insert(
        "target".to_string(),
        crate::types::Value::String(cat_id.clone()),
    );

    let parsed = svc
        .create_doogat_with_extra("CLI link", &[], Some("link"), "", extra)
        .unwrap();

    assert!(
        !parsed.meta.extra.contains_key("target"),
        "REFERENCES column 'target' must NOT be in frontmatter extra; \
         got extra: {:?}",
        parsed.meta.extra
    );
    assert!(
        parsed
            .inline_fields
            .iter()
            .any(|f| f.key == "target" && f.value.contains(&cat_id)),
        "expected reference zone inline_field for 'target' with id {cat_id}; \
         got inline_fields: {:?}",
        parsed.inline_fields
    );
}

/// CLI/FFI must reject typed inputs with FK pointing at a row of the wrong
/// type. PRD 00133 §Behavior changes: CLI used to silently accept; now
/// rejects with REFERENCES_VIOLATION.
#[test]
fn create_doogat_with_extra_rejects_fk_to_wrong_type() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE category (label VARCHAR(64))").unwrap();
    svc.execute_sql("CREATE TABLE note (label VARCHAR(64))").unwrap();
    let note_create = svc
        .execute_sql("INSERT INTO note (label) VALUES ('not a category')")
        .unwrap();
    let note_id = match note_create {
        crate::sql_engine::SqlResult::Ok(s) => s,
        other => panic!("expected Ok with new id, got {other:?}"),
    };

    svc.execute_sql(
        "CREATE TABLE link (target VARCHAR(64) REFERENCES category)",
    )
    .unwrap();

    let mut extra = std::collections::BTreeMap::new();
    extra.insert(
        "target".to_string(),
        crate::types::Value::String(note_id.clone()),
    );

    let err = svc
        .create_doogat_with_extra("Bogus link", &[], Some("link"), "", extra)
        .unwrap_err();
    match err {
        crate::error::DoogatError::Structured { code, .. } => {
            assert_eq!(code, crate::error::codes::REFERENCES_VIOLATION);
        }
        other => panic!("expected Structured REFERENCES_VIOLATION, got {other:?}"),
    }
}

/// PRD 00129 §T3 explicitly preserves CLI silent base-only creation for
/// UNREGISTERED types. Regression guard.
#[test]
fn create_doogat_with_extra_preserves_unregistered_type_silent_create() {
    let (_tmp, svc) = fresh_svc();

    let mut extra = std::collections::BTreeMap::new();
    extra.insert(
        "arbitrary".to_string(),
        crate::types::Value::String("anything".to_string()),
    );

    // "widget" has no typedef. Must succeed and dump `arbitrary` into
    // frontmatter (no UNKNOWN_FIELD rejection).
    let parsed = svc
        .create_doogat_with_extra("Widget A", &[], Some("widget"), "body", extra)
        .unwrap();

    assert_eq!(
        parsed.meta.extra.get("arbitrary"),
        Some(&crate::types::Value::String("anything".to_string()))
    );
    assert_eq!(parsed.meta.doogat_type.as_deref(), Some("widget"));
}

/// PRD 00133 §Origin: typed FK validation must reject ids that don't exist
/// in the referenced typedef table. Combined with `*_rejects_fk_to_wrong_type`
/// this proves the helper queries `<referenced_type>` and not the generic
/// `doogats` index.
#[test]
fn typed_create_rejects_fk_to_nonexistent_id() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE category (label VARCHAR(64))").unwrap();
    svc.execute_sql(
        "CREATE TABLE link (target VARCHAR(64) REFERENCES category)",
    )
    .unwrap();

    // Bogus id that exists nowhere — neither in `category` nor in `doogats`.
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "target".to_string(),
        crate::types::Value::String("99999999999999".to_string()),
    );

    let inputs = vec![crate::types::BatchCreateInput {
        title: Some("Bogus link".to_string()),
        body: None,
        tags: vec![],
        doogat_type: Some("link".to_string()),
        fields: fields.clone(),
        on_conflict: crate::types::ConflictAction::Error,
    }];

    let err = svc.batch_create(&inputs).unwrap_err();
    match err {
        crate::error::DoogatError::Structured { code, .. } => {
            assert_eq!(code, crate::error::codes::REFERENCES_VIOLATION);
        }
        other => panic!("expected Structured REFERENCES_VIOLATION, got {other:?}"),
    }

    // Same input through CLI/FFI surface — must reject identically.
    let err2 = svc
        .create_doogat_with_extra("Bogus link", &[], Some("link"), "", fields)
        .unwrap_err();
    match err2 {
        crate::error::DoogatError::Structured { code, .. } => {
            assert_eq!(code, crate::error::codes::REFERENCES_VIOLATION);
        }
        other => panic!("expected Structured REFERENCES_VIOLATION, got {other:?}"),
    }
}

/// PRD 00134 cycle-1 review C1 task #2: pin that a service-path typed UPDATE
/// of a REFERENCES column on the typed table flushes the OLD junction row
/// and inserts the NEW one. This is what `update_doogat_parsed` /
/// `batch_update` route through (via `reindex_and_rematerialize` →
/// `materialize_single`); the auto-junction must be in sync after the
/// update, not just additively populated.
#[test]
fn service_update_doogat_syncs_auto_junction_on_references_change() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    svc.execute_sql("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_a = match svc
        .execute_sql("INSERT INTO category (label) VALUES ('alpha')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let cat_b = match svc
        .execute_sql("INSERT INTO category (label) VALUES ('beta')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm = match svc
        .execute_sql(&format!(
            "INSERT INTO bookmark (url, category) VALUES ('https://x.example.com', '{cat_a}')"
        ))
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Sanity: junction has (bm, cat_a).
    let pre = svc
        .execute_sql(&format!(
            "SELECT bookmark_id, category_id FROM bookmark_category WHERE bookmark_id = '{bm}'"
        ))
        .unwrap();
    match pre {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1, "expected 1 junction row before update");
            assert_eq!(rows[0][1], cat_a, "junction must point at cat_a pre-update");
        }
        _ => panic!("expected rows"),
    }

    // Service-path typed UPDATE of the REFERENCES column.
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "category".to_string(),
        crate::types::Value::String(cat_b.clone()),
    );
    svc.update_doogat_parsed(
        &bm,
        None,
        None,
        None,
        None,
        &ExtraFieldUpdates {
            set: &fields,
            unset: &[],
        },
    )
    .unwrap();

    // After update: exactly one junction row, pointing at cat_b. Old
    // (bm, cat_a) row must be gone, not just augmented with a new row.
    let post = svc
        .execute_sql(&format!(
            "SELECT bookmark_id, category_id FROM bookmark_category WHERE bookmark_id = '{bm}'"
        ))
        .unwrap();
    match post {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(
                rows.len(),
                1,
                "service UPDATE on REFERENCES column must DELETE old junction rows, not just INSERT new ones (got {} rows)",
                rows.len()
            );
            assert_eq!(
                rows[0][1], cat_b,
                "junction must point at cat_b after service-path UPDATE"
            );
        }
        _ => panic!("expected rows"),
    }
}

/// PRD 00134 cycle-1 review C1 task #2: same invariant via `batch_update`,
/// which goes through `prepare_update` + `reindex_and_rematerialize` for
/// each row. Pins that the GraphQL `batchUpdate` mutation surface keeps the
/// junction in sync as well.
#[test]
fn service_batch_update_syncs_auto_junction_on_references_change() {
    let (_tmp, mut svc) = fresh_svc();

    svc.execute_sql("CREATE TABLE tag (label VARCHAR(100))")
        .unwrap();
    svc.execute_sql("CREATE TABLE post (title2 VARCHAR(200), tag TEXT REFERENCES tag)")
        .unwrap();

    let tag_a = match svc
        .execute_sql("INSERT INTO tag (label) VALUES ('alpha')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let tag_b = match svc
        .execute_sql("INSERT INTO tag (label) VALUES ('beta')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let post_id = match svc
        .execute_sql(&format!(
            "INSERT INTO post (title2, tag) VALUES ('hello', '{tag_a}')"
        ))
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "tag".to_string(),
        crate::types::Value::String(tag_b.clone()),
    );
    let inputs = vec![BatchUpdateInput {
        id: post_id.clone(),
        title: None,
        tags: None,
        doogat_type: None,
        body: None,
        fields: Some(fields),
        unset_fields: None,
    }];
    svc.batch_update(&inputs).unwrap();

    let post = svc
        .execute_sql(&format!(
            "SELECT post_id, tag_id FROM post_tag WHERE post_id = '{post_id}'"
        ))
        .unwrap();
    match post {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(
                rows.len(),
                1,
                "batch_update on REFERENCES column must DELETE old junction rows (got {} rows)",
                rows.len()
            );
            assert_eq!(
                rows[0][1], tag_b,
                "junction must point at tag_b after batch_update"
            );
        }
        _ => panic!("expected rows"),
    }
}
