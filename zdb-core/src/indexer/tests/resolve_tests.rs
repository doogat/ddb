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

        let zettel = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId("20240101120000".to_string())),
                title: Some("Project X".to_string()),
                date: Some("2024-01-01".to_string()),
                zettel_type: None,
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
            path: "zettelkasten/20240101120000.md".to_string(),
        };

        index.index_zettel(&zettel).unwrap();

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
    fn alias_removed_on_zettel_delete() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        let mut extra = std::collections::BTreeMap::new();
        extra.insert(
            "aliases".to_string(),
            crate::types::Value::List(vec![crate::types::Value::String("alias1".to_string())]),
        );

        let zettel = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId("20240101120000".to_string())),
                title: Some("Test".to_string()),
                date: None,
                zettel_type: None,
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
            path: "zettelkasten/20240101120000.md".to_string(),
        };

        index.index_zettel(&zettel).unwrap();
        assert!(index.resolve_alias("alias1").unwrap().is_some());

        index.remove_zettel("20240101120000").unwrap();
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

        let zettel = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId("20240101120000".to_string())),
                title: Some("Note".to_string()),
                date: None,
                zettel_type: None,
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
            path: "zettelkasten/20240101120000.md".to_string(),
        };

        index.index_zettel(&zettel).unwrap();

        // Resolves via ID
        let result = index.resolve_wikilink("20240101120000").unwrap();
        assert_eq!(result, Some("zettelkasten/20240101120000.md".to_string()));

        // Resolves via alias
        let result = index.resolve_wikilink("My Note").unwrap();
        assert_eq!(result, Some("zettelkasten/20240101120000.md".to_string()));

        // No match
        let result = index.resolve_wikilink("nonexistent").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_wikilink_path_takes_precedence() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        // Zettel A: its *path* is the collision target
        let zettel_a = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId("20240101120000".to_string())),
                title: Some("Contact A".to_string()),
                date: None,
                zettel_type: Some("contact".to_string()),
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
            path: "zettelkasten/contact/20240101120000.md".to_string(),
        };

        // Zettel B: its *ID* equals A's full path — contrived but tests precedence
        let zettel_b = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId(
                    "zettelkasten/contact/20240101120000.md".to_string(),
                )),
                title: Some("Zettel B".to_string()),
                date: None,
                zettel_type: None,
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
            path: "zettelkasten/20240202120000.md".to_string(),
        };

        index.index_zettel(&zettel_a).unwrap();
        index.index_zettel(&zettel_b).unwrap();

        // Target matches A's path AND B's ID — path lookup must win
        let result = index
            .resolve_wikilink("zettelkasten/contact/20240101120000.md")
            .unwrap();
        assert_eq!(
            result,
            Some("zettelkasten/contact/20240101120000.md".to_string()),
            "path lookup should take precedence over ID lookup"
        );

        // Bare ID still resolves via ID fallback (step 2)
        let result = index.resolve_wikilink("20240101120000").unwrap();
        assert_eq!(
            result,
            Some("zettelkasten/contact/20240101120000.md".to_string())
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
            "zettelkasten/meeting-notes.md",
            "---\nid: 20260301000000\ntitle: Meeting Notes\n---\n",
            "add",
        )
        .unwrap();
        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let result = idx.resolve_wikilink("meeting-notes").unwrap();
        assert_eq!(result, Some("zettelkasten/meeting-notes.md".into()));
    }

    #[test]
    fn resolve_partial_path_nested() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.commit_file(
            "zettelkasten/projects/meeting-notes.md",
            "---\nid: 20260301000000\ntitle: Meeting Notes\n---\n",
            "add",
        )
        .unwrap();
        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let result = idx.resolve_wikilink("meeting-notes").unwrap();
        assert_eq!(
            result,
            Some("zettelkasten/projects/meeting-notes.md".into())
        );
    }

    #[test]
    fn resolve_partial_path_ambiguous_shortest_wins() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.commit_file(
            "zettelkasten/meeting-notes.md",
            "---\nid: 20260301000000\ntitle: Short\n---\n",
            "add short",
        )
        .unwrap();
        repo.commit_file(
            "zettelkasten/projects/acme/meeting-notes.md",
            "---\nid: 20260301000001\ntitle: Long\n---\n",
            "add long",
        )
        .unwrap();
        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let result = idx.resolve_wikilink("meeting-notes").unwrap();
        assert_eq!(result, Some("zettelkasten/meeting-notes.md".into()));
    }

    #[test]
    fn resolve_partial_path_with_md_suffix() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.commit_file(
            "zettelkasten/meeting-notes.md",
            "---\nid: 20260301000000\ntitle: Notes\n---\n",
            "add",
        )
        .unwrap();
        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let result = idx.resolve_wikilink("meeting-notes.md").unwrap();
        assert_eq!(result, Some("zettelkasten/meeting-notes.md".into()));
    }

    #[test]
    fn resolve_partial_path_no_match() {
        let idx = in_memory_index();
        let result = idx.resolve_wikilink("nonexistent-thing").unwrap();
        assert_eq!(result, None);
    }

