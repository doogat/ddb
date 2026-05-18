use super::*;

#[test]
fn rebuild_and_staleness() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let doogat_content =
        "---\nid: 20260226120000\ntitle: Rebuild Test\ntags:\n  - test\n---\nBody here.";
    repo.commit_file("ddb/20260226120000.md", doogat_content, "add doogat")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    // Initially stale (no head recorded)
    assert!(idx.is_stale(&repo).unwrap());

    let report = idx.rebuild(&repo).unwrap();
    assert_eq!(report.indexed, 1);

    // No longer stale
    assert!(!idx.is_stale(&repo).unwrap());

    // rebuild_if_stale should skip
    assert!(idx.rebuild_if_stale(&repo).unwrap().is_none());

    // After new commit, should be stale again
    repo.commit_file(
        "ddb/20260226130000.md",
        "---\ntitle: New\n---\nNew body.",
        "add another",
    )
    .unwrap();
    assert!(idx.is_stale(&repo).unwrap());

    // Incremental reindex only processes changed files (1 new doogat)
    let report = idx.rebuild_if_stale(&repo).unwrap().unwrap();
    assert_eq!(report.indexed, 1);
}

#[test]
fn rebuild_materializes_user_tables() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    // Create a typedef doogat
    let schema_content = "\
---
id: 20260226140000
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
        "ddb/_typedef/20260226140000.md",
        schema_content,
        "add typedef",
    )
    .unwrap();

    // Create a data doogat matching the schema
    let data_content = "\
---
id: 20260226140100
title: Widget
type: items
count: 42
---

## name

Widget
";
    repo.commit_file("ddb/20260226140100.md", data_content, "add item")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    let report = idx.rebuild(&repo).unwrap();
    assert_eq!(report.indexed, 2);

    // Materialized table should exist and have data
    let rows = idx.query_raw("SELECT name, count FROM items").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "Widget");
    assert_eq!(rows[0][1], "42");
}

#[test]
fn materialize_from_cached_matches_repo() {
    use crate::traits::mock::MockSource;

    // Typedef doogat
    let typedef_content = "\
---
id: 20260226140000
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

    // Data doogat
    let data_content = "\
---
id: 20260226140100
title: Widget
type: items
count: 42
---

## name

Widget
";

    let mut source = MockSource::new();
    source.files.insert(
        "ddb/_typedef/20260226140000.md".into(),
        typedef_content.into(),
    );
    source
        .files
        .insert("ddb/20260226140100.md".into(), data_content.into());

    // Build parsed doogats
    let paths = source.list_doogats().unwrap();
    let parsed: Vec<ParsedDoogat> = paths
        .iter()
        .map(|p| {
            let c = source.read_file(p).unwrap();
            crate::parser::parse(&c, p).unwrap()
        })
        .collect();

    // Path A: repo-based materialization
    let idx_repo = in_memory_index();
    idx_repo.batch_index(&parsed).unwrap();
    let (mat_a, inf_a) = idx_repo.materialize_all_types(&source).unwrap();

    // Path B: cached materialization
    let idx_cached = in_memory_index();
    idx_cached.batch_index(&parsed).unwrap();
    let (mat_b, inf_b) = idx_cached.materialize_all_types_from(&parsed).unwrap();

    assert_eq!(mat_a, mat_b);
    assert_eq!(inf_a, inf_b);

    // Compare materialized table contents
    let rows_a = idx_repo.query_raw("SELECT name, count FROM items").unwrap();
    let rows_b = idx_cached
        .query_raw("SELECT name, count FROM items")
        .unwrap();
    assert_eq!(rows_a, rows_b);
    assert_eq!(rows_a[0][0], "Widget");
    assert_eq!(rows_a[0][1], "42");
}

#[test]
fn rebuild_deterministic_across_runs() {
    use crate::traits::mock::MockSource;

    let mut source = MockSource::new();
    // 20 doogats of mixed types
    for i in 0..15 {
        let id = format!("{:014}", 20260226120000u64 + i);
        let content = format!(
                "---\nid: {id}\ntitle: Note {i}\ndate: 2026-02-26\ntype: permanent\ntags:\n  - test\n---\nBody of {i}.\n---\n- source:: ref-{i}"
            );
        source.files.insert(format!("ddb/{id}.md"), content);
    }
    for i in 0..5 {
        let id = format!("{:014}", 20260226130000u64 + i);
        let content = format!(
                "---\nid: {id}\ntitle: Task {i}\ndate: 2026-02-26\ntype: task\npriority: {i}\n---\nTask body {i}."
            );
        source.files.insert(format!("ddb/{id}.md"), content);
    }

    // Rebuild twice into separate indexes
    let idx_a = in_memory_index();
    let report_a = idx_a.rebuild(&source).unwrap();

    let idx_b = in_memory_index();
    let report_b = idx_b.rebuild(&source).unwrap();

    assert_eq!(report_a.indexed, report_b.indexed);

    // Compare all core tables
    for table in &[
        "doogats",
        "_ddb_tags",
        "_ddb_fields",
        "_ddb_links",
        "_ddb_checkboxes",
    ] {
        let rows_a = dump_table(&idx_a, table);
        let rows_b = dump_table(&idx_b, table);
        if *table == "doogats" {
            // Skip updated_at column
            for (a, b) in rows_a.iter().zip(rows_b.iter()) {
                assert_eq!(&a[..6], &b[..6], "doogats row mismatch");
            }
        } else {
            assert_eq!(rows_a, rows_b, "mismatch in {table}");
        }
    }

    // Verify FTS produces same results
    let fts_a = idx_a.search("Body").unwrap();
    let fts_b = idx_b.search("Body").unwrap();
    assert_eq!(fts_a.len(), fts_b.len());
}

