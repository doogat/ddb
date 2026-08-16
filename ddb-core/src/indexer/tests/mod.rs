use super::*;
use crate::git_ops::GitRepo;
use crate::types::{
    DoogatId, DoogatMeta, InlineField, Link, SearchFieldFilter, SearchFieldOp, SearchFilters,
    TagQueryFilter, Value, Zone,
};

mod graph_tests;
mod materialize_tests;
mod resolve_tests;

fn sample_doogat() -> ParsedDoogat {
    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(DoogatId("20260226120000".into())),
            title: Some("Test Note".into()),
            date: Some("2026-02-26".into()),
            doogat_type: Some("permanent".into()),
            tags: vec!["client/acme".into(), "test".into()],
            extra: Default::default(),
        },
        body: "Body with searchable content and [[20260101000000|Link]]".into(),
        sections: vec![],
        reference_section: "- source:: Wikipedia".into(),
        inline_fields: vec![InlineField {
            key: "source".into(),
            value: "Wikipedia".into(),
            zone: Zone::Reference,
        }],
        links: vec![Link {
            target: "20260101000000".into(),
            display: Some("Link".into()),
            section: None,
            kind: crate::types::LinkKind::WikiLink,
            zone: Zone::Body,
        }],
        body_tags: vec![],
        checkboxes: vec![],
        path: "ddb/20260226120000.md".into(),
        updated_at: None,
    }
}

fn in_memory_index() -> Index {
    Index::open(Path::new(":memory:")).unwrap()
}

fn make_sample_doogats(n: usize) -> Vec<ParsedDoogat> {
    (0..n)
        .map(|i| {
            let id = format!("{:014}", 20260226120000u64 + i as u64);
            ParsedDoogat {
                meta: DoogatMeta {
                    id: Some(DoogatId(id.clone())),
                    title: Some(format!("Note {i}")),
                    date: Some("2026-02-26".into()),
                    doogat_type: Some("permanent".into()),
                    tags: vec!["test".into()],
                    extra: Default::default(),
                },
                body: format!("Body of doogat {i}"),
                sections: vec![],
                reference_section: String::new(),
                inline_fields: vec![],
                links: vec![],
                body_tags: vec![],
                checkboxes: vec![],
                path: format!("ddb/{id}.md"),
                updated_at: None,
            }
        })
        .collect()
}

fn dump_table(idx: &Index, table: &str) -> Vec<Vec<String>> {
    idx.query_raw(&format!("SELECT * FROM \"{table}\" ORDER BY 1"))
        .unwrap()
}

fn make_doogat(n: usize) -> ParsedDoogat {
    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(DoogatId(format!("2026022612{n:04}"))),
            title: Some(format!("Note {n}")),
            date: Some("2026-02-26".into()),
            doogat_type: Some("permanent".into()),
            tags: vec!["test".into()],
            extra: Default::default(),
        },
        body: format!("Searchable body number {n}"),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: format!("ddb/2026022612{n:04}.md"),
        updated_at: None,
    }
}

fn seq_doogat(id: &str, title: &str, parent: Option<&str>) -> ParsedDoogat {
    let mut extra = std::collections::BTreeMap::new();
    if let Some(pid) = parent {
        extra.insert("sequence".into(), Value::String(pid.into()));
    }
    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(DoogatId(id.into())),
            title: Some(title.into()),
            date: None,
            doogat_type: None,
            tags: vec![],
            extra,
        },
        body: String::new(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: format!("ddb/{id}.md"),
        updated_at: None,
    }
}

/// Helper: commit a file with a custom git timestamp (epoch seconds).
fn commit_file_with_time(
    repo: &GitRepo,
    rel_path: &str,
    content: &str,
    message: &str,
    epoch_secs: i64,
) {
    let full_path = repo.path.join(rel_path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&full_path, content).unwrap();

    let git_repo = &repo.repo;
    let mut index = git_repo.index().unwrap();
    index.add_path(std::path::Path::new(rel_path)).unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();

    let sig = git2::Signature::new("ddb", "ddb@test", &git2::Time::new(epoch_secs, 0)).unwrap();

    let parents: Vec<git2::Commit<'_>> = match git_repo.head() {
        Ok(head) => vec![head.peel_to_commit().unwrap()],
        Err(_) => vec![],
    };
    let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();

    git_repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
        .unwrap();
}

#[test]
fn schema_creation_idempotent() {
    let idx = in_memory_index();
    // Opening again should not error
    let _idx2 = Index::open(Path::new(":memory:")).unwrap();
    // Verify tables exist
    let count: i64 = idx
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='doogats'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn batch_index_matches_sequential() {
    let doogats = make_sample_doogats(10);

    // Sequential: index one-by-one
    let idx_seq = in_memory_index();
    for z in &doogats {
        idx_seq.index_doogat(z).unwrap();
    }

    // Batch: single transaction
    let idx_batch = in_memory_index();
    let count = idx_batch.batch_index(&doogats).unwrap();
    assert_eq!(count, 10);

    // Compare all tables
    for table in &[
        "doogats",
        "_ddb_tags",
        "_ddb_fields",
        "_ddb_links",
        "_ddb_aliases",
        "_ddb_checkboxes",
    ] {
        let seq_rows = dump_table(&idx_seq, table);
        let batch_rows = dump_table(&idx_batch, table);
        assert_eq!(
            seq_rows.len(),
            batch_rows.len(),
            "row count mismatch in {table}"
        );
        // Compare non-timestamp columns (updated_at varies)
        if *table == "doogats" {
            for (s, b) in seq_rows.iter().zip(batch_rows.iter()) {
                // Compare all columns except updated_at (index 6)
                assert_eq!(&s[..6], &b[..6], "doogats row mismatch");
            }
        } else {
            assert_eq!(seq_rows, batch_rows, "mismatch in {table}");
        }
    }

    // Verify FTS also works
    let seq_fts = idx_seq.search("Body").unwrap();
    let batch_fts = idx_batch.search("Body").unwrap();
    assert_eq!(seq_fts.len(), batch_fts.len());
}

#[test]
fn parallel_parse_error_resilience() {
    use crate::traits::mock::MockSource;

    let mut source = MockSource::new();
    // 9 valid doogats
    for i in 0..9 {
        let id = format!("{:014}", 20260226120000u64 + i);
        let content = format!("---\nid: {id}\ntitle: Note {i}\ndate: 2026-02-26\n---\nBody {i}");
        source.files.insert(format!("ddb/{id}.md"), content);
    }
    // 1 malformed doogat (invalid YAML frontmatter)
    source.files.insert(
        "ddb/20260226129999.md".into(),
        "---\n: invalid yaml [\n---\nbody".into(),
    );

    let paths = source.list_doogats().unwrap();
    let (parsed, warnings) = Index::parallel_parse(&source, &paths).unwrap();

    assert_eq!(parsed.len(), 9);
    assert_eq!(warnings.len(), 1);
    assert!(matches!(
        &warnings[0],
        crate::types::ConsistencyWarning::MalformedYaml { path, .. }
        if path == "ddb/20260226129999.md"
    ));
}

#[test]
fn index_and_query_doogat() {
    let idx = in_memory_index();
    let z = sample_doogat();
    idx.index_doogat(&z).unwrap();

    // Query back
    let rows = idx.query_raw("SELECT id, title FROM doogats").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "20260226120000");
    assert_eq!(rows[0][1], "Test Note");
}

#[test]
fn body_hashtags_indexed() {
    let idx = in_memory_index();
    let mut z = sample_doogat();
    z.body = "Some text #gtd/act/next here".into();
    z.body_tags = vec!["gtd/act/next".into()];
    idx.index_doogat(&z).unwrap();

    let rows = idx
        .query_raw("SELECT tag, source FROM _ddb_tags WHERE tag = 'gtd/act/next'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "gtd/act/next");
    assert_eq!(rows[0][1], "body");
}

#[test]
fn body_and_frontmatter_tags_unified() {
    let idx = in_memory_index();
    let mut z = sample_doogat();
    // sample_doogat has frontmatter tags: ["client/acme", "test"]
    z.body_tags = vec!["gtd/wait".into()];
    idx.index_doogat(&z).unwrap();

    let id = z.meta.id.as_ref().unwrap().0.as_str();
    let rows = idx
        .query_raw(&format!(
            "SELECT tag FROM _ddb_tags WHERE doogat_id = '{id}' ORDER BY tag"
        ))
        .unwrap();
    let tags: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
    assert!(tags.contains(&"client/acme"), "missing frontmatter tag");
    assert!(tags.contains(&"test"), "missing frontmatter tag");
    assert!(tags.contains(&"gtd/wait"), "missing body tag");
}

#[test]
fn tag_source_column() {
    let idx = in_memory_index();
    let mut z = sample_doogat();
    z.body_tags = vec!["gtd/act/next".into()];
    idx.index_doogat(&z).unwrap();

    let id = z.meta.id.as_ref().unwrap().0.as_str();

    // Frontmatter tags have source='frontmatter'
    let rows = idx
        .query_raw(&format!(
            "SELECT source FROM _ddb_tags WHERE doogat_id = '{id}' AND tag = 'test'"
        ))
        .unwrap();
    assert_eq!(rows[0][0], "frontmatter");

    // Body tags have source='body'
    let rows = idx
        .query_raw(&format!(
            "SELECT source FROM _ddb_tags WHERE doogat_id = '{id}' AND tag = 'gtd/act/next'"
        ))
        .unwrap();
    assert_eq!(rows[0][0], "body");
}

#[test]
fn checkboxes_indexed() {
    let idx = in_memory_index();
    let mut z = sample_doogat();
    z.body = "- [ ] open task\n- [x] done task\n- [i] 2026-01-01 10:00 - note".into();
    z.checkboxes = vec![
        crate::types::CheckboxItem {
            state: crate::types::CheckboxState::Open,
            content: "open task".into(),
            date: None,
            due_date: None,
            line_number: 1,
            indent_level: 0,
        },
        crate::types::CheckboxItem {
            state: crate::types::CheckboxState::Done,
            content: "done task".into(),
            date: None,
            due_date: None,
            line_number: 2,
            indent_level: 0,
        },
        crate::types::CheckboxItem {
            state: crate::types::CheckboxState::Info,
            content: "note".into(),
            date: Some("2026-01-01 10:00".into()),
            due_date: None,
            line_number: 3,
            indent_level: 0,
        },
    ];
    idx.index_doogat(&z).unwrap();

    let rows = idx
        .query_raw("SELECT state, content FROM _ddb_checkboxes ORDER BY line_number")
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][0], "open");
    assert_eq!(rows[1][0], "done");
    assert_eq!(rows[2][0], "info");
}

#[test]
fn checkbox_state_query() {
    let idx = in_memory_index();
    let mut z = sample_doogat();
    z.checkboxes = vec![
        crate::types::CheckboxItem {
            state: crate::types::CheckboxState::Open,
            content: "pending".into(),
            date: None,
            due_date: None,
            line_number: 1,
            indent_level: 0,
        },
        crate::types::CheckboxItem {
            state: crate::types::CheckboxState::Done,
            content: "finished".into(),
            date: None,
            due_date: None,
            line_number: 2,
            indent_level: 0,
        },
    ];
    idx.index_doogat(&z).unwrap();

    let open = idx
        .query_raw("SELECT content FROM _ddb_checkboxes WHERE state = 'open'")
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0][0], "pending");
}

#[test]
fn checkbox_reindex_state_change() {
    let idx = in_memory_index();
    let mut z = sample_doogat();

    // Initial: one open item
    z.checkboxes = vec![crate::types::CheckboxItem {
        state: crate::types::CheckboxState::Open,
        content: "buy milk".into(),
        date: None,
        due_date: None,
        line_number: 1,
        indent_level: 0,
    }];
    idx.index_doogat(&z).unwrap();

    let open = idx
        .query_raw("SELECT content FROM _ddb_checkboxes WHERE state = 'open'")
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0][0], "buy milk");

    // Reindex with state changed to done
    z.checkboxes = vec![crate::types::CheckboxItem {
        state: crate::types::CheckboxState::Done,
        content: "buy milk".into(),
        date: None,
        due_date: None,
        line_number: 1,
        indent_level: 0,
    }];
    idx.index_doogat(&z).unwrap();

    let open = idx
        .query_raw("SELECT content FROM _ddb_checkboxes WHERE state = 'open'")
        .unwrap();
    assert_eq!(open.len(), 0);

    let done = idx
        .query_raw("SELECT content FROM _ddb_checkboxes WHERE state = 'done'")
        .unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0][0], "buy milk");
}

#[test]
fn gtd_state_aggregated_from_all_zones() {
    let idx = in_memory_index();

    // Parse a doogat with GTD-relevant data in all zones:
    // - frontmatter: processed=true, tags=[gtd/ignore]
    // - body: checkboxes + hashtag #gtd/act/next
    let content = "\
---
id: 20260301120000
title: GTD Aggregation Test
date: 2026-03-01
tags:
  - gtd/ignore
processed: true
---
- [ ] task one #gtd/act/next
- [x] done task
- [i] info entry
---
- source:: Wikipedia";

    let parsed = crate::parser::parse(content, "ddb/20260301120000.md").unwrap();
    idx.index_doogat(&parsed).unwrap();

    let id = "20260301120000";

    // Verify _ddb_fields has processed=true (from frontmatter extra)
    let fields = idx
        .query_raw(&format!(
            "SELECT key, value FROM _ddb_fields WHERE doogat_id = '{id}' AND key = 'processed'"
        ))
        .unwrap();
    assert_eq!(fields.len(), 1, "should have processed field");
    assert_eq!(fields[0][0], "processed");
    assert_eq!(fields[0][1], "true");

    // Verify _ddb_tags has both frontmatter tag and body hashtag
    let tags = idx
        .query_raw(&format!(
            "SELECT tag, source FROM _ddb_tags WHERE doogat_id = '{id}' ORDER BY tag"
        ))
        .unwrap();
    let tag_names: Vec<&str> = tags.iter().map(|r| r[0].as_str()).collect();
    assert!(
        tag_names.contains(&"gtd/ignore"),
        "should have frontmatter tag gtd/ignore: {tag_names:?}"
    );
    assert!(
        tag_names.contains(&"gtd/act/next"),
        "should have body hashtag gtd/act/next: {tag_names:?}"
    );

    // Verify sources are correct
    let fm_tag = tags.iter().find(|r| r[0] == "gtd/ignore").unwrap();
    assert_eq!(
        fm_tag[1], "frontmatter",
        "gtd/ignore should be from frontmatter"
    );
    let body_tag = tags.iter().find(|r| r[0] == "gtd/act/next").unwrap();
    assert_eq!(body_tag[1], "body", "gtd/act/next should be from body");

    // Verify _ddb_checkboxes has 3 rows with correct states
    let checkboxes = idx
            .query_raw(&format!(
                "SELECT state, content FROM _ddb_checkboxes WHERE doogat_id = '{id}' ORDER BY line_number"
            ))
            .unwrap();
    assert_eq!(checkboxes.len(), 3, "should have 3 checkboxes");
    assert_eq!(checkboxes[0][0], "open", "first checkbox should be open");
    assert!(
        checkboxes[0][1].contains("task one"),
        "first checkbox content: {}",
        checkboxes[0][1]
    );
    assert_eq!(checkboxes[1][0], "done", "second checkbox should be done");
    assert!(
        checkboxes[1][1].contains("done task"),
        "second checkbox content: {}",
        checkboxes[1][1]
    );
    assert_eq!(checkboxes[2][0], "info", "third checkbox should be info");
    assert!(
        checkboxes[2][1].contains("info entry"),
        "third checkbox content: {}",
        checkboxes[2][1]
    );
}

#[test]
fn fts_search() {
    let idx = in_memory_index();
    idx.index_doogat(&sample_doogat()).unwrap();

    let results = idx.search("searchable").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "20260226120000");
}

#[test]
fn tag_prefix_query() {
    let idx = in_memory_index();
    idx.index_doogat(&sample_doogat()).unwrap();

    let ids = idx.by_tag("client/").unwrap();
    assert!(ids.contains(&"20260226120000".to_string()));

    let ids = idx.by_tag("test").unwrap();
    assert!(ids.contains(&"20260226120000".to_string()));

    let ids = idx.by_tag("nonexistent").unwrap();
    assert!(ids.is_empty());
}

#[test]
fn index_all_link_kinds() {
    let idx = in_memory_index();
    let z = ParsedDoogat {
        meta: DoogatMeta {
            id: Some(DoogatId("20260226120000".into())),
            title: Some("Mixed Links".into()),
            date: None,
            doogat_type: None,
            tags: vec![],
            extra: Default::default(),
        },
        body: String::new(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![
            crate::types::Link {
                target: "wiki_target".into(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: Zone::Body,
            },
            crate::types::Link {
                target: "path.md".into(),
                display: Some("title".into()),
                section: None,
                kind: crate::types::LinkKind::MarkdownLink,
                zone: Zone::Body,
            },
            crate::types::Link {
                target: "embed_file".into(),
                display: None,
                section: Some("sec".into()),
                kind: crate::types::LinkKind::Embed,
                zone: Zone::Body,
            },
            crate::types::Link {
                target: "https://example.com".into(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::BareUrl,
                zone: Zone::Body,
            },
        ],
        body_tags: vec![],
        checkboxes: vec![],
        path: "ddb/20260226120000.md".into(),
        updated_at: None,
    };
    idx.index_doogat(&z).unwrap();

    let rows = idx
        .query_raw("SELECT target_path, kind FROM _ddb_links ORDER BY kind")
        .unwrap();
    assert_eq!(rows.len(), 4);
    let kinds: Vec<&str> = rows.iter().map(|r| r[1].as_str()).collect();
    assert!(kinds.contains(&"wikilink"));
    assert!(kinds.contains(&"markdown"));
    assert!(kinds.contains(&"embed"));
    assert!(kinds.contains(&"url"));
}

#[test]
fn query_raw_join() {
    let idx = in_memory_index();
    idx.index_doogat(&sample_doogat()).unwrap();

    let rows = idx.query_raw(
            "SELECT z.title, t.tag FROM doogats z JOIN _ddb_tags t ON t.doogat_id = z.id ORDER BY t.tag"
        ).unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn upsert_replaces_old_data() {
    let idx = in_memory_index();
    let mut z = sample_doogat();
    idx.index_doogat(&z).unwrap();

    // Update title and tags
    z.meta.title = Some("Updated Title".into());
    z.meta.tags = vec!["newtag".into()];
    idx.index_doogat(&z).unwrap();

    let rows = idx
        .query_raw("SELECT title FROM doogats WHERE id = '20260226120000'")
        .unwrap();
    assert_eq!(rows[0][0], "Updated Title");

    let rows = idx
        .query_raw("SELECT COUNT(*) FROM _ddb_tags WHERE doogat_id = '20260226120000'")
        .unwrap();
    assert_eq!(rows[0][0], "1");
}

#[test]
fn integration_inferred_type_full_cycle() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    // Create doogats with type "foo" — no _typedef exists
    let z1 = "---\nid: 20260226220000\ntitle: Foo 1\ntype: foo\npriority: 3\n---\n\n## Description\n\nFirst foo\n\n---\n\n- owner:: [[20260226220100]]";
    let z2 = "---\nid: 20260226220100\ntitle: Foo 2\ntype: foo\npriority: 7\n---\n\n## Description\n\nSecond foo\n\n---\n\n- owner:: [[20260226220000]]";
    repo.commit_file("ddb/20260226220000.md", z1, "add foo 1")
        .unwrap();
    repo.commit_file("ddb/20260226220100.md", z2, "add foo 2")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    let report = idx.rebuild(&repo).unwrap();

    // Table "foo" should exist with inferred columns
    assert!(report.types_inferred.contains(&"foo".to_string()));
    assert!(report.tables_materialized > 0);

    // SELECT should return data
    let rows = idx
        .query_raw("SELECT id, priority FROM foo ORDER BY id")
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][1], "3");
    assert_eq!(rows[1][1], "7");
}

#[test]
fn integration_external_edit_reconciliation() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    // Initial doogat with type "doc" and one field
    let z1 = "---\nid: 20260226240000\ntitle: Doc 1\ntype: doc\nversion: 1\n---\nBody";
    repo.commit_file("ddb/20260226240000.md", z1, "add doc")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    // Externally add a doogat with a new field
    let z2 =
        "---\nid: 20260226240100\ntitle: Doc 2\ntype: doc\nversion: 2\nauthor: Alice\n---\nBody";
    repo.commit_file("ddb/20260226240100.md", z2, "add doc externally")
        .unwrap();

    // Rebuild picks up new fields
    let report = idx.rebuild(&repo).unwrap();
    assert_eq!(report.indexed, 2);

    // Table should now have "author" column from inferred merge
    let rows = idx
        .query_raw("SELECT id, author FROM doc WHERE author != ''")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1], "Alice");
}

#[test]
fn check_integrity_healthy_db() {
    let idx = in_memory_index();
    assert!(idx.check_integrity().unwrap());
}

#[test]
fn check_integrity_missing_table() {
    // Open a fresh db without the schema setup — simulate partial corruption
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("index.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE doogats (id TEXT PRIMARY KEY)")
        .unwrap();
    drop(conn);

    // Open via Index — schema creates missing tables, but let's test
    // a scenario where we drop a table after open
    let idx = Index::open(&db_path).unwrap();
    idx.conn.execute_batch("DROP TABLE _ddb_fts").unwrap();
    assert!(!idx.check_integrity().unwrap());
}

#[test]
fn schema_parses_allowed_values_and_default() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let typedef = "---\nid: 20260301100000\ntitle: task\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n    allowed_values:\n      - todo\n      - doing\n      - done\n    default_value: todo\n  - name: priority\n    data_type: TEXT\n    zone: frontmatter\n---\n";
    repo.commit_file("ddb/_typedef/20260301100000.md", typedef, "add typedef")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    let schemas = idx.load_all_typedefs(&repo);
    let schema = schemas.get("task").unwrap();
    let status_col = schema.columns.iter().find(|c| c.name == "status").unwrap();
    assert_eq!(
        status_col.allowed_values.as_ref().unwrap(),
        &["todo", "doing", "done"]
    );
    assert_eq!(status_col.default_value.as_deref(), Some("todo"));

    let priority_col = schema
        .columns
        .iter()
        .find(|c| c.name == "priority")
        .unwrap();
    assert!(priority_col.allowed_values.is_none());
    assert!(priority_col.default_value.is_none());
}

#[test]
fn paginated_search_basic() {
    let idx = in_memory_index();
    for i in 0..30 {
        idx.index_doogat(&make_doogat(i)).unwrap();
    }

    let result = idx.search_paginated("searchable", 10, 0).unwrap();
    assert_eq!(result.hits.len(), 10);
    assert_eq!(result.total_count, 30);
}

#[test]
fn paginated_search_offset_beyond() {
    let idx = in_memory_index();
    for i in 0..5 {
        idx.index_doogat(&make_doogat(i)).unwrap();
    }

    let result = idx.search_paginated("searchable", 10, 100).unwrap();
    assert!(result.hits.is_empty());
    assert_eq!(result.total_count, 5);
}

#[test]
fn paginated_search_no_results() {
    let idx = in_memory_index();
    idx.index_doogat(&make_doogat(0)).unwrap();

    let result = idx.search_paginated("nonexistent", 10, 0).unwrap();
    assert!(result.hits.is_empty());
    assert_eq!(result.total_count, 0);
}

