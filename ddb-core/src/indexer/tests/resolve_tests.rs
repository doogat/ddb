use super::*;

    #[test]
    fn alias_indexed_and_resolved() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        let mut extra = std::collections::BTreeMap::new();
        extra.insert(
            "aliases".to_string(),
            crate::types::Value::List(vec![
                crate::types::Value::String("My Project".to_string()),
                crate::types::Value::String("proj-x".to_string()),
            ]),
        );

        let doogat = crate::types::ParsedDoogat {
            meta: crate::types::DoogatMeta {
                id: Some(crate::types::DoogatId("20240101120000".to_string())),
                title: Some("Project X".to_string()),
                date: Some("2024-01-01".to_string()),
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
            path: "ddb/20240101120000.md".to_string(),
            updated_at: None,
        };

        index.index_doogat(&doogat).unwrap();

        // Resolve by alias
        assert_eq!(
            index.resolve_alias("My Project").unwrap(),
            Some("20240101120000".to_string())
        );
        assert_eq!(
            index.resolve_alias("proj-x").unwrap(),
            Some("20240101120000".to_string())
        );
        // Case-insensitive
        assert_eq!(
            index.resolve_alias("my project").unwrap(),
            Some("20240101120000".to_string())
        );
        // No match
        assert_eq!(index.resolve_alias("nonexistent").unwrap(), None);
    }

    #[test]
    fn alias_removed_on_doogat_delete() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        let mut extra = std::collections::BTreeMap::new();
        extra.insert(
            "aliases".to_string(),
            crate::types::Value::List(vec![crate::types::Value::String("alias1".to_string())]),
        );

        let doogat = crate::types::ParsedDoogat {
            meta: crate::types::DoogatMeta {
                id: Some(crate::types::DoogatId("20240101120000".to_string())),
                title: Some("Test".to_string()),
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
            path: "ddb/20240101120000.md".to_string(),
            updated_at: None,
        };

        index.index_doogat(&doogat).unwrap();
        assert!(index.resolve_alias("alias1").unwrap().is_some());

        index.remove_doogat("20240101120000").unwrap();
        assert_eq!(index.resolve_alias("alias1").unwrap(), None);
    }

    #[test]
    fn wikilink_resolves_via_alias() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        let mut extra = std::collections::BTreeMap::new();
        extra.insert(
            "aliases".to_string(),
            crate::types::Value::List(vec![crate::types::Value::String("My Note".to_string())]),
        );

        let doogat = crate::types::ParsedDoogat {
            meta: crate::types::DoogatMeta {
                id: Some(crate::types::DoogatId("20240101120000".to_string())),
                title: Some("Note".to_string()),
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
            path: "ddb/20240101120000.md".to_string(),
            updated_at: None,
        };

        index.index_doogat(&doogat).unwrap();

        // Resolves via ID
        let result = index.resolve_wikilink("20240101120000").unwrap();
        assert_eq!(result, Some("ddb/20240101120000.md".to_string()));

        // Resolves via alias
        let result = index.resolve_wikilink("My Note").unwrap();
        assert_eq!(result, Some("ddb/20240101120000.md".to_string()));

        // No match
        let result = index.resolve_wikilink("nonexistent").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_wikilink_path_takes_precedence() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        // Doogat A: its *path* is the collision target
        let doogat_a = crate::types::ParsedDoogat {
            meta: crate::types::DoogatMeta {
                id: Some(crate::types::DoogatId("20240101120000".to_string())),
                title: Some("Contact A".to_string()),
                date: None,
                doogat_type: Some("contact".to_string()),
                tags: vec![],
                extra: std::collections::BTreeMap::new(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/contact/20240101120000.md".to_string(),
            updated_at: None,
        };

        // Doogat B: its *ID* equals A's full path — contrived but tests precedence
        let doogat_b = crate::types::ParsedDoogat {
            meta: crate::types::DoogatMeta {
                id: Some(crate::types::DoogatId(
                    "ddb/contact/20240101120000.md".to_string(),
                )),
                title: Some("Doogat B".to_string()),
                date: None,
                doogat_type: None,
                tags: vec![],
                extra: std::collections::BTreeMap::new(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20240202120000.md".to_string(),
            updated_at: None,
        };

        index.index_doogat(&doogat_a).unwrap();
        index.index_doogat(&doogat_b).unwrap();

        // Target matches A's path AND B's ID — path lookup must win
        let result = index
            .resolve_wikilink("ddb/contact/20240101120000.md")
            .unwrap();
        assert_eq!(
            result,
            Some("ddb/contact/20240101120000.md".to_string()),
            "path lookup should take precedence over ID lookup"
        );

        // Bare ID still resolves via ID fallback (step 2)
        let result = index.resolve_wikilink("20240101120000").unwrap();
        assert_eq!(
            result,
            Some("ddb/contact/20240101120000.md".to_string())
        );

        // Nonexistent returns None
        let result = index.resolve_wikilink("nonexistent").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_partial_path_basic() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.commit_file(
            "ddb/meeting-notes.md",
            "---\nid: 20260301000000\ntitle: Meeting Notes\n---\n",
            "add",
        )
        .unwrap();
        let db_path = dir.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let result = idx.resolve_wikilink("meeting-notes").unwrap();
        assert_eq!(result, Some("ddb/meeting-notes.md".into()));
    }

    #[test]
    fn resolve_partial_path_nested() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.commit_file(
            "ddb/projects/meeting-notes.md",
            "---\nid: 20260301000000\ntitle: Meeting Notes\n---\n",
            "add",
        )
        .unwrap();
        let db_path = dir.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let result = idx.resolve_wikilink("meeting-notes").unwrap();
        assert_eq!(
            result,
            Some("ddb/projects/meeting-notes.md".into())
        );
    }

    #[test]
    fn resolve_partial_path_ambiguous_shortest_wins() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.commit_file(
            "ddb/meeting-notes.md",
            "---\nid: 20260301000000\ntitle: Short\n---\n",
            "add short",
        )
        .unwrap();
        repo.commit_file(
            "ddb/projects/acme/meeting-notes.md",
            "---\nid: 20260301000001\ntitle: Long\n---\n",
            "add long",
        )
        .unwrap();
        let db_path = dir.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let result = idx.resolve_wikilink("meeting-notes").unwrap();
        assert_eq!(result, Some("ddb/meeting-notes.md".into()));
    }

    #[test]
    fn resolve_partial_path_with_md_suffix() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.commit_file(
            "ddb/meeting-notes.md",
            "---\nid: 20260301000000\ntitle: Notes\n---\n",
            "add",
        )
        .unwrap();
        let db_path = dir.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let result = idx.resolve_wikilink("meeting-notes.md").unwrap();
        assert_eq!(result, Some("ddb/meeting-notes.md".into()));
    }

    #[test]
    fn resolve_partial_path_no_match() {
        let idx = in_memory_index();
        let result = idx.resolve_wikilink("nonexistent-thing").unwrap();
        assert_eq!(result, None);
    }