#[test]
fn infer_schema_frontmatter_types() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let z1 = "---\nid: 20260226150000\ntitle: Task 1\ntype: task\npriority: 1\ndone: true\nscore: 3.5\n---\nBody.";
    let z2 = "---\nid: 20260226150100\ntitle: Task 2\ntype: task\npriority: 2\ndone: false\nscore: 7.0\n---\nBody.";
    repo.commit_file("ddb/20260226150000.md", z1, "add task 1")
        .unwrap();
    repo.commit_file("ddb/20260226150100.md", z2, "add task 2")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    let schema = idx.infer_schema("task", &repo).unwrap();
    assert_eq!(schema.table_name, "task");

    let find = |name: &str| schema.columns.iter().find(|c| c.name == name);

    let done = find("done").expect("done column");
    assert_eq!(done.data_type, "BOOLEAN");
    assert_eq!(done.zone, Some(Zone::Frontmatter));

    let priority = find("priority").expect("priority column");
    assert_eq!(priority.data_type, "INTEGER");

    let score = find("score").expect("score column");
    assert_eq!(score.data_type, "REAL");
}

#[test]
fn infer_schema_body_headings() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let z1 = "---\nid: 20260226160000\ntitle: Note 1\ntype: article\n---\n\n## Summary\n\nSome text\n\n## Details\n\nMore text";
    repo.commit_file("ddb/20260226160000.md", z1, "add article")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    let schema = idx.infer_schema("article", &repo).unwrap();
    let find = |name: &str| schema.columns.iter().find(|c| c.name == name);

    let summary = find("summary").expect("summary column");
    assert_eq!(summary.data_type, "TEXT");
    assert_eq!(summary.zone, Some(Zone::Body));

    let details = find("details").expect("details column");
    assert_eq!(details.data_type, "TEXT");
    assert_eq!(details.zone, Some(Zone::Body));
}

#[test]
fn infer_schema_ignores_code_block_headings() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let z1 = "---\nid: 20260226160100\ntitle: Code\ntype: article\n---\n\n## Real\n\nContent\n\n```\n## Fake\ncode block\n```";
    repo.commit_file("ddb/20260226160100.md", z1, "add")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    let schema = idx.infer_schema("article", &repo).unwrap();
    let find = |name: &str| schema.columns.iter().find(|c| c.name == name);

    assert!(find("real").is_some(), "Real heading should be a column");
    assert!(
        find("fake").is_none(),
        "Code block heading should not be a column"
    );
}

#[test]
fn infer_schema_reference_fields() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let z1 = "---\nid: 20260226170000\ntitle: Proj 1\ntype: project\n---\n\nBody\n\n---\n\n- parent:: [[20260226170100]]\n- ticket:: JIRA-123";
    repo.commit_file("ddb/20260226170000.md", z1, "add project")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    let schema = idx.infer_schema("project", &repo).unwrap();
    let find = |name: &str| schema.columns.iter().find(|c| c.name == name);

    let parent = find("parent").expect("parent column");
    assert_eq!(parent.zone, Some(Zone::Reference));

    let ticket = find("ticket").expect("ticket column");
    assert_eq!(ticket.zone, Some(Zone::Reference));
}

#[test]
fn infer_schema_empty_type() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file(
        "ddb/20260226180000.md",
        "---\ntitle: Dummy\n---\nBody",
        "add",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    let schema = idx.infer_schema("nonexistent", &repo).unwrap();
    assert!(schema.columns.is_empty());
    assert_eq!(schema.table_name, "nonexistent");
}

#[test]
fn infer_schema_type_widening() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let z1 = "---\nid: 20260226190000\ntitle: A\ntype: mixed\ncount: 5\n---\nBody.";
    let z2 = "---\nid: 20260226190100\ntitle: B\ntype: mixed\ncount: many\n---\nBody.";
    repo.commit_file("ddb/20260226190000.md", z1, "add A")
        .unwrap();
    repo.commit_file("ddb/20260226190100.md", z2, "add B")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    let schema = idx.infer_schema("mixed", &repo).unwrap();
    let count = schema
        .columns
        .iter()
        .find(|c| c.name == "count")
        .expect("count column");
    assert_eq!(count.data_type, "TEXT");
}

#[test]
fn infer_schema_case_variant_keys_deduplicated() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    // Frontmatter with case-variant keys: xP and xp
    let z1 = "---\nid: 20260226200000\ntitle: Dupe\ntype: dupe\nxP: a\nxp: A\n---\nBody.";
    repo.commit_file("ddb/20260226200000.md", z1, "add dupe")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    let schema = idx.infer_schema("dupe", &repo).unwrap();
    let xp_cols: Vec<_> = schema
        .columns
        .iter()
        .filter(|c| c.name.eq_ignore_ascii_case("xp"))
        .collect();
    assert_eq!(
        xp_cols.len(),
        1,
        "case-variant keys should merge into one column"
    );
    assert_eq!(xp_cols[0].name, "xp");
}