#[test]
fn search_returns_same_hits_as_paginated() {
    let idx = in_memory_index();
    for i in 0..5 {
        idx.index_doogat(&make_doogat(i)).unwrap();
    }

    let results = idx.search("searchable").unwrap();
    let paginated = idx.search_paginated("searchable", usize::MAX, 0).unwrap();

    assert_eq!(results.len(), 5);
    assert_eq!(results.len(), paginated.hits.len());
    assert_eq!(
        results.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        paginated
            .hits
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn attachments_indexed_and_queried() {
    let idx = in_memory_index();
    let mut extra = std::collections::BTreeMap::new();
    extra.insert(
        "attachments".into(),
        Value::List(vec![
            Value::Map({
                let mut m = std::collections::BTreeMap::new();
                m.insert("name".into(), Value::String("photo.jpg".into()));
                m.insert("mime".into(), Value::String("image/jpeg".into()));
                m.insert("size".into(), Value::Number(1024.0));
                m
            }),
            Value::Map({
                let mut m = std::collections::BTreeMap::new();
                m.insert("name".into(), Value::String("doc.pdf".into()));
                m.insert("mime".into(), Value::String("application/pdf".into()));
                m.insert("size".into(), Value::Number(2048.0));
                m
            }),
        ]),
    );
    let doogat = ParsedDoogat {
        meta: DoogatMeta {
            id: Some(DoogatId("20260301130000".into())),
            title: Some("Test".into()),
            extra,
            ..Default::default()
        },
        body: String::new(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: "ddb/20260301130000.md".into(),
        updated_at: None,
    };
    idx.index_doogat(&doogat).unwrap();

    let rows: Vec<(String, String, String, i64)> = idx
        .conn
        .prepare("SELECT doogat_id, name, mime, size FROM _ddb_attachments ORDER BY name")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].1, "doc.pdf");
    assert_eq!(rows[1].1, "photo.jpg");
    assert_eq!(rows[1].3, 1024);
}

#[test]
fn incremental_batch_mode_multi_change() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    // Create 5 doogats
    for i in 0..5 {
        repo.commit_file(
            &format!("ddb/2024010{i}000000.md"),
            &format!("---\ntitle: Note {i}\n---\nBody {i}."),
            &format!("add {i}"),
        )
        .unwrap();
    }

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();
    let old_head = idx.stored_head_oid().unwrap();

    // Modify 3 doogats in a single commit
    let modifications: Vec<(&str, &str)> = vec![
        (
            "ddb/20240100000000.md",
            "---\ntitle: Modified 0\n---\nUpdated body 0.",
        ),
        (
            "ddb/20240101000000.md",
            "---\ntitle: Modified 1\n---\nUpdated body 1.",
        ),
        (
            "ddb/20240102000000.md",
            "---\ntitle: Modified 2\n---\nUpdated body 2.",
        ),
    ];
    repo.commit_files(&modifications, "modify 3").unwrap();

    let report = idx.incremental_reindex(&repo, &old_head, false).unwrap();
    assert_eq!(report.indexed, 3);

    // Verify modifications
    let rows = idx
        .query_raw("SELECT title FROM doogats WHERE id = '20240100000000'")
        .unwrap();
    assert_eq!(rows[0][0], "Modified 0");

    let rows = idx
        .query_raw("SELECT title FROM doogats WHERE id = '20240102000000'")
        .unwrap();
    assert_eq!(rows[0][0], "Modified 2");
}

#[test]
fn frontmatter_extras_indexed_as_fields() {
    let idx = in_memory_index();
    let mut z = sample_doogat();
    z.meta
        .extra
        .insert("resurrected".into(), crate::types::Value::Bool(true));
    z.meta
        .extra
        .insert("priority".into(), crate::types::Value::Number(3.0));
    z.meta.extra.insert(
        "source_url".into(),
        crate::types::Value::String("https://example.com".into()),
    );
    idx.index_doogat(&z).unwrap();

    let id = z.meta.id.as_ref().unwrap().0.as_str();
    let rows: Vec<(String, String, String)> = idx
            .conn
            .prepare("SELECT key, value, zone FROM _ddb_fields WHERE doogat_id = ?1 AND zone = 'Frontmatter'")
            .unwrap()
            .query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();

    assert!(rows
        .iter()
        .any(|(k, v, _)| k == "resurrected" && v == "true"));
    assert!(rows.iter().any(|(k, v, _)| k == "priority" && v == "3"));
    assert!(rows
        .iter()
        .any(|(k, v, _)| k == "source_url" && v == "https://example.com"));
    // List/Map extras should NOT appear
    assert!(!rows
        .iter()
        .any(|(k, _, _)| k == "aliases" || k == "attachments"));
}

#[test]
fn concurrent_read_during_write() {
    // Simulates widget/extension reading index while host app writes.
    // Two Index instances on the same DB — WAL + busy_timeout must handle this.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("index.db");

    let writer = Index::open(&db_path).unwrap();

    // Index a doogat via the writer
    let doogat = sample_doogat();
    writer.index_doogat(&doogat).unwrap();

    // Open a second read-only connection (simulates widget process)
    let reader = Index::open(&db_path).unwrap();

    // Reader sees the doogat written by writer
    let results = reader.search("searchable").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "20260226120000");

    // Writer can still write while reader is open
    let doogat2 = ParsedDoogat {
        meta: DoogatMeta {
            id: Some(DoogatId("20260226120001".into())),
            title: Some("Second Note".into()),
            date: Some("2026-02-26".into()),
            doogat_type: Some("permanent".into()),
            tags: vec![],
            extra: Default::default(),
        },
        body: "Another body".into(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: "ddb/20260226120001.md".into(),
        updated_at: None,
    };
    writer.index_doogat(&doogat2).unwrap();

    // Reader sees both doogats (WAL allows concurrent read + write)
    let all = reader
        .conn
        .prepare("SELECT id FROM doogats ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn concurrent_readers_no_contention() {
    // Multiple simultaneous readers should never block each other.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("index.db");

    let writer = Index::open(&db_path).unwrap();
    let doogat = sample_doogat();
    writer.index_doogat(&doogat).unwrap();

    // Open three concurrent readers
    let r1 = Index::open(&db_path).unwrap();
    let r2 = Index::open(&db_path).unwrap();
    let r3 = Index::open(&db_path).unwrap();

    // All three read successfully
    assert_eq!(r1.search("searchable").unwrap().len(), 1);
    assert_eq!(r2.search("searchable").unwrap().len(), 1);
    assert_eq!(r3.search("searchable").unwrap().len(), 1);
}

#[test]
fn busy_timeout_is_set() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("index.db");
    let index = Index::open(&db_path).unwrap();

    let timeout: i64 = index
        .conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .unwrap();
    assert_eq!(timeout, 5000);
}

#[test]
fn list_tags_counts_and_ordering() {
    let idx = in_memory_index();

    // Doogat 1: frontmatter tags "rust" and "cli", body tag "tools"
    let mut z1 = make_doogat(1);
    z1.meta.tags = vec!["rust".into(), "cli".into()];
    z1.body_tags = vec!["tools".into()];
    idx.index_doogat(&z1).unwrap();

    // Doogat 2: frontmatter tag "rust", body tag "tools"
    let mut z2 = make_doogat(2);
    z2.meta.tags = vec!["rust".into()];
    z2.body_tags = vec!["tools".into()];
    idx.index_doogat(&z2).unwrap();

    // Doogat 3: frontmatter tag "cli"
    let mut z3 = make_doogat(3);
    z3.meta.tags = vec!["cli".into()];
    z3.body_tags = vec![];
    idx.index_doogat(&z3).unwrap();

    let tags = idx.list_tags().unwrap();

    // Expected counts: cli=2, rust=2, tools=2
    // All tied at count=2, so ordered alphabetically: cli, rust, tools
    assert_eq!(tags.len(), 3);
    assert_eq!(tags[0], ("cli".into(), 2));
    assert_eq!(tags[1], ("rust".into(), 2));
    assert_eq!(tags[2], ("tools".into(), 2));

    // Add a 4th doogat with "rust" to break the tie
    let mut z4 = make_doogat(4);
    z4.meta.tags = vec!["rust".into()];
    z4.body_tags = vec![];
    idx.index_doogat(&z4).unwrap();

    let tags = idx.list_tags().unwrap();

    // rust=3 now leads, then cli=2, tools=2 (alphabetical tiebreak)
    assert_eq!(tags.len(), 3);
    assert_eq!(tags[0], ("rust".into(), 3));
    assert_eq!(tags[1], ("cli".into(), 2));
    assert_eq!(tags[2], ("tools".into(), 2));
}

// ── query_tags tests ───────────────────────────────────────────

fn make_tagged_doogat(n: usize, tags: Vec<&str>, body_tags: Vec<&str>) -> ParsedDoogat {
    let id = format!("2026040112{n:04}");
    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(DoogatId(id.clone())),
            title: Some(format!("Tagged {n}")),
            date: Some("2026-04-01".into()),
            doogat_type: Some("permanent".into()),
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
            extra: Default::default(),
        },
        body: format!("Body of tagged doogat {n}"),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: body_tags.into_iter().map(|s| s.to_string()).collect(),
        checkboxes: vec![],
        path: format!("ddb/{id}.md"),
        updated_at: None,
    }
}

#[test]
fn query_tags_no_filter_returns_all() {
    let idx = in_memory_index();
    idx.index_doogat(&make_tagged_doogat(0, vec!["rust", "cli"], vec!["tools"]))
        .unwrap();
    idx.index_doogat(&make_tagged_doogat(1, vec!["rust"], vec![]))
        .unwrap();

    let filter = TagQueryFilter::default();
    let entries = idx.query_tags(&filter).unwrap();

    // doogat 0: rust(fm), cli(fm), tools(body) = 3
    // doogat 1: rust(fm) = 1
    assert_eq!(entries.len(), 4);
}

#[test]
fn query_tags_doogat_id_eq() {
    let idx = in_memory_index();
    idx.index_doogat(&make_tagged_doogat(0, vec!["rust", "cli"], vec![]))
        .unwrap();
    idx.index_doogat(&make_tagged_doogat(1, vec!["python"], vec![]))
        .unwrap();

    let filter = TagQueryFilter {
        doogat_id_eq: Some("20260401120000".into()),
        ..Default::default()
    };
    let entries = idx.query_tags(&filter).unwrap();

    assert_eq!(entries.len(), 2);
    for entry in &entries {
        assert_eq!(entry.doogat_id, "20260401120000");
    }
    let tag_names: Vec<&str> = entries.iter().map(|e| e.tag.as_str()).collect();
    assert!(tag_names.contains(&"rust"));
    assert!(tag_names.contains(&"cli"));
}

#[test]
fn query_tags_doogat_id_in() {
    let idx = in_memory_index();
    idx.index_doogat(&make_tagged_doogat(0, vec!["rust"], vec![]))
        .unwrap();
    idx.index_doogat(&make_tagged_doogat(1, vec!["python"], vec![]))
        .unwrap();
    idx.index_doogat(&make_tagged_doogat(2, vec!["go"], vec![]))
        .unwrap();

    let filter = TagQueryFilter {
        doogat_id_in: Some(vec!["20260401120000".into(), "20260401120002".into()]),
        ..Default::default()
    };
    let entries = idx.query_tags(&filter).unwrap();

    assert_eq!(entries.len(), 2);
    let ids: Vec<&str> = entries.iter().map(|e| e.doogat_id.as_str()).collect();
    assert!(ids.contains(&"20260401120000"));
    assert!(ids.contains(&"20260401120002"));
    assert!(!ids.contains(&"20260401120001"));
}

#[test]
fn query_tags_tag_eq() {
    let idx = in_memory_index();
    idx.index_doogat(&make_tagged_doogat(0, vec!["rust", "cli"], vec![]))
        .unwrap();
    idx.index_doogat(&make_tagged_doogat(1, vec!["rust"], vec![]))
        .unwrap();
    idx.index_doogat(&make_tagged_doogat(2, vec!["python"], vec![]))
        .unwrap();

    let filter = TagQueryFilter {
        tag_eq: Some("rust".into()),
        ..Default::default()
    };
    let entries = idx.query_tags(&filter).unwrap();

    assert_eq!(entries.len(), 2);
    for entry in &entries {
        assert_eq!(entry.tag, "rust");
        assert_eq!(entry.source, "frontmatter");
    }
}

#[test]
fn query_tags_tag_contains() {
    let idx = in_memory_index();
    idx.index_doogat(&make_tagged_doogat(
        0,
        vec!["client/acme", "client/beta"],
        vec![],
    ))
    .unwrap();
    idx.index_doogat(&make_tagged_doogat(1, vec!["server"], vec![]))
        .unwrap();

    let filter = TagQueryFilter {
        tag_contains: Some("client".into()),
        ..Default::default()
    };
    let entries = idx.query_tags(&filter).unwrap();

    assert_eq!(entries.len(), 2);
    for entry in &entries {
        assert!(entry.tag.contains("client"));
    }
}

#[test]
fn query_tags_tag_in() {
    let idx = in_memory_index();
    idx.index_doogat(&make_tagged_doogat(0, vec!["rust", "cli", "tools"], vec![]))
        .unwrap();
    idx.index_doogat(&make_tagged_doogat(1, vec!["python"], vec![]))
        .unwrap();

    let filter = TagQueryFilter {
        tag_in: Some(vec!["rust".into(), "python".into()]),
        ..Default::default()
    };
    let entries = idx.query_tags(&filter).unwrap();

    assert_eq!(entries.len(), 2);
    let tags: Vec<&str> = entries.iter().map(|e| e.tag.as_str()).collect();
    assert!(tags.contains(&"rust"));
    assert!(tags.contains(&"python"));
    assert!(!tags.contains(&"cli"));
    assert!(!tags.contains(&"tools"));
}

#[test]
fn query_tags_combined_filters() {
    let idx = in_memory_index();
    idx.index_doogat(&make_tagged_doogat(0, vec!["rust", "cli"], vec![]))
        .unwrap();
    idx.index_doogat(&make_tagged_doogat(1, vec!["rust", "python"], vec![]))
        .unwrap();

    let filter = TagQueryFilter {
        doogat_id_eq: Some("20260401120000".into()),
        tag_eq: Some("rust".into()),
        ..Default::default()
    };
    let entries = idx.query_tags(&filter).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].doogat_id, "20260401120000");
    assert_eq!(entries[0].tag, "rust");
    assert_eq!(entries[0].source, "frontmatter");
}

#[test]
fn query_tags_empty_result() {
    let idx = in_memory_index();
    idx.index_doogat(&make_tagged_doogat(0, vec!["rust"], vec![]))
        .unwrap();

    let filter = TagQueryFilter {
        tag_eq: Some("nonexistent".into()),
        ..Default::default()
    };
    let entries = idx.query_tags(&filter).unwrap();
    assert!(entries.is_empty());

    let filter = TagQueryFilter {
        doogat_id_eq: Some("99999999999999".into()),
        ..Default::default()
    };
    let entries = idx.query_tags(&filter).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn query_tags_empty_in_list_returns_empty() {
    let idx = in_memory_index();
    idx.index_doogat(&make_tagged_doogat(0, vec!["rust"], vec![]))
        .unwrap();

    let filter = TagQueryFilter {
        doogat_id_in: Some(vec![]),
        ..Default::default()
    };
    let entries = idx.query_tags(&filter).unwrap();
    assert!(entries.is_empty());

    let filter = TagQueryFilter {
        tag_in: Some(vec![]),
        ..Default::default()
    };
    let entries = idx.query_tags(&filter).unwrap();
    assert!(entries.is_empty());
}

// ── Search filter tests ────────────────────────────────────────

fn make_typed_doogat(n: usize, dtype: &str, tags: Vec<&str>) -> ParsedDoogat {
    let id = format!("2026030112{n:04}");
    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(DoogatId(id.clone())),
            title: Some(format!("Typed {dtype} {n}")),
            date: Some("2026-03-01".into()),
            doogat_type: Some(dtype.into()),
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
            extra: Default::default(),
        },
        body: format!("Searchable content for {dtype} number {n}"),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: format!("ddb/{id}.md"),
        updated_at: None,
    }
}

#[test]
fn search_filter_by_type() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "link", vec![]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(1, "link", vec![]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(2, "note", vec![]))
        .unwrap();

    let filters = SearchFilters {
        types: Some(vec!["link".into()]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.total_count, 2);
    for hit in &result.hits {
        assert!(hit.id.starts_with("2026030112000") || hit.id == "20260301120001");
    }
}

#[test]
fn search_filter_by_tag() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec!["rust"]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(1, "note", vec!["python"]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(2, "note", vec!["rust", "cli"]))
        .unwrap();

    let filters = SearchFilters {
        tag: Some("rust".into()),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.total_count, 2);
}

#[test]
fn search_in_query_tag_filter_routes_through_extracted_filters() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec!["rust"]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(1, "note", vec!["python"]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(2, "note", vec!["rust", "cli"]))
        .unwrap();

    let result = idx
        .search_paginated_filtered("tag=rust", 10, 0, &SearchFilters::default())
        .unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.total_count, 2);
    for hit in &result.hits {
        assert!(hit.tags.iter().any(|t| t == "rust"));
    }
}

#[test]
fn search_filter_by_field_eq() {
    let idx = in_memory_index();
    let mut z0 = make_typed_doogat(0, "note", vec![]);
    z0.meta
        .extra
        .insert("status".into(), Value::String("active".into()));
    let mut z1 = make_typed_doogat(1, "note", vec![]);
    z1.meta
        .extra
        .insert("status".into(), Value::String("archived".into()));
    let mut z2 = make_typed_doogat(2, "note", vec![]);
    z2.meta
        .extra
        .insert("status".into(), Value::String("active".into()));

    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "status".into(),
            op: SearchFieldOp::Eq("active".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.total_count, 2);
}

#[test]
fn search_filter_by_field_contains() {
    let idx = in_memory_index();
    let mut z0 = make_typed_doogat(0, "note", vec![]);
    z0.meta
        .extra
        .insert("source".into(), Value::String("Wikipedia article".into()));
    let mut z1 = make_typed_doogat(1, "note", vec![]);
    z1.meta
        .extra
        .insert("source".into(), Value::String("Blog post".into()));

    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "source".into(),
            op: SearchFieldOp::Contains("Wiki".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.total_count, 1);
    assert_eq!(result.hits[0].id, "20260301120000");
}

#[test]
fn search_filter_combined() {
    let idx = in_memory_index();
    let mut z0 = make_typed_doogat(0, "link", vec!["rust"]);
    z0.meta
        .extra
        .insert("status".into(), Value::String("active".into()));
    let mut z1 = make_typed_doogat(1, "link", vec!["python"]);
    z1.meta
        .extra
        .insert("status".into(), Value::String("active".into()));
    let mut z2 = make_typed_doogat(2, "note", vec!["rust"]);
    z2.meta
        .extra
        .insert("status".into(), Value::String("active".into()));
    let mut z3 = make_typed_doogat(3, "link", vec!["rust"]);
    z3.meta
        .extra
        .insert("status".into(), Value::String("archived".into()));

    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();
    idx.index_doogat(&z3).unwrap();

    // type=link AND tag=rust AND status=active → only z0
    let filters = SearchFilters {
        types: Some(vec!["link".into()]),
        tag: Some("rust".into()),
        where_filters: Some(vec![SearchFieldFilter {
            field: "status".into(),
            op: SearchFieldOp::Eq("active".into()),
        }]),
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.total_count, 1);
    assert_eq!(result.hits[0].id, "20260301120000");
}

// ── Materialized column filter resolution tests ───────────────

#[test]
fn search_filter_by_materialized_column_eq() {
    let idx = in_memory_index();

    // Index doogats of type "link" — no extra fields in meta (so nothing in _ddb_fields for "url")
    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    let z2 = make_typed_doogat(2, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    // Create materialized "link" type table with a "url" column
    idx.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS link (
                    id TEXT PRIMARY KEY REFERENCES doogats(id),
                    title TEXT,
                    date TEXT,
                    updated_at TEXT,
                    url TEXT,
                    description TEXT
                );
                INSERT INTO link (id, title, date, url) VALUES ('20260301120000', 'Typed link 0', '2026-03-01', 'https://example.com');
                INSERT INTO link (id, title, date, url) VALUES ('20260301120001', 'Typed link 1', '2026-03-01', 'https://other.org');
                INSERT INTO link (id, title, date, url) VALUES ('20260301120002', 'Typed link 2', '2026-03-01', 'https://example.com');",
            )
            .unwrap();

    // Filter by url = "https://example.com" — should resolve against materialized "link" table
    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "url".into(),
            op: SearchFieldOp::Eq("https://example.com".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        2,
        "should match z0 and z2 via materialized link table"
    );
    assert_eq!(result.total_count, 2);
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120002"]);
}

#[test]
fn search_filter_by_materialized_column_contains() {
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    // Materialized "link" table with url values
    idx.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS link (
                    id TEXT PRIMARY KEY REFERENCES doogats(id),
                    title TEXT,
                    date TEXT,
                    updated_at TEXT,
                    url TEXT
                );
                INSERT INTO link (id, title, date, url) VALUES ('20260301120000', 'Typed link 0', '2026-03-01', 'https://example.com/page');
                INSERT INTO link (id, title, date, url) VALUES ('20260301120001', 'Typed link 1', '2026-03-01', 'https://other.org/page');",
            )
            .unwrap();

    // Contains "example" — should match z0 via materialized table
    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "url".into(),
            op: SearchFieldOp::Contains("example".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        1,
        "should match z0 via materialized link.url LIKE"
    );
    assert_eq!(result.total_count, 1);
    assert_eq!(result.hits[0].id, "20260301120000");
}

#[test]
fn search_filter_materialized_with_type_filter() {
    let idx = in_memory_index();

    // Index a "link" and a "note" doogat
    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "note", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    // Create materialized tables for both types, both having a "url" column
    idx.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS link (
                    id TEXT PRIMARY KEY REFERENCES doogats(id),
                    title TEXT,
                    date TEXT,
                    updated_at TEXT,
                    url TEXT
                );
                INSERT INTO link (id, title, date, url) VALUES ('20260301120000', 'Typed link 0', '2026-03-01', 'https://example.com');

                CREATE TABLE IF NOT EXISTS note (
                    id TEXT PRIMARY KEY REFERENCES doogats(id),
                    title TEXT,
                    date TEXT,
                    updated_at TEXT,
                    url TEXT
                );
                INSERT INTO note (id, title, date, url) VALUES ('20260301120001', 'Typed note 1', '2026-03-01', 'https://example.com');",
            )
            .unwrap();

    // Filter types=["link"] AND url="https://example.com" — should only check "link" table
    let filters = SearchFilters {
        types: Some(vec!["link".into()]),
        tag: None,
        where_filters: Some(vec![SearchFieldFilter {
            field: "url".into(),
            op: SearchFieldOp::Eq("https://example.com".into()),
        }]),
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        1,
        "should only match z0 (link), not z1 (note)"
    );
    assert_eq!(result.total_count, 1);
    assert_eq!(result.hits[0].id, "20260301120000");
}

