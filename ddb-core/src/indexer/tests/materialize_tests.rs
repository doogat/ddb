use super::*;

    #[test]
    fn rebuild_and_staleness() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        let doogat_content =
            "---\nid: 20260226120000\ntitle: Rebuild Test\ntags:\n  - test\n---\nBody here.";
        repo.commit_file(
            "ddb/20260226120000.md",
            doogat_content,
            "add doogat",
        )
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
            source
                .files
                .insert(format!("ddb/{id}.md"), content);
        }
        for i in 0..5 {
            let id = format!("{:014}", 20260226130000u64 + i);
            let content = format!(
                "---\nid: {id}\ntitle: Task {i}\ndate: 2026-02-26\ntype: task\npriority: {i}\n---\nTask body {i}."
            );
            source
                .files
                .insert(format!("ddb/{id}.md"), content);
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
                },
            ],
            crdt_strategy: Some("preset:default".into()),
            template_sections: vec!["A".into()],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
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
            }],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
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
            }],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
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
                },
            ],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
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
            }],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
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
                },
            ],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
        };

        let merged = Index::merge_schemas(Some(typedef), inferred);
        assert_eq!(merged.columns.len(), 3);
    }

    #[test]
    fn consistency_warnings_valid_doogat() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        let z = "---\nid: 20260226200000\ntitle: Valid\ntype: note\n---\nBody text.";
        repo.commit_file("ddb/20260226200000.md", z, "add")
            .unwrap();

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
        let has_missing = warnings.iter().any(|w| matches!(w,
            crate::types::ConsistencyWarning::MissingRequired { field, .. } if field == "priority"
        ));
        assert!(
            has_missing,
            "should warn about missing required 'priority' field"
        );
    }

    #[test]
    fn integration_typedef_plus_inferred_merge() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Create typedef with 2 columns
        let typedef = "---\nid: 20260226230000\ntitle: widget\ntype: _typedef\ncolumns:\n  - name: weight\n    data_type: REAL\n    zone: frontmatter\n  - name: color\n    data_type: TEXT\n    zone: frontmatter\n---\n";
        repo.commit_file(
            "ddb/_typedef/20260226230000.md",
            typedef,
            "add typedef",
        )
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
        repo.commit_file(
            "ddb/_typedef/20260226250000.md",
            typedef,
            "add typedef",
        )
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
        repo.commit_file(
            "ddb/_typedef/20260301100100.md",
            typedef,
            "add typedef",
        )
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
        repo.commit_file(
            "ddb/_typedef/20260301100100.md",
            typedef,
            "add typedef",
        )
        .unwrap();

        let db_path = dir.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();
        let old_head = idx.stored_head_oid().unwrap();

        // Modify the typedef (add a column)
        let typedef2 = "---\nid: 20260301100100\ntitle: task\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n  - name: priority\n    data_type: INTEGER\n    zone: frontmatter\n---\n";
        repo.commit_file(
            "ddb/_typedef/20260301100100.md",
            typedef2,
            "modify typedef",
        )
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
        assert_eq!(rows.len(), 1, "junction table bookmark_category should exist");

        // Verify junction table schema has correct columns
        let info = idx.query_raw("PRAGMA table_info('bookmark_category')").unwrap();
        let col_names: Vec<&str> = info.iter().map(|r| r[1].as_str()).collect();
        assert!(col_names.contains(&"bookmark_id"), "should have bookmark_id column");
        assert!(col_names.contains(&"category_id"), "should have category_id column");
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
        repo.commit_file(
            "ddb/20260301140100.md",
            data_content,
            "add bookmark",
        )
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