#[test]
fn merge_schemas_typedef_only() {
    use crate::types::{ColumnDef, TableSchema};

    let typedef = TableSchema {
        table_name: "foo".to_string(),
        columns: vec![
            ColumnDef {
                name: "a".into(),
                data_type: "TEXT".into(),
                references: None,
                zone: Some(Zone::Body),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
                on_delete: crate::types::OnDeleteAction::Restrict,
            },
            ColumnDef {
                name: "b".into(),
                data_type: "INTEGER".into(),
                references: None,
                zone: Some(Zone::Frontmatter),
                required: true,
                search_boost: None,
                allowed_values: None,
                default_value: None,
                on_delete: crate::types::OnDeleteAction::Restrict,
            },
        ],
        crdt_strategy: Some("preset:default".into()),
        template_sections: vec!["A".into()],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
        search_key: None,
        singleton: false,
    };
    let inferred = TableSchema {
        table_name: "foo".to_string(),
        columns: vec![],
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
        search_key: None,
        singleton: false,
    };

    let merged = Index::merge_schemas(Some(typedef), inferred);
    assert_eq!(merged.columns.len(), 2);
    assert_eq!(merged.crdt_strategy, Some("preset:default".to_string()));
}

#[test]
fn merge_schemas_inferred_only() {
    use crate::types::{ColumnDef, TableSchema};

    let inferred = TableSchema {
        table_name: "bar".to_string(),
        columns: vec![ColumnDef {
            name: "x".into(),
            data_type: "INTEGER".into(),
            references: None,
            zone: Some(Zone::Frontmatter),
            required: false,
            search_boost: None,
            allowed_values: None,
            default_value: None,
            on_delete: crate::types::OnDeleteAction::Restrict,
        }],
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
        search_key: None,
        singleton: false,
    };

    let merged = Index::merge_schemas(None, inferred);
    assert_eq!(merged.columns.len(), 1);
    assert_eq!(merged.table_name, "bar");
}

#[test]
fn merge_schemas_overlap() {
    use crate::types::{ColumnDef, TableSchema};

    let typedef = TableSchema {
        table_name: "baz".to_string(),
        columns: vec![ColumnDef {
            name: "shared".into(),
            data_type: "INTEGER".into(),
            references: None,
            zone: Some(Zone::Frontmatter),
            required: true,
            search_boost: Some(2.0),
            allowed_values: None,
            default_value: None,
            on_delete: crate::types::OnDeleteAction::Restrict,
        }],
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
        search_key: None,
        singleton: false,
    };
    let inferred = TableSchema {
        table_name: "baz".to_string(),
        columns: vec![
            ColumnDef {
                name: "shared".into(),
                data_type: "TEXT".into(),
                references: None,
                zone: Some(Zone::Body),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
                on_delete: crate::types::OnDeleteAction::Restrict,
            },
            ColumnDef {
                name: "extra".into(),
                data_type: "TEXT".into(),
                references: None,
                zone: Some(Zone::Body),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
                on_delete: crate::types::OnDeleteAction::Restrict,
            },
        ],
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
        search_key: None,
        singleton: false,
    };

    let merged = Index::merge_schemas(Some(typedef), inferred);
    assert_eq!(merged.columns.len(), 2);
    let shared = merged.columns.iter().find(|c| c.name == "shared").unwrap();
    assert_eq!(shared.data_type, "INTEGER");
    assert!(shared.required);
    assert!(merged.columns.iter().any(|c| c.name == "extra"));
}

#[test]
fn merge_schemas_no_overlap() {
    use crate::types::{ColumnDef, TableSchema};

    let typedef = TableSchema {
        table_name: "qux".to_string(),
        columns: vec![ColumnDef {
            name: "a".into(),
            data_type: "TEXT".into(),
            references: None,
            zone: Some(Zone::Body),
            required: false,
            search_boost: None,
            allowed_values: None,
            default_value: None,
            on_delete: crate::types::OnDeleteAction::Restrict,
        }],
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
        search_key: None,
        singleton: false,
    };
    let inferred = TableSchema {
        table_name: "qux".to_string(),
        columns: vec![
            ColumnDef {
                name: "b".into(),
                data_type: "INTEGER".into(),
                references: None,
                zone: Some(Zone::Frontmatter),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
                on_delete: crate::types::OnDeleteAction::Restrict,
            },
            ColumnDef {
                name: "c".into(),
                data_type: "REAL".into(),
                references: None,
                zone: Some(Zone::Frontmatter),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
                on_delete: crate::types::OnDeleteAction::Restrict,
            },
        ],
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
        search_key: None,
        singleton: false,
    };

    let merged = Index::merge_schemas(Some(typedef), inferred);
    assert_eq!(merged.columns.len(), 3);
}

#[test]
fn consistency_warnings_valid_doogat() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let z = "---\nid: 20260226200000\ntitle: Valid\ntype: note\n---\nBody text.";
    repo.commit_file("ddb/20260226200000.md", z, "add").unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    let warnings = idx.collect_consistency_warnings(&repo);
    assert!(warnings.is_empty());
}