#[test]
fn search_filter_falls_back_to_ddb_fields() {
    // When no materialized type table has the field, fall back to _ddb_fields
    let idx = in_memory_index();
    let mut z0 = make_typed_doogat(0, "note", vec![]);
    z0.meta
        .extra
        .insert("status".into(), Value::String("active".into()));
    let mut z1 = make_typed_doogat(1, "note", vec![]);
    z1.meta
        .extra
        .insert("status".into(), Value::String("archived".into()));

    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    // No materialized type table exists — "status" is only in _ddb_fields
    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "status".into(),
            op: SearchFieldOp::Eq("active".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        1,
        "should fall back to _ddb_fields for status"
    );
    assert_eq!(result.total_count, 1);
    assert_eq!(result.hits[0].id, "20260301120000");
}

#[test]
fn search_filter_materialized_column_multiple_types() {
    let idx = in_memory_index();

    // Index doogats of two different types
    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "bookmark", vec![]);
    let z2 = make_typed_doogat(2, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    // Create two type tables both with "url" column
    idx.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS link (
                    id TEXT PRIMARY KEY REFERENCES doogats(id),
                    title TEXT,
                    date TEXT,
                    updated_at TEXT,
                    url TEXT
                );
                INSERT INTO link (id, title, date, url) VALUES ('20260301120000', 'Typed link 0', '2026-03-01', 'https://example.com');
                INSERT INTO link (id, title, date, url) VALUES ('20260301120002', 'Typed link 2', '2026-03-01', 'https://other.org');

                CREATE TABLE IF NOT EXISTS bookmark (
                    id TEXT PRIMARY KEY REFERENCES doogats(id),
                    title TEXT,
                    date TEXT,
                    updated_at TEXT,
                    url TEXT
                );
                INSERT INTO bookmark (id, title, date, url) VALUES ('20260301120001', 'Typed bookmark 1', '2026-03-01', 'https://example.com');",
            )
            .unwrap();

    // Filter url="https://example.com" without type restriction — should UNION across both tables
    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "url".into(),
            op: SearchFieldOp::Eq("https://example.com".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        2,
        "should match z0 (link) and z1 (bookmark) via UNION"
    );
    assert_eq!(result.total_count, 2);
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120001"]);
}

// ── Core column where filter tests ──────────────────────────────

#[test]
fn search_filter_by_core_column_title() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec![]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(1, "note", vec![]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(2, "link", vec![]))
        .unwrap();

    // Titles are "Typed note 0", "Typed note 1", "Typed link 2"
    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "title".into(),
            op: SearchFieldOp::Contains("note".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        2,
        "should match titles containing 'note'"
    );
    assert_eq!(result.total_count, 2);
}

#[test]
fn search_filter_by_core_column_date_eq() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec![]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(1, "note", vec![]))
        .unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "date".into(),
            op: SearchFieldOp::Eq("2026-03-01".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        2,
        "should match doogats with date 2026-03-01"
    );
}

// ── Tag via where filter tests ─────────────────────────────────

#[test]
fn search_filter_tag_via_where_eq() {
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "note", vec!["rust"]);
    let z1 = make_typed_doogat(1, "note", vec!["python"]);
    let z2 = make_typed_doogat(2, "note", vec!["rust", "wasm"]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "tag".into(),
            op: SearchFieldOp::Eq("rust".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 2, "should match z0 and z2 tagged 'rust'");
    assert_eq!(result.total_count, 2);
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120002"]);
}

#[test]
fn search_filter_tag_via_where_contains() {
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "note", vec!["javascript"]);
    let z1 = make_typed_doogat(1, "note", vec!["java"]);
    let z2 = make_typed_doogat(2, "note", vec!["python"]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "tag".into(),
            op: SearchFieldOp::Contains("java".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        2,
        "should match z0 ('javascript') and z1 ('java')"
    );
    assert_eq!(result.total_count, 2);
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120001"]);
}

#[test]
fn search_filter_tag_via_where_combined_with_type() {
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec!["rust"]);
    let z1 = make_typed_doogat(1, "note", vec!["rust"]);
    let z2 = make_typed_doogat(2, "link", vec!["python"]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    let filters = SearchFilters {
        types: Some(vec!["link".into()]),
        where_filters: Some(vec![SearchFieldFilter {
            field: "tag".into(),
            op: SearchFieldOp::Eq("rust".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 1, "should only match z0 (link + rust)");
    assert_eq!(result.total_count, 1);
    assert_eq!(result.hits[0].id, "20260301120000");
}

// ── Boolean / phrase search tests ─────────────────────────────

fn make_search_doogat(n: usize, title: &str, body: &str) -> ParsedDoogat {
    let id = format!("2026040112{n:04}");
    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(DoogatId(id.clone())),
            title: Some(title.into()),
            date: Some("2026-04-01".into()),
            doogat_type: None,
            tags: vec![],
            extra: Default::default(),
        },
        body: body.into(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: format!("ddb/{id}.md"),
        updated_at: None,
    }
}

#[test]
fn search_boolean_and() {
    let idx = in_memory_index();
    idx.index_doogat(&make_search_doogat(
        0,
        "Rust CRDT Guide",
        "rust and crdt patterns",
    ))
    .unwrap();
    idx.index_doogat(&make_search_doogat(
        1,
        "Rust Only",
        "rust programming basics",
    ))
    .unwrap();
    idx.index_doogat(&make_search_doogat(
        2,
        "CRDT Only",
        "crdt conflict resolution",
    ))
    .unwrap();

    let results = idx.search("rust AND crdt").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "20260401120000");
}

#[test]
fn search_boolean_or() {
    let idx = in_memory_index();
    idx.index_doogat(&make_search_doogat(0, "Rust Guide", "rust programming"))
        .unwrap();
    idx.index_doogat(&make_search_doogat(1, "Golang Guide", "golang programming"))
        .unwrap();
    idx.index_doogat(&make_search_doogat(2, "Python Guide", "python programming"))
        .unwrap();

    let results = idx.search("rust OR golang").unwrap();
    assert_eq!(results.len(), 2);
    let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"20260401120000"));
    assert!(ids.contains(&"20260401120001"));
}

#[test]
fn search_boolean_not() {
    let idx = in_memory_index();
    idx.index_doogat(&make_search_doogat(0, "Rust CRDT", "rust crdt patterns"))
        .unwrap();
    idx.index_doogat(&make_search_doogat(
        1,
        "Rust Basics",
        "rust programming basics",
    ))
    .unwrap();

    let results = idx.search("rust NOT crdt").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "20260401120001");
}

#[test]
fn search_quoted_phrase() {
    let idx = in_memory_index();
    idx.index_doogat(&make_search_doogat(
        0,
        "Conflict Resolution",
        "conflict resolution strategies",
    ))
    .unwrap();
    idx.index_doogat(&make_search_doogat(
        1,
        "Resolution Conflict",
        "resolution of a conflict",
    ))
    .unwrap();

    // Exact phrase should match only the first
    let results = idx.search("\"conflict resolution\"").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "20260401120000");
}

#[test]
fn search_malformed_fts5_query_returns_bad_request() {
    let idx = in_memory_index();
    idx.index_doogat(&make_search_doogat(0, "Test", "content"))
        .unwrap();

    let err = idx.search("AND AND").unwrap_err();
    match err {
        crate::error::DoogatError::BadRequest(msg) => {
            assert!(
                msg.contains("invalid search query"),
                "expected user-facing message, got: {msg}"
            );
        }
        other => panic!("expected BadRequest, got: {other:?}"),
    }
}

#[test]
fn search_no_filters_unchanged() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec!["rust"]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(1, "link", vec!["python"]))
        .unwrap();

    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &SearchFilters::default())
        .unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.total_count, 2);

    // Should match unfiltered paginated search
    let unfiltered = idx.search_paginated("Searchable", 100, 0).unwrap();
    assert_eq!(result.hits.len(), unfiltered.hits.len());
    assert_eq!(result.total_count, unfiltered.total_count);
}

// ── PRD 00121: in-query field filter alignment ─────────────────

#[test]
fn search_in_query_tag_matches_argument_tag_filter() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec!["prd121-rust"]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(1, "note", vec!["prd121-python"]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(
        2,
        "note",
        vec!["prd121-rust", "prd121-cli"],
    ))
    .unwrap();

    let in_query = idx
        .search_paginated_filtered("tag=prd121-rust", 100, 0, &SearchFilters::default())
        .unwrap();
    let arg = idx
        .search_paginated_filtered(
            "",
            100,
            0,
            &SearchFilters {
                tag: Some("prd121-rust".into()),
                ..Default::default()
            },
        )
        .unwrap();

    let mut in_query_ids: Vec<String> = in_query.hits.iter().map(|h| h.id.clone()).collect();
    let mut arg_ids: Vec<String> = arg.hits.iter().map(|h| h.id.clone()).collect();
    in_query_ids.sort();
    arg_ids.sort();
    assert_eq!(in_query_ids, arg_ids);
    assert_eq!(in_query.total_count, arg.total_count);
}

#[test]
fn search_in_query_field_filter_on_materialized_column() {
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    let z2 = make_typed_doogat(2, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    idx.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS link (
                    id TEXT PRIMARY KEY REFERENCES doogats(id),
                    title TEXT,
                    date TEXT,
                    updated_at TEXT,
                    url TEXT,
                    description TEXT
                );
                INSERT INTO link (id, title, date, url) VALUES ('20260301120000', 'Typed link 0', '2026-03-01', 'https://example.com');
                INSERT INTO link (id, title, date, url) VALUES ('20260301120001', 'Typed link 1', '2026-03-01', 'https://other.org');
                INSERT INTO link (id, title, date, url) VALUES ('20260301120002', 'Typed link 2', '2026-03-01', 'https://example.com');",
            )
            .unwrap();

    let result = idx
        .search_paginated_filtered("url=https://example.com", 100, 0, &SearchFilters::default())
        .unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.total_count, 2);
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120002"]);
}

#[test]
fn search_in_query_filter_combined_with_text() {
    let idx = in_memory_index();

    let mut z0 = make_typed_doogat(0, "note", vec!["combined-rust"]);
    z0.body = "searchable notebook content for rust".into();
    let mut z1 = make_typed_doogat(1, "note", vec!["combined-python"]);
    z1.body = "searchable notebook content for python".into();

    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    let result = idx
        .search_paginated_filtered(
            "notebook tag=combined-rust",
            100,
            0,
            &SearchFilters::default(),
        )
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.total_count, 1);
    assert_eq!(result.hits[0].id, "20260301120000");
}

#[test]
fn search_in_query_filter_intersects_with_argument_where_filter() {
    let idx = in_memory_index();

    let mut z0 = make_typed_doogat(0, "note", vec!["int-rust"]);
    z0.meta
        .extra
        .insert("status".into(), Value::String("active".into()));
    let mut z1 = make_typed_doogat(1, "note", vec!["int-rust"]);
    z1.meta
        .extra
        .insert("status".into(), Value::String("archived".into()));
    let mut z2 = make_typed_doogat(2, "note", vec!["int-rust"]);
    z2.meta
        .extra
        .insert("status".into(), Value::String("active".into()));

    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "status".into(),
            op: SearchFieldOp::Eq("active".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("tag=int-rust", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.total_count, 2);
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120002"]);
}

#[test]
fn search_non_tag_negated_field_filter_returns_bad_request() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec![]))
        .unwrap();
    let err = idx.search("NOT url=example.com").unwrap_err();
    match err {
        crate::error::DoogatError::BadRequest(msg) => {
            assert!(
                msg.contains("NOT") || msg.contains("tag"),
                "expected message to mention NOT/tag limitation, got: {msg}"
            );
        }
        other => panic!("expected BadRequest, got: {other:?}"),
    }
}

#[test]
fn search_tag_negation_still_works() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec!["rust", "archive"]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(1, "note", vec!["rust"]))
        .unwrap();
    // NOT tag=archive should still narrow the result set.
    let result = idx.search("rust NOT tag=archive").unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, "20260301120001");
}

#[test]
fn search_bare_wildcard_returns_bad_request() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec![]))
        .unwrap();

    for q in &["*", "**", ".*"] {
        let err = idx.search(q).unwrap_err();
        assert!(
            !matches!(err, crate::error::DoogatError::Sql(_)),
            "query {q:?} should not return Sql error, got {err:?}"
        );
        match err {
            crate::error::DoogatError::BadRequest(msg) => {
                assert!(
                    msg.contains("invalid search query"),
                    "query {q:?}: expected user-facing message, got: {msg}"
                );
            }
            other => panic!("query {q:?}: expected BadRequest, got: {other:?}"),
        }
    }
}

#[test]
fn search_unbalanced_paren_returns_bad_request() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec![]))
        .unwrap();

    let err = idx.search("(unbalanced").unwrap_err();
    match err {
        crate::error::DoogatError::BadRequest(msg) => {
            assert!(
                msg.contains("invalid search query"),
                "expected user-facing message, got: {msg}"
            );
        }
        other => panic!("expected BadRequest, got: {other:?}"),
    }
}

#[test]
fn search_bare_operator_returns_bad_request() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec![]))
        .unwrap();

    for q in &["AND", "OR", "NOT"] {
        let err = idx.search(q).unwrap_err();
        match err {
            crate::error::DoogatError::BadRequest(msg) => {
                assert!(
                    msg.contains("invalid search query"),
                    "query {q:?}: expected user-facing message, got: {msg}"
                );
            }
            other => panic!("query {q:?}: expected BadRequest, got: {other:?}"),
        }
    }
}

#[test]
fn search_never_returns_sql_error_for_user_input() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec![]))
        .unwrap();

    let inputs = [
        "*",
        "**",
        ".*",
        "(unbalanced",
        "AND",
        "NOT",
        ")",
        "tag=",
        "=",
        ")))bad(((",
        "",
    ];
    for q in &inputs {
        match idx.search(q) {
            Ok(_) => {}
            Err(crate::error::DoogatError::BadRequest(_)) => {}
            Err(crate::error::DoogatError::Sql(msg)) => {
                panic!("query {q:?} returned DoogatError::Sql({msg}); expected BadRequest or Ok");
            }
            Err(other) => panic!("query {q:?} returned unexpected error variant: {other:?}"),
        }
    }
}

#[test]
fn search_empty_query_with_no_filters_returns_bad_request() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec![]))
        .unwrap();

    let err = idx.search("").unwrap_err();
    assert!(
        matches!(err, crate::error::DoogatError::BadRequest(_)),
        "expected BadRequest for empty query with no filters, got: {err:?}"
    );
}

#[test]
fn search_empty_query_with_tag_filter_still_works() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec!["rust"]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(1, "note", vec!["python"]))
        .unwrap();

    let result = idx
        .search_paginated_filtered(
            "",
            100,
            0,
            &SearchFilters {
                tag: Some("rust".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(result.hits.len(), 1);
}

// ── FTS5 schema + _ddb_boost table tests ───────────────────────────

#[test]
fn new_index_fts5_has_fields_column() {
    let idx = in_memory_index();

    // Insert a row with 4 FTS columns: title, body, tags, fields
    idx.conn
        .execute(
            "INSERT INTO _ddb_fts (title, body, tags, fields) VALUES (?1, ?2, ?3, ?4)",
            params!["t", "b", "tag1", "key=val"],
        )
        .expect("FTS5 table should accept 4 columns (title, body, tags, fields)");

    // Read it back to verify the fields column is present
    let fields_val: String = idx
        .conn
        .query_row("SELECT fields FROM _ddb_fts LIMIT 1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(fields_val, "key=val");
}

#[test]
fn new_index_has_ddb_boost_table() {
    let idx = in_memory_index();

    // _ddb_boost table should exist with correct schema
    let table_exists: bool = idx
        .conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='_ddb_boost'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(table_exists, "_ddb_boost table should exist after open");

    // Verify columns: type_name TEXT PK, max_boost REAL NOT NULL DEFAULT 1.0
    idx.conn
        .execute(
            "INSERT INTO _ddb_boost (type_name) VALUES (?1)",
            params!["contact"],
        )
        .expect("should accept insert with only type_name (max_boost has default)");

    let boost: f64 = idx
        .conn
        .query_row(
            "SELECT max_boost FROM _ddb_boost WHERE type_name = 'contact'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        (boost - 1.0).abs() < f64::EPSILON,
        "default max_boost should be 1.0, got {boost}"
    );

    // type_name should be PK (duplicate insert fails)
    let dup = idx.conn.execute(
        "INSERT INTO _ddb_boost (type_name, max_boost) VALUES (?1, ?2)",
        params!["contact", 2.0],
    );
    assert!(
        dup.is_err(),
        "duplicate type_name should violate PK constraint"
    );
}

#[test]
fn upgrade_old_3col_fts_to_4col() {
    // Simulate an OLD database with the 3-column FTS schema
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    conn.execute_batch("PRAGMA busy_timeout=5000;").unwrap();

    // Create old-style tables (3-column FTS, no _ddb_boost)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS doogats (
                id TEXT PRIMARY KEY,
                title TEXT,
                date TEXT,
                type TEXT,
                path TEXT UNIQUE NOT NULL,
                body TEXT,
                updated_at TEXT
            );
            CREATE TABLE IF NOT EXISTS _ddb_tags (
                doogat_id TEXT NOT NULL REFERENCES doogats(id),
                tag TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'frontmatter'
            );
            CREATE TABLE IF NOT EXISTS _ddb_fields (
                doogat_id TEXT NOT NULL REFERENCES doogats(id),
                key TEXT NOT NULL,
                value TEXT,
                zone TEXT
            );
            CREATE TABLE IF NOT EXISTS _ddb_links (
                source_id TEXT NOT NULL REFERENCES doogats(id),
                target_path TEXT NOT NULL,
                display TEXT,
                zone TEXT,
                kind TEXT NOT NULL DEFAULT 'wikilink'
            );
            CREATE TABLE IF NOT EXISTS _ddb_aliases (
                doogat_id TEXT NOT NULL REFERENCES doogats(id),
                alias TEXT COLLATE NOCASE NOT NULL
            );
            CREATE TABLE IF NOT EXISTS _ddb_meta (
                key TEXT PRIMARY KEY,
                value TEXT
            );
            CREATE TABLE IF NOT EXISTS _ddb_attachments (
                doogat_id TEXT NOT NULL REFERENCES doogats(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                mime TEXT,
                size INTEGER,
                path TEXT,
                PRIMARY KEY (doogat_id, name)
            );
            CREATE TABLE IF NOT EXISTS _ddb_checkboxes (
                doogat_id TEXT NOT NULL REFERENCES doogats(id),
                state TEXT NOT NULL CHECK (state IN ('open', 'done', 'info')),
                content TEXT NOT NULL,
                date TEXT,
                due_date TEXT,
                line_number INTEGER,
                indent_level INTEGER DEFAULT 0
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS _ddb_fts USING fts5(
                title, body, tags,
                tokenize = 'porter unicode61'
            );",
    )
    .unwrap();

    // Verify old schema only has 3 FTS columns
    let old_insert = conn.execute(
        "INSERT INTO _ddb_fts (title, body, tags) VALUES ('t', 'b', 'tag')",
        [],
    );
    assert!(old_insert.is_ok(), "old 3-column insert should work");

    // Now run configure_connection which should detect and upgrade
    let idx = Index::configure_connection(conn, None)
        .expect("configure_connection should upgrade old schema");

    // After upgrade: FTS should accept 4 columns
    idx.conn
        .execute(
            "INSERT INTO _ddb_fts (title, body, tags, fields) VALUES (?1, ?2, ?3, ?4)",
            params!["t2", "b2", "tag2", "field_data"],
        )
        .expect("after upgrade, FTS5 should accept 4 columns");

    // After upgrade: _ddb_boost should exist
    let boost_exists: bool = idx
        .conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='_ddb_boost'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(boost_exists, "_ddb_boost should exist after upgrade");
}

#[test]
fn reopening_an_up_to_date_on_disk_index_does_not_rebuild_and_keeps_rows() {
    // AC3: a DB already stamped with the current SCHEMA_VERSION must trigger
    // no drop+recreate on open — rows written before the reopen must survive
    // it. AC4 (fresh-DB leg): the first open of a brand-new file stamps
    // user_version to SCHEMA_VERSION once SCHEMA_DDL has run.
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("index.db");

    let idx = Index::open(&db_path).unwrap();
    let version: i64 = idx
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, 1,
        "a freshly created index must be stamped with the current schema \
         version once SCHEMA_DDL has run"
    );

    idx.conn
        .execute(
            "INSERT INTO _ddb_meta (key, value) VALUES ('sentinel', 'present')",
            [],
        )
        .unwrap();
    drop(idx);

    let idx2 = Index::open(&db_path).unwrap();
    let sentinel: String = idx2
        .conn
        .query_row(
            "SELECT value FROM _ddb_meta WHERE key = 'sentinel'",
            [],
            |row| row.get(0),
        )
        .expect(
            "the row written before the second open must survive it — an \
             unconditional rebuild would drop and recreate the table that \
             held it",
        );
    assert_eq!(sentinel, "present");

    let version2: i64 = idx2
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        version2, 1,
        "reopening an up-to-date index must leave it stamped at the current \
         schema version"
    );
}

#[test]
fn version_mismatch_forces_drop_without_consulting_legacy_probe() {
    // AC2: a `user_version` that is non-zero and does not equal SCHEMA_VERSION
    // must force the drop+recreate unconditionally, even when the legacy
    // FTS-column-text probe would say the schema needs no upgrade (a
    // 4-column FTS table is already present). If the version check deferred
    // to the legacy probe here, the sentinel row below would survive.
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS _ddb_fts USING fts5(
            title, body, tags, fields,
            tokenize = 'porter unicode61'
        );
        CREATE TABLE IF NOT EXISTS _ddb_meta (
            key TEXT PRIMARY KEY,
            value TEXT
        );
        INSERT INTO _ddb_meta (key, value) VALUES ('sentinel', 'present');
        PRAGMA user_version = 2;",
    )
    .unwrap();

    let idx = Index::configure_connection(conn, None)
        .expect("configure_connection should upgrade a version-mismatched schema");

    let sentinel_count: i64 = idx
        .conn
        .query_row(
            "SELECT COUNT(*) FROM _ddb_meta WHERE key = 'sentinel'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        sentinel_count, 0,
        "a user_version of 2 (neither 0 nor SCHEMA_VERSION) must force the \
         drop+recreate unconditionally; the sentinel row survived, which \
         means the legacy FTS probe (which would say this 4-column schema \
         needs no upgrade) was consulted instead of the version stamp"
    );

    let version: i64 = idx
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, 1,
        "after the forced drop+recreate, the schema must be restamped to \
         SCHEMA_VERSION"
    );
}

