use super::*;

    #[test]
    fn backlinks_include_all_link_kinds() {
        let idx = in_memory_index();

        // Target doogat
        let target = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260301120000".into())),
                title: Some("Target".into()),
                date: None,
                doogat_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260301120000.md".into(),
            updated_at: None,
        };
        idx.index_doogat(&target).unwrap();

        // Source doogat linking via all 4 kinds
        let source = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260301100000".into())),
                title: Some("Source".into()),
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
                    target: "20260301120000".into(),
                    display: None,
                    section: None,
                    kind: crate::types::LinkKind::WikiLink,
                    zone: Zone::Body,
                },
                crate::types::Link {
                    target: "20260301120000".into(),
                    display: Some("t".into()),
                    section: None,
                    kind: crate::types::LinkKind::MarkdownLink,
                    zone: Zone::Body,
                },
                crate::types::Link {
                    target: "20260301120000".into(),
                    display: None,
                    section: None,
                    kind: crate::types::LinkKind::Embed,
                    zone: Zone::Body,
                },
            ],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260301100000.md".into(),
            updated_at: None,
        };
        idx.index_doogat(&source).unwrap();

        // backlinks() returns the source regardless of link kind
        let bl = idx.backlinks("20260301120000").unwrap();
        assert_eq!(bl.len(), 1);
        assert_eq!(bl[0], "20260301100000");
    }

    #[test]
    fn backlink_query() {
        let idx = in_memory_index();
        idx.index_doogat(&sample_doogat()).unwrap();

        let ids = idx.backlinks("20260101000000").unwrap();
        assert!(ids.contains(&"20260226120000".to_string()));
    }

    #[test]
    fn resurrected_doogat_not_duplicated_after_reindex() {
        let idx = in_memory_index();
        let mut z = sample_doogat();
        z.meta
            .extra
            .insert("resurrected".into(), crate::types::Value::Bool(true));
        idx.index_doogat(&z).unwrap();
        // Reindex same doogat
        idx.index_doogat(&z).unwrap();

        let id = z.meta.id.as_ref().unwrap().0.as_str();
        let count: i64 = idx
            .conn
            .query_row(
                "SELECT COUNT(*) FROM doogats WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Also verify the resurrected field isn't duplicated
        let field_count: i64 = idx
            .conn
            .query_row(
                "SELECT COUNT(*) FROM _ddb_fields WHERE doogat_id = ?1 AND key = 'resurrected'",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(field_count, 1);
    }

    #[test]
    fn resurrected_doogats_query() {
        let idx = in_memory_index();

        // Doogat with resurrected: true
        let mut z1 = sample_doogat();
        z1.meta.extra.insert(
            "resurrected".into(),
            crate::types::Value::String("true".into()),
        );
        idx.index_doogat(&z1).unwrap();

        // Normal doogat without resurrected
        let z2 = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260302120000".into())),
                title: Some("Normal".into()),
                date: None,
                doogat_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260302120000.md".into(),
            updated_at: None,
        };
        idx.index_doogat(&z2).unwrap();

        let results = idx.resurrected_doogats().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, z1.meta.id.as_ref().unwrap().0);
        assert_eq!(results[0].1, "Test Note");
    }

    #[test]
    fn resurrected_doogats_empty_when_none() {
        let idx = in_memory_index();
        let z = sample_doogat();
        idx.index_doogat(&z).unwrap();
        assert!(idx.resurrected_doogats().unwrap().is_empty());
    }

    #[test]
    fn backlinking_doogat_paths_returns_source_id_and_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        // Doogat A links to target B
        let doogat_a = crate::types::ParsedDoogat {
            meta: crate::types::DoogatMeta {
                id: Some(crate::types::DoogatId("20260301100000".to_string())),
                title: Some("A".to_string()),
                date: None,
                doogat_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: "See [[20260301120000]]".to_string(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![crate::types::Link {
                target: "20260301120000".to_string(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: crate::types::Zone::Body,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260301100000.md".to_string(),
            updated_at: None,
        };

        // Doogat B is the target (no outgoing links)
        let doogat_b = crate::types::ParsedDoogat {
            meta: crate::types::DoogatMeta {
                id: Some(crate::types::DoogatId("20260301120000".to_string())),
                title: Some("B".to_string()),
                date: None,
                doogat_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260301120000.md".to_string(),
            updated_at: None,
        };

        index.index_doogat(&doogat_a).unwrap();
        index.index_doogat(&doogat_b).unwrap();

        let results = index.backlinking_doogat_paths("20260301120000").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "20260301100000");
        assert_eq!(results[0].1, "ddb/20260301100000.md");

        // No backlinks for A
        let empty = index.backlinking_doogat_paths("20260301100000").unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn broken_backlinks_after_delete() {
        let index = in_memory_index();

        // Create target doogat A
        let a = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260301100000".into())),
                title: Some("Target".into()),
                date: None,
                doogat_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260301100000.md".into(),
            updated_at: None,
        };

        // Create doogat B that links to A
        let b = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260301100001".into())),
                title: Some("Linker".into()),
                date: None,
                doogat_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: "See [[20260301100000]]".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![Link {
                target: "20260301100000".into(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: Zone::Body,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260301100001.md".into(),
            updated_at: None,
        };

        index.index_doogat(&a).unwrap();
        index.index_doogat(&b).unwrap();

        // No broken backlinks yet
        let broken = index.broken_backlinks().unwrap();
        assert!(broken.is_empty());

        // Delete A
        index.remove_doogat("20260301100000").unwrap();

        // B's link to A is now broken
        let broken = index.broken_backlinks().unwrap();
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].0, "20260301100001");
        assert_eq!(broken[0].1, "20260301100000");
    }

    #[test]
    fn unlinked_mentions_basic() {
        let idx = in_memory_index();

        // Doogat A: title "Project Alpha"
        let a = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260301000000".into())),
                title: Some("Project Alpha".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "This is Project Alpha.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260301000000.md".into(),
            updated_at: None,
        };

        // Doogat B: body mentions "Project Alpha" but does NOT link to A
        let b = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260301000001".into())),
                title: Some("Meeting Notes".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "Discussed Project Alpha progress today.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260301000001.md".into(),
            updated_at: None,
        };

        idx.index_doogat(&a).unwrap();
        idx.index_doogat(&b).unwrap();

        let mentions = idx.unlinked_mentions("20260301000000").unwrap();
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].source_id, "20260301000001");
    }

    #[test]
    fn unlinked_mentions_excludes_linked() {
        let idx = in_memory_index();

        // Doogat A
        let a = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260302000000".into())),
                title: Some("Project Beta".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "This is Project Beta.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260302000000.md".into(),
            updated_at: None,
        };

        // Doogat B: mentions "Project Beta" AND links to A via wikilink
        let b = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260302000001".into())),
                title: Some("Status Update".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "Project Beta is on track. See [[20260302000000]].".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![Link {
                target: "20260302000000".into(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: Zone::Body,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260302000001.md".into(),
            updated_at: None,
        };

        idx.index_doogat(&a).unwrap();
        idx.index_doogat(&b).unwrap();

        let mentions = idx.unlinked_mentions("20260302000000").unwrap();
        assert!(
            mentions.is_empty(),
            "linked doogat should not appear in unlinked mentions"
        );
    }

    #[test]
    fn unlinked_mentions_excludes_self() {
        let idx = in_memory_index();

        // Doogat whose body mentions its own title
        let a = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260303000000".into())),
                title: Some("Self Reference".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "This doogat is about Self Reference patterns.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260303000000.md".into(),
            updated_at: None,
        };

        idx.index_doogat(&a).unwrap();

        let mentions = idx.unlinked_mentions("20260303000000").unwrap();
        assert!(
            mentions.is_empty(),
            "doogat should not appear in its own unlinked mentions"
        );
    }

    #[test]
    fn suggest_links_tag_overlap() {
        let idx = in_memory_index();

        // Source: tags [a, b, c]
        let source = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260304000000".into())),
                title: Some("Source".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec!["a".into(), "b".into(), "c".into()],
                extra: Default::default(),
            },
            body: "Source body.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260304000000.md".into(),
            updated_at: None,
        };

        // Candidate1: tags [a, b] — 2 shared tags
        let c1 = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260304000001".into())),
                title: Some("Candidate One".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec!["a".into(), "b".into()],
                extra: Default::default(),
            },
            body: "Candidate one body.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260304000001.md".into(),
            updated_at: None,
        };

        // Candidate2: tags [a] — 1 shared tag
        let c2 = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260304000002".into())),
                title: Some("Candidate Two".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec!["a".into()],
                extra: Default::default(),
            },
            body: "Candidate two body.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260304000002.md".into(),
            updated_at: None,
        };

        idx.index_doogat(&source).unwrap();
        idx.index_doogat(&c1).unwrap();
        idx.index_doogat(&c2).unwrap();

        let suggestions = idx.suggest_links("20260304000000", 10).unwrap();
        assert!(
            suggestions.len() >= 2,
            "should suggest at least 2 candidates"
        );

        // Candidate1 (2 shared tags) should rank higher than candidate2 (1 shared tag)
        let pos_c1 = suggestions.iter().position(|s| s.id == "20260304000001");
        let pos_c2 = suggestions.iter().position(|s| s.id == "20260304000002");
        assert!(
            pos_c1.unwrap() < pos_c2.unwrap(),
            "candidate with more shared tags should rank higher"
        );
    }

    #[test]
    fn suggest_links_excludes_linked() {
        let idx = in_memory_index();

        // Source links to candidate
        let source = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260305000000".into())),
                title: Some("Source".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec!["shared".into()],
                extra: Default::default(),
            },
            body: "Source body with [[20260305000001]].".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![Link {
                target: "20260305000001".into(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: Zone::Body,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260305000000.md".into(),
            updated_at: None,
        };

        // Candidate: same tag as source
        let candidate = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260305000001".into())),
                title: Some("Candidate".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec!["shared".into()],
                extra: Default::default(),
            },
            body: "Candidate body.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260305000001.md".into(),
            updated_at: None,
        };

        idx.index_doogat(&source).unwrap();
        idx.index_doogat(&candidate).unwrap();

        let suggestions = idx.suggest_links("20260305000000", 10).unwrap();
        assert!(
            !suggestions.iter().any(|s| s.id == "20260305000001"),
            "already-linked doogat should be excluded from suggestions"
        );
    }

    #[test]
    fn suggest_links_respects_limit() {
        let idx = in_memory_index();

        // Source with tags
        let source = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260306000000".into())),
                title: Some("Source".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec!["common".into()],
                extra: Default::default(),
            },
            body: "Source body.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260306000000.md".into(),
            updated_at: None,
        };
        idx.index_doogat(&source).unwrap();

        // Create 5 candidates all sharing the tag
        for i in 1..=5 {
            let id = format!("2026030600000{i}");
            let c = ParsedDoogat {
                meta: DoogatMeta {
                    id: Some(DoogatId(id.clone())),
                    title: Some(format!("Candidate {i}")),
                    date: None,
                    doogat_type: Some("note".into()),
                    tags: vec!["common".into()],
                    extra: Default::default(),
                },
                body: format!("Candidate {i} body."),
                sections: vec![],
                reference_section: String::new(),
                inline_fields: vec![],
                links: vec![],
                body_tags: vec![],
                checkboxes: vec![],
                path: format!("ddb/{id}.md"),
                updated_at: None,
            };
            idx.index_doogat(&c).unwrap();
        }

        let suggestions = idx.suggest_links("20260306000000", 2).unwrap();
        assert!(
            suggestions.len() <= 2,
            "should respect limit of 2, got {}",
            suggestions.len()
        );
    }

    #[test]
    fn suggest_links_content_similarity() {
        let idx = in_memory_index();

        // Doogat A: no tags, title "Machine Learning"
        let a = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260314000000".into())),
                title: Some("Machine Learning".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "An overview of ML techniques.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260314000000.md".into(),
            updated_at: None,
        };

        // Doogat B: no shared tags, body contains "machine learning"
        let b = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260314000001".into())),
                title: Some("Deep Learning".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "This explores machine learning algorithms and neural networks.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260314000001.md".into(),
            updated_at: None,
        };

        idx.index_doogat(&a).unwrap();
        idx.index_doogat(&b).unwrap();

        // A has no tags, so suggest_links falls back to content-only similarity.
        // B's body contains "machine learning" which matches A's title via FTS5.
        let suggestions = idx.suggest_links("20260314000000", 5).unwrap();
        assert!(
            suggestions.iter().any(|s| s.id == "20260314000001"),
            "B should appear via content similarity; got: {:?}",
            suggestions.iter().map(|s| &s.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn stale_doogats_basic() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Create a _typedef with stale_after_days: 1
        let typedef =
            "---\nid: 20260307000000\ntitle: task\ntype: _typedef\nstale_after_days: 1\n---\n";
        repo.commit_file(
            "ddb/_typedef/20260307000000.md",
            typedef,
            "add typedef",
        )
        .unwrap();

        // Create a doogat of type "task" with an OLD git commit time (2020-01-01)
        let doogat =
            "---\nid: 20260307000001\ntitle: Old Task\ntype: task\ndate: 2020-01-01\n---\nBody.";
        commit_file_with_time(
            &repo,
            "ddb/20260307000001.md",
            doogat,
            "add old task",
            1577836800, // 2020-01-01T00:00:00 UTC
        );

        let db_path = dir.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let stale = idx.stale_doogats(&repo, None).unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, "20260307000001");
        assert_eq!(stale[0].doogat_type, "task");
    }

    #[test]
    fn stale_doogats_respects_type() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Type A: stale_after_days: 1
        let typedef_a =
            "---\nid: 20260313000000\ntitle: taskA\ntype: _typedef\nstale_after_days: 1\n---\n";
        repo.commit_file(
            "ddb/_typedef/20260313000000.md",
            typedef_a,
            "add typedef A",
        )
        .unwrap();

        // Type B: stale_after_days: 1
        let typedef_b =
            "---\nid: 20260313000001\ntitle: taskB\ntype: _typedef\nstale_after_days: 1\n---\n";
        repo.commit_file(
            "ddb/_typedef/20260313000001.md",
            typedef_b,
            "add typedef B",
        )
        .unwrap();

        // Doogat of type A with old git commit time
        let doogat_a = "---\nid: 20260313000002\ntitle: Old A\ntype: taskA\n---\nBody A.";
        commit_file_with_time(
            &repo,
            "ddb/20260313000002.md",
            doogat_a,
            "add old A",
            1577836800, // 2020-01-01
        );

        // Doogat of type B with old git commit time
        let doogat_b = "---\nid: 20260313000003\ntitle: Old B\ntype: taskB\n---\nBody B.";
        commit_file_with_time(
            &repo,
            "ddb/20260313000003.md",
            doogat_b,
            "add old B",
            1577836800, // 2020-01-01
        );

        let db_path = dir.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        // Filter by type A — only type A doogat should be returned
        let stale = idx.stale_doogats(&repo, Some("taskA")).unwrap();
        assert_eq!(stale.len(), 1, "should return exactly one stale doogat");
        assert_eq!(stale[0].id, "20260313000002");
        assert_eq!(stale[0].doogat_type, "taskA");

        // Unfiltered — both should appear
        let all_stale = idx.stale_doogats(&repo, None).unwrap();
        assert_eq!(
            all_stale.len(),
            2,
            "unfiltered should return both stale doogats"
        );
    }

    #[test]
    fn stale_doogats_no_threshold() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // _typedef without stale_after_days
        let typedef = "---\nid: 20260308000000\ntitle: note\ntype: _typedef\n---\n";
        repo.commit_file(
            "ddb/_typedef/20260308000000.md",
            typedef,
            "add typedef",
        )
        .unwrap();

        // Doogat of type "note" with old date
        let doogat =
            "---\nid: 20260308000001\ntitle: Old Note\ntype: note\ndate: 2020-01-01\n---\nBody.";
        repo.commit_file("ddb/20260308000001.md", doogat, "add note")
            .unwrap();

        let db_path = dir.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let stale = idx.stale_doogats(&repo, None).unwrap();
        assert!(
            stale.is_empty(),
            "type without stale_after_days should not report stale doogats"
        );
    }

    #[test]
    fn orphan_doogats_basic() {
        let idx = in_memory_index();

        // Doogat with no incoming links
        let orphan = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260309000000".into())),
                title: Some("Orphan".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "Nobody links to me.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260309000000.md".into(),
            updated_at: None,
        };
        idx.index_doogat(&orphan).unwrap();

        let orphans = idx.orphan_doogats(None).unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].id, "20260309000000");
    }

    #[test]
    fn orphan_doogats_excludes_linked() {
        let idx = in_memory_index();

        // Doogat B: target of a link
        let b = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260310000001".into())),
                title: Some("Linked Target".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "I have an incoming link.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260310000001.md".into(),
            updated_at: None,
        };

        // Doogat A: links to B
        let a = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260310000000".into())),
                title: Some("Linker".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "See [[20260310000001]].".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![Link {
                target: "20260310000001".into(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: Zone::Body,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260310000000.md".into(),
            updated_at: None,
        };

        idx.index_doogat(&b).unwrap();
        idx.index_doogat(&a).unwrap();

        let orphans = idx.orphan_doogats(None).unwrap();
        assert!(
            !orphans.iter().any(|o| o.id == "20260310000001"),
            "doogat with incoming link should not be an orphan"
        );
    }

    #[test]
    fn orphan_doogats_excludes_typedef() {
        let idx = in_memory_index();

        // _typedef doogat (no incoming links)
        let typedef = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260311000000".into())),
                title: Some("task".into()),
                date: None,
                doogat_type: Some("_typedef".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/_typedef/20260311000000.md".into(),
            updated_at: None,
        };
        idx.index_doogat(&typedef).unwrap();

        let orphans = idx.orphan_doogats(None).unwrap();
        assert!(
            !orphans.iter().any(|o| o.id == "20260311000000"),
            "_typedef doogats should never appear in orphan results"
        );
    }

    #[test]
    fn orphan_doogats_includes_outgoing_count() {
        let idx = in_memory_index();

        // Orphan doogat with 2 outgoing links (but no incoming)
        let orphan = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260312000000".into())),
                title: Some("Orphan With Links".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "Links to [[20260312000001]] and [[20260312000002]].".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![
                Link {
                    target: "20260312000001".into(),
                    display: None,
                    section: None,
                    kind: crate::types::LinkKind::WikiLink,
                    zone: Zone::Body,
                },
                Link {
                    target: "20260312000002".into(),
                    display: None,
                    section: None,
                    kind: crate::types::LinkKind::WikiLink,
                    zone: Zone::Body,
                },
            ],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260312000000.md".into(),
            updated_at: None,
        };
        idx.index_doogat(&orphan).unwrap();

        let orphans = idx.orphan_doogats(None).unwrap();
        let found = orphans.iter().find(|o| o.id == "20260312000000");
        assert!(found.is_some(), "orphan should be returned");
        assert_eq!(found.unwrap().outgoing_links, 2);
    }

    #[test]
    fn sequence_children_basic() {
        let idx = in_memory_index();
        let parent = seq_doogat("20260315100000", "Root", None);
        let child1 = seq_doogat("20260315100001", "Child A", Some("20260315100000"));
        let child2 = seq_doogat("20260315100002", "Child B", Some("20260315100000"));
        idx.index_doogat(&parent).unwrap();
        idx.index_doogat(&child1).unwrap();
        idx.index_doogat(&child2).unwrap();

        let children = idx.sequence_children("20260315100000").unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].id, "20260315100001");
        assert_eq!(children[1].id, "20260315100002");
    }

    #[test]
    fn sequence_children_empty() {
        let idx = in_memory_index();
        let z = seq_doogat("20260315110000", "Standalone", None);
        idx.index_doogat(&z).unwrap();

        let children = idx.sequence_children("20260315110000").unwrap();
        assert!(children.is_empty());
    }

    #[test]
    fn sequence_breadcrumb_chain() {
        let idx = in_memory_index();
        let root = seq_doogat("20260315120000", "Root", None);
        let mid = seq_doogat("20260315120001", "Mid", Some("20260315120000"));
        let leaf = seq_doogat("20260315120002", "Leaf", Some("20260315120001"));
        idx.index_doogat(&root).unwrap();
        idx.index_doogat(&mid).unwrap();
        idx.index_doogat(&leaf).unwrap();

        let bc = idx.sequence_breadcrumb("20260315120002").unwrap();
        assert_eq!(bc.len(), 3);
        assert_eq!(bc[0].id, "20260315120000");
        assert_eq!(bc[1].id, "20260315120001");
        assert_eq!(bc[2].id, "20260315120002");
    }

    #[test]
    fn sequence_breadcrumb_root() {
        let idx = in_memory_index();
        let root = seq_doogat("20260315130000", "Root", None);
        idx.index_doogat(&root).unwrap();

        let bc = idx.sequence_breadcrumb("20260315130000").unwrap();
        assert_eq!(bc.len(), 1);
        assert_eq!(bc[0].id, "20260315130000");
    }

    #[test]
    fn sequence_breadcrumb_cycle() {
        let idx = in_memory_index();
        let a = seq_doogat("20260315140000", "A", Some("20260315140001"));
        let b = seq_doogat("20260315140001", "B", Some("20260315140000"));
        idx.index_doogat(&a).unwrap();
        idx.index_doogat(&b).unwrap();

        let bc = idx.sequence_breadcrumb("20260315140000").unwrap();
        // Should not hang; returns partial chain
        assert!(bc.len() <= 3);
    }

    #[test]
    fn sequence_info_complete() {
        let idx = in_memory_index();
        let root = seq_doogat("20260315150000", "Root", None);
        let mid = seq_doogat("20260315150001", "Mid", Some("20260315150000"));
        let child1 = seq_doogat("20260315150002", "Child C", Some("20260315150001"));
        let child2 = seq_doogat("20260315150003", "Child D", Some("20260315150001"));
        idx.index_doogat(&root).unwrap();
        idx.index_doogat(&mid).unwrap();
        idx.index_doogat(&child1).unwrap();
        idx.index_doogat(&child2).unwrap();

        let info = idx.sequence_info("20260315150001").unwrap();
        assert!(info.parent.is_some());
        assert_eq!(info.parent.unwrap().id, "20260315150000");
        assert_eq!(info.children.len(), 2);
        assert_eq!(info.children[0].id, "20260315150002");
        assert_eq!(info.breadcrumb.len(), 2);
        assert_eq!(info.breadcrumb[0].id, "20260315150000");
        assert_eq!(info.breadcrumb[1].id, "20260315150001");
    }

    #[test]
    fn broken_sequence_detected() {
        let idx = in_memory_index();
        let z = seq_doogat("20260315160000", "Orphan", Some("99999999999999"));
        idx.index_doogat(&z).unwrap();

        let broken = idx.broken_sequences().unwrap();
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].doogat_id, "20260315160000");
        assert_eq!(broken[0].broken_parent_id, "99999999999999");
    }

    #[test]
    fn broken_sequence_clean() {
        let idx = in_memory_index();
        let root = seq_doogat("20260315170000", "Root", None);
        let child = seq_doogat("20260315170001", "Child", Some("20260315170000"));
        idx.index_doogat(&root).unwrap();
        idx.index_doogat(&child).unwrap();

        let broken = idx.broken_sequences().unwrap();
        assert!(broken.is_empty());
    }

    #[test]
    fn sequence_tree_recursive() {
        let idx = in_memory_index();
        let root = seq_doogat("20260315180000", "Root", None);
        let mid = seq_doogat("20260315180001", "Mid", Some("20260315180000"));
        let leaf = seq_doogat("20260315180002", "Leaf", Some("20260315180001"));
        idx.index_doogat(&root).unwrap();
        idx.index_doogat(&mid).unwrap();
        idx.index_doogat(&leaf).unwrap();

        let tree = idx.sequence_tree("20260315180000", 100).unwrap();
        assert_eq!(tree.len(), 3);
        assert_eq!(tree[0].0.id, "20260315180000");
        assert_eq!(tree[0].1, 0);
        assert_eq!(tree[1].0.id, "20260315180001");
        assert_eq!(tree[1].1, 1);
        assert_eq!(tree[2].0.id, "20260315180002");
        assert_eq!(tree[2].1, 2);
    }

    #[test]
    fn sequence_breadcrumb_broken_parent() {
        let idx = in_memory_index();
        // Doogat points to nonexistent parent
        let z = seq_doogat("20260315190000", "Orphan", Some("99999999999999"));
        idx.index_doogat(&z).unwrap();

        let bc = idx.sequence_breadcrumb("20260315190000").unwrap();
        // Should return just self, not a phantom node for the missing parent
        assert_eq!(bc.len(), 1);
        assert_eq!(bc[0].id, "20260315190000");
    }

    // ---- recent_doogats tests ----

    fn make_doogat_with_date(id: &str, title: &str, date: &str, dtype: &str, path: &str) -> ParsedDoogat {
        ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId(id.into())),
                title: Some(title.into()),
                date: Some(date.into()),
                doogat_type: Some(dtype.into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: path.into(),
            updated_at: None,
        }
    }

    #[test]
    fn recent_doogats_returns_recently_modified() {
        let idx = in_memory_index();
        let today = chrono::Utc::now().date_naive().format("%Y-%m-%d").to_string();

        let z = make_doogat_with_date(
            "20260403120000",
            "Today Note",
            &today,
            "note",
            "ddb/20260403120000.md",
        );
        idx.index_doogat(&z).unwrap();

        let results = idx.recent_doogats(7, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "20260403120000");
        assert_eq!(results[0].title, "Today Note");
        assert_eq!(results[0].doogat_type, "note");
    }

    #[test]
    fn recent_doogats_excludes_old_entries() {
        let idx = in_memory_index();

        let z = make_doogat_with_date(
            "20250101120000",
            "Old Note",
            "2025-01-01",
            "note",
            "ddb/20250101120000.md",
        );
        idx.index_doogat(&z).unwrap();

        let results = idx.recent_doogats(7, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn recent_doogats_respects_type_filter() {
        let idx = in_memory_index();
        let today = chrono::Utc::now().date_naive().format("%Y-%m-%d").to_string();

        let note = make_doogat_with_date(
            "20260403120000",
            "A Note",
            &today,
            "note",
            "ddb/20260403120000.md",
        );
        let project = make_doogat_with_date(
            "20260403120001",
            "A Project",
            &today,
            "project",
            "ddb/20260403120001.md",
        );
        idx.index_doogat(&note).unwrap();
        idx.index_doogat(&project).unwrap();

        let results = idx.recent_doogats(7, Some("project")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "20260403120001");
        assert_eq!(results[0].doogat_type, "project");
    }

    #[test]
    fn recent_doogats_excludes_typedefs() {
        let idx = in_memory_index();
        let today = chrono::Utc::now().date_naive().format("%Y-%m-%d").to_string();

        let typedef = make_doogat_with_date(
            "20260403120000",
            "My Typedef",
            &today,
            "note",
            "ddb/_typedef/20260403120000.md",
        );
        idx.index_doogat(&typedef).unwrap();

        let results = idx.recent_doogats(7, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn recent_doogats_sorted_by_date_descending() {
        let idx = in_memory_index();
        let today = chrono::Utc::now().date_naive();
        let yesterday = (today - chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
        let today_str = today.format("%Y-%m-%d").to_string();

        let older = make_doogat_with_date(
            "20260402120000",
            "Yesterday Note",
            &yesterday,
            "note",
            "ddb/20260402120000.md",
        );
        let newer = make_doogat_with_date(
            "20260403120000",
            "Today Note",
            &today_str,
            "note",
            "ddb/20260403120000.md",
        );
        // Index older first to confirm sorting isn't insertion-order
        idx.index_doogat(&older).unwrap();
        idx.index_doogat(&newer).unwrap();

        let results = idx.recent_doogats(7, None).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "20260403120000", "most recent should be first");
        assert_eq!(results[1].id, "20260402120000");
    }

    #[test]
    fn recent_doogats_falls_back_to_updated_at_when_no_date() {
        let idx = in_memory_index();

        // Doogat with no frontmatter date - updated_at is set to now() by
        // the indexer, so it should appear as recent
        let z = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260403120000".into())),
                title: Some("No Date Note".into()),
                date: None,
                doogat_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260403120000.md".into(),
            updated_at: None,
        };
        idx.index_doogat(&z).unwrap();

        let results = idx.recent_doogats(7, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "20260403120000");
    }

    #[test]
    fn recent_doogats_empty_when_none_recent() {
        let idx = in_memory_index();

        // Index nothing
        let results = idx.recent_doogats(7, None).unwrap();
        assert!(results.is_empty());
    }

    // ---- link_density tests ----

    /// Helper: create a doogat with outbound links to given target IDs.
    fn make_linked_doogat(
        id: &str,
        title: &str,
        dtype: &str,
        path: &str,
        link_targets: &[&str],
    ) -> ParsedDoogat {
        let links = link_targets
            .iter()
            .map(|target| crate::types::Link {
                target: (*target).into(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: Zone::Body,
            })
            .collect();

        ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId(id.into())),
                title: Some(title.into()),
                date: Some("2026-04-03".into()),
                doogat_type: Some(dtype.into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links,
            body_tags: vec![],
            checkboxes: vec![],
            path: path.into(),
            updated_at: None,
        }
    }

    #[test]
    fn link_density_counts_outbound_links() {
        let idx = in_memory_index();

        let a = make_linked_doogat(
            "20260403100000",
            "Alpha",
            "note",
            "ddb/20260403100000.md",
            &["20260403100001", "20260403100002"],
        );
        let b = make_doogat_with_date(
            "20260403100001",
            "Bravo",
            "2026-04-03",
            "note",
            "ddb/20260403100001.md",
        );
        let c = make_doogat_with_date(
            "20260403100002",
            "Charlie",
            "2026-04-03",
            "note",
            "ddb/20260403100002.md",
        );
        idx.index_doogat(&a).unwrap();
        idx.index_doogat(&b).unwrap();
        idx.index_doogat(&c).unwrap();

        let results = idx.link_density(None).unwrap();
        let alpha = results.iter().find(|e| e.id == "20260403100000").unwrap();
        assert_eq!(alpha.outbound_links, 2);
    }

    #[test]
    fn link_density_counts_inbound_links() {
        let idx = in_memory_index();

        // A and C both link to B
        let a = make_linked_doogat(
            "20260403100000",
            "Alpha",
            "note",
            "ddb/20260403100000.md",
            &["20260403100001"],
        );
        let b = make_doogat_with_date(
            "20260403100001",
            "Bravo",
            "2026-04-03",
            "note",
            "ddb/20260403100001.md",
        );
        let c = make_linked_doogat(
            "20260403100002",
            "Charlie",
            "note",
            "ddb/20260403100002.md",
            &["20260403100001"],
        );
        idx.index_doogat(&a).unwrap();
        idx.index_doogat(&b).unwrap();
        idx.index_doogat(&c).unwrap();

        let results = idx.link_density(None).unwrap();
        let bravo = results.iter().find(|e| e.id == "20260403100001").unwrap();
        assert_eq!(bravo.inbound_links, 2);
    }

    #[test]
    fn link_density_score_is_sum() {
        let idx = in_memory_index();

        // A links to B; C links to A. So A has outbound=1, inbound=1, density=2
        let a = make_linked_doogat(
            "20260403100000",
            "Alpha",
            "note",
            "ddb/20260403100000.md",
            &["20260403100001"],
        );
        let b = make_doogat_with_date(
            "20260403100001",
            "Bravo",
            "2026-04-03",
            "note",
            "ddb/20260403100001.md",
        );
        let c = make_linked_doogat(
            "20260403100002",
            "Charlie",
            "note",
            "ddb/20260403100002.md",
            &["20260403100000"],
        );
        idx.index_doogat(&a).unwrap();
        idx.index_doogat(&b).unwrap();
        idx.index_doogat(&c).unwrap();

        let results = idx.link_density(None).unwrap();
        let alpha = results.iter().find(|e| e.id == "20260403100000").unwrap();
        assert_eq!(alpha.inbound_links, 1);
        assert_eq!(alpha.outbound_links, 1);
        assert_eq!(alpha.density_score, alpha.inbound_links + alpha.outbound_links);
        assert_eq!(alpha.density_score, 2);
    }

    #[test]
    fn link_density_sorted_by_density_descending() {
        let idx = in_memory_index();

        // A links to B, C, D (outbound=3, density=3)
        let a = make_linked_doogat(
            "20260403100000",
            "Alpha",
            "note",
            "ddb/20260403100000.md",
            &["20260403100001", "20260403100002", "20260403100003"],
        );
        // B links to C (outbound=1, density=1 + inbound from A = 2)
        let b = make_linked_doogat(
            "20260403100001",
            "Bravo",
            "note",
            "ddb/20260403100001.md",
            &["20260403100002"],
        );
        // C has no outbound (inbound from A and B = 2, density=2)
        let c = make_doogat_with_date(
            "20260403100002",
            "Charlie",
            "2026-04-03",
            "note",
            "ddb/20260403100002.md",
        );
        // D has no outbound (inbound from A = 1, density=1)
        let d = make_doogat_with_date(
            "20260403100003",
            "Delta",
            "2026-04-03",
            "note",
            "ddb/20260403100003.md",
        );
        idx.index_doogat(&a).unwrap();
        idx.index_doogat(&b).unwrap();
        idx.index_doogat(&c).unwrap();
        idx.index_doogat(&d).unwrap();

        let results = idx.link_density(None).unwrap();

        // Verify descending order
        for w in results.windows(2) {
            assert!(
                w[0].density_score >= w[1].density_score,
                "expected {} (density {}) >= {} (density {})",
                w[0].id,
                w[0].density_score,
                w[1].id,
                w[1].density_score,
            );
        }

        // Alpha has highest density (outbound=3)
        assert_eq!(results[0].id, "20260403100000");
        assert_eq!(results[0].density_score, 3);
    }

    #[test]
    fn link_density_respects_type_filter() {
        let idx = in_memory_index();

        let note = make_linked_doogat(
            "20260403100000",
            "A Note",
            "note",
            "ddb/20260403100000.md",
            &["20260403100001"],
        );
        let project = make_linked_doogat(
            "20260403100001",
            "A Project",
            "project",
            "ddb/20260403100001.md",
            &["20260403100000"],
        );
        idx.index_doogat(&note).unwrap();
        idx.index_doogat(&project).unwrap();

        let results = idx.link_density(Some("project")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "20260403100001");
        assert_eq!(results[0].doogat_type, "project");
    }

    #[test]
    fn link_density_excludes_typedefs() {
        let idx = in_memory_index();

        let regular = make_linked_doogat(
            "20260403100000",
            "Regular",
            "note",
            "ddb/20260403100000.md",
            &["20260403100001"],
        );
        let typedef = make_linked_doogat(
            "20260403100001",
            "My Typedef",
            "note",
            "ddb/_typedef/20260403100001.md",
            &["20260403100000"],
        );
        idx.index_doogat(&regular).unwrap();
        idx.index_doogat(&typedef).unwrap();

        let results = idx.link_density(None).unwrap();
        // Only the regular doogat should appear
        assert!(
            results.iter().all(|e| e.id != "20260403100001"),
            "typedef should be excluded from results"
        );
        assert!(results.iter().any(|e| e.id == "20260403100000"));
    }

    #[test]
    fn link_density_empty_index() {
        let idx = in_memory_index();
        let results = idx.link_density(None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn link_density_zero_links() {
        let idx = in_memory_index();

        let lonely = make_doogat_with_date(
            "20260403100000",
            "Lonely",
            "2026-04-03",
            "note",
            "ddb/20260403100000.md",
        );
        idx.index_doogat(&lonely).unwrap();

        let results = idx.link_density(None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "20260403100000");
        assert_eq!(results[0].inbound_links, 0);
        assert_eq!(results[0].outbound_links, 0);
        assert_eq!(results[0].density_score, 0);
    }