#[test]
fn consistency_warnings_missing_required() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let typedef_content = "---\nid: 20260226210000\ntitle: task\ntype: _typedef\ncolumns:\n  - name: priority\n    data_type: INTEGER\n    zone: frontmatter\n    required: true\n---\n";
    repo.commit_file(
        "ddb/_typedef/20260226210000.md",
        typedef_content,
        "add typedef",
    )
    .unwrap();

    let z = "---\nid: 20260226210100\ntitle: My Task\ntype: task\n---\nBody.";
    repo.commit_file("ddb/20260226210100.md", z, "add task")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    let warnings = idx.collect_consistency_warnings(&repo);
    assert!(!warnings.is_empty());
    let has_missing = warnings.iter().any(|w| {
        matches!(w,
            crate::types::ConsistencyWarning::MissingRequired { field, .. } if field == "priority"
        )
    });
    assert!(
        has_missing,
        "should warn about missing required 'priority' field"
    );
}

/// PRD 00140 review cycle 1: a doogat with malformed frontmatter must surface
/// as a structured `MalformedYaml` warning rather than being silently dropped
/// from the consistency scan. Pins the read-path "surface, don't silently
/// drop" contract for the one failure branch reachable through the public API.
///
/// Implementation note — why the sibling warn branches are not failure-tested:
/// `collect_consistency_warnings`' `list_doogats`/`read_file` error arms, and
/// the `read_file`/`resolve_path` drop arms in `service::search::typed_filtered_list`
/// and `indexer::materialize::rematerialize_type`/`load_all_typedefs`, fire only
/// on index/git divergence (a path present in one but absent from the other).
/// That state is unreachable through the public API — the index reconciles
/// against git HEAD via staleness detection before any of these paths run — so
/// direct failure injection would require deliberately-inconsistent test-only
/// infrastructure. Those arms are verified by inspection against this
/// already-tested malformed-input path and the shipped `list_doogats_filtered`
/// sibling pattern; they add `tracing::warn!` diagnostics for latent corruption.
#[test]
fn consistency_warnings_surface_malformed_yaml() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let valid = "---\nid: 20260226260000\ntitle: Valid\ntype: note\n---\nBody.";
    repo.commit_file("ddb/20260226260000.md", valid, "add valid")
        .unwrap();

    // Unclosed YAML flow sequence in the frontmatter zone — fails to parse.
    let malformed = "---\nid: 20260226260100\ntitle: [unclosed\ntype: note\n---\nBody.";
    repo.commit_file("ddb/20260226260100.md", malformed, "add malformed")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    let warnings = idx.collect_consistency_warnings(&repo);
    let has_malformed = warnings.iter().any(|w| {
        matches!(w,
            crate::types::ConsistencyWarning::MalformedYaml { path, .. }
                if path == "ddb/20260226260100.md"
        )
    });
    assert!(
        has_malformed,
        "malformed doogat must surface as a MalformedYaml warning, not be silently dropped; got: {warnings:?}"
    );
}

#[test]
fn integration_typedef_plus_inferred_merge() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    // Create typedef with 2 columns
    let typedef = "---\nid: 20260226230000\ntitle: widget\ntype: _typedef\ncolumns:\n  - name: weight\n    data_type: REAL\n    zone: frontmatter\n  - name: color\n    data_type: TEXT\n    zone: frontmatter\n---\n";
    repo.commit_file("ddb/_typedef/20260226230000.md", typedef, "add typedef")
        .unwrap();

    // Create doogat with 3 extra fields (2 from typedef + 1 new)
    let z = "---\nid: 20260226230100\ntitle: Red Widget\ntype: widget\nweight: 2.5\ncolor: red\nsize: large\n---\n\nBody";
    repo.commit_file("ddb/20260226230100.md", z, "add widget")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    // Table should have 3 columns (2 typedef + 1 inferred "size")
    let rows = idx
        .query_raw("SELECT weight, color, size FROM widget")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "2.5");
    assert_eq!(rows[0][1], "red");
    assert_eq!(rows[0][2], "large");
}

#[test]
fn integration_consistency_warnings_in_rebuild() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    // Create typedef with required field
    let typedef = "---\nid: 20260226250000\ntitle: strict\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n    required: true\n---\n";
    repo.commit_file("ddb/_typedef/20260226250000.md", typedef, "add typedef")
        .unwrap();

    // Create doogat missing required field
    let z = "---\nid: 20260226250100\ntitle: Incomplete\ntype: strict\n---\nBody";
    repo.commit_file("ddb/20260226250100.md", z, "add incomplete")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    let report = idx.rebuild(&repo).unwrap();

    // Should have warnings but still index
    assert!(!report.warnings.is_empty());
    assert_eq!(report.indexed, 2); // typedef + data doogat both indexed

    // Data should still be accessible
    let rows = idx
        .query_raw("SELECT id FROM doogats WHERE type = 'strict'")
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn rebuild_via_mock_source() {
    use crate::traits::mock::MockSource;

    let mut source = MockSource::new();
    source.files.insert(
        "ddb/20260226120000.md".into(),
        "---\ntitle: Mock Note\ntype: permanent\ntags:\n  - test\n---\nBody text.\n".into(),
    );
    source.files.insert(
        "ddb/20260226120001.md".into(),
        "---\ntitle: Second Note\ntype: permanent\n---\nMore text.\n".into(),
    );

    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("index.db");
    let idx = Index::open(&db_path).unwrap();
    let report = idx.rebuild(&source).unwrap();

    assert_eq!(report.indexed, 2);
    assert!(!idx.is_stale(&source).unwrap());

    let results = idx.search("Mock").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Mock Note");
}