#[test]
fn unstamped_but_compliant_schema_is_not_dropped_and_gets_stamped() {
    // AC1: user_version == 0 means "unstamped", not "needs upgrade". Every
    // index that exists on a user's disk today is exactly this case:
    // unstamped (0, SQLite's default) AND already 4-column FTS, because the
    // 3-col upgrade shipped long ago. For a 0 stamp the legacy
    // needs_schema_upgrade probe must be consulted, and it must say "no
    // upgrade needed" here. A wrong implementation that reads
    // `0 != SCHEMA_VERSION` as "needs drop" would silently blow away and
    // rebuild the index of every existing user on their next command — the
    // worst regression this task could ship.
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS _ddb_fts USING fts5(
            title, body, tags, fields,
            tokenize = 'porter unicode61'
        );
        CREATE TABLE IF NOT EXISTS _ddb_meta (
            key TEXT PRIMARY KEY,
            value TEXT
        );
        INSERT INTO _ddb_meta (key, value) VALUES ('sentinel', 'present');",
    )
    .unwrap();
    // user_version is deliberately left unset — SQLite defaults it to 0.

    let idx = Index::configure_connection(conn, None).expect(
        "configure_connection should succeed for an unstamped, already-compliant schema",
    );

    let sentinel_count: i64 = idx
        .conn
        .query_row(
            "SELECT COUNT(*) FROM _ddb_meta WHERE key = 'sentinel'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        sentinel_count, 1,
        "an unstamped (user_version == 0) schema that is already 4-column FTS \
         must not be dropped — the legacy probe says no upgrade is needed, but \
         the sentinel row was destroyed anyway, which means the version check \
         forced a drop without consulting it"
    );

    let version: i64 = idx
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, 1,
        "an unstamped but compliant schema must be stamped to SCHEMA_VERSION \
         so the next open does not re-run the legacy probe"
    );
}

#[test]
fn rebuild_produces_new_schema() {
    use crate::traits::mock::MockSource;

    let mut source = MockSource::new();
    let content =
        "---\nid: 20260401120000\ntitle: Rebuild Test\ndate: 2026-04-01\n---\nBody content";
    source
        .files
        .insert("ddb/20260401120000.md".into(), content.into());

    let idx = in_memory_index();
    let _report = idx.rebuild(&source).unwrap();

    // FTS should have the fields column
    idx.conn
        .execute(
            "INSERT INTO _ddb_fts (title, body, tags, fields) VALUES (?1, ?2, ?3, ?4)",
            params!["t", "b", "tags", "extra"],
        )
        .expect("after rebuild, FTS5 should have fields column");

    // _ddb_boost should exist
    let boost_exists: bool = idx
        .conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='_ddb_boost'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(boost_exists, "_ddb_boost should exist after rebuild");
}

// ── Enriched search result tests ────────────────────────────────

#[test]
fn search_enriches_tags_from_frontmatter() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec!["rust", "cli"]))
        .unwrap();

    let hits = idx.search("Searchable").unwrap();
    assert_eq!(hits.len(), 1);
    let mut tags = hits[0].tags.clone();
    tags.sort();
    assert_eq!(tags, vec!["cli", "rust"]);
}

#[test]
fn search_enriches_tags_from_body_hashtags() {
    let idx = in_memory_index();
    let mut z = make_typed_doogat(0, "note", vec!["frontmatter-tag"]);
    z.body_tags = vec!["body-tag".into()];
    idx.index_doogat(&z).unwrap();

    let hits = idx.search("Searchable").unwrap();
    assert_eq!(hits.len(), 1);
    let mut tags = hits[0].tags.clone();
    tags.sort();
    assert_eq!(tags, vec!["body-tag", "frontmatter-tag"]);
}

#[test]
fn search_enriches_doogat_type() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "link", vec![]))
        .unwrap();

    let hits = idx.search("Searchable").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].doogat_type.as_deref(), Some("link"));
}

#[test]
fn search_enriches_fields() {
    let idx = in_memory_index();
    let mut z = make_typed_doogat(0, "link", vec![]);
    z.meta
        .extra
        .insert("url".into(), Value::String("https://example.com".into()));
    z.meta
        .extra
        .insert("description".into(), Value::String("Example site".into()));
    idx.index_doogat(&z).unwrap();

    let hits = idx.search("Searchable").unwrap();
    assert_eq!(hits.len(), 1);
    let fields = hits[0].fields.as_ref().expect("fields should be Some");
    assert_eq!(fields.get("url").unwrap(), "https://example.com");
    assert_eq!(fields.get("description").unwrap(), "Example site");
}

#[test]
fn search_enriches_created_at() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec![]))
        .unwrap();

    let hits = idx.search("Searchable").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].created_at.as_deref(), Some("2026-03-01"));
}

#[test]
fn search_untyped_doogat_has_none_type_and_fields() {
    let idx = in_memory_index();
    let z = ParsedDoogat {
        meta: DoogatMeta {
            id: Some(DoogatId("20260301120000".into())),
            title: Some("Untyped Note".into()),
            date: Some("2026-03-01".into()),
            doogat_type: None,
            tags: vec![],
            extra: Default::default(),
        },
        body: "Searchable untyped content".into(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: "ddb/20260301120000.md".into(),
        updated_at: None,
    };
    idx.index_doogat(&z).unwrap();

    let hits = idx.search("untyped").unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].doogat_type.is_none());
    assert!(hits[0].fields.is_none());
    assert!(hits[0].tags.is_empty());
}

#[test]
fn search_paginated_also_enriches() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "link", vec!["rust"]))
        .unwrap();

    let result = idx.search_paginated("Searchable", 10, 0).unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].tags, vec!["rust"]);
    assert_eq!(result.hits[0].doogat_type.as_deref(), Some("link"));
}

// ── FTS negation tests ────────────────────────────────────────────

#[test]
fn search_negation_positive_not_negative() {
    let idx = in_memory_index();
    idx.index_doogat(&make_search_doogat(
        0,
        "Important Meeting",
        "important meeting notes",
    ))
    .unwrap();
    idx.index_doogat(&make_search_doogat(
        1,
        "Important Design",
        "important design review",
    ))
    .unwrap();
    idx.index_doogat(&make_search_doogat(
        2,
        "Meeting Only",
        "weekly meeting agenda",
    ))
    .unwrap();

    let results = idx.search("important NOT meeting").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "20260401120001");
}

#[test]
fn search_negation_all_negative() {
    let idx = in_memory_index();
    idx.index_doogat(&make_search_doogat(
        0,
        "Archive Note",
        "archive old content",
    ))
    .unwrap();
    idx.index_doogat(&make_search_doogat(
        1,
        "Active Note",
        "active current content",
    ))
    .unwrap();
    idx.index_doogat(&make_search_doogat(2, "Another Active", "fresh content"))
        .unwrap();

    let results = idx.search("NOT archive").unwrap();
    assert_eq!(results.len(), 2);
    let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"20260401120001"));
    assert!(ids.contains(&"20260401120002"));
}

#[test]
fn search_negation_multiple_nots() {
    let idx = in_memory_index();
    idx.index_doogat(&make_search_doogat(
        0,
        "Rust CRDT Design",
        "rust crdt design patterns",
    ))
    .unwrap();
    idx.index_doogat(&make_search_doogat(
        1,
        "Rust Basics",
        "rust programming basics",
    ))
    .unwrap();
    idx.index_doogat(&make_search_doogat(
        2,
        "CRDT Theory",
        "crdt conflict resolution",
    ))
    .unwrap();
    idx.index_doogat(&make_search_doogat(
        3,
        "Design Review",
        "design review notes",
    ))
    .unwrap();

    let results = idx.search("rust NOT crdt NOT design").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "20260401120001");
}

#[test]
fn search_negation_ranking_based_on_positive_only() {
    let idx = in_memory_index();
    // Doc with "important" in title and body should rank higher
    idx.index_doogat(&make_search_doogat(
        0,
        "Important Notes",
        "important important important stuff",
    ))
    .unwrap();
    // Doc with "important" only once
    idx.index_doogat(&make_search_doogat(1, "Some Notes", "important stuff"))
        .unwrap();
    // Doc that should be excluded
    idx.index_doogat(&make_search_doogat(
        2,
        "Meeting Archive",
        "important meeting archive",
    ))
    .unwrap();

    let results = idx.search("important NOT meeting").unwrap();
    assert_eq!(results.len(), 2);
    let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"20260401120000"));
    assert!(ids.contains(&"20260401120001"));
    assert!(!ids.contains(&"20260401120002"));
}

#[test]
fn search_negation_with_tag_filter() {
    let idx = in_memory_index();
    idx.index_doogat(&make_typed_doogat(0, "note", vec!["archive"]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(1, "note", vec!["active"]))
        .unwrap();
    idx.index_doogat(&make_typed_doogat(2, "note", vec!["archive", "pinned"]))
        .unwrap();

    let results = idx.search("NOT tag=archive").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "20260301120001");
}

#[test]
fn search_negation_positive_with_not_tag() {
    let idx = in_memory_index();
    let mut d0 = make_typed_doogat(0, "note", vec!["archive"]);
    d0.body = "searchable important content".into();
    let mut d1 = make_typed_doogat(1, "note", vec!["active"]);
    d1.body = "searchable important content".into();
    let mut d2 = make_typed_doogat(2, "note", vec!["archive"]);
    d2.body = "searchable other content".into();

    idx.index_doogat(&d0).unwrap();
    idx.index_doogat(&d1).unwrap();
    idx.index_doogat(&d2).unwrap();

    let results = idx.search("important NOT tag=archive").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "20260301120001");
}

#[test]
fn search_negation_no_results_when_all_excluded() {
    let idx = in_memory_index();
    idx.index_doogat(&make_search_doogat(0, "Meeting One", "meeting agenda"))
        .unwrap();
    idx.index_doogat(&make_search_doogat(1, "Meeting Two", "meeting notes"))
        .unwrap();

    let results = idx.search("NOT meeting").unwrap();
    assert_eq!(results.len(), 0);
}

#[test]
fn search_negation_paginated_total_count_correct() {
    let idx = in_memory_index();
    idx.index_doogat(&make_search_doogat(0, "Important A", "important alpha"))
        .unwrap();
    idx.index_doogat(&make_search_doogat(1, "Important B", "important beta"))
        .unwrap();
    idx.index_doogat(&make_search_doogat(2, "Important C", "important gamma"))
        .unwrap();
    idx.index_doogat(&make_search_doogat(3, "Meeting X", "important meeting"))
        .unwrap();

    let result = idx.search_paginated("important NOT meeting", 2, 0).unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.total_count, 3);

    // Second page
    let result2 = idx.search_paginated("important NOT meeting", 2, 2).unwrap();
    assert_eq!(result2.hits.len(), 1);
    assert_eq!(result2.total_count, 3);
}

#[test]
fn search_negation_all_negative_paginated() {
    let idx = in_memory_index();
    idx.index_doogat(&make_search_doogat(0, "Archive Doc", "archive stuff"))
        .unwrap();
    idx.index_doogat(&make_search_doogat(1, "Active One", "active content"))
        .unwrap();
    idx.index_doogat(&make_search_doogat(2, "Active Two", "active things"))
        .unwrap();

    let result = idx.search_paginated("NOT archive", 10, 0).unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.total_count, 2);
}

#[test]
fn search_negation_with_type_filter() {
    let idx = in_memory_index();
    let mut d0 = make_typed_doogat(0, "note", vec![]);
    d0.body = "searchable content about rust".into();
    let mut d1 = make_typed_doogat(1, "link", vec![]);
    d1.body = "searchable content about rust".into();
    let mut d2 = make_typed_doogat(2, "note", vec![]);
    d2.body = "searchable content about rust meetings".into();

    idx.index_doogat(&d0).unwrap();
    idx.index_doogat(&d1).unwrap();
    idx.index_doogat(&d2).unwrap();

    let filters = SearchFilters {
        types: Some(vec!["note".into()]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("rust NOT meetings", 10, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].id, "20260301120000");
    assert_eq!(result.total_count, 1);
}

#[test]
fn search_negation_nested_not_and() {
    let idx = in_memory_index();
    idx.index_doogat(&make_search_doogat(0, "Rust CRDT", "rust crdt guide"))
        .unwrap();
    idx.index_doogat(&make_search_doogat(1, "Rust Only", "rust programming"))
        .unwrap();
    idx.index_doogat(&make_search_doogat(2, "CRDT Only", "crdt patterns"))
        .unwrap();
    idx.index_doogat(&make_search_doogat(3, "Other", "unrelated content"))
        .unwrap();

    // NOT(AND(rust, crdt)) is an all-negative query with a compound negated term.
    // extract_negations returns (None, [And(rust, crdt)]).
    // The compound term becomes a single FTS MATCH subquery: "rust AND crdt".
    // Only docs matching BOTH rust AND crdt are excluded.
    let results = idx.search("NOT (rust AND crdt)").unwrap();
    assert_eq!(results.len(), 3);
    let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
    assert!(!ids.contains(&"20260401120000")); // "Rust CRDT" excluded
    assert!(ids.contains(&"20260401120001")); // "Rust Only" kept
    assert!(ids.contains(&"20260401120002")); // "CRDT Only" kept
    assert!(ids.contains(&"20260401120003")); // "Other" kept
}

#[test]
fn search_negation_stemming() {
    let idx = in_memory_index();
    // "meeting" and "meetings" share stem "meet" via porter stemmer
    idx.index_doogat(&make_search_doogat(
        0,
        "Important Design",
        "important design review",
    ))
    .unwrap();
    idx.index_doogat(&make_search_doogat(
        1,
        "Meeting Notes",
        "important meeting notes",
    ))
    .unwrap();
    idx.index_doogat(&make_search_doogat(
        2,
        "Meetings List",
        "important meetings summary",
    ))
    .unwrap();

    // Negating "meetings" (plural) should also exclude doc with "meeting" (singular)
    let results = idx.search("important NOT meetings").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "20260401120000");
}

#[test]
fn search_negation_all_negative_with_tag_and_type_filter() {
    let idx = in_memory_index();
    let mut d0 = make_typed_doogat(0, "note", vec!["rust"]);
    d0.body = "archive old content".into();
    let mut d1 = make_typed_doogat(1, "note", vec!["rust"]);
    d1.body = "active current content".into();
    let mut d2 = make_typed_doogat(2, "link", vec!["rust"]);
    d2.body = "active link content".into();

    idx.index_doogat(&d0).unwrap();
    idx.index_doogat(&d1).unwrap();
    idx.index_doogat(&d2).unwrap();

    // All-negative with type + tag filters (2+ filter params)
    let filters = SearchFilters {
        types: Some(vec!["note".into()]),
        tag: Some("rust".into()),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("NOT archive", 10, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].id, "20260301120001");
    assert_eq!(result.total_count, 1);
}

#[test]
fn search_negation_all_negative_with_type_filter() {
    let idx = in_memory_index();
    let mut d0 = make_typed_doogat(0, "note", vec![]);
    d0.body = "archive old content".into();
    let mut d1 = make_typed_doogat(1, "note", vec![]);
    d1.body = "active current content".into();
    let mut d2 = make_typed_doogat(2, "link", vec![]);
    d2.body = "active link content".into();

    idx.index_doogat(&d0).unwrap();
    idx.index_doogat(&d1).unwrap();
    idx.index_doogat(&d2).unwrap();

    let filters = SearchFilters {
        types: Some(vec!["note".into()]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("NOT archive", 10, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].id, "20260301120001");
    assert_eq!(result.total_count, 1);
}

// ── In operator tests ─────────────────────────────

#[test]
fn search_filter_tag_via_where_in() {
    // tags "rust", "svelte", "python" - In ["rust", "svelte"] returns 2
    let idx = in_memory_index();
    let z0 = make_typed_doogat(0, "note", vec!["rust"]);
    let z1 = make_typed_doogat(1, "note", vec!["svelte"]);
    let z2 = make_typed_doogat(2, "note", vec!["python"]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "tag".into(),
            op: SearchFieldOp::In(vec!["rust".into(), "svelte".into()]),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        2,
        "should match z0 (rust) and z1 (svelte)"
    );
    assert_eq!(result.total_count, 2);
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120001"]);
}

#[test]
fn search_filter_core_column_in() {
    // type In ["link", "note"] should match both types
    let idx = in_memory_index();
    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "note", vec![]);
    let z2 = make_typed_doogat(2, "contact", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "type".into(),
            op: SearchFieldOp::In(vec!["link".into(), "note".into()]),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        2,
        "should match link and note, not contact"
    );
    assert_eq!(result.total_count, 2);
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120001"]);
}

#[test]
fn search_filter_fields_fallback_in() {
    // field not in any type table - uses _ddb_fields fallback
    let idx = in_memory_index();
    let mut z0 = make_typed_doogat(0, "note", vec![]);
    z0.meta
        .extra
        .insert("color".into(), Value::String("red".into()));
    let mut z1 = make_typed_doogat(1, "note", vec![]);
    z1.meta
        .extra
        .insert("color".into(), Value::String("blue".into()));
    let mut z2 = make_typed_doogat(2, "note", vec![]);
    z2.meta
        .extra
        .insert("color".into(), Value::String("green".into()));
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "color".into(),
            op: SearchFieldOp::In(vec!["red".into(), "green".into()]),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 2, "should match red and green, not blue");
    assert_eq!(result.total_count, 2);
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120002"]);
}

#[test]
fn search_filter_in_empty_returns_no_results() {
    let idx = in_memory_index();
    let z0 = make_typed_doogat(0, "note", vec!["rust"]);
    idx.index_doogat(&z0).unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "tag".into(),
            op: SearchFieldOp::In(vec![]),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 0, "empty In list should match nothing");
    assert_eq!(result.total_count, 0);
}

#[test]
fn search_filter_in_single_element() {
    let idx = in_memory_index();
    let z0 = make_typed_doogat(0, "note", vec!["rust"]);
    let z1 = make_typed_doogat(1, "note", vec!["python"]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "tag".into(),
            op: SearchFieldOp::In(vec!["rust".into()]),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        1,
        "single-element In should work like Eq"
    );
    assert_eq!(result.total_count, 1);
    assert_eq!(result.hits[0].id, "20260301120000");
}

#[test]
fn search_filter_materialized_column_in() {
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    let z2 = make_typed_doogat(2, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    // Create materialized "link" type table with a "url" column
    idx.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS link (
                    id TEXT PRIMARY KEY REFERENCES doogats(id),
                    title TEXT,
                    date TEXT,
                    updated_at TEXT,
                    url TEXT
                );
                INSERT INTO link (id, title, date, url) VALUES ('20260301120000', 'Link 0', '2026-03-01', 'https://github.com');
                INSERT INTO link (id, title, date, url) VALUES ('20260301120001', 'Link 1', '2026-03-01', 'https://gitlab.com');
                INSERT INTO link (id, title, date, url) VALUES ('20260301120002', 'Link 2', '2026-03-01', 'https://sr.ht');",
            )
            .unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "url".into(),
            op: SearchFieldOp::In(vec!["https://github.com".into(), "https://sr.ht".into()]),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        2,
        "should match z0 and z2 via materialized link table"
    );
    assert_eq!(result.total_count, 2);
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120002"]);
}

#[test]
fn search_filter_by_junction_eq() {
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    // Create type table + junction table; link z0 to cat001
    idx.conn
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS link (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT\
                 );\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120000', 'Link 0', '2026-03-01');\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120001', 'Link 1', '2026-03-01');\
                 CREATE TABLE IF NOT EXISTS link_category (\
                     \"link_id\" TEXT NOT NULL, \
                     \"category_id\" TEXT NOT NULL, \
                     PRIMARY KEY (\"link_id\", \"category_id\")\
                 );\
                 INSERT INTO link_category VALUES ('20260301120000', 'cat001');",
        )
        .unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "category".into(),
            op: SearchFieldOp::Eq("cat001".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 1, "junction Eq should match z0 only");
    assert_eq!(result.hits[0].id, "20260301120000");
}

#[test]
fn search_filter_by_junction_contains() {
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    // Insert category doogats so title-based Contains can join them
    idx.conn
        .execute_batch(
            "INSERT INTO doogats (id, title, type, path) \
                     VALUES ('cat001', 'Technology Hub', 'category', 'ddb/cat001.md');\
                 INSERT INTO doogats (id, title, type, path) \
                     VALUES ('cat002', 'Science Corner', 'category', 'ddb/cat002.md');\
                 CREATE TABLE IF NOT EXISTS link (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT\
                 );\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120000', 'Link 0', '2026-03-01');\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120001', 'Link 1', '2026-03-01');\
                 CREATE TABLE IF NOT EXISTS link_category (\
                     \"link_id\" TEXT NOT NULL, \
                     \"category_id\" TEXT NOT NULL, \
                     PRIMARY KEY (\"link_id\", \"category_id\")\
                 );\
                 INSERT INTO link_category VALUES ('20260301120000', 'cat001');\
                 INSERT INTO link_category VALUES ('20260301120001', 'cat002');",
        )
        .unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "category".into(),
            op: SearchFieldOp::Contains("Tech".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        1,
        "junction Contains should match z0 via 'Technology Hub' title"
    );
    assert_eq!(result.hits[0].id, "20260301120000");
}

#[test]
fn search_filter_by_junction_in() {
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    let z2 = make_typed_doogat(2, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    idx.conn
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS link (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT\
                 );\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120000', 'Link 0', '2026-03-01');\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120001', 'Link 1', '2026-03-01');\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120002', 'Link 2', '2026-03-01');\
                 CREATE TABLE IF NOT EXISTS link_category (\
                     \"link_id\" TEXT NOT NULL, \
                     \"category_id\" TEXT NOT NULL, \
                     PRIMARY KEY (\"link_id\", \"category_id\")\
                 );\
                 INSERT INTO link_category VALUES ('20260301120000', 'cat001');\
                 INSERT INTO link_category VALUES ('20260301120001', 'cat002');\
                 INSERT INTO link_category VALUES ('20260301120002', 'cat003');",
        )
        .unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "category".into(),
            op: SearchFieldOp::In(vec!["cat001".into(), "cat002".into()]),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 2, "junction In should match z0 and z1");
    assert_eq!(result.total_count, 2);
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120001"]);
}

#[test]
fn search_filter_by_junction_contains_with_materialized_column() {
    // Exercises the else branch: materialized `link` table HAS a `category` column
    // (as real REFERENCES columns do), so tables_with_field is NOT empty.
    // Contains should still route through the junction JOIN to match on title,
    // not the raw ID stored in the materialized column.
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    idx.conn
        .execute_batch(
            "INSERT INTO doogats (id, title, type, path) \
                     VALUES ('cat001', 'Technology Hub', 'category', 'ddb/cat001.md');\
                 INSERT INTO doogats (id, title, type, path) \
                     VALUES ('cat002', 'Science Corner', 'category', 'ddb/cat002.md');\
                 CREATE TABLE IF NOT EXISTS link (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT, \
                     \"category\" TEXT\
                 );\
                 INSERT OR REPLACE INTO link (id, title, date, \"category\") \
                     VALUES ('20260301120000', 'Link 0', '2026-03-01', 'cat001');\
                 INSERT OR REPLACE INTO link (id, title, date, \"category\") \
                     VALUES ('20260301120001', 'Link 1', '2026-03-01', 'cat002');\
                 CREATE TABLE IF NOT EXISTS link_category (\
                     \"link_id\" TEXT NOT NULL, \
                     \"category_id\" TEXT NOT NULL, \
                     PRIMARY KEY (\"link_id\", \"category_id\")\
                 );\
                 INSERT INTO link_category VALUES ('20260301120000', 'cat001');\
                 INSERT INTO link_category VALUES ('20260301120001', 'cat002');",
        )
        .unwrap();

    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "category".into(),
            op: SearchFieldOp::Contains("Tech".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        1,
        "junction Contains with materialized column should match z0 via title JOIN, got: {:?}",
        result.hits.iter().map(|h| &h.id).collect::<Vec<_>>()
    );
    assert_eq!(result.hits[0].id, "20260301120000");
}

