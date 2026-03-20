use super::*;

    #[test]
    fn backlinks_include_all_link_kinds() {
        let idx = in_memory_index();

        // Target zettel
        let target = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301120000".into())),
                title: Some("Target".into()),
                date: None,
                zettel_type: None,
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
            path: "zettelkasten/20260301120000.md".into(),
        };
        idx.index_zettel(&target).unwrap();

        // Source zettel linking via all 4 kinds
        let source = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301100000".into())),
                title: Some("Source".into()),
                date: None,
                zettel_type: None,
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
            path: "zettelkasten/20260301100000.md".into(),
        };
        idx.index_zettel(&source).unwrap();

        // backlinks() returns the source regardless of link kind
        let bl = idx.backlinks("20260301120000").unwrap();
        assert_eq!(bl.len(), 1);
        assert_eq!(bl[0], "20260301100000");
    }

    #[test]
    fn backlink_query() {
        let idx = in_memory_index();
        idx.index_zettel(&sample_zettel()).unwrap();

        let ids = idx.backlinks("20260101000000").unwrap();
        assert!(ids.contains(&"20260226120000".to_string()));
    }

    #[test]
    fn resurrected_zettel_not_duplicated_after_reindex() {
        let idx = in_memory_index();
        let mut z = sample_zettel();
        z.meta
            .extra
            .insert("resurrected".into(), crate::types::Value::Bool(true));
        idx.index_zettel(&z).unwrap();
        // Reindex same zettel
        idx.index_zettel(&z).unwrap();

        let id = z.meta.id.as_ref().unwrap().0.as_str();
        let count: i64 = idx
            .conn
            .query_row(
                "SELECT COUNT(*) FROM zettels WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Also verify the resurrected field isn't duplicated
        let field_count: i64 = idx
            .conn
            .query_row(
                "SELECT COUNT(*) FROM _zdb_fields WHERE zettel_id = ?1 AND key = 'resurrected'",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(field_count, 1);
    }

    #[test]
    fn resurrected_zettels_query() {
        let idx = in_memory_index();

        // Zettel with resurrected: true
        let mut z1 = sample_zettel();
        z1.meta.extra.insert(
            "resurrected".into(),
            crate::types::Value::String("true".into()),
        );
        idx.index_zettel(&z1).unwrap();

        // Normal zettel without resurrected
        let z2 = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260302120000".into())),
                title: Some("Normal".into()),
                date: None,
                zettel_type: None,
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
            path: "zettelkasten/20260302120000.md".into(),
        };
        idx.index_zettel(&z2).unwrap();

        let results = idx.resurrected_zettels().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, z1.meta.id.as_ref().unwrap().0);
        assert_eq!(results[0].1, "Test Note");
    }

    #[test]
    fn resurrected_zettels_empty_when_none() {
        let idx = in_memory_index();
        let z = sample_zettel();
        idx.index_zettel(&z).unwrap();
        assert!(idx.resurrected_zettels().unwrap().is_empty());
    }

    #[test]
    fn backlinking_zettel_paths_returns_source_id_and_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        // Zettel A links to target B
        let zettel_a = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId("20260301100000".to_string())),
                title: Some("A".to_string()),
                date: None,
                zettel_type: None,
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
            path: "zettelkasten/20260301100000.md".to_string(),
        };

        // Zettel B is the target (no outgoing links)
        let zettel_b = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId("20260301120000".to_string())),
                title: Some("B".to_string()),
                date: None,
                zettel_type: None,
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
            path: "zettelkasten/20260301120000.md".to_string(),
        };

        index.index_zettel(&zettel_a).unwrap();
        index.index_zettel(&zettel_b).unwrap();

        let results = index.backlinking_zettel_paths("20260301120000").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "20260301100000");
        assert_eq!(results[0].1, "zettelkasten/20260301100000.md");

        // No backlinks for A
        let empty = index.backlinking_zettel_paths("20260301100000").unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn broken_backlinks_after_delete() {
        let index = in_memory_index();

        // Create target zettel A
        let a = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301100000".into())),
                title: Some("Target".into()),
                date: None,
                zettel_type: None,
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
            path: "zettelkasten/20260301100000.md".into(),
        };

        // Create zettel B that links to A
        let b = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301100001".into())),
                title: Some("Linker".into()),
                date: None,
                zettel_type: None,
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
            path: "zettelkasten/20260301100001.md".into(),
        };

        index.index_zettel(&a).unwrap();
        index.index_zettel(&b).unwrap();

        // No broken backlinks yet
        let broken = index.broken_backlinks().unwrap();
        assert!(broken.is_empty());

        // Delete A
        index.remove_zettel("20260301100000").unwrap();

        // B's link to A is now broken
        let broken = index.broken_backlinks().unwrap();
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].0, "20260301100001");
        assert_eq!(broken[0].1, "20260301100000");
    }

    #[test]
    fn unlinked_mentions_basic() {
        let idx = in_memory_index();

        // Zettel A: title "Project Alpha"
        let a = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301000000".into())),
                title: Some("Project Alpha".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260301000000.md".into(),
        };

        // Zettel B: body mentions "Project Alpha" but does NOT link to A
        let b = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301000001".into())),
                title: Some("Meeting Notes".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260301000001.md".into(),
        };

        idx.index_zettel(&a).unwrap();
        idx.index_zettel(&b).unwrap();

        let mentions = idx.unlinked_mentions("20260301000000").unwrap();
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].source_id, "20260301000001");
    }

    #[test]
    fn unlinked_mentions_excludes_linked() {
        let idx = in_memory_index();

        // Zettel A
        let a = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260302000000".into())),
                title: Some("Project Beta".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260302000000.md".into(),
        };

        // Zettel B: mentions "Project Beta" AND links to A via wikilink
        let b = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260302000001".into())),
                title: Some("Status Update".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260302000001.md".into(),
        };

        idx.index_zettel(&a).unwrap();
        idx.index_zettel(&b).unwrap();

        let mentions = idx.unlinked_mentions("20260302000000").unwrap();
        assert!(
            mentions.is_empty(),
            "linked zettel should not appear in unlinked mentions"
        );
    }

    #[test]
    fn unlinked_mentions_excludes_self() {
        let idx = in_memory_index();

        // Zettel whose body mentions its own title
        let a = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260303000000".into())),
                title: Some("Self Reference".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "This zettel is about Self Reference patterns.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260303000000.md".into(),
        };

        idx.index_zettel(&a).unwrap();

        let mentions = idx.unlinked_mentions("20260303000000").unwrap();
        assert!(
            mentions.is_empty(),
            "zettel should not appear in its own unlinked mentions"
        );
    }

    #[test]
    fn suggest_links_tag_overlap() {
        let idx = in_memory_index();

        // Source: tags [a, b, c]
        let source = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260304000000".into())),
                title: Some("Source".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260304000000.md".into(),
        };

        // Candidate1: tags [a, b] — 2 shared tags
        let c1 = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260304000001".into())),
                title: Some("Candidate One".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260304000001.md".into(),
        };

        // Candidate2: tags [a] — 1 shared tag
        let c2 = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260304000002".into())),
                title: Some("Candidate Two".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260304000002.md".into(),
        };

        idx.index_zettel(&source).unwrap();
        idx.index_zettel(&c1).unwrap();
        idx.index_zettel(&c2).unwrap();

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
        let source = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260305000000".into())),
                title: Some("Source".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260305000000.md".into(),
        };

        // Candidate: same tag as source
        let candidate = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260305000001".into())),
                title: Some("Candidate".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260305000001.md".into(),
        };

        idx.index_zettel(&source).unwrap();
        idx.index_zettel(&candidate).unwrap();

        let suggestions = idx.suggest_links("20260305000000", 10).unwrap();
        assert!(
            !suggestions.iter().any(|s| s.id == "20260305000001"),
            "already-linked zettel should be excluded from suggestions"
        );
    }

    #[test]
    fn suggest_links_respects_limit() {
        let idx = in_memory_index();

        // Source with tags
        let source = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260306000000".into())),
                title: Some("Source".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260306000000.md".into(),
        };
        idx.index_zettel(&source).unwrap();

        // Create 5 candidates all sharing the tag
        for i in 1..=5 {
            let id = format!("2026030600000{i}");
            let c = ParsedZettel {
                meta: ZettelMeta {
                    id: Some(ZettelId(id.clone())),
                    title: Some(format!("Candidate {i}")),
                    date: None,
                    zettel_type: Some("note".into()),
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
                path: format!("zettelkasten/{id}.md"),
            };
            idx.index_zettel(&c).unwrap();
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

        // Zettel A: no tags, title "Machine Learning"
        let a = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260314000000".into())),
                title: Some("Machine Learning".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260314000000.md".into(),
        };

        // Zettel B: no shared tags, body contains "machine learning"
        let b = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260314000001".into())),
                title: Some("Deep Learning".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260314000001.md".into(),
        };

        idx.index_zettel(&a).unwrap();
        idx.index_zettel(&b).unwrap();

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
    fn stale_zettels_basic() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Create a _typedef with stale_after_days: 1
        let typedef =
            "---\nid: 20260307000000\ntitle: task\ntype: _typedef\nstale_after_days: 1\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260307000000.md",
            typedef,
            "add typedef",
        )
        .unwrap();

        // Create a zettel of type "task" with an OLD git commit time (2020-01-01)
        let zettel =
            "---\nid: 20260307000001\ntitle: Old Task\ntype: task\ndate: 2020-01-01\n---\nBody.";
        commit_file_with_time(
            &repo,
            "zettelkasten/20260307000001.md",
            zettel,
            "add old task",
            1577836800, // 2020-01-01T00:00:00 UTC
        );

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let stale = idx.stale_zettels(&repo, None).unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, "20260307000001");
        assert_eq!(stale[0].zettel_type, "task");
    }

    #[test]
    fn stale_zettels_respects_type() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Type A: stale_after_days: 1
        let typedef_a =
            "---\nid: 20260313000000\ntitle: taskA\ntype: _typedef\nstale_after_days: 1\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260313000000.md",
            typedef_a,
            "add typedef A",
        )
        .unwrap();

        // Type B: stale_after_days: 1
        let typedef_b =
            "---\nid: 20260313000001\ntitle: taskB\ntype: _typedef\nstale_after_days: 1\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260313000001.md",
            typedef_b,
            "add typedef B",
        )
        .unwrap();

        // Zettel of type A with old git commit time
        let zettel_a = "---\nid: 20260313000002\ntitle: Old A\ntype: taskA\n---\nBody A.";
        commit_file_with_time(
            &repo,
            "zettelkasten/20260313000002.md",
            zettel_a,
            "add old A",
            1577836800, // 2020-01-01
        );

        // Zettel of type B with old git commit time
        let zettel_b = "---\nid: 20260313000003\ntitle: Old B\ntype: taskB\n---\nBody B.";
        commit_file_with_time(
            &repo,
            "zettelkasten/20260313000003.md",
            zettel_b,
            "add old B",
            1577836800, // 2020-01-01
        );

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        // Filter by type A — only type A zettel should be returned
        let stale = idx.stale_zettels(&repo, Some("taskA")).unwrap();
        assert_eq!(stale.len(), 1, "should return exactly one stale zettel");
        assert_eq!(stale[0].id, "20260313000002");
        assert_eq!(stale[0].zettel_type, "taskA");

        // Unfiltered — both should appear
        let all_stale = idx.stale_zettels(&repo, None).unwrap();
        assert_eq!(
            all_stale.len(),
            2,
            "unfiltered should return both stale zettels"
        );
    }

    #[test]
    fn stale_zettels_no_threshold() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // _typedef without stale_after_days
        let typedef = "---\nid: 20260308000000\ntitle: note\ntype: _typedef\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260308000000.md",
            typedef,
            "add typedef",
        )
        .unwrap();

        // Zettel of type "note" with old date
        let zettel =
            "---\nid: 20260308000001\ntitle: Old Note\ntype: note\ndate: 2020-01-01\n---\nBody.";
        repo.commit_file("zettelkasten/20260308000001.md", zettel, "add note")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let stale = idx.stale_zettels(&repo, None).unwrap();
        assert!(
            stale.is_empty(),
            "type without stale_after_days should not report stale zettels"
        );
    }

    #[test]
    fn orphan_zettels_basic() {
        let idx = in_memory_index();

        // Zettel with no incoming links
        let orphan = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260309000000".into())),
                title: Some("Orphan".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260309000000.md".into(),
        };
        idx.index_zettel(&orphan).unwrap();

        let orphans = idx.orphan_zettels(None).unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].id, "20260309000000");
    }

    #[test]
    fn orphan_zettels_excludes_linked() {
        let idx = in_memory_index();

        // Zettel B: target of a link
        let b = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260310000001".into())),
                title: Some("Linked Target".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260310000001.md".into(),
        };

        // Zettel A: links to B
        let a = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260310000000".into())),
                title: Some("Linker".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260310000000.md".into(),
        };

        idx.index_zettel(&b).unwrap();
        idx.index_zettel(&a).unwrap();

        let orphans = idx.orphan_zettels(None).unwrap();
        assert!(
            !orphans.iter().any(|o| o.id == "20260310000001"),
            "zettel with incoming link should not be an orphan"
        );
    }

    #[test]
    fn orphan_zettels_excludes_typedef() {
        let idx = in_memory_index();

        // _typedef zettel (no incoming links)
        let typedef = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260311000000".into())),
                title: Some("task".into()),
                date: None,
                zettel_type: Some("_typedef".into()),
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
            path: "zettelkasten/_typedef/20260311000000.md".into(),
        };
        idx.index_zettel(&typedef).unwrap();

        let orphans = idx.orphan_zettels(None).unwrap();
        assert!(
            !orphans.iter().any(|o| o.id == "20260311000000"),
            "_typedef zettels should never appear in orphan results"
        );
    }

    #[test]
    fn orphan_zettels_includes_outgoing_count() {
        let idx = in_memory_index();

        // Orphan zettel with 2 outgoing links (but no incoming)
        let orphan = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260312000000".into())),
                title: Some("Orphan With Links".into()),
                date: None,
                zettel_type: Some("note".into()),
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
            path: "zettelkasten/20260312000000.md".into(),
        };
        idx.index_zettel(&orphan).unwrap();

        let orphans = idx.orphan_zettels(None).unwrap();
        let found = orphans.iter().find(|o| o.id == "20260312000000");
        assert!(found.is_some(), "orphan should be returned");
        assert_eq!(found.unwrap().outgoing_links, 2);
    }

    #[test]
    fn sequence_children_basic() {
        let idx = in_memory_index();
        let parent = seq_zettel("20260315100000", "Root", None);
        let child1 = seq_zettel("20260315100001", "Child A", Some("20260315100000"));
        let child2 = seq_zettel("20260315100002", "Child B", Some("20260315100000"));
        idx.index_zettel(&parent).unwrap();
        idx.index_zettel(&child1).unwrap();
        idx.index_zettel(&child2).unwrap();

        let children = idx.sequence_children("20260315100000").unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].id, "20260315100001");
        assert_eq!(children[1].id, "20260315100002");
    }

    #[test]
    fn sequence_children_empty() {
        let idx = in_memory_index();
        let z = seq_zettel("20260315110000", "Standalone", None);
        idx.index_zettel(&z).unwrap();

        let children = idx.sequence_children("20260315110000").unwrap();
        assert!(children.is_empty());
    }

    #[test]
    fn sequence_breadcrumb_chain() {
        let idx = in_memory_index();
        let root = seq_zettel("20260315120000", "Root", None);
        let mid = seq_zettel("20260315120001", "Mid", Some("20260315120000"));
        let leaf = seq_zettel("20260315120002", "Leaf", Some("20260315120001"));
        idx.index_zettel(&root).unwrap();
        idx.index_zettel(&mid).unwrap();
        idx.index_zettel(&leaf).unwrap();

        let bc = idx.sequence_breadcrumb("20260315120002").unwrap();
        assert_eq!(bc.len(), 3);
        assert_eq!(bc[0].id, "20260315120000");
        assert_eq!(bc[1].id, "20260315120001");
        assert_eq!(bc[2].id, "20260315120002");
    }

    #[test]
    fn sequence_breadcrumb_root() {
        let idx = in_memory_index();
        let root = seq_zettel("20260315130000", "Root", None);
        idx.index_zettel(&root).unwrap();

        let bc = idx.sequence_breadcrumb("20260315130000").unwrap();
        assert_eq!(bc.len(), 1);
        assert_eq!(bc[0].id, "20260315130000");
    }

    #[test]
    fn sequence_breadcrumb_cycle() {
        let idx = in_memory_index();
        let a = seq_zettel("20260315140000", "A", Some("20260315140001"));
        let b = seq_zettel("20260315140001", "B", Some("20260315140000"));
        idx.index_zettel(&a).unwrap();
        idx.index_zettel(&b).unwrap();

        let bc = idx.sequence_breadcrumb("20260315140000").unwrap();
        // Should not hang; returns partial chain
        assert!(bc.len() <= 3);
    }

    #[test]
    fn sequence_info_complete() {
        let idx = in_memory_index();
        let root = seq_zettel("20260315150000", "Root", None);
        let mid = seq_zettel("20260315150001", "Mid", Some("20260315150000"));
        let child1 = seq_zettel("20260315150002", "Child C", Some("20260315150001"));
        let child2 = seq_zettel("20260315150003", "Child D", Some("20260315150001"));
        idx.index_zettel(&root).unwrap();
        idx.index_zettel(&mid).unwrap();
        idx.index_zettel(&child1).unwrap();
        idx.index_zettel(&child2).unwrap();

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
        let z = seq_zettel("20260315160000", "Orphan", Some("99999999999999"));
        idx.index_zettel(&z).unwrap();

        let broken = idx.broken_sequences().unwrap();
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].zettel_id, "20260315160000");
        assert_eq!(broken[0].broken_parent_id, "99999999999999");
    }

    #[test]
    fn broken_sequence_clean() {
        let idx = in_memory_index();
        let root = seq_zettel("20260315170000", "Root", None);
        let child = seq_zettel("20260315170001", "Child", Some("20260315170000"));
        idx.index_zettel(&root).unwrap();
        idx.index_zettel(&child).unwrap();

        let broken = idx.broken_sequences().unwrap();
        assert!(broken.is_empty());
    }

    #[test]
    fn sequence_tree_recursive() {
        let idx = in_memory_index();
        let root = seq_zettel("20260315180000", "Root", None);
        let mid = seq_zettel("20260315180001", "Mid", Some("20260315180000"));
        let leaf = seq_zettel("20260315180002", "Leaf", Some("20260315180001"));
        idx.index_zettel(&root).unwrap();
        idx.index_zettel(&mid).unwrap();
        idx.index_zettel(&leaf).unwrap();

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
        // Zettel points to nonexistent parent
        let z = seq_zettel("20260315190000", "Orphan", Some("99999999999999"));
        idx.index_zettel(&z).unwrap();

        let bc = idx.sequence_breadcrumb("20260315190000").unwrap();
        // Should return just self, not a phantom node for the missing parent
        assert_eq!(bc.len(), 1);
        assert_eq!(bc[0].id, "20260315190000");
    }