#[test]
fn infer_schema_via_mock_source() {
    use crate::traits::mock::MockSource;

    let mut source = MockSource::new();
    source.files.insert(
            "ddb/20260226120000.md".into(),
            "---\ntitle: Project A\ntype: project\npriority: 1\nactive: true\n---\n## Notes\nSome notes.\n".into(),
        );

    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("index.db");
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&source).unwrap();

    let schema = idx.infer_schema("project", &source).unwrap();
    let col_names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
    assert!(col_names.contains(&"priority"));
    assert!(col_names.contains(&"active"));
    assert!(col_names.contains(&"notes"));
}

#[test]
fn materialize_emits_check_constraint() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let typedef = "---\nid: 20260301100100\ntitle: task\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n    allowed_values:\n      - todo\n      - doing\n      - done\n---\n";
    repo.commit_file("ddb/_typedef/20260301100100.md", typedef, "add typedef")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    // Verify CHECK constraint exists by reading table info
    let sql = idx
        .conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='task'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert!(sql.contains("CHECK"), "expected CHECK constraint in: {sql}");
    assert!(sql.contains("'todo'"));
    assert!(sql.contains("'doing'"));
    assert!(sql.contains("'done'"));
}

#[test]
fn unique_index_created_for_unique_together_constraint() {
    use crate::traits::mock::MockSource;

    let typedef_content = "\
---
id: 20260401100000
title: membership
type: _typedef
columns:
  - name: link_id
    data_type: TEXT
    zone: frontmatter
  - name: cat
    data_type: TEXT
    zone: frontmatter
unique_together:
  - - link_id
    - cat
---\n";

    let mut source = MockSource::new();
    source.files.insert(
        "ddb/_typedef/20260401100000.md".into(),
        typedef_content.into(),
    );

    let idx = in_memory_index();
    idx.rebuild(&source).unwrap();

    // Query for a unique index covering both link_id and cat columns
    let mut stmt = idx
            .conn
            .prepare(
                "SELECT sql FROM sqlite_master WHERE type='index' AND tbl_name='membership' AND sql LIKE '%UNIQUE%'",
            )
            .unwrap();
    let index_sqls: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    let has_composite_unique = index_sqls
        .iter()
        .any(|sql| sql.contains("link_id") && sql.contains("cat"));

    assert!(
        has_composite_unique,
        "expected a unique index covering (link_id, cat) on 'membership', found: {index_sqls:?}"
    );
}

#[test]
fn unique_constraint_enforced_on_duplicate_values() {
    use crate::traits::mock::MockSource;

    let typedef_content = "\
---
id: 20260401110000
title: membership
type: _typedef
columns:
  - name: link_id
    data_type: TEXT
    zone: frontmatter
  - name: cat
    data_type: TEXT
    zone: frontmatter
unique_together:
  - - link_id
    - cat
---\n";

    let mut source = MockSource::new();
    source.files.insert(
        "ddb/_typedef/20260401110000.md".into(),
        typedef_content.into(),
    );

    let idx = in_memory_index();
    idx.rebuild(&source).unwrap();

    // First insert should succeed
    idx.conn
            .execute(
                "INSERT INTO membership (id, title, date, updated_at, link_id, cat) VALUES ('1', 'A', NULL, NULL, 'L1', 'work')",
                [],
            )
            .expect("first insert should succeed");

    // Duplicate (link_id, cat) must fail with a UNIQUE constraint violation
    let result = idx.conn.execute(
            "INSERT INTO membership (id, title, date, updated_at, link_id, cat) VALUES ('2', 'B', NULL, NULL, 'L1', 'work')",
            [],
        );
    assert!(
        result.is_err(),
        "duplicate (link_id, cat) insert should fail"
    );

    // Different cat value should succeed
    idx.conn
            .execute(
                "INSERT INTO membership (id, title, date, updated_at, link_id, cat) VALUES ('3', 'C', NULL, NULL, 'L1', 'tech')",
                [],
            )
            .expect("insert with different cat should succeed");
}

#[test]
fn singleton_lock_index_created_for_singleton_typedef() {
    // PRD 00139 §3 layer 3: rebuild emits the singleton-lock UNIQUE
    // expression-index for typedefs marked `singleton: true`.
    use crate::traits::mock::MockSource;

    let typedef_content = "\
---
id: 20260510120000
title: app_config
type: _typedef
singleton: true
columns:
  - name: theme
    data_type: TEXT
    zone: frontmatter
---\n";

    let mut source = MockSource::new();
    source.files.insert(
        "ddb/_typedef/20260510120000.md".into(),
        typedef_content.into(),
    );

    let idx = in_memory_index();
    idx.rebuild(&source).unwrap();

    let mut stmt = idx
            .conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='app_config' AND name='app_config_singleton_lock'",
            )
            .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(
        names,
        vec!["app_config_singleton_lock".to_string()],
        "expected singleton lock index on 'app_config'"
    );
}