#[test]
fn search_filter_by_junction_union_multi_type() {
    // When two different types both have a junction table for the same field,
    // build_filter_clauses should generate a UNION across both junction tables
    // so items from either type are matched.
    let idx = in_memory_index();

    // link 0 and article 1 will be linked to cat001; link 2 will not
    let link0 = make_typed_doogat(0, "link", vec![]);
    let article1 = make_typed_doogat(1, "article", vec![]);
    let link2 = make_typed_doogat(2, "link", vec![]);
    idx.index_doogat(&link0).unwrap();
    idx.index_doogat(&article1).unwrap();
    idx.index_doogat(&link2).unwrap();

    idx.conn
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS link (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT\
                 );\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120000', 'Link 0', '2026-03-01');\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120002', 'Link 2', '2026-03-01');\
                 CREATE TABLE IF NOT EXISTS article (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT\
                 );\
                 INSERT OR REPLACE INTO article (id, title, date) \
                     VALUES ('20260301120001', 'Article 1', '2026-03-01');\
                 CREATE TABLE IF NOT EXISTS link_category (\
                     \"link_id\" TEXT NOT NULL, \
                     \"category_id\" TEXT NOT NULL, \
                     PRIMARY KEY (\"link_id\", \"category_id\")\
                 );\
                 CREATE TABLE IF NOT EXISTS article_category (\
                     \"article_id\" TEXT NOT NULL, \
                     \"category_id\" TEXT NOT NULL, \
                     PRIMARY KEY (\"article_id\", \"category_id\")\
                 );\
                 INSERT INTO link_category VALUES ('20260301120000', 'cat001');\
                 INSERT INTO article_category VALUES ('20260301120001', 'cat001');",
        )
        .unwrap();

    // No types filter: both link and article junction tables should be searched via UNION
    let filters = SearchFilters {
        where_filters: Some(vec![SearchFieldFilter {
            field: "category".into(),
            op: SearchFieldOp::Eq("cat001".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        2,
        "junction UNION should match link0 and article1 across both type tables"
    );
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120001"]);
}

// --- ddb#15 follow-up: user-defined membership tables and `<field>_id` self-route ---

#[test]
fn search_filter_by_user_junction_contains() {
    // Jink-shape membership: a user-defined `category-membership` table
    // carries `link_id` + `category_id` instead of the auto-junction
    // `link_category` shape. `category=Tech` with types=["link"] must
    // route through the membership table by JOINing to category title.
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    idx.conn
        .execute_batch(
            "INSERT INTO doogats (id, title, type, path) \
                     VALUES ('cat001', 'Technology Hub', 'category', 'ddb/cat001.md');\
                 INSERT INTO doogats (id, title, type, path) \
                     VALUES ('cat002', 'Science Corner', 'category', 'ddb/cat002.md');\
                 CREATE TABLE IF NOT EXISTS link (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT, \
                     url TEXT\
                 );\
                 INSERT OR REPLACE INTO link (id, title, date, url) \
                     VALUES ('20260301120000', 'Link 0', '2026-03-01', 'https://a');\
                 INSERT OR REPLACE INTO link (id, title, date, url) \
                     VALUES ('20260301120001', 'Link 1', '2026-03-01', 'https://b');\
                 CREATE TABLE IF NOT EXISTS category (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT\
                 );\
                 INSERT OR REPLACE INTO category (id, title, date) \
                     VALUES ('cat001', 'Technology Hub', '2026-03-01');\
                 INSERT OR REPLACE INTO category (id, title, date) \
                     VALUES ('cat002', 'Science Corner', '2026-03-01');\
                 CREATE TABLE IF NOT EXISTS \"category-membership\" (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT, \
                     \"link_id\" TEXT NOT NULL, \"category_id\" TEXT NOT NULL\
                 );\
                 INSERT INTO \"category-membership\" (id, title, link_id, category_id) \
                     VALUES ('m1', 'Link 0 in Technology Hub', '20260301120000', 'cat001');\
                 INSERT INTO \"category-membership\" (id, title, link_id, category_id) \
                     VALUES ('m2', 'Link 1 in Science Corner', '20260301120001', 'cat002');",
        )
        .unwrap();

    let filters = SearchFilters {
        types: Some(vec!["link".into()]),
        where_filters: Some(vec![SearchFieldFilter {
            field: "category".into(),
            op: SearchFieldOp::Contains("Tech".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        1,
        "user-junction Contains should match link0 via Technology Hub title, got: {:?}",
        result.hits.iter().map(|h| &h.id).collect::<Vec<_>>()
    );
    assert_eq!(result.hits[0].id, "20260301120000");
}

#[test]
fn search_filter_by_user_junction_eq() {
    // Same membership shape, Eq op: filter by raw category id.
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    idx.conn
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS link (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT\
                 );\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120000', 'Link 0', '2026-03-01');\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120001', 'Link 1', '2026-03-01');\
                 CREATE TABLE IF NOT EXISTS \"category-membership\" (\
                     id TEXT PRIMARY KEY, title TEXT, \
                     \"link_id\" TEXT NOT NULL, \"category_id\" TEXT NOT NULL\
                 );\
                 INSERT INTO \"category-membership\" (id, title, link_id, category_id) \
                     VALUES ('m1', 'membership 1', '20260301120000', 'cat001');\
                 INSERT INTO \"category-membership\" (id, title, link_id, category_id) \
                     VALUES ('m2', 'membership 2', '20260301120001', 'cat002');",
        )
        .unwrap();

    let filters = SearchFilters {
        types: Some(vec!["link".into()]),
        where_filters: Some(vec![SearchFieldFilter {
            field: "category".into(),
            op: SearchFieldOp::Eq("cat001".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(result.hits.len(), 1, "user-junction Eq should match link0");
    assert_eq!(result.hits[0].id, "20260301120000");
}

#[test]
fn search_filter_by_user_junction_in() {
    // Same membership shape, In op.
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    let z2 = make_typed_doogat(2, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();
    idx.index_doogat(&z2).unwrap();

    idx.conn
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS link (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT\
                 );\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120000', 'Link 0', '2026-03-01');\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120001', 'Link 1', '2026-03-01');\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120002', 'Link 2', '2026-03-01');\
                 CREATE TABLE IF NOT EXISTS \"category-membership\" (\
                     id TEXT PRIMARY KEY, title TEXT, \
                     \"link_id\" TEXT NOT NULL, \"category_id\" TEXT NOT NULL\
                 );\
                 INSERT INTO \"category-membership\" (id, title, link_id, category_id) \
                     VALUES ('m1', 'membership 1', '20260301120000', 'cat001');\
                 INSERT INTO \"category-membership\" (id, title, link_id, category_id) \
                     VALUES ('m2', 'membership 2', '20260301120001', 'cat002');\
                 INSERT INTO \"category-membership\" (id, title, link_id, category_id) \
                     VALUES ('m3', 'membership 3', '20260301120002', 'cat003');",
        )
        .unwrap();

    let filters = SearchFilters {
        types: Some(vec!["link".into()]),
        where_filters: Some(vec![SearchFieldFilter {
            field: "category".into(),
            op: SearchFieldOp::In(vec!["cat001".into(), "cat002".into()]),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        2,
        "user-junction In should match link0 and link1"
    );
    let mut ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["20260301120000", "20260301120001"]);
}

#[test]
fn search_filter_self_route_field_id_contains() {
    // Self-route: the type table itself carries `<field>_id` (no separate
    // junction). `category=Tech` with types=["link"] must JOIN through
    // link.category_id to the category typedef title. Covers the
    // proposal's literal Case 2 in ddb#15 follow-up.
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    idx.conn
        .execute_batch(
            "INSERT INTO doogats (id, title, type, path) \
                     VALUES ('cat001', 'Technology Hub', 'category', 'ddb/cat001.md');\
                 INSERT INTO doogats (id, title, type, path) \
                     VALUES ('cat002', 'Science Corner', 'category', 'ddb/cat002.md');\
                 CREATE TABLE IF NOT EXISTS link (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT, \
                     \"category_id\" TEXT\
                 );\
                 INSERT OR REPLACE INTO link (id, title, date, category_id) \
                     VALUES ('20260301120000', 'Link 0', '2026-03-01', 'cat001');\
                 INSERT OR REPLACE INTO link (id, title, date, category_id) \
                     VALUES ('20260301120001', 'Link 1', '2026-03-01', 'cat002');",
        )
        .unwrap();

    let filters = SearchFilters {
        types: Some(vec!["link".into()]),
        where_filters: Some(vec![SearchFieldFilter {
            field: "category".into(),
            op: SearchFieldOp::Contains("Tech".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        1,
        "self-route Contains should match link0 via category_id JOIN"
    );
    assert_eq!(result.hits[0].id, "20260301120000");
}

// --- ddb#15 follow-up #2: SET SEARCH KEY redirects path-(a) match column ---

#[test]
fn search_filter_user_junction_contains_uses_search_key() {
    // Jink-shape: category typedef has fqn="work.portals" but title="Portals".
    // After `ALTER TABLE category SET SEARCH KEY fqn`, refresh_boost_table
    // writes `_ddb_meta(key='search_key:category', value='fqn')`.
    // FK-route Contains must JOIN through the typed `category` table on
    // its `fqn` column (not `doogats.title`), so `category=work.portals`
    // hits link0 instead of returning 0.
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    let z1 = make_typed_doogat(1, "link", vec![]);
    idx.index_doogat(&z0).unwrap();
    idx.index_doogat(&z1).unwrap();

    idx.conn
        .execute_batch(
            "INSERT INTO doogats (id, title, type, path) \
                     VALUES ('cat001', 'Marketing Hub', 'category', 'ddb/cat001.md');\
                 INSERT INTO doogats (id, title, type, path) \
                     VALUES ('cat002', 'Notes', 'category', 'ddb/cat002.md');\
                 CREATE TABLE IF NOT EXISTS link (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT\
                 );\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120000', 'Link 0', '2026-03-01');\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120001', 'Link 1', '2026-03-01');\
                 CREATE TABLE IF NOT EXISTS category (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT, \
                     fqn TEXT, space TEXT\
                 );\
                 INSERT OR REPLACE INTO category (id, title, date, fqn, space) \
                     VALUES ('cat001', 'Marketing Hub', '2026-03-01', 'work.portals', 'work');\
                 INSERT OR REPLACE INTO category (id, title, date, fqn, space) \
                     VALUES ('cat002', 'Notes', '2026-03-01', 'home.notes', 'home');\
                 CREATE TABLE IF NOT EXISTS \"category-membership\" (\
                     id TEXT PRIMARY KEY, title TEXT, \
                     \"link_id\" TEXT NOT NULL, \"category_id\" TEXT NOT NULL\
                 );\
                 INSERT INTO \"category-membership\" (id, title, link_id, category_id) \
                     VALUES ('m1', 'Link 0 in work.portals', '20260301120000', 'cat001');\
                 INSERT INTO \"category-membership\" (id, title, link_id, category_id) \
                     VALUES ('m2', 'Link 1 in home.notes', '20260301120001', 'cat002');\
                 INSERT OR REPLACE INTO _ddb_meta (key, value) \
                     VALUES ('search_key:category', 'fqn');",
        )
        .unwrap();

    let filters = SearchFilters {
        types: Some(vec!["link".into()]),
        where_filters: Some(vec![SearchFieldFilter {
            field: "category".into(),
            op: SearchFieldOp::Contains("work.portals".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        1,
        "search_key=fqn should make `category=work.portals` match link0"
    );
    assert_eq!(result.hits[0].id, "20260301120000");

    // The reverse query — matching by title under the FQN search key —
    // should NOT find anything: the typedef opted in to fqn matching.
    // "Marketing" appears only in the title, never in fqn.
    let filters_title = SearchFilters {
        types: Some(vec!["link".into()]),
        where_filters: Some(vec![SearchFieldFilter {
            field: "category".into(),
            op: SearchFieldOp::Contains("Marketing".into()),
        }]),
        ..Default::default()
    };
    let result_title = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters_title)
        .unwrap();
    assert_eq!(
        result_title.hits.len(),
        0,
        "with search_key=fqn, title-only Contains must miss"
    );
}

#[test]
fn search_filter_user_junction_contains_default_title_when_unset() {
    // Without SET SEARCH KEY, FK-route Contains keeps the legacy
    // `JOIN doogats … WHERE d.title LIKE` behaviour. Identical setup
    // to the previous test minus the `_ddb_meta` row.
    let idx = in_memory_index();

    let z0 = make_typed_doogat(0, "link", vec![]);
    idx.index_doogat(&z0).unwrap();

    idx.conn
        .execute_batch(
            "INSERT INTO doogats (id, title, type, path) \
                     VALUES ('cat001', 'Portals', 'category', 'ddb/cat001.md');\
                 CREATE TABLE IF NOT EXISTS link (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT\
                 );\
                 INSERT OR REPLACE INTO link (id, title, date) \
                     VALUES ('20260301120000', 'Link 0', '2026-03-01');\
                 CREATE TABLE IF NOT EXISTS category (\
                     id TEXT PRIMARY KEY, title TEXT, date TEXT, updated_at TEXT, \
                     fqn TEXT\
                 );\
                 INSERT OR REPLACE INTO category (id, title, date, fqn) \
                     VALUES ('cat001', 'Portals', '2026-03-01', 'work.portals');\
                 CREATE TABLE IF NOT EXISTS \"category-membership\" (\
                     id TEXT PRIMARY KEY, title TEXT, \
                     \"link_id\" TEXT NOT NULL, \"category_id\" TEXT NOT NULL\
                 );\
                 INSERT INTO \"category-membership\" (id, title, link_id, category_id) \
                     VALUES ('m1', 'Link 0 in Portals', '20260301120000', 'cat001');",
        )
        .unwrap();

    let filters = SearchFilters {
        types: Some(vec!["link".into()]),
        where_filters: Some(vec![SearchFieldFilter {
            field: "category".into(),
            op: SearchFieldOp::Contains("Portals".into()),
        }]),
        ..Default::default()
    };
    let result = idx
        .search_paginated_filtered("Searchable", 100, 0, &filters)
        .unwrap();
    assert_eq!(
        result.hits.len(),
        1,
        "default behaviour: title-based match must still hit link0"
    );
}

#[test]
fn with_immediate_transaction_commits_on_ok() {
    let idx = in_memory_index();
    idx.conn
        .execute("CREATE TABLE wit_t (x INTEGER)", [])
        .unwrap();

    let result = with_immediate_transaction(&idx.conn, || {
        idx.conn
            .execute("INSERT INTO wit_t (x) VALUES (1)", [])
            .unwrap();
        Ok(())
    });
    assert!(result.is_ok());
    assert!(
        idx.conn.is_autocommit(),
        "transaction must be properly closed by COMMIT"
    );

    let count: i64 = idx
        .conn
        .query_row("SELECT COUNT(*) FROM wit_t", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "row inserted in an Ok closure must be committed");
}

#[test]
fn with_immediate_transaction_rolls_back_on_err() {
    let idx = in_memory_index();
    idx.conn
        .execute("CREATE TABLE wit_t (x INTEGER)", [])
        .unwrap();

    // Use a distinctive `Sql` variant (distinct from `Validation`) with a
    // unique payload no implementation could plausibly hardcode. This forces
    // the real closure error to be threaded through unchanged: a
    // constant-substitution shortcut would return a different variant/message
    // and fail the match arm below.
    let result = with_immediate_transaction(&idx.conn, || {
        idx.conn
            .execute("INSERT INTO wit_t (x) VALUES (1)", [])
            .unwrap();
        assert!(
            !idx.conn.is_autocommit(),
            "a real transaction must be open during the closure"
        );
        Err::<(), _>(crate::error::DoogatError::Sql(
            "wit-rollback-probe-9f3c1a sentinel payload".into(),
        ))
    });

    match result {
        Err(crate::error::DoogatError::Sql(msg)) => {
            assert_eq!(
                msg, "wit-rollback-probe-9f3c1a sentinel payload",
                "error must propagate unchanged (same variant and payload)"
            );
        }
        other => {
            panic!("expected the closure's exact Sql error to propagate unchanged, got {other:?}")
        }
    }

    let count: i64 = idx
        .conn
        .query_row("SELECT COUNT(*) FROM wit_t", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        count, 0,
        "row inserted in an Err closure must be rolled back"
    );
}

#[test]
fn with_immediate_transaction_passes_through_return_value() {
    let idx = in_memory_index();

    let result = with_immediate_transaction(&idx.conn, || Ok(42));
    assert_eq!(
        result.unwrap(),
        42,
        "Ok return value must pass through unchanged"
    );
}

#[test]
fn batch_index_joins_an_open_transaction_instead_of_nesting() {
    // Regression (PRD 00140 review cycle 1): batch_index used a raw
    // `BEGIN IMMEDIATE`, which SQLite rejects with "cannot start a transaction
    // within a transaction" when the connection is already in one — the path a
    // nested `ensure_fresh` (→ rebuild/incremental_reindex → batch_index) hits
    // inside a SINGLETON write's IMMEDIATE window.
    let idx = in_memory_index();
    let doogats = make_sample_doogats(5);

    // Simulate the enclosing SINGLETON-write transaction.
    idx.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(!idx.conn.is_autocommit(), "outer transaction must be open");

    let count = idx
        .batch_index(&doogats)
        .expect("batch_index must join the open transaction, not fail on a nested BEGIN");
    assert_eq!(
        count, 5,
        "all 5 doogats indexed inside the joined transaction"
    );

    // batch_index must NOT have committed the enclosing transaction — the
    // outer SINGLETON write still owns commit/rollback.
    assert!(
        !idx.conn.is_autocommit(),
        "batch_index must leave the enclosing transaction open"
    );
    idx.conn.execute_batch("COMMIT").unwrap();

    let rows: i64 = idx
        .conn
        .query_row("SELECT COUNT(*) FROM doogats", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        rows, 5,
        "doogats indexed in the joined transaction must persist after the outer COMMIT"
    );
}

#[test]
fn rebuild_if_stale_inside_transaction_skips_destructive_full_rebuild() {
    // PRD 00157 regression: a nested `ensure_fresh` reaching `rebuild_if_stale`
    // from INSIDE an open write transaction (the path upsert_singleton's
    // BEGIN IMMEDIATE → UPDATE branch → update_doogat → ensure_fresh takes)
    // must not run a full `rebuild`. `drop_all_tables` toggles
    // `PRAGMA foreign_keys`, which SQLite ignores inside a transaction, so its
    // `DROP TABLE doogats` would fail with "FOREIGN KEY constraint failed"
    // while child tables still reference it. Pre-fix, the corrupt branch ran
    // here and errored; the fix skips the destructive rebuild inside a
    // transaction (the outermost ensure_fresh already ran it before BEGIN).
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    // A tag gives the doogat a child `_ddb_tags` row that REFERENCES doogats(id),
    // so a full rebuild's `DROP TABLE doogats` (with FK still enforced inside the
    // transaction) reproduces the exact "FOREIGN KEY constraint failed" symptom.
    repo.commit_file(
        "ddb/20260226120000.md",
        "---\nid: 20260226120000\ntitle: T\ntags:\n  - regression\n---\nBody.",
        "add doogat",
    )
    .unwrap();
    let idx = in_memory_index();
    idx.rebuild(&repo).unwrap(); // fresh index; drop_all_tables leaves FK enforcement ON

    // Force the corrupt trigger: drop a child relation table so check_integrity
    // reports corruption (the pre-fix code would force a full rebuild here).
    idx.conn.execute_batch("DROP TABLE _ddb_aliases").unwrap();
    assert!(
        !idx.check_integrity().unwrap(),
        "precondition: index must read as corrupt outside a transaction"
    );

    // Inside a write transaction the destructive full-rebuild path is unsafe
    // (PRAGMA foreign_keys is a no-op here) and redundant. rebuild_if_stale must
    // skip it and return without error rather than FK-failing in DROP TABLE.
    idx.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let result = idx.rebuild_if_stale(&repo);
    idx.conn.execute_batch("ROLLBACK").unwrap();

    assert!(
        result.is_ok(),
        "rebuild_if_stale inside a transaction must not run a destructive rebuild (got {result:?})"
    );
    assert!(
        result.unwrap().is_none(),
        "inside a transaction with a fresh HEAD, rebuild_if_stale must be a no-op, not a rebuild"
    );

    // The core `doogats` table must survive — the guard prevented
    // drop_all_tables from running inside the transaction.
    let doogats_exists: bool = idx
        .conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='doogats'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        doogats_exists,
        "the core `doogats` table must survive a nested in-transaction rebuild_if_stale"
    );
}

#[test]
fn rebuild_if_stale_inside_transaction_skips_full_rebuild_on_diff_failure() {
    // PRD 00157 doubt-review #1 regression: the in-transaction guard in
    // `rebuild_if_stale` must ALSO cover the path where `incremental_reindex`
    // itself falls back to a destructive full `rebuild` because `diff_paths`
    // fails (e.g. the stored HEAD is unreachable after a gc/compaction).
    // `rebuild_if_stale` calls `incremental_reindex` whenever a stored HEAD
    // exists, even inside an open write transaction; pre-fix, the diff-failure
    // fallback ran `rebuild` -> `drop_all_tables`, whose `PRAGMA foreign_keys=OFF`
    // is a no-op in a transaction, so `DROP TABLE doogats` failed with
    // "FOREIGN KEY constraint failed". The fix skips that destructive fallback
    // inside a transaction (the outermost ensure_fresh already ran any full
    // rebuild before BEGIN).
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    // The tag gives a child `_ddb_tags` row REFERENCING doogats(id), so a full
    // rebuild's `DROP TABLE doogats` (FK still enforced inside the transaction)
    // reproduces the exact "FOREIGN KEY constraint failed" symptom.
    repo.commit_file(
        "ddb/20260227120000.md",
        "---\nid: 20260227120000\ntitle: T\ntags:\n  - regression\n---\nBody.",
        "add doogat",
    )
    .unwrap();
    let idx = in_memory_index();
    idx.rebuild(&repo).unwrap();

    // Make the index stale with an UNREACHABLE stored HEAD: is_stale() is true
    // (bogus != current HEAD) and `diff_paths(bogus, current)` fails, so
    // `incremental_reindex` takes its full-rebuild fallback. Integrity is left
    // healthy so the corrupt branch (a separate guard) does not fire — this
    // isolates the diff-failure fallback path.
    idx.store_head("0000000000000000000000000000000000000000")
        .unwrap();
    assert!(
        idx.is_stale(&repo).unwrap(),
        "precondition: index must read as stale with the bogus stored HEAD"
    );
    assert!(
        idx.check_integrity().unwrap(),
        "precondition: index must be healthy so only the diff-failure path is exercised"
    );

    idx.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let result = idx.rebuild_if_stale(&repo);
    idx.conn.execute_batch("ROLLBACK").unwrap();

    assert!(
        result.is_ok(),
        "rebuild_if_stale inside a transaction must not run a destructive rebuild \
         when incremental_reindex's diff fails (got {result:?})"
    );

    // The core `doogats` table must survive — the guard prevented
    // drop_all_tables from running inside the transaction.
    let doogats_exists: bool = idx
        .conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='doogats'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        doogats_exists,
        "the core `doogats` table must survive a nested in-transaction diff-failure fallback"
    );
}

#[test]
fn rematerialize_never_exposes_missing_table_to_concurrent_reader() {
    // §55.A residual race: `materialize_all_types` dropped and recreated each
    // user table in SEPARATE autocommit statements, so a second connection (WAL)
    // could read between the committed DROP and the committed CREATE and see
    // "no such table". Two concurrent `ddb create` into a SINGLETON typedef hit
    // exactly this: the loser surfaced "no such table" instead of the structured
    // SINGLETON message, failing integration.sh §55.A intermittently on linux CI.
    // The fix wraps the rematerialize in one transaction, so a concurrent reader
    // sees the OLD table or the NEW table, never an absent one.
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    // A _typedef-backed table (like §55.A's `CREATE TABLE ... SINGLETON`): it is
    // in the orphan-cleanup keep-set, so it persists across rematerialize passes.
    // (An inferred-only table would be dropped by drop_orphan_materialized_tables
    // every pass, which is a legitimate absence, not the race under test.)
    repo.commit_file(
        "ddb/_typedef/20260101000000.md",
        "---\nid: 20260101000000\ntitle: widget\ntype: _typedef\ncolumns:\n  - name: theme\n    data_type: TEXT\n    zone: frontmatter\n---\n",
        "add widget typedef",
    )
    .unwrap();
    // A data row so rematerialize takes the type_names drop+create+populate path
    // (the path two racing `ddb create` invocations exercise in §55.A).
    repo.commit_file(
        "ddb/20260101000100.md",
        "---\nid: 20260101000100\ntitle: W\ntype: widget\ntheme: dark\n---\nBody.",
        "add widget row",
    )
    .unwrap();

    let db_path = dir.path().join("index-concurrency.db");
    let writer = Index::open(&db_path).unwrap();
    writer.rebuild(&repo).unwrap();
    // Precondition: the materialized table exists and is committed before the
    // reader connection opens.
    writer
        .conn
        .query_row("SELECT COUNT(*) FROM widget", [], |r| r.get::<_, i64>(0))
        .expect("precondition: `widget` table must exist after rebuild");
    assert!(
        writer.conn.is_autocommit(),
        "precondition: writer must be in autocommit so materialize_all_types opens its own transaction"
    );

    let stop = Arc::new(AtomicBool::new(false));
    let missing = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(AtomicUsize::new(0));

    let reader = Index::open(&db_path).unwrap();
    let stop_r = Arc::clone(&stop);
    let missing_r = Arc::clone(&missing);
    let seen_r = Arc::clone(&seen);
    let handle = std::thread::spawn(move || {
        while !stop_r.load(Ordering::Relaxed) {
            match reader
                .conn
                .query_row("SELECT COUNT(*) FROM widget", [], |r| r.get::<_, i64>(0))
            {
                Ok(_) => {
                    seen_r.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) if e.to_string().contains("no such table") => {
                    missing_r.fetch_add(1, Ordering::Relaxed);
                }
                // WAL readers don't block writers; any other transient is ignored.
                Err(_) => {}
            }
        }
    });

    for _ in 0..500 {
        writer
            .materialize_all_types(&repo)
            .expect("rematerialize must succeed");
    }
    stop.store(true, Ordering::Relaxed);
    handle.join().unwrap();

    assert!(
        seen.load(Ordering::Relaxed) > 0,
        "test sanity: the reader must have observed the table at least once"
    );
    let missing = missing.load(Ordering::Relaxed);
    assert_eq!(
        missing, 0,
        "a concurrent reader observed `widget` missing {missing} time(s) during \
         rematerialize; the drop+recreate must be atomic to other connections"
    );
}

// ── incremental reindex: collect-and-skip per-file failures, strict opt-in ──

/// Helper: commit a file with raw (possibly non-UTF-8) byte content, so a
/// read/decode failure can be triggered against a real `GitRepo`.
fn commit_file_bytes(repo: &GitRepo, rel_path: &str, content: &[u8], message: &str) {
    let full_path = repo.path.join(rel_path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&full_path, content).unwrap();

    let git_repo = &repo.repo;
    let mut index = git_repo.index().unwrap();
    index.add_path(std::path::Path::new(rel_path)).unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();

    let sig = git2::Signature::now("ddb", "ddb@test").unwrap();

    let parents: Vec<git2::Commit<'_>> = match git_repo.head() {
        Ok(head) => vec![head.peel_to_commit().unwrap()],
        Err(_) => vec![],
    };
    let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();

    git_repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
        .unwrap();
}

#[test]
fn batch_index_changes_skips_malformed_file_and_indexes_rest() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20270101000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20270102000000.md",
        "---\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20270103000000.md",
        "---\n: invalid yaml [\n---\nbody",
        "add bad",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let paths = vec![
        "ddb/20270101000000.md".to_string(),
        "ddb/20270102000000.md".to_string(),
        "ddb/20270103000000.md".to_string(),
    ];
    let (indexed, parsed, warnings) = idx.batch_index_changes(&repo, &paths, false).unwrap();

    assert_eq!(indexed, 2);
    assert_eq!(parsed.len(), 2);
    assert_eq!(warnings.len(), 1);
    if let crate::types::ConsistencyWarning::MalformedYaml { path, .. } = &warnings[0] {
        assert_eq!(path, "ddb/20270103000000.md");
    } else {
        panic!("expected MalformedYaml warning, got {:?}", warnings[0]);
    }
}

#[test]
fn batch_index_changes_strict_mode_returns_err_on_malformed_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20270201000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20270202000000.md",
        "---\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20270203000000.md",
        "---\n: invalid yaml [\n---\nbody",
        "add bad",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let paths = vec![
        "ddb/20270201000000.md".to_string(),
        "ddb/20270202000000.md".to_string(),
        "ddb/20270203000000.md".to_string(),
    ];
    let result = idx.batch_index_changes(&repo, &paths, true);
    assert!(
        result.is_err(),
        "strict mode must fail the whole batch on a malformed file"
    );
}

#[test]
fn batch_index_changes_skips_unreadable_file_and_indexes_rest() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20270301000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20270302000000.md",
        "---\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();
    commit_file_bytes(
        &repo,
        "ddb/20270303000000.md",
        &[0xFF, 0xFE, 0xFD],
        "add unreadable",
    );

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let paths = vec![
        "ddb/20270301000000.md".to_string(),
        "ddb/20270302000000.md".to_string(),
        "ddb/20270303000000.md".to_string(),
    ];
    let (indexed, parsed, warnings) = idx.batch_index_changes(&repo, &paths, false).unwrap();

    assert_eq!(indexed, 2);
    assert_eq!(parsed.len(), 2);
    assert_eq!(warnings.len(), 1);
    if let crate::types::ConsistencyWarning::UnreadableFile { path, .. } = &warnings[0] {
        assert_eq!(path, "ddb/20270303000000.md");
    } else {
        panic!("expected UnreadableFile warning, got {:?}", warnings[0]);
    }
}

#[test]
fn batch_index_changes_strict_mode_returns_err_on_unreadable_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20270401000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20270402000000.md",
        "---\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();
    commit_file_bytes(
        &repo,
        "ddb/20270403000000.md",
        &[0xFF, 0xFE, 0xFD],
        "add unreadable",
    );

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let paths = vec![
        "ddb/20270401000000.md".to_string(),
        "ddb/20270402000000.md".to_string(),
        "ddb/20270403000000.md".to_string(),
    ];
    let result = idx.batch_index_changes(&repo, &paths, true);
    assert!(
        result.is_err(),
        "strict mode must fail the whole batch on an unreadable file"
    );
}

#[test]
fn batch_index_changes_distinguishes_read_and_parse_failures() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20270501000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20270502000000.md",
        "---\n: invalid yaml [\n---\nbody",
        "add malformed",
    )
    .unwrap();
    commit_file_bytes(
        &repo,
        "ddb/20270503000000.md",
        &[0xFF, 0xFE, 0xFD],
        "add unreadable",
    );

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let paths = vec![
        "ddb/20270501000000.md".to_string(),
        "ddb/20270502000000.md".to_string(),
        "ddb/20270503000000.md".to_string(),
    ];
    let (indexed, parsed, warnings) = idx.batch_index_changes(&repo, &paths, false).unwrap();

    assert_eq!(indexed, 1);
    assert_eq!(parsed.len(), 1);
    assert_eq!(warnings.len(), 2);

    let malformed = warnings.iter().find(|w| {
        matches!(
            w,
            crate::types::ConsistencyWarning::MalformedYaml { path, .. }
            if path == "ddb/20270502000000.md"
        )
    });
    assert!(
        malformed.is_some(),
        "expected a MalformedYaml warning for the malformed path, got {warnings:?}"
    );

    let unreadable = warnings.iter().find(|w| {
        matches!(
            w,
            crate::types::ConsistencyWarning::UnreadableFile { path, .. }
            if path == "ddb/20270503000000.md"
        )
    });
    assert!(
        unreadable.is_some(),
        "expected an UnreadableFile warning for the unreadable path, got {warnings:?}"
    );
}

#[test]
fn batch_index_changes_single_malformed_file_skipped_with_warning() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20270601000000.md",
        "---\n: invalid yaml [\n---\nbody",
        "add bad",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let paths = vec!["ddb/20270601000000.md".to_string()];
    let (indexed, parsed, warnings) = idx.batch_index_changes(&repo, &paths, false).unwrap();

    assert_eq!(indexed, 0);
    assert!(parsed.is_empty());
    assert_eq!(warnings.len(), 1);
    if let crate::types::ConsistencyWarning::MalformedYaml { path, .. } = &warnings[0] {
        assert_eq!(path, "ddb/20270601000000.md");
    } else {
        panic!("expected MalformedYaml warning, got {:?}", warnings[0]);
    }
}

#[test]
fn batch_index_changes_all_valid_files_returns_zero_warnings() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20270701000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20270702000000.md",
        "---\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20270703000000.md",
        "---\ntitle: C\n---\nBody C.",
        "add c",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let paths = vec![
        "ddb/20270701000000.md".to_string(),
        "ddb/20270702000000.md".to_string(),
        "ddb/20270703000000.md".to_string(),
    ];
    let (indexed, parsed, warnings) = idx.batch_index_changes(&repo, &paths, false).unwrap();

    assert_eq!(indexed, 3);
    assert_eq!(parsed.len(), 3);
    assert!(warnings.is_empty());
}

#[test]
fn incremental_reindex_report_carries_warnings_for_poison_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20270801000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20270802000000.md",
        "---\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20270803000000.md",
        "---\ntitle: C\n---\nBody C.",
        "add c",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    let report = idx.rebuild(&repo).unwrap();
    assert_eq!(report.indexed, 3);
    let old_head = idx.stored_head_oid().unwrap();

    // Modify all 3 doogats in a single commit; poison the middle one with
    // malformed YAML so the change-set has both good and bad files.
    let modifications: Vec<(&str, &str)> = vec![
        (
            "ddb/20270801000000.md",
            "---\ntitle: Modified A\n---\nUpdated body A.",
        ),
        ("ddb/20270802000000.md", "---\n: invalid yaml [\n---\nbody"),
        (
            "ddb/20270803000000.md",
            "---\ntitle: Modified C\n---\nUpdated body C.",
        ),
    ];
    repo.commit_files(&modifications, "modify 3, poison 1")
        .unwrap();

    let report = idx.incremental_reindex(&repo, &old_head, false).unwrap();
    assert_eq!(report.indexed, 2);
    assert_eq!(report.warnings.len(), 1);
    if let crate::types::ConsistencyWarning::MalformedYaml { path, .. } = &report.warnings[0] {
        assert_eq!(path, "ddb/20270802000000.md");
    } else {
        panic!(
            "expected RebuildReport.warnings to carry a MalformedYaml warning, got {:?}",
            report.warnings[0]
        );
    }
}

#[test]
fn batch_index_changes_single_unreadable_file_skipped_with_warning() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    commit_file_bytes(
        &repo,
        "ddb/20270901000000.md",
        &[0xFF, 0xFE, 0xFD],
        "add unreadable",
    );

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let paths = vec!["ddb/20270901000000.md".to_string()];
    let (indexed, parsed, warnings) = idx.batch_index_changes(&repo, &paths, false).unwrap();

    assert_eq!(indexed, 0);
    assert!(parsed.is_empty());
    assert_eq!(warnings.len(), 1);
    if let crate::types::ConsistencyWarning::UnreadableFile { path, .. } = &warnings[0] {
        assert_eq!(path, "ddb/20270901000000.md");
    } else {
        panic!("expected UnreadableFile warning, got {:?}", warnings[0]);
    }
}

#[test]
fn batch_index_changes_single_unreadable_file_strict_mode_returns_err() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    commit_file_bytes(
        &repo,
        "ddb/20271001000000.md",
        &[0xFF, 0xFE, 0xFD],
        "add unreadable",
    );

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let paths = vec!["ddb/20271001000000.md".to_string()];
    let result = idx.batch_index_changes(&repo, &paths, true);
    assert!(
        result.is_err(),
        "strict mode must fail on a single unreadable file"
    );
}

#[test]
fn batch_index_changes_single_malformed_file_strict_mode_returns_err() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20271101000000.md",
        "---\n: invalid yaml [\n---\nbody",
        "add bad",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let paths = vec!["ddb/20271101000000.md".to_string()];
    let result = idx.batch_index_changes(&repo, &paths, true);
    assert!(
        result.is_err(),
        "strict mode must fail on a single malformed file"
    );
}

// ── cross-process serialization of the destructive full rebuild ──

/// A `DoogatSource` that delegates to a real `GitRepo` but parks inside
/// `list_doogats` on its FIRST call until the test releases it.
///
/// `Index::rebuild` calls `list_doogats` AFTER `drop_all_tables()` +
/// `SCHEMA_DDL`, so a parked call means the rebuild is sitting inside the
/// destructive section right now. That is what lets these tests assert on
/// CONTENTION — no second party can take the lock while a rebuild is mid-flight
/// — instead of on elapsed time or a lock file's existence. Both of those are
/// forgeable by an implementation that serializes nothing: a `sleep` fakes the
/// timing and a `File::create` fakes the file.
struct ParkingSource {
    inner: GitRepo,
    entered: std::sync::mpsc::Sender<()>,
    resume: std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    parked_once: std::sync::atomic::AtomicBool,
}

impl ParkingSource {
    /// Returns the source, the `entered` receiver (signalled once the rebuild
    /// is inside the destructive section) and the `resume` sender (which lets
    /// the rebuild finish).
    fn new(
        inner: GitRepo,
    ) -> (
        Self,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        (
            Self {
                inner,
                entered: entered_tx,
                resume: std::sync::Mutex::new(Some(resume_rx)),
                parked_once: std::sync::atomic::AtomicBool::new(false),
            },
            entered_rx,
            resume_tx,
        )
    }
}

impl DoogatSource for ParkingSource {
    fn list_doogats(&self) -> Result<Vec<String>> {
        if !self
            .parked_once
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            // First call only. Later calls (the rebuild's consistency pass)
            // run straight through, so the rebuild completes once resumed.
            let resume = self
                .resume
                .lock()
                .unwrap()
                .take()
                .expect("the resume channel is taken exactly once");
            self.entered.send(()).unwrap();
            resume.recv().unwrap();
        }
        self.inner.list_doogats()
    }

    fn read_file(&self, path: &str) -> Result<String> {
        self.inner.read_file(path)
    }

    fn head_oid(&self) -> Result<crate::types::CommitHash> {
        self.inner.head_oid()
    }

    fn diff_paths(
        &self,
        old_oid: &str,
        new_oid: &str,
    ) -> Result<Vec<(crate::types::DiffKind, String)>> {
        self.inner.diff_paths(old_oid, new_oid)
    }
}

/// Run `idx.rebuild_if_stale(&repo)` on a worker thread, block until it parks
/// inside the destructive section, run `probe` while it is stuck there, then
/// let it finish. Returns the index (so the caller can query the resulting
/// rows) and the call's own result.
fn probe_rebuild_mid_flight<P: FnOnce()>(
    idx: Index,
    repo: GitRepo,
    probe: P,
) -> (Index, Result<Option<crate::types::RebuildReport>>) {
    let (source, entered, resume) = ParkingSource::new(repo);
    let worker = std::thread::spawn(move || {
        let result = idx.rebuild_if_stale(&source);
        (idx, result)
    });

    entered.recv().expect(
        "the rebuild never reached list_doogats — it returned before entering the \
         destructive section, so there was nothing to probe",
    );
    probe();
    resume.send(()).expect("the parked rebuild vanished");
    worker.join().unwrap()
}

/// Try to take `<dir>/ddb-rebuild.lock` without waiting: `Err` means someone
/// else owns it at this instant, `Ok` means it is free. `write_lock::acquire`
/// checks `start.elapsed() >= timeout` on contention, so a zero timeout answers
/// immediately instead of blocking.
fn try_take_rebuild_lock(dir: &Path) -> Result<crate::git_ops::write_lock::WriteLockGuard> {
    crate::git_ops::write_lock::acquire(
        dir,
        "ddb-rebuild.lock",
        std::time::Duration::from_millis(0),
    )
}

#[test]
fn rebuild_if_stale_waits_for_a_held_rebuild_lock_before_rebuilding() {
    // N cold-start processes each decide the index is stale and run the
    // destructive `rebuild` (drop_all_tables + recreate). Unserialized, one
    // drops the tables while another queries and the loser sees
    // "no such table: _ddb_fts". Every implicit path to the destructive
    // rebuild must therefore hold <index-dir>/ddb-rebuild.lock.
    //
    // Discriminator: while the rebuild is provably parked inside the
    // destructive section, a second party must NOT be able to take that lock.
    // This is the invariant itself, not a proxy for it — an implementation that
    // only sleeps, or that takes the guard with `let _ = acquire(..)` (dropped
    // at the end of the statement, before the destructive work), lets the probe
    // succeed and fails here.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file(
        "ddb/20260401000000.md",
        "---\nid: 20260401000000\ntitle: Contended\n---\nBody.",
        "add doogat",
    )
    .unwrap();

    let db_dir = dir.path().join(".ddb");
    std::fs::create_dir_all(&db_dir).unwrap();
    // Fresh index: no stored HEAD, so rebuild_if_stale takes the destructive
    // full-rebuild branch rather than an incremental reindex.
    let idx = Index::open(&db_dir.join("index.db")).unwrap();

    let probe_dir = db_dir.clone();
    let (idx, result) = probe_rebuild_mid_flight(idx, repo, move || {
        assert!(
            try_take_rebuild_lock(&probe_dir).is_err(),
            "a second party took <index-dir>/ddb-rebuild.lock while a rebuild was \
             mid-flight between drop_all_tables() and its repopulation — the \
             destructive section runs unlocked"
        );
    });

    let report = result.expect("a rebuild holding the lock must still succeed");
    let report = report.expect("a fresh index with no stored HEAD must run the full rebuild");
    assert_eq!(
        report.indexed, 1,
        "the serialized rebuild must still index the repo's doogat"
    );
    let rows: i64 = idx
        .conn
        .query_row("SELECT COUNT(*) FROM doogats", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        rows, 1,
        "the serialized rebuild must leave the index populated, not dropped"
    );
    // A guard that is acquired but never dropped deadlocks the next caller.
    assert!(
        try_take_rebuild_lock(&db_dir).is_ok(),
        "the rebuild lock was still held after rebuild_if_stale returned — the \
         guard is never released and the next process would block forever"
    );
}

#[test]
fn rebuild_if_stale_blocks_until_a_foreign_rebuild_lock_holder_releases() {
    // The mirror direction of the test above. There, a rebuild held the lock and
    // an outside probe had to fail; here an outside holder owns the lock and the
    // REBUILD must wait for it. Without this direction, an implementation that
    // asks for the lock with a zero timeout and reads contention as "someone
    // else is already rebuilding, so there is nothing left for me to do" passes:
    // it returns Ok in under a millisecond while the holder may be parked
    // between drop_all_tables() and repopulation, and its caller then serves an
    // empty index as success. Waiting, not giving up, is what the lock buys.
    //
    // The TEST is the foreign lock holder here, standing in for the other
    // process that is already rebuilding, so no ParkingSource is needed.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file(
        "ddb/20260408000000.md",
        "---\nid: 20260408000000\ntitle: Waiting\n---\nBody.",
        "add doogat",
    )
    .unwrap();

    let db_dir = dir.path().join(".ddb");
    std::fs::create_dir_all(&db_dir).unwrap();
    // Fresh index: no stored HEAD, so rebuild_if_stale takes the destructive
    // full-rebuild branch and must therefore contend for the lock.
    let idx = Index::open(&db_dir.join("index.db")).unwrap();

    let held = crate::git_ops::write_lock::acquire(
        &db_dir,
        "ddb-rebuild.lock",
        std::time::Duration::from_secs(10),
    )
    .expect("the test must be able to take the rebuild lock first");

    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = idx.rebuild_if_stale(&repo);
        tx.send(result).expect("the waiting test hung up early");
        idx
    });

    // Bounded window, well under the acquire timeout, so a correct
    // implementation waits it out and still succeeds afterwards.
    match rx.recv_timeout(std::time::Duration::from_millis(200)) {
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("the rebuild worker died before returning a result")
        }
        Ok(early) => panic!(
            "rebuild_if_stale returned {early:?} while another holder still owned \
             <index-dir>/ddb-rebuild.lock — it skipped the lock instead of waiting for \
             it, so it can report success over an index the holder has dropped and not \
             yet repopulated"
        ),
    }

    std::mem::drop(held);

    let idx = worker.join().unwrap();
    let report = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the rebuild never returned after the lock was released")
        .expect("a rebuild that waited for the lock must then succeed, not error out")
        .expect("a fresh index with no stored HEAD must run the full rebuild");
    assert_eq!(
        report.indexed, 1,
        "the rebuild that waited for the lock must still index the repo's doogat"
    );
    let rows: i64 = idx
        .conn
        .query_row("SELECT COUNT(*) FROM doogats", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        rows, 1,
        "the rebuild that waited for the lock must leave the index populated"
    );
}

