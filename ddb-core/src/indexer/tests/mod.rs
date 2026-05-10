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

    let report = idx.incremental_reindex(&repo, &old_head).unwrap();
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
    let idx =
        Index::configure_connection(conn).expect("configure_connection should upgrade old schema");

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