#[test]
fn singleton_materialize_second_row_returns_structured_violation() {
    // PRD 00139 §3 layer 3: the production materialize path must reject a
    // second row in a SINGLETON typedef with a structured error, not
    // silently replace the existing row.
    use crate::error::{codes, DoogatError, ErrorValue};
    use crate::sql_engine::schema_from_parsed;
    use crate::traits::mock::MockSource;

    let typedef_content = "\
---
id: 20260510121000
title: app_config
type: _typedef
singleton: true
columns:
  - name: theme
    data_type: TEXT
    zone: frontmatter
---\n";

    let mut source = MockSource::new();
    source.files.insert(
        "ddb/_typedef/20260510121000.md".into(),
        typedef_content.into(),
    );

    let typedef = crate::parser::parse(typedef_content, "ddb/_typedef/20260510121000.md").unwrap();
    let schema = schema_from_parsed(&typedef).unwrap();
    let idx = in_memory_index();
    idx.rebuild(&source).unwrap();

    let first = crate::parser::parse(
        "\
---
id: 20260510121100
title: First Config
type: app_config
theme: dark
---\n",
        "ddb/20260510121100.md",
    )
    .unwrap();
    idx.index_doogat(&first).unwrap();
    idx.materialize_single(&schema, "20260510121100", &first)
        .expect("first materialize into singleton typedef should succeed");

    let second = crate::parser::parse(
        "\
---
id: 20260510121200
title: Second Config
type: app_config
theme: light
---\n",
        "ddb/20260510121200.md",
    )
    .unwrap();
    idx.index_doogat(&second).unwrap();
    let err = idx
        .materialize_single(&schema, "20260510121200", &second)
        .expect_err("second materialize must reject with singleton violation");
    match err {
        DoogatError::Structured { code, context, .. } => {
            assert_eq!(code, codes::SINGLETON_VIOLATION);
            let table = context
                .iter()
                .find(|(k, _)| k == "table")
                .map(|(_, v)| v)
                .expect("table context entry");
            assert_eq!(table, &ErrorValue::String("app_config".into()));
            let existing = context
                .iter()
                .find(|(k, _)| k == "existing_id")
                .map(|(_, v)| v)
                .expect("existing_id context entry");
            assert_eq!(existing, &ErrorValue::String("20260510121100".into()));
        }
        other => panic!("expected Structured SINGLETON_VIOLATION, got {other:?}"),
    }
}

#[test]
fn singleton_materialize_same_id_rematerialize_succeeds() {
    // PRD 00139 §3 layer 3: re-materializing the same row id during a
    // rebuild/update is still legal; the singleton guard only blocks a
    // different id from taking a second slot.
    use crate::sql_engine::schema_from_parsed;
    use crate::traits::mock::MockSource;

    let typedef_content = "\
---
id: 20260510121300
title: app_config
type: _typedef
singleton: true
columns:
  - name: theme
    data_type: TEXT
    zone: frontmatter
---\n";

    let mut source = MockSource::new();
    source.files.insert(
        "ddb/_typedef/20260510121300.md".into(),
        typedef_content.into(),
    );

    let typedef = crate::parser::parse(typedef_content, "ddb/_typedef/20260510121300.md").unwrap();
    let schema = schema_from_parsed(&typedef).unwrap();
    let idx = in_memory_index();
    idx.rebuild(&source).unwrap();

    let original = crate::parser::parse(
        "\
---
id: 20260510121400
title: App Config
type: app_config
theme: dark
---\n",
        "ddb/20260510121400.md",
    )
    .unwrap();
    idx.index_doogat(&original).unwrap();
    idx.materialize_single(&schema, "20260510121400", &original)
        .expect("first materialize should succeed");

    let updated = crate::parser::parse(
        "\
---
id: 20260510121400
title: App Config
type: app_config
theme: light
---\n",
        "ddb/20260510121400.md",
    )
    .unwrap();
    idx.index_doogat(&updated).unwrap();
    idx.materialize_single(&schema, "20260510121400", &updated)
        .expect("same-id rematerialize must succeed");

    let rows = idx
        .query_raw("SELECT id, theme FROM app_config")
        .expect("query rematerialized singleton row");
    assert_eq!(
        rows,
        vec![vec![String::from("20260510121400"), String::from("light"),]]
    );
}

#[test]
fn singleton_lock_index_absent_for_non_singleton_typedef() {
    // PRD 00139 §3 layer 3: non-singleton typedefs must not get the
    // lock index — otherwise plain typedefs could only ever hold one
    // row.
    use crate::traits::mock::MockSource;

    let typedef_content = "\
---
id: 20260510122000
title: link
type: _typedef
columns:
  - name: url
    data_type: TEXT
    zone: frontmatter
---\n";

    let mut source = MockSource::new();
    source.files.insert(
        "ddb/_typedef/20260510122000.md".into(),
        typedef_content.into(),
    );

    let idx = in_memory_index();
    idx.rebuild(&source).unwrap();

    let mut stmt = idx
            .conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='link' AND name='link_singleton_lock'",
            )
            .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(
        names.is_empty(),
        "non-singleton typedef must not have a singleton lock index"
    );
}