#[test]
fn rebuild_if_stale_after_a_completed_rebuild_reports_nothing_and_keeps_rows() {
    // A second rebuild_if_stale over an index whose stored HEAD is current
    // reports nothing to do AND leaves the earlier rows in place, so an
    // implementation that rebuilds unconditionally inside the lock (or drops
    // the tables without repopulating them) cannot pass.
    //
    // Scope note: this is the SEQUENTIAL case, which short-circuits on the
    // outer staleness check before the lock is ever reached. The post-lock
    // re-check — the loser of a real race skipping the winner's work — is
    // bound by `two_racing_cold_start_rebuilds_...` below.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file(
        "ddb/20260402000000.md",
        "---\nid: 20260402000000\ntitle: Once\n---\nBody.",
        "add doogat",
    )
    .unwrap();

    let db_dir = dir.path().join(".ddb");
    std::fs::create_dir_all(&db_dir).unwrap();
    let idx = Index::open(&db_dir.join("index.db")).unwrap();

    let first = idx
        .rebuild_if_stale(&repo)
        .unwrap()
        .expect("the first call must run the full rebuild and store HEAD");
    assert_eq!(first.indexed, 1);

    let second = idx.rebuild_if_stale(&repo).unwrap();
    assert!(
        second.is_none(),
        "a second rebuild_if_stale over a healthy, current index must report \
         nothing to do, not repeat the destructive rebuild (got {second:?})"
    );

    let ids = idx.query_raw("SELECT id FROM doogats ORDER BY id").unwrap();
    assert_eq!(
        ids.len(),
        1,
        "the doogats indexed by the first pass must survive the second call"
    );
    assert_eq!(ids[0][0], "20260402000000");
}