#[test]
fn no_unique_index_when_unique_together_absent() {
    use crate::traits::mock::MockSource;

    let typedef_content = "\
---
id: 20260401120000
title: item
type: _typedef
columns:
  - name: name
    data_type: TEXT
    zone: frontmatter
---\n";

    let mut source = MockSource::new();
    source.files.insert(
        "ddb/_typedef/20260401120000.md".into(),
        typedef_content.into(),
    );

    let idx = in_memory_index();
    idx.rebuild(&source).unwrap();

    // Both inserts with the same name should succeed (no unique constraint)
    idx.conn
            .execute(
                "INSERT INTO item (id, title, date, updated_at, name) VALUES ('1', 'A', NULL, NULL, 'foo')",
                [],
            )
            .expect("first insert should succeed");

    idx.conn
            .execute(
                "INSERT INTO item (id, title, date, updated_at, name) VALUES ('2', 'B', NULL, NULL, 'foo')",
                [],
            )
            .expect("second insert with same name should succeed when unique_together is absent");
}

#[test]
fn incremental_reindex_only_processes_changed_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    // Create 3 doogats
    repo.commit_file(
        "ddb/20240101000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20240102000000.md",
        "---\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20240103000000.md",
        "---\ntitle: C\n---\nBody C.",
        "add c",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    let report = idx.rebuild(&repo).unwrap();
    assert_eq!(report.indexed, 3);

    // Modify one doogat
    repo.commit_file(
        "ddb/20240101000000.md",
        "---\ntitle: A Modified\n---\nBody A modified.",
        "modify a",
    )
    .unwrap();
    let old_head = idx.stored_head_oid().unwrap();

    let report = idx.incremental_reindex(&repo, &old_head).unwrap();
    assert_eq!(report.indexed, 1); // Only the modified file

    // Verify the modification is reflected
    let rows = idx
        .query_raw("SELECT title FROM doogats WHERE id = '20240101000000'")
        .unwrap();
    assert_eq!(rows[0][0], "A Modified");
}

#[test]
fn incremental_reindex_handles_deletes() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20240101000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20240102000000.md",
        "---\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    // Delete one doogat
    repo.delete_file("ddb/20240102000000.md", "delete b")
        .unwrap();
    let old_head = idx.stored_head_oid().unwrap();

    let report = idx.incremental_reindex(&repo, &old_head).unwrap();
    assert_eq!(report.indexed, 0); // No adds/modifies

    // Verify deletion
    let rows = idx
        .query_raw("SELECT id FROM doogats WHERE id = '20240102000000'")
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn incremental_reindex_fallback_on_bad_oid() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
        "ddb/20240101000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();

    // Use a fake old HEAD — should fall back to full rebuild
    let report = idx
        .incremental_reindex(&repo, "0000000000000000000000000000000000000000")
        .unwrap();
    assert_eq!(report.indexed, 1); // Full rebuild found 1 doogat
}

#[test]
fn typedef_change_triggers_rematerialization() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    // Create a typedef
    let typedef = "---\nid: 20260301100100\ntitle: task\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n---\n";
    repo.commit_file("ddb/_typedef/20260301100100.md", typedef, "add typedef")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();
    let old_head = idx.stored_head_oid().unwrap();

    // Modify the typedef (add a column)
    let typedef2 = "---\nid: 20260301100100\ntitle: task\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n  - name: priority\n    data_type: INTEGER\n    zone: frontmatter\n---\n";
    repo.commit_file("ddb/_typedef/20260301100100.md", typedef2, "modify typedef")
        .unwrap();

    let report = idx.incremental_reindex(&repo, &old_head).unwrap();
    assert!(
        report.tables_materialized > 0,
        "typedef change should trigger rematerialization"
    );
}

#[test]
fn create_table_creates_junction_table_for_references() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let schema_content = "\
---
id: 20260301140000
title: bookmark
type: _typedef
columns:
  - name: url
    data_type: TEXT
    zone: body
  - name: category
    data_type: TEXT
    references: category
    zone: reference
---\n";
    repo.commit_file(
        "ddb/_typedef/20260301140000.md",
        schema_content,
        "add typedef",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    // Junction table should exist in sqlite_master
    let rows = idx
        .query_raw("SELECT name FROM sqlite_master WHERE type='table' AND name='bookmark_category'")
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "junction table bookmark_category should exist"
    );

    // Verify junction table schema has correct columns
    let info = idx
        .query_raw("PRAGMA table_info('bookmark_category')")
        .unwrap();
    let col_names: Vec<&str> = info.iter().map(|r| r[1].as_str()).collect();
    assert!(
        col_names.contains(&"bookmark_id"),
        "should have bookmark_id column"
    );
    assert!(
        col_names.contains(&"category_id"),
        "should have category_id column"
    );
}

#[test]
fn multi_value_refs_populate_junction_and_main() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let schema_content = "\
---
id: 20260301140000
title: bookmark
type: _typedef
columns:
  - name: url
    data_type: TEXT
    zone: body
  - name: category
    data_type: TEXT
    references: category
    zone: reference
---\n";
    repo.commit_file(
        "ddb/_typedef/20260301140000.md",
        schema_content,
        "add typedef",
    )
    .unwrap();

    let data_content = "\
---
id: 20260301140100
title: My Bookmark
type: bookmark
---

## url

https://example.com

---
- category:: [[20260301120100]]
- category:: [[20260301120101]]
- category:: [[20260301120102]]
";
    repo.commit_file("ddb/20260301140100.md", data_content, "add bookmark")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    // Junction table should have 3 rows
    let junc_rows = idx
        .query_raw("SELECT bookmark_id, category_id FROM bookmark_category ORDER BY category_id")
        .unwrap();
    assert_eq!(junc_rows.len(), 3, "junction table should have 3 rows");
    assert_eq!(junc_rows[0][1], "20260301120100");
    assert_eq!(junc_rows[1][1], "20260301120101");
    assert_eq!(junc_rows[2][1], "20260301120102");

    // Main table should have comma-separated value
    let main_rows = idx
        .query_raw("SELECT category FROM bookmark WHERE id = '20260301140100'")
        .unwrap();
    assert_eq!(main_rows.len(), 1);
    assert_eq!(
        main_rows[0][0], "20260301120100,20260301120101,20260301120102",
        "main table should have comma-separated refs"
    );
}

#[test]
fn type_table_includes_core_doogat_columns() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let typedef = "\
---
id: 20260331100000
title: task
type: _typedef
columns:
  - name: priority
    data_type: TEXT
    zone: frontmatter
---\n";
    repo.commit_file("ddb/_typedef/20260331100000.md", typedef, "add typedef")
        .unwrap();

    let data = "\
---
id: 20260331100100
title: Buy milk
type: task
priority: high
---
Task body.
";
    repo.commit_file("ddb/20260331100100.md", data, "add task")
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    // SELECT all core doogat columns + type column directly from type table - no JOIN
    let rows = idx
        .query_raw("SELECT id, title, date, updated_at, priority FROM task")
        .unwrap();
    assert_eq!(rows.len(), 1, "expected exactly one row in task table");
    assert_eq!(rows[0][0], "20260331100100", "id mismatch");
    assert_eq!(rows[0][1], "Buy milk", "title mismatch");
    assert!(!rows[0][2].is_empty(), "date should be populated");
    assert!(!rows[0][3].is_empty(), "updated_at should be populated");
    assert_eq!(rows[0][4], "high", "priority mismatch");
}

#[test]
fn incremental_reindex_materializes_new_typed_doogat() {
    // ddb#15 follow-up #2: a `ddb create` (or external git pull) of a
    // typed doogat must populate the materialized type table without
    // requiring a full `ddb reindex`. Otherwise FK-route Contains
    // queries through that type return 0 hits until the next rebuild.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
            "ddb/_typedef/20240101000000.md",
            "---\nid: 20240101000000\ntitle: category\ntype: _typedef\ncolumns:\n  - {name: fqn, data_type: TEXT}\n---\n",
            "add typedef",
        )
        .unwrap();
    repo.commit_file(
        "ddb/20240102000000.md",
        "---\nid: 20240102000000\ntitle: First\ntype: category\nfqn: a.b\n---\n",
        "add first",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();

    let rows0 = idx
        .query_raw("SELECT id FROM category ORDER BY id")
        .unwrap();
    assert_eq!(rows0.len(), 1, "seed materialization missing");

    repo.commit_file(
        "ddb/20240103000000.md",
        "---\nid: 20240103000000\ntitle: Second\ntype: category\nfqn: c.d\n---\n",
        "add second",
    )
    .unwrap();
    let old_head = idx.stored_head_oid().unwrap();
    let report = idx.incremental_reindex(&repo, &old_head).unwrap();
    assert_eq!(report.indexed, 1);

    let rows1 = idx
        .query_raw("SELECT id, title FROM category ORDER BY id")
        .unwrap();
    assert_eq!(
        rows1.len(),
        2,
        "incremental did not materialize the new typed doogat: {:?}",
        rows1
    );
    let ids: Vec<&str> = rows1.iter().map(|r| r[0].as_str()).collect();
    assert_eq!(ids, vec!["20240102000000", "20240103000000"]);
}

#[test]
fn incremental_reindex_unmaterializes_deleted_typed_doogat() {
    // Mirror for deletions: a deleted typed doogat must be evicted from
    // its type table so subsequent FK-route JOINs don't see stale rows.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    repo.commit_file(
            "ddb/_typedef/20240101000000.md",
            "---\nid: 20240101000000\ntitle: category\ntype: _typedef\ncolumns:\n  - {name: fqn, data_type: TEXT}\n---\n",
            "add typedef",
        )
        .unwrap();
    repo.commit_file(
        "ddb/20240102000000.md",
        "---\nid: 20240102000000\ntitle: First\ntype: category\nfqn: a.b\n---\n",
        "add first",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20240103000000.md",
        "---\nid: 20240103000000\ntitle: Second\ntype: category\nfqn: c.d\n---\n",
        "add second",
    )
    .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let idx = Index::open(&db_path).unwrap();
    idx.rebuild(&repo).unwrap();
    assert_eq!(
        idx.query_raw("SELECT id FROM category").unwrap().len(),
        2,
        "seed materialization wrong"
    );

    repo.delete_file("ddb/20240103000000.md", "delete second")
        .unwrap();
    let old_head = idx.stored_head_oid().unwrap();
    idx.incremental_reindex(&repo, &old_head).unwrap();

    let rows = idx
        .query_raw("SELECT id FROM category ORDER BY id")
        .unwrap();
    assert_eq!(rows.len(), 1, "deletion did not propagate to type table");
    assert_eq!(rows[0][0], "20240102000000");
}