#[test]
fn in_memory_index_rebuild_creates_no_rebuild_lock_file() {
    // `Index::open_in_memory()` has no db file and therefore no directory to
    // lock: the rebuild must degrade to unlocked instead of locking a bogus
    // path.
    //
    // Scope note: this binds the DEGRADATION only — no stray lock artifacts,
    // and a rebuild that still works without a lock directory. It says nothing
    // about serialization; the contention tests above own that.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file(
        "ddb/20260403000000.md",
        "---\nid: 20260403000000\ntitle: Memory\n---\nBody.",
        "add doogat",
    )
    .unwrap();

    let idx = Index::open_in_memory().unwrap();
    let report = idx
        .rebuild_if_stale(&repo)
        .expect("an in-memory index must rebuild without a lock directory")
        .expect("a fresh in-memory index with no stored HEAD must run the full rebuild");
    assert_eq!(report.indexed, 1);

    for candidate in [
        dir.path().join("ddb-rebuild.lock"),
        dir.path().join(".git").join("ddb-rebuild.lock"),
        dir.path().join(".ddb").join("ddb-rebuild.lock"),
        std::env::current_dir().unwrap().join("ddb-rebuild.lock"),
    ] {
        assert!(
            !candidate.exists(),
            "an in-memory index has no directory to lock, but a rebuild through \
             it created {}",
            candidate.display()
        );
    }
}

#[test]
fn rebuild_through_open_memory_path_creates_no_lock_file_in_the_working_dir() {
    // `Path::new(":memory:").parent()` is Some("") — NOT None. An
    // implementation that derives the lock directory from the db path without
    // handling this creates a stray `ddb-rebuild.lock` in the process's cwd
    // (the crate root under `cargo test`), polluting the repo and making every
    // `:memory:`-backed test contend with every other.
    //
    // Scope note: this binds the lock-DIRECTORY derivation for the `":memory:"`
    // path, not serialization. The contention tests above own that.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file(
        "ddb/20260404000000.md",
        "---\nid: 20260404000000\ntitle: Trap\n---\nBody.",
        "add doogat",
    )
    .unwrap();

    let idx = in_memory_index(); // Index::open(Path::new(":memory:"))
    let report = idx
        .rebuild_if_stale(&repo)
        .expect("a `:memory:`-pathed index must rebuild without erroring on an empty lock dir")
        .expect("a fresh `:memory:`-pathed index with no stored HEAD must run the full rebuild");
    assert_eq!(report.indexed, 1);

    let stray = std::env::current_dir().unwrap().join("ddb-rebuild.lock");
    assert!(
        !stray.exists(),
        "a rebuild through Index::open(\":memory:\") created a stray lock file at {} — \
         `Path::new(\":memory:\").parent()` is Some(\"\"), which resolves to the cwd",
        stray.display()
    );
}

#[test]
fn rebuild_lock_file_sits_beside_the_index_db_not_in_the_git_dir() {
    // The rebuild lock guards the SQLite index and lives in the index db's own
    // directory. It must not be relocated into `.git/`, where it would collide
    // with the git write lock's scope; the two are different files in
    // different directories and are never held together.
    //
    // Location is asserted by WHERE THE CONTENTION IS, not by `Path::exists()`
    // — an implementation that merely touches a file satisfies existence
    // without locking anything. Mid-rebuild, the lock beside the db must be
    // unavailable while the same name inside `.git/` is still free.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file(
        "ddb/20260405000000.md",
        "---\nid: 20260405000000\ntitle: Located\n---\nBody.",
        "add doogat",
    )
    .unwrap();

    let db_dir = dir.path().join(".ddb");
    std::fs::create_dir_all(&db_dir).unwrap();
    let idx = Index::open(&db_dir.join("index.db")).unwrap();

    let probe_db_dir = db_dir.clone();
    let probe_git_dir = dir.path().join(".git");
    let (_idx, result) = probe_rebuild_mid_flight(idx, repo, move || {
        assert!(
            try_take_rebuild_lock(&probe_db_dir).is_err(),
            "no lock was held beside the index db at {} while the rebuild was \
             mid-flight",
            probe_db_dir.display()
        );
        // Checked BEFORE the probe below, which would create the file itself.
        assert!(
            !probe_git_dir.join("ddb-rebuild.lock").exists(),
            "the rebuild lock must not live in .git/ — that is the git write lock's directory"
        );
        assert!(
            !probe_db_dir.join("ddb-write.lock").exists(),
            "the git write lock must not be taken in the index directory; the rebuild \
             lock is a distinct lock with a distinct name"
        );
        assert!(
            try_take_rebuild_lock(&probe_git_dir).is_ok(),
            "a rebuild lock inside .git/ was held during the rebuild — the lock \
             belongs beside the index db, not in the git write lock's directory"
        );
    });

    result
        .expect("the rebuild must succeed")
        .expect("a fresh index with no stored HEAD must run the full rebuild");
}

#[test]
fn rebuild_if_stale_waits_for_the_rebuild_lock_when_diff_paths_fails() {
    // The destructive rebuild has a THIRD implicit entry point, in a different
    // function from the two branches of `rebuild_if_stale`:
    // `incremental_reindex` falls back to the full `rebuild` when
    // `repo.diff_paths(old_head, new_head)` errors (e.g. the stored HEAD went
    // unreachable after a gc). That fallback drops and recreates the tables
    // just like the others, so it must hold <index-dir>/ddb-rebuild.lock too.
    //
    // Fixture: seeding stored HEAD to the all-zeros OID is what steers
    // execution here, and it is load-bearing — do not "simplify" it away. It
    // makes `is_stale` true (it differs from the real HEAD) AND
    // `stored_head_oid()` return `Some`, so `rebuild_if_stale` routes into
    // `incremental_reindex`; the zeros OID is unreachable in the repo, so
    // `diff_paths` errors and execution lands in the fallback. The parking
    // source delegates `diff_paths` to the real `GitRepo`, so that routing
    // still happens through it.
    //
    // Discriminator, as above: while the fallback rebuild is parked inside the
    // destructive section, nobody else may take the lock.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file(
        "ddb/20260406000000.md",
        "---\nid: 20260406000000\ntitle: Fallback\n---\nBody.",
        "add doogat",
    )
    .unwrap();

    let db_dir = dir.path().join(".ddb");
    std::fs::create_dir_all(&db_dir).unwrap();
    let idx = Index::open(&db_dir.join("index.db")).unwrap();
    idx.conn
        .execute(
            "INSERT OR REPLACE INTO _ddb_meta (key, value) VALUES ('head', ?1)",
            rusqlite::params!["0000000000000000000000000000000000000000"],
        )
        .unwrap();

    let probe_dir = db_dir.clone();
    let (idx, result) = probe_rebuild_mid_flight(idx, repo, move || {
        assert!(
            try_take_rebuild_lock(&probe_dir).is_err(),
            "a second party took <index-dir>/ddb-rebuild.lock while the \
             diff-failure fallback was mid-flight — incremental_reindex reaches \
             the destructive rebuild unserialized"
        );
    });

    let report = result.expect("a diff-failure fallback that holds the lock must still succeed");
    let report = report.expect("a stale index with an unreachable stored HEAD must rebuild");
    assert_eq!(
        report.indexed, 1,
        "the serialized fallback rebuild must still index the repo's doogat"
    );
    let rows: i64 = idx
        .conn
        .query_row("SELECT COUNT(*) FROM doogats", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        rows, 1,
        "the serialized fallback rebuild must leave the index populated, not dropped"
    );
    // A guard that is acquired but never dropped deadlocks the next caller.
    assert!(
        try_take_rebuild_lock(&db_dir).is_ok(),
        "the rebuild lock was still held after the fallback returned — the guard \
         is never released and the next process would block forever"
    );
}

#[test]
fn two_racing_cold_start_rebuilds_run_the_destructive_work_only_once() {
    // Double-checked locking: the loser must re-check integrity/staleness AFTER
    // it wins the lock, not only before it asks for one. An implementation that
    // re-checks only outside the lock still passes the sequential
    // `..._after_a_completed_rebuild_...` case above (that one short-circuits on
    // the outer staleness check and never reaches the lock), but here it drops
    // and recreates the tables a second time on top of the winner's rows.
    //
    // The loser has TWO legal shapes and this test accepts both on purpose:
    //   - it evaluated the OUTER staleness check after the winner had already
    //     finished -> `Ok(None)`;
    //   - it got past that check first, then blocked on the lock, and its
    //     post-lock re-check found the index fresh -> `Ok(Some(report))` with
    //     `indexed == 0`.
    // Do not "tighten" this into "exactly one returns Some" — which of the two
    // shapes appears is a timing coin flip and the tightened form would flake.
    // The destructive work is the unambiguous signal: exactly one call may
    // report having indexed anything.
    //
    // Each worker also reads the index on ITS OWN handle the instant its call
    // returns, before any thread joins. That binds the invariant the caller
    // actually depends on — when rebuild_if_stale returns Ok, the index is
    // complete and queryable NOW — which is strictly stronger than counting who
    // did the work. A loser that treats contention as "nothing left to do" and
    // returns while the winner is still repopulating reads 0 rows here (or no
    // table at all) and fails, even though its report looks innocent.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file(
        "ddb/20260407000000.md",
        "---\nid: 20260407000000\ntitle: Raced\n---\nBody.",
        "add doogat",
    )
    .unwrap();

    let db_dir = dir.path().join(".ddb");
    std::fs::create_dir_all(&db_dir).unwrap();
    let db_path = db_dir.join("index.db");

    // Genuinely cold start on both sides: two independent index handles over
    // one db file, two independent repo handles, released together.
    let start = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let idx = Index::open(&db_path).unwrap();
        let repo = GitRepo::open(dir.path()).unwrap();
        let start = std::sync::Arc::clone(&start);
        workers.push(std::thread::spawn(move || {
            start.wait();
            let result = idx.rebuild_if_stale(&repo);
            // Read on this call's own handle, immediately, before any join:
            // what did THIS caller get to see the moment it was told "Ok"?
            let seen: std::result::Result<i64, String> = idx
                .conn
                .query_row("SELECT COUNT(*) FROM doogats", [], |row| row.get(0))
                .map_err(|e| e.to_string());
            (result, seen)
        }));
    }

    let outcomes: Vec<(Option<crate::types::RebuildReport>, _)> = workers
        .into_iter()
        .map(|w| {
            let (result, seen) = w.join().unwrap();
            (
                result.expect(
                    "the loser of the race must wait for the lock and return Ok, not error out",
                ),
                seen,
            )
        })
        .collect();

    for (report, seen) in &outcomes {
        match seen {
            Ok(1) => {}
            other => panic!(
                "a racing rebuild_if_stale returned Ok({report:?}) but the index it handed \
                 back to its own caller held {other:?} rows, not the repo's 1 — it returned \
                 while the other call was still between drop_all_tables() and repopulation, \
                 so success was reported over a wiped index"
            ),
        }
    }

    let rebuilders = outcomes
        .iter()
        .filter(|(r, _)| r.as_ref().is_some_and(|report| report.indexed > 0))
        .count();
    assert_eq!(
        rebuilders, 1,
        "exactly one of two racing cold-start calls may run the destructive \
         rebuild; the other must re-check after winning the lock and skip it \
         (got {outcomes:?})"
    );

    let idx = Index::open(&db_path).unwrap();
    let ids = idx.query_raw("SELECT id FROM doogats ORDER BY id").unwrap();
    assert_eq!(
        ids.len(),
        1,
        "a raced double rebuild leaves the index dropped or duplicated; it must \
         hold exactly the repo's one doogat (got {ids:?})"
    );
    assert_eq!(ids[0][0], "20260407000000");
}

// ── cross-process serialization of the schema-upgrade drop ──

#[test]
fn configure_connection_with_no_db_dir_upgrades_without_leaving_a_stray_rebuild_lock_file() {
    // AC6: the lock is skipped when there is no directory to lock (db_dir is
    // `None` — this is what `open_in_memory()` and the bare `":memory:"`
    // path both resolve to). A fresh in-memory DB has no `_ddb_fts` table,
    // so needs_schema_upgrade returns false and the destructive branch never
    // runs — meaning no implementation would attempt to take a lock there
    // anyway, correct or not, so that shape cannot bind this criterion.
    // Building an old 3-column-FTS schema (the upgrade_old_3col_fts_to_4col
    // fixture shape) forces the drop+recreate branch to actually fire with
    // `db_dir == None`, which is the only moment a bogus lock directory
    // could be derived.
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS _ddb_fts USING fts5(
            title, body, tags,
            tokenize = 'porter unicode61'
        );",
    )
    .unwrap();

    let idx = Index::configure_connection(conn, None)
        .expect("configure_connection should upgrade an old schema even with no db_dir");

    idx.conn
        .execute(
            "INSERT INTO _ddb_fts (title, body, tags, fields) VALUES (?1, ?2, ?3, ?4)",
            params!["t", "b", "tag", "field_data"],
        )
        .expect("after the upgrade, FTS5 should accept 4 columns");

    let stray = std::env::current_dir().unwrap().join("ddb-rebuild.lock");
    assert!(
        !stray.exists(),
        "configure_connection(conn, None) created a stray rebuild lock file \
         at {} while running the destructive schema-upgrade drop — a `None` \
         db_dir must skip the lock, not derive a bogus lock directory",
        stray.display()
    );
}

#[test]
fn configure_connection_holds_the_rebuild_lock_during_the_destructive_drop() {
    // AC5: the destructive branch — whichever check fired — must run while
    // holding the same cross-process lock `locked_rebuild` uses:
    // write_lock::acquire(db_dir, "ddb-rebuild.lock", ...) on
    // <index-dir>/ddb-rebuild.lock.
    //
    // `configure_connection`'s drop branch makes no external call to park
    // inside (unlike `Index::rebuild`, which calls out to a `DoogatSource`),
    // so this observes contention from the other direction: hold the lock
    // ourselves first, and confirm the destructive open does not complete
    // until we release it.
    let dir = tempfile::TempDir::new().unwrap();

    let held = crate::git_ops::write_lock::acquire(
        dir.path(),
        "ddb-rebuild.lock",
        std::time::Duration::from_secs(10),
    )
    .expect("the test must be able to take the rebuild lock first");

    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS _ddb_fts USING fts5(
            title, body, tags,
            tokenize = 'porter unicode61'
        );",
    )
    .unwrap();

    let lock_dir = dir.path().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = Index::configure_connection(conn, Some(lock_dir));
        tx.send(result).expect("the waiting test hung up early");
    });

    match rx.recv_timeout(std::time::Duration::from_millis(200)) {
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("the worker thread died before returning a result")
        }
        Ok(_) => panic!(
            "configure_connection returned while the rebuild lock was still \
             held externally — the destructive schema-upgrade drop does not \
             hold <index-dir>/ddb-rebuild.lock"
        ),
    }

    std::mem::drop(held);

    let idx = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("configure_connection never returned after the lock was released")
        .expect("configure_connection should upgrade the schema once it can take the lock");
    worker.join().unwrap();

    idx.conn
        .execute(
            "INSERT INTO _ddb_fts (title, body, tags, fields) VALUES (?1, ?2, ?3, ?4)",
            params!["t", "b", "tag", "field_data"],
        )
        .expect("after the upgrade, FTS5 should accept 4 columns");
}

#[test]
fn configure_connection_for_a_schema_already_at_current_version_does_not_wait_on_the_rebuild_lock()
{
    // AC7: the non-destructive open path must not serialize against a
    // concurrent rebuild. Hold the rebuild lock externally (standing in for
    // another process's in-flight destructive rebuild) and confirm that
    // opening a schema already at SCHEMA_VERSION completes without waiting
    // for it — only the drop+recreate branch may hold that lock.
    let dir = tempfile::TempDir::new().unwrap();

    let held = crate::git_ops::write_lock::acquire(
        dir.path(),
        "ddb-rebuild.lock",
        std::time::Duration::from_secs(10),
    )
    .expect("the test must be able to take the rebuild lock first");

    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS _ddb_fts USING fts5(
            title, body, tags, fields,
            tokenize = 'porter unicode61'
        );
        PRAGMA user_version = 1;",
    )
    .unwrap();

    let lock_dir = dir.path().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = Index::configure_connection(conn, Some(lock_dir));
        tx.send(result).expect("the waiting test hung up early");
    });

    let idx = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect(
            "configure_connection for a schema already at SCHEMA_VERSION \
             waited on <index-dir>/ddb-rebuild.lock even though it needed no \
             destructive work",
        )
        .expect("configure_connection should succeed for a schema already at SCHEMA_VERSION");
    worker.join().unwrap();

    let version: i64 = idx
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, 1,
        "an already-current schema must remain stamped at SCHEMA_VERSION"
    );

    std::mem::drop(held);
}

// ── --strict full rebuild + unconditional explicit reindex (`ddb reindex --strict`) ──

#[test]
fn rebuild_strict_reports_err_naming_the_malformed_yaml_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20280101000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20280102000000.md",
        "---\n: invalid yaml [\n---\nbody",
        "add bad",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let result = idx.rebuild_strict(&repo);
    let err = result.expect_err("a strict rebuild must abort on a malformed-YAML file");
    assert!(
        err.to_string().contains("20280102000000.md"),
        "strict rebuild error must name the offending path, got: {err}"
    );
}

#[test]
fn rebuild_strict_reports_err_naming_the_unreadable_file_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20280201000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    commit_file_bytes(
        &repo,
        "ddb/20280202000000.md",
        &[0xFF, 0xFE, 0xFD],
        "add unreadable",
    );

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let result = idx.rebuild_strict(&repo);
    let err = result.expect_err("a strict rebuild must abort on an unreadable file");
    assert!(
        err.to_string().contains("20280202000000.md"),
        "strict rebuild error must name the offending path, got: {err}"
    );
}

#[test]
fn rebuild_strict_over_a_clean_corpus_indexes_everything_with_no_warnings() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20280301000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20280302000000.md",
        "---\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let report = idx
        .rebuild_strict(&repo)
        .expect("a strict rebuild over a clean corpus must succeed");
    assert_eq!(report.indexed, 2);
    assert!(
        report.warnings.is_empty(),
        "a clean corpus must produce no warnings, got {:?}",
        report.warnings
    );
}

#[test]
fn rebuild_strict_aborting_leaves_previously_indexed_rows_intact() {
    // The single most important property: the abort must happen BEFORE the
    // destructive drop+recreate. Build a good index first, then commit a
    // poison file, then call rebuild_strict — it must abort, and the rows
    // indexed by the earlier, successful rebuild must still be there
    // afterwards. An implementation that drops the tables first and only then
    // discovers the poison file wipes the index and reports an error, which
    // is strictly worse than either lenient collect-and-skip or a clean
    // failure.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20280401000000.md",
        "---\nid: 20280401000000\ntitle: Good\n---\nBody good.",
        "add good",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let first = idx.rebuild(&repo).unwrap();
    assert_eq!(
        first.indexed, 1,
        "test sanity: the good doogat must be indexed before the poison file lands"
    );

    repo.commit_file(
        "ddb/20280402000000.md",
        "---\n: invalid yaml [\n---\nbody",
        "add bad",
    )
    .unwrap();

    let result = idx.rebuild_strict(&repo);
    assert!(
        result.is_err(),
        "a strict rebuild over a corpus containing a poison file must abort"
    );

    let ids = idx.query_raw("SELECT id FROM doogats ORDER BY id").unwrap();
    assert_eq!(
        ids.len(),
        1,
        "the row indexed before the aborted strict rebuild must still be \
         present (got {ids:?}) — the destructive drop must not run before the \
         strict rebuild detects the poison file"
    );
    assert_eq!(ids[0][0], "20280401000000");
}

#[test]
fn explicit_reindex_rebuilds_unconditionally_even_when_index_is_fresh_and_healthy() {
    // Regression test for the `indexed 0 doogats` bug: `DoogatService::reindex()`
    // used to route through `locked_rebuild`, which re-checks
    // integrity/staleness after taking the lock and skips the rebuild entirely
    // once the index is healthy and current. That re-check is correct for the
    // IMPLICIT paths, but the EXPLICIT `ddb reindex` command must rebuild
    // unconditionally.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20280501000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20280502000000.md",
        "---\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    // Get the index healthy and current first.
    idx.rebuild_if_stale(&repo)
        .unwrap()
        .expect("the first rebuild_if_stale must run the full rebuild");
    assert!(
        idx.check_integrity().unwrap(),
        "test sanity: the index must be structurally sound before the explicit reindex"
    );
    assert!(
        !idx.is_stale(&repo).unwrap(),
        "test sanity: the index must be current before the explicit reindex"
    );

    let report = idx.locked_explicit_rebuild(&repo, false).unwrap();
    assert_eq!(
        report.indexed, 2,
        "an explicit reindex over a fresh, healthy index must still rebuild \
         and report the repo's 2 doogats, not 0 — routing through the \
         implicit re-check silently skips the rebuild (the `indexed 0 \
         doogats` bug)"
    );
}

#[test]
fn explicit_reindex_still_serializes_against_a_held_rebuild_lock() {
    // The explicit path must not have traded the cross-process rebuild lock
    // away along with the re-check: it still takes
    // <index-dir>/ddb-rebuild.lock, same as locked_rebuild.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file(
        "ddb/20280601000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();

    let db_dir = dir.path().join(".ddb");
    std::fs::create_dir_all(&db_dir).unwrap();
    let idx = Index::open(&db_dir.join("index.db")).unwrap();

    let held = crate::git_ops::write_lock::acquire(
        &db_dir,
        "ddb-rebuild.lock",
        std::time::Duration::from_secs(10),
    )
    .expect("the test must be able to take the rebuild lock first");

    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = idx.locked_explicit_rebuild(&repo, false);
        tx.send(result).expect("the waiting test hung up early");
        idx
    });

    match rx.recv_timeout(std::time::Duration::from_millis(200)) {
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("the explicit-reindex worker died before returning a result")
        }
        Ok(early) => panic!(
            "locked_explicit_rebuild returned {early:?} while another holder \
             still owned <index-dir>/ddb-rebuild.lock — the explicit reindex \
             traded the lock away along with the re-check"
        ),
    }

    std::mem::drop(held);

    let idx = worker.join().unwrap();
    let report = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("locked_explicit_rebuild never returned after the lock was released")
        .expect("locked_explicit_rebuild that waited for the lock must then succeed, not error out");
    assert_eq!(
        report.indexed, 1,
        "the explicit reindex that waited for the lock must still index the repo's doogat"
    );
    let rows: i64 = idx
        .conn
        .query_row("SELECT COUNT(*) FROM doogats", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        rows, 1,
        "the explicit reindex that waited for the lock must leave the index populated"
    );
}

#[test]
fn lenient_explicit_reindex_over_a_poison_file_indexes_the_good_doogats_and_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20280701000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20280702000000.md",
        "---\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20280703000000.md",
        "---\n: invalid yaml [\n---\nbody",
        "add bad",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let report = idx.locked_explicit_rebuild(&repo, false).unwrap();

    assert_eq!(
        report.indexed, 2,
        "the lenient explicit reindex must index both good doogats"
    );

    // The poison file is currently reported TWICE, from two independent
    // stages that `Index::rebuild` runs in sequence: `parallel_parse` (the
    // `crate::parser::parse` Err arm in rebuild.rs) pushes a MalformedYaml
    // warning for it, and `collect_consistency_warnings` then walks the
    // corpus again, re-parses every file, and pushes a second MalformedYaml
    // warning for the same path. That duplication is a known, pre-existing
    // wart tracked for the review phase — fixing it is out of scope here.
    // Asserting `warnings.len() == 1` would bind the wart and break the day
    // someone legitimately de-duplicates the list, so instead this test
    // asserts on the SET of paths the warnings name: it must contain the
    // poison file and nothing else (never either good doogat), regardless of
    // how many times that one path is repeated.
    assert!(
        !report.warnings.is_empty(),
        "a poison file must produce at least one warning"
    );
    let warned_paths: std::collections::BTreeSet<&str> = report
        .warnings
        .iter()
        .map(|w| match w {
            crate::types::ConsistencyWarning::MalformedYaml { path, .. }
            | crate::types::ConsistencyWarning::UnreadableFile { path, .. }
            | crate::types::ConsistencyWarning::CrossZoneDuplicate { path, .. }
            | crate::types::ConsistencyWarning::MissingRequired { path, .. } => path.as_str(),
        })
        .collect();
    assert_eq!(
        warned_paths,
        std::collections::BTreeSet::from(["ddb/20280703000000.md"]),
        "every warning must name the poison file and no other path, got {:?}",
        report.warnings
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, crate::types::ConsistencyWarning::MalformedYaml { .. })),
        "at least one warning must be the MalformedYaml variant, got {:?}",
        report.warnings
    );
}

// -- over-cap frontmatter is a poison file like any other --

#[test]
fn rebuild_lenient_skips_the_over_cap_frontmatter_path_and_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20280801000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();

    // A 300 KiB frontmatter value comfortably exceeds the 256 KiB
    // MAX_FRONTMATTER_BYTES cap while remaining syntactically valid YAML (a
    // plain scalar value can be any length), so the rejection can only be
    // attributed to the cap, never to a YAML syntax error.
    let oversized_value = "a".repeat(300 * 1024);
    let content = format!("---\ntitle: {oversized_value}\n---\nBody B.");
    repo.commit_file("ddb/20280802000000.md", &content, "add oversized")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let report = idx.rebuild(&repo).unwrap();

    assert_eq!(
        report.indexed, 1,
        "the lenient rebuild must index the normal-sized doogat"
    );

    // The over-cap file is a poison file like any other malformed-YAML file:
    // `Index::rebuild` reports it from two independent stages
    // (`parallel_parse`'s `crate::parser::parse` Err arm, and
    // `collect_consistency_warnings` re-walking and re-parsing the corpus
    // afterwards), so ONE over-cap file yields TWO warnings. Assert on the
    // SET of warned paths, not the count, so a future de-duplication does
    // not turn this test red.
    let warned: std::collections::BTreeSet<&str> = report
        .warnings
        .iter()
        .map(|w| match w {
            crate::types::ConsistencyWarning::MalformedYaml { path, .. }
            | crate::types::ConsistencyWarning::UnreadableFile { path, .. }
            | crate::types::ConsistencyWarning::CrossZoneDuplicate { path, .. }
            | crate::types::ConsistencyWarning::MissingRequired { path, .. } => path.as_str(),
        })
        .collect();
    assert!(
        warned.contains("ddb/20280802000000.md"),
        "the over-cap file must be warned about, got {:?}",
        report.warnings
    );
    assert!(
        !warned.contains("ddb/20280801000000.md"),
        "the normal-sized file must NOT be warned about, got {:?}",
        report.warnings
    );
}

#[test]
fn rebuild_strict_reports_err_naming_the_over_cap_frontmatter_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20280901000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();

    let oversized_value = "a".repeat(300 * 1024);
    let content = format!("---\ntitle: {oversized_value}\n---\nBody B.");
    repo.commit_file("ddb/20280902000000.md", &content, "add oversized")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let result = idx.rebuild_strict(&repo);
    let err = result.expect_err("a strict rebuild must abort on an over-cap frontmatter file");
    assert!(
        err.to_string().contains("20280902000000.md"),
        "strict rebuild error must name the offending path, got: {err}"
    );
}

// -- a file modified into a poison file loses its stale rows --
//
// Every assertion below reads `idx.conn` directly. Reading back through a
// CLI/GraphQL/service path would call `ensure_fresh`, which reindexes before
// answering and would repair the very staleness under test, so those
// assertions could never fail.

/// The SET of paths named by a report's warnings. Asserting on the set rather
/// than the count keeps these tests from binding how many times one bad path
/// gets reported.
fn warned_paths(warnings: &[crate::types::ConsistencyWarning]) -> std::collections::BTreeSet<&str> {
    warnings
        .iter()
        .map(|w| match w {
            crate::types::ConsistencyWarning::MalformedYaml { path, .. }
            | crate::types::ConsistencyWarning::UnreadableFile { path, .. }
            | crate::types::ConsistencyWarning::CrossZoneDuplicate { path, .. }
            | crate::types::ConsistencyWarning::MissingRequired { path, .. } => path.as_str(),
        })
        .collect()
}

fn count_doogat_rows(idx: &Index, id: &str) -> i64 {
    idx.conn
        .query_row(
            "SELECT COUNT(*) FROM doogats WHERE id = ?1",
            rusqlite::params![id],
            |row| row.get(0),
        )
        .unwrap()
}

fn count_type_table_rows(idx: &Index, table: &str, id: &str) -> i64 {
    idx.conn
        .query_row(
            &format!("SELECT COUNT(*) FROM \"{table}\" WHERE id = ?1"),
            rusqlite::params![id],
            |row| row.get(0),
        )
        .unwrap()
}

fn doogat_title(idx: &Index, id: &str) -> String {
    idx.conn
        .query_row(
            "SELECT title FROM doogats WHERE id = ?1",
            rusqlite::params![id],
            |row| row.get(0),
        )
        .unwrap()
}

/// Every satellite table that hangs off a doogat. Eviction has to empty all of
/// them: leaving one populated keeps the doogat reachable through joins after
/// the index has declared the file unreadable.
const SATELLITE_TABLES: [&str; 5] = [
    "_ddb_tags",
    "_ddb_fields",
    "_ddb_links",
    "_ddb_aliases",
    "_ddb_checkboxes",
];

/// Rows a doogat still owns in one satellite table. `_ddb_links` names its
/// owning column `source_id`; the rest use `doogat_id`.
fn count_satellite_rows(idx: &Index, table: &str, id: &str) -> i64 {
    let owner = if table == "_ddb_links" {
        "source_id"
    } else {
        "doogat_id"
    };
    idx.conn
        .query_row(
            &format!("SELECT COUNT(*) FROM \"{table}\" WHERE \"{owner}\" = ?1"),
            rusqlite::params![id],
            |row| row.get(0),
        )
        .unwrap()
}

/// Rows left in the full-text index carrying `title`.
///
/// `_ddb_fts` is keyed by the `doogats` rowid, so once the `doogats` row is
/// deleted the FTS row is orphaned and can only be observed by its content —
/// which is exactly the state that keeps `ddb search` serving pre-edit text
/// for a file the index has declared unreadable.
fn count_fts_rows_titled(idx: &Index, title: &str) -> i64 {
    idx.conn
        .query_row(
            "SELECT COUNT(*) FROM _ddb_fts WHERE title = ?1",
            rusqlite::params![title],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn incremental_reindex_evicts_rows_for_a_doogat_modified_into_malformed_yaml() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20290101000000.md",
        "---\nid: 20290101000000\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    // B carries something in every satellite table plus a title no other row
    // in this repo shares, so a partial eviction that misses one table — or
    // leaves the orphaned FTS row behind — is observable.
    let b_content = "\
---
id: 20290102000000
title: Quixotic Beacon B
tags:
  - beacon/poison
aliases:
  - Beacon Alias
beacon_field: kept
---
- [ ] beacon task

See [[beacon-target]] for details.
";
    repo.commit_file("ddb/20290102000000.md", b_content, "add b")
        .unwrap();
    repo.commit_file(
        "ddb/20290103000000.md",
        "---\nid: 20290103000000\ntitle: C\n---\nBody C.",
        "add c",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    assert_eq!(idx.rebuild(&repo).unwrap().indexed, 3);
    let old_head = idx.stored_head_oid().unwrap();

    // Without these pre-asserts the eviction assertions below could pass
    // against an index that never held B's rows in the first place.
    assert_eq!(
        count_doogat_rows(&idx, "20290102000000"),
        1,
        "B must be indexed before it is poisoned, or the eviction assertion cannot fail"
    );
    assert_eq!(
        count_fts_rows_titled(&idx, "Quixotic Beacon B"),
        1,
        "B must be searchable before it is poisoned, or the FTS assertion cannot fail"
    );
    for table in SATELLITE_TABLES {
        assert!(
            count_satellite_rows(&idx, table, "20290102000000") > 0,
            "B must own rows in {table} before it is poisoned, \
             or the eviction assertion for that table cannot fail"
        );
    }

    // One commit modifies all three: A and C stay well-formed, B turns into
    // malformed YAML.
    let modifications: Vec<(&str, &str)> = vec![
        (
            "ddb/20290101000000.md",
            "---\nid: 20290101000000\ntitle: Modified A\n---\nUpdated body A.",
        ),
        ("ddb/20290102000000.md", "---\n: invalid yaml [\n---\nbody"),
        (
            "ddb/20290103000000.md",
            "---\nid: 20290103000000\ntitle: Modified C\n---\nUpdated body C.",
        ),
    ];
    repo.commit_files(&modifications, "modify 3, poison 1")
        .unwrap();

    let report = idx.incremental_reindex(&repo, &old_head, false).unwrap();

    assert_eq!(
        count_doogat_rows(&idx, "20290102000000"),
        0,
        "a doogat modified into malformed YAML must have its pre-edit rows evicted, \
         not left behind while the stored HEAD advances"
    );

    // Eviction has to be COMPLETE. A surviving FTS row keeps `ddb search`
    // answering with the pre-edit title and body of a file the index has
    // already declared unreadable — the exact stale read this rule prevents.
    assert_eq!(
        count_fts_rows_titled(&idx, "Quixotic Beacon B"),
        0,
        "the poisoned doogat must leave no full-text row behind, \
         or search still returns its pre-edit content"
    );
    for table in SATELLITE_TABLES {
        assert_eq!(
            count_satellite_rows(&idx, table, "20290102000000"),
            0,
            "the poisoned doogat must leave no row behind in {table}"
        );
    }

    // The batch must not be aborted, and the surviving rows must carry the
    // NEW content — an implementation that evicts the whole change-set, or
    // one that skips indexing after a poison file, fails here.
    assert_eq!(
        report.indexed, 2,
        "both well-formed files in the change-set must still be indexed"
    );
    assert_eq!(
        doogat_title(&idx, "20290101000000"),
        "Modified A",
        "A's row must reflect its post-edit content"
    );
    assert_eq!(
        doogat_title(&idx, "20290103000000"),
        "Modified C",
        "C's row must reflect its post-edit content"
    );

    // Eviction is in ADDITION to the warning, not instead of it.
    assert_eq!(
        warned_paths(&report.warnings),
        std::collections::BTreeSet::from(["ddb/20290102000000.md"]),
        "the skip warning must name the poison file and no other path, got {:?}",
        report.warnings
    );
    assert!(
        report.warnings.iter().any(|w| matches!(
            w,
            crate::types::ConsistencyWarning::MalformedYaml { path, .. }
            if path == "ddb/20290102000000.md"
        )),
        "eviction must not replace the MalformedYaml warning for the poison file, got {:?}",
        report.warnings
    );

    // Skipping a poison file must still CONVERGE. Gating `store_head` on an
    // empty warning list would leave the index permanently stale for any repo
    // holding one bad file: every `ensure_fresh` re-diffs the same range and
    // re-emits the same warnings, forever.
    let new_head = repo.head_oid().unwrap();
    assert_eq!(
        idx.stored_head_oid().as_deref(),
        Some(new_head.0.as_str()),
        "a lenient reindex that skipped a poison file must still advance the stored HEAD"
    );
    assert!(
        !idx.is_stale(&repo).unwrap(),
        "the index must report itself fresh after a lenient reindex, \
         or it never converges on a repo containing a poison file"
    );
}

#[test]
fn incremental_reindex_evicts_materialized_type_rows_for_a_poisoned_typed_doogat() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let typedef_content = "\
---
id: 20290201000000
title: items
type: _typedef
columns:
  - name: name
    data_type: TEXT
    zone: body
  - name: count
    data_type: INTEGER
    zone: frontmatter
---\n";
    repo.commit_file(
        "ddb/_typedef/20290201000000.md",
        typedef_content,
        "add typedef",
    )
    .unwrap();

    let data_content = "\
---
id: 20290202000000
title: Widget
type: items
count: 42
---

## name

Widget
";
    repo.commit_file("ddb/20290202000000.md", data_content, "add item")
        .unwrap();

    // A sibling of the SAME type, left untouched by the poisoning commit. With
    // only one row in `items`, "delete the poisoned row" and "empty the whole
    // table" are indistinguishable. It carries something in every satellite
    // table plus a title no other row shares, so an "eviction" that wipes the
    // satellite tables or the full-text index corpus-wide is observable on a
    // doogat the change-set never touched.
    let sibling_content = "\
---
id: 20290203000000
title: Serendipitous Gadget
type: items
count: 7
tags:
  - gadget/survivor
aliases:
  - Gadget Alias
gadget_field: kept
---

- [ ] gadget task

See [[gadget-target]] for details.

## name

Gadget
";
    repo.commit_file("ddb/20290203000000.md", sibling_content, "add sibling item")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    assert_eq!(idx.rebuild(&repo).unwrap().indexed, 3);
    let old_head = idx.stored_head_oid().unwrap();

    assert_eq!(
        count_type_table_rows(&idx, "items", "20290202000000"),
        1,
        "the typed doogat must be materialized before it is poisoned, \
         or the eviction assertion cannot fail"
    );
    assert_eq!(
        count_type_table_rows(&idx, "items", "20290203000000"),
        1,
        "the sibling must be materialized too, or its survival cannot be checked"
    );
    // The survivor is checked at the same resolution as the victim: without
    // these pre-asserts, a corpus-wide wipe of the satellite tables and the FTS
    // index would leave the survival assertions below trivially true.
    assert_eq!(
        count_fts_rows_titled(&idx, "Serendipitous Gadget"),
        1,
        "the sibling must be searchable before the poisoning, \
         or its FTS survival cannot be checked"
    );
    for table in SATELLITE_TABLES {
        assert!(
            count_satellite_rows(&idx, table, "20290203000000") > 0,
            "the sibling must own rows in {table} before the poisoning, \
             or its survival in that table cannot be checked"
        );
    }

    repo.commit_file(
        "ddb/20290202000000.md",
        "---\n: invalid yaml [\n---\nbody",
        "poison the item",
    )
    .unwrap();

    let report = idx.incremental_reindex(&repo, &old_head, false).unwrap();

    assert_eq!(
        count_type_table_rows(&idx, "items", "20290202000000"),
        0,
        "a typed doogat modified into malformed YAML must lose its materialized \
         type-table row, not keep serving pre-edit column values"
    );
    assert_eq!(
        count_doogat_rows(&idx, "20290202000000"),
        0,
        "the poisoned typed doogat must also lose its row in doogats"
    );
    // Eviction is scoped to the poisoned doogat. Emptying the type table is a
    // failure, not a pass.
    assert_eq!(
        count_type_table_rows(&idx, "items", "20290203000000"),
        1,
        "an untouched sibling row in the same type table must survive the eviction"
    );
    assert_eq!(
        count_doogat_rows(&idx, "20290203000000"),
        1,
        "the untouched sibling must also keep its row in doogats"
    );
    // Scoped means scoped everywhere, not just in `doogats` and the type table.
    // Wiping the satellite tables or the FTS index corpus-wide would strip every
    // untouched doogat of its tags, fields, links, aliases, checkboxes and
    // full-text row, so `ddb search` answers nothing until a full reindex.
    assert_eq!(
        count_fts_rows_titled(&idx, "Serendipitous Gadget"),
        1,
        "the untouched sibling must keep its full-text row: evicting one poison file \
         must not empty the search index for the rest of the corpus"
    );
    for table in SATELLITE_TABLES {
        assert!(
            count_satellite_rows(&idx, table, "20290203000000") > 0,
            "the untouched sibling must keep its rows in {table}: eviction is scoped to \
             the poisoned doogat, not a corpus-wide wipe"
        );
    }
    assert_eq!(
        count_doogat_rows(&idx, "20290201000000"),
        1,
        "the untouched typedef must survive the eviction"
    );
    assert_eq!(
        warned_paths(&report.warnings),
        std::collections::BTreeSet::from(["ddb/20290202000000.md"]),
        "the skip warning must still be reported for the poisoned typed doogat, got {:?}",
        report.warnings
    );

    // Skipping a poison file must still CONVERGE. Gating `store_head` on an
    // empty warning list would leave the index permanently stale for any repo
    // holding one bad file: every `ensure_fresh` re-diffs the same range and
    // re-emits the same warning, forever.
    let new_head = repo.head_oid().unwrap();
    assert_eq!(
        idx.stored_head_oid().as_deref(),
        Some(new_head.0.as_str()),
        "a lenient reindex that skipped a poison file must still advance the stored HEAD"
    );
    assert!(
        !idx.is_stale(&repo).unwrap(),
        "the index must report itself fresh after a lenient reindex, \
         or it never converges on a repo containing a poison file"
    );
}

#[test]
fn incremental_reindex_evicts_rows_for_a_doogat_modified_into_unreadable_bytes() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20290301000000.md",
        "---\nid: 20290301000000\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20290302000000.md",
        "---\nid: 20290302000000\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    assert_eq!(idx.rebuild(&repo).unwrap().indexed, 2);
    let old_head = idx.stored_head_oid().unwrap();

    assert_eq!(
        count_doogat_rows(&idx, "20290302000000"),
        1,
        "B must be indexed before it is poisoned, or the eviction assertion cannot fail"
    );

    commit_file_bytes(
        &repo,
        "ddb/20290302000000.md",
        &[0xFF, 0xFE, 0xFD],
        "poison b with invalid utf-8",
    );

    let report = idx.incremental_reindex(&repo, &old_head, false).unwrap();

    assert_eq!(
        count_doogat_rows(&idx, "20290302000000"),
        0,
        "a doogat modified into unreadable bytes must have its pre-edit rows evicted"
    );
    assert_eq!(
        count_doogat_rows(&idx, "20290301000000"),
        1,
        "a doogat outside the change-set must keep its rows"
    );
    assert_eq!(
        warned_paths(&report.warnings),
        std::collections::BTreeSet::from(["ddb/20290302000000.md"]),
        "the skip warning must name the unreadable file and no other path, got {:?}",
        report.warnings
    );
    assert!(
        report.warnings.iter().any(|w| matches!(
            w,
            crate::types::ConsistencyWarning::UnreadableFile { path, .. }
            if path == "ddb/20290302000000.md"
        )),
        "eviction must not replace the UnreadableFile warning, got {:?}",
        report.warnings
    );

    // Skipping a poison file must still CONVERGE. Gating `store_head` on an
    // empty warning list would leave the index permanently stale for any repo
    // holding one bad file: every `ensure_fresh` re-diffs the same range and
    // re-emits the same warnings, forever.
    let new_head = repo.head_oid().unwrap();
    assert_eq!(
        idx.stored_head_oid().as_deref(),
        Some(new_head.0.as_str()),
        "a lenient reindex that skipped a poison file must still advance the stored HEAD"
    );
    assert!(
        !idx.is_stale(&repo).unwrap(),
        "the index must report itself fresh after a lenient reindex, \
         or it never converges on a repo containing a poison file"
    );
}

#[test]
fn incremental_reindex_skips_a_newly_added_malformed_file_without_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20290401000000.md",
        "---\nid: 20290401000000\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    assert_eq!(idx.rebuild(&repo).unwrap().indexed, 1);
    let old_head = idx.stored_head_oid().unwrap();

    // Added, never indexed before: there is nothing to evict.
    repo.commit_file(
        "ddb/20290402000000.md",
        "---\n: invalid yaml [\n---\nbody",
        "add a malformed doogat",
    )
    .unwrap();

    let report = idx
        .incremental_reindex(&repo, &old_head, false)
        .expect("a newly added malformed file must be skipped, not turned into an error");

    assert_eq!(
        report.indexed, 0,
        "the malformed addition is the only change, so nothing can be indexed"
    );
    assert_eq!(
        count_doogat_rows(&idx, "20290402000000"),
        0,
        "a never-indexed malformed file must have no row"
    );
    assert_eq!(
        count_doogat_rows(&idx, "20290401000000"),
        1,
        "evicting nothing must not disturb the already-indexed doogat"
    );
    assert_eq!(
        warned_paths(&report.warnings),
        std::collections::BTreeSet::from(["ddb/20290402000000.md"]),
        "the newly added malformed file must still be warned about, got {:?}",
        report.warnings
    );
}

#[test]
fn incremental_reindex_strict_returns_err_for_a_change_set_with_a_poisoned_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20290501000000.md",
        "---\nid: 20290501000000\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20290502000000.md",
        "---\nid: 20290502000000\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    assert_eq!(idx.rebuild(&repo).unwrap().indexed, 2);
    let old_head = idx.stored_head_oid().unwrap();

    let modifications: Vec<(&str, &str)> = vec![
        ("ddb/20290501000000.md", "---\n: invalid yaml [\n---\nbody"),
        (
            "ddb/20290502000000.md",
            "---\nid: 20290502000000\ntitle: Modified B\n---\nUpdated body B.",
        ),
    ];
    repo.commit_files(&modifications, "modify 2, poison 1")
        .unwrap();

    let result = idx.incremental_reindex(&repo, &old_head, true);
    assert!(
        result.is_err(),
        "strict mode must fail the reindex on a poison file instead of skipping it"
    );

    // Returning Err is only half the contract: the abort must leave the index
    // exactly as it was. An implementation that evicts (or re-indexes) first
    // and errors afterwards has already damaged a previously-good index by the
    // time the caller sees the failure. The full-rebuild path defends against
    // this same ordering hazard; the incremental path must too.
    assert_eq!(
        count_doogat_rows(&idx, "20290501000000"),
        1,
        "a strict abort must not evict the poisoned doogat's previously-indexed row"
    );
    assert_eq!(
        doogat_title(&idx, "20290501000000"),
        "A",
        "the poisoned doogat's row must still carry its ORIGINAL pre-edit title after a strict abort"
    );
    assert_eq!(
        doogat_title(&idx, "20290502000000"),
        "B",
        "the good file in the same change-set must not have been re-indexed by an aborted strict run"
    );
    assert_eq!(
        idx.stored_head_oid().as_deref(),
        Some(old_head.as_str()),
        "a strict abort must leave the stored HEAD at old_head"
    );
}
