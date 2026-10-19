use super::frontmatter::derive_actor;
use super::*;
use crate::hlc::Hlc;
use crate::parser;
use crate::types::ConflictFile;

#[test]
fn frontmatter_different_fields() {
    let ancestor = "title: Original\ndate: 2026-01-01";
    let ours = "title: Changed by us\ndate: 2026-01-01";
    let theirs = "title: Original\ndate: 2026-01-01\nauthor: Them";

    let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
    assert!(merged.contains("Changed by us"));
    assert!(merged.contains("author"));
}

#[test]
fn frontmatter_same_field_conflict() {
    let ancestor = "title: Original";
    let ours = "title: Ours";
    let theirs = "title: Theirs";

    // Automerge picks one deterministically
    let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
    assert!(merged.contains("title:"));
}

#[test]
fn frontmatter_field_removal() {
    let ancestor = "title: Test\nauthor: Bob";
    let ours = "title: Test"; // removed author
    let theirs = "title: Test\nauthor: Bob";

    let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
    assert!(merged.contains("title:"));
    // author should be removed since ours removed it
    assert!(!merged.contains("author"));
}

#[test]
fn frontmatter_preserves_quotes_for_yaml_special_chars() {
    let ancestor = "title: Original";
    let ours = "title: \"[[note|My Note]]\"";
    let theirs = "title: Original";

    let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
    assert!(merged.contains("title: \"[[note|My Note]]\""));
}

#[test]
fn frontmatter_unquotes_flow_sequence_values() {
    let ancestor = "tags:\n  - shared";
    let ours = "tags:\n  - shared\n  - ours";
    let theirs = "tags:\n  - shared";

    let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
    let parsed = parser::parse_frontmatter(&merged, "test.md").unwrap();
    assert_eq!(parsed.tags, vec!["shared", "ours"]);
}

#[test]
fn frontmatter_concurrent_tag_additions_merge() {
    let ancestor = "title: Test\ntags:\n  - shared";
    let ours = "title: Test\ntags:\n  - shared\n  - ours-tag";
    let theirs = "title: Test\ntags:\n  - shared\n  - theirs-tag";

    let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
    let parsed = parser::parse_frontmatter(&merged, "test.md").unwrap();
    // Both concurrent additions must survive
    assert!(parsed.tags.contains(&"shared".to_string()));
    assert!(parsed.tags.contains(&"ours-tag".to_string()));
    assert!(parsed.tags.contains(&"theirs-tag".to_string()));
    assert_eq!(parsed.tags.len(), 3);
}

#[test]
fn frontmatter_tag_removal_honored() {
    let ancestor = "tags:\n  - keep\n  - remove";
    let ours = "tags:\n  - keep"; // removed 'remove'
    let theirs = "tags:\n  - keep\n  - remove";

    let (merged, _) = merge_frontmatter(ancestor, ours, theirs).unwrap();
    let parsed = parser::parse_frontmatter(&merged, "test.md").unwrap();
    assert_eq!(parsed.tags, vec!["keep"]);
}

#[test]
fn frontmatter_returns_loadable_automerge_bytes() {
    let ancestor = "title: Original\ndate: 2026-01-01";
    let ours = "title: Changed\ndate: 2026-01-01";
    let theirs = "title: Original\ndate: 2026-01-01\nauthor: Them";

    let (yaml, doc_bytes) = merge_frontmatter(ancestor, ours, theirs).unwrap();
    assert!(!yaml.is_empty());
    assert!(!doc_bytes.is_empty());

    // Bytes must be a valid automerge document
    let loaded = automerge::AutoCommit::load(&doc_bytes);
    assert!(loaded.is_ok(), "automerge doc bytes should be loadable");
}

#[test]
fn body_non_overlapping_edits() {
    let ancestor = "Line 1\nLine 2\nLine 3\n";
    let ours = "Line 1 edited\nLine 2\nLine 3\n";
    let theirs = "Line 1\nLine 2\nLine 3 edited\n";

    let merged = merge_body(ancestor, ours, theirs).unwrap();
    assert!(merged.contains("Line 1 edited"));
    assert!(merged.contains("Line 3 edited"));
}

#[test]
fn body_append_from_both() {
    let ancestor = "Line 1\n";
    let ours = "Line 1\nOurs appended\n";
    let theirs = "Line 1\nTheirs appended\n";

    let merged = merge_body(ancestor, ours, theirs).unwrap();
    assert!(merged.contains("Line 1"));
    // Both appends should be present in some order
    assert!(merged.contains("appended"));
}

#[test]
fn reference_union_additions() {
    let ancestor = "- source:: Wikipedia";
    let ours = "- source:: Wikipedia\n- author:: Bob";
    let theirs = "- source:: Wikipedia\n- year:: 2026";

    let merged = merge_reference(ancestor, ours, theirs).unwrap();
    assert!(merged.contains("- source:: Wikipedia"));
    assert!(merged.contains("- author:: Bob"));
    assert!(merged.contains("- year:: 2026"));
}

#[test]
fn reference_removal() {
    let ancestor = "- source:: Wikipedia\n- author:: Bob";
    let ours = "- source:: Wikipedia"; // removed author
    let theirs = "- source:: Wikipedia\n- author:: Bob";

    let merged = merge_reference(ancestor, ours, theirs).unwrap();
    assert!(merged.contains("- source:: Wikipedia"));
    assert!(!merged.contains("author"));
}

#[test]
fn reference_same_key_conflict() {
    let ancestor = "- source:: Wikipedia";
    let ours = "- source:: Ours Source";
    let theirs = "- source:: Theirs Source";

    let merged = merge_reference(ancestor, ours, theirs).unwrap();
    assert!(merged.contains("- source::"));
}

#[test]
fn reference_concurrent_additions_both_present() {
    let ancestor = "- source:: Wikipedia";
    let ours = "- source:: Wikipedia\n- author:: Alice";
    let theirs = "- source:: Wikipedia\n- year:: 2026";

    let merged = merge_reference(ancestor, ours, theirs).unwrap();
    assert!(merged.contains("- source:: Wikipedia"));
    assert!(merged.contains("- author:: Alice"));
    assert!(merged.contains("- year:: 2026"));
}

#[test]
fn full_pipeline_resolve() {
    let ancestor = "---\ntitle: Original\n---\nOriginal body.\n---\n- source:: Wikipedia";
    let ours = "---\ntitle: Changed\n---\nOurs body.\n---\n- source:: Wikipedia";
    let theirs =
        "---\ntitle: Original\n---\nOriginal body.\n---\n- source:: Wikipedia\n- author:: Bob";

    let conflicts = vec![ConflictFile {
        path: "ddb/20260226120000.md".into(),
        ancestor: Some(ancestor.into()),
        ours: ours.into(),
        theirs: theirs.into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: None,
        theirs_blob_oid: None,
    }];

    let resolved = resolve_conflicts(conflicts, None).unwrap();
    assert_eq!(resolved.len(), 1);

    // Should be valid parseable markdown
    let parsed = parser::parse(&resolved[0].content, &resolved[0].path).unwrap();
    assert!(parsed.meta.title.is_some());
}

#[test]
fn full_pipeline_no_ancestor() {
    let ours = "---\ntitle: Ours\n---\nOurs body.";
    let theirs = "---\ntitle: Theirs\n---\nTheirs body.";

    let conflicts = vec![ConflictFile {
        path: "ddb/new.md".into(),
        ancestor: None,
        ours: ours.into(),
        theirs: theirs.into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: None,
        theirs_blob_oid: None,
    }];

    let resolved = resolve_conflicts(conflicts, None).unwrap();
    assert_eq!(resolved.len(), 1);
    let parsed = parser::parse(&resolved[0].content, &resolved[0].path).unwrap();
    assert!(parsed.meta.title.is_some());
}

#[test]
fn body_intra_line_char_level_merge() {
    let ancestor = "hello world";
    let ours = "hello brave world";
    let theirs = "hello world!";

    let merged = merge_body(ancestor, ours, theirs).unwrap();
    assert!(merged.contains("brave"));
    assert!(merged.contains("!"));
}

// --- lww_pick: the single tie-break rule shared by all three sites ---

#[test]
fn lww_pick_both_hlc_differ_higher_hlc_wins_theirs() {
    let ours_hlc = Hlc {
        wall_ms: 100,
        counter: 0,
        node: "aaaaaaaa".into(),
    };
    let theirs_hlc = Hlc {
        wall_ms: 200,
        counter: 0,
        node: "bbbbbbbb".into(),
    };
    // Content keys deliberately favour ours; the higher HLC must still decide.
    let pick = lww_pick(Some(&ours_hlc), Some(&theirs_hlc), "zzz_ours", "aaa_theirs");
    assert_eq!(pick, LwwSide::Theirs);
}

#[test]
fn lww_pick_both_hlc_differ_higher_hlc_wins_ours() {
    let ours_hlc = Hlc {
        wall_ms: 300,
        counter: 0,
        node: "aaaaaaaa".into(),
    };
    let theirs_hlc = Hlc {
        wall_ms: 200,
        counter: 0,
        node: "bbbbbbbb".into(),
    };
    // Content keys deliberately favour theirs; the higher HLC must still decide.
    let pick = lww_pick(Some(&ours_hlc), Some(&theirs_hlc), "aaa_ours", "zzz_theirs");
    assert_eq!(pick, LwwSide::Ours);
}

#[test]
fn lww_pick_equal_hlc_picks_higher_content_key() {
    let hlc = Hlc {
        wall_ms: 100,
        counter: 0,
        node: "samenode".into(),
    };
    // Exactly-equal HLCs → content key decides. "theirs_key" > "ours_key" by str `>`.
    assert_eq!(
        lww_pick(Some(&hlc), Some(&hlc), "ours_key", "theirs_key"),
        LwwSide::Theirs
    );
    // And the other direction: higher key on ours.
    assert_eq!(
        lww_pick(Some(&hlc), Some(&hlc), "zzz_key", "aaa_key"),
        LwwSide::Ours
    );
}

#[test]
fn lww_pick_equal_wall_ms_and_node_orders_by_counter_not_content() {
    // Same wall_ms AND same node: the HLC `counter` component must break the
    // tie, NOT the content key. The higher-`counter` side is given the
    // LEXICALLY-LOWER content key, so "order by counter" and "fall back to
    // content key" DISAGREE. Guards against an impl that compares
    // wall_ms + node but drops counter.
    let ours_hlc = Hlc {
        wall_ms: 100,
        counter: 3, // lower counter
        node: "samenode".into(),
    };
    let theirs_hlc = Hlc {
        wall_ms: 100,
        counter: 5, // higher counter
        node: "samenode".into(),
    };
    // Content key favours ours ("zzz" > "aaa"); the higher counter must win.
    assert_eq!(
        lww_pick(Some(&ours_hlc), Some(&theirs_hlc), "zzz", "aaa"),
        LwwSide::Theirs
    );
}

#[test]
fn lww_pick_cross_magnitude_wall_ms_compares_numerically_not_lexically() {
    // Cross-magnitude wall_ms: the numerically-later side is LEXICALLY lower
    // in the HLC Display form ("900-..." vs "1000-...", '9' > '1'). The rule
    // must compare wall_ms numerically (Hlc::Ord), not as a string. Content
    // key favours ours ("z" > "a") so a fallback would also pick the wrong side.
    let ours = Hlc {
        wall_ms: 900,
        counter: 0,
        node: "aaaaaaaa".into(),
    };
    let theirs = Hlc {
        wall_ms: 1000,
        counter: 0,
        node: "bbbbbbbb".into(),
    };
    assert_eq!(lww_pick(Some(&ours), Some(&theirs), "z", "a"), LwwSide::Theirs);
}

#[test]
fn lww_pick_missing_hlc_picks_higher_content_key() {
    let hlc = Hlc {
        wall_ms: 100,
        counter: 0,
        node: "somenode".into(),
    };
    // Ours None → content key decides.
    assert_eq!(lww_pick(None, Some(&hlc), "aaa", "bbb"), LwwSide::Theirs);
    // Theirs None → content key decides.
    assert_eq!(lww_pick(Some(&hlc), None, "bbb", "aaa"), LwwSide::Ours);
    // Both None → content key decides.
    assert_eq!(lww_pick(None, None, "aaa", "bbb"), LwwSide::Theirs);
    assert_eq!(lww_pick(None, None, "bbb", "aaa"), LwwSide::Ours);
}

#[test]
fn lww_pick_role_swap_converges_on_same_content_key() {
    // Missing/equal case: the winning CONTENT KEY is invariant under role swap.
    let ours_key = "alpha";
    let theirs_key = "omega"; // omega > alpha by str `>`

    let orig = lww_pick(None, None, ours_key, theirs_key);
    let winning_key_orig = match orig {
        LwwSide::Ours => ours_key,
        LwwSide::Theirs => theirs_key,
    };

    // Swap the (hlc, key) argument pairs; the winning key must not change.
    let swapped = lww_pick(None, None, theirs_key, ours_key);
    let winning_key_swapped = match swapped {
        LwwSide::Ours => theirs_key,
        LwwSide::Theirs => ours_key,
    };

    assert_eq!(
        winning_key_orig, winning_key_swapped,
        "winning content key must be invariant under role swap"
    );
    assert_eq!(winning_key_orig, theirs_key, "the higher content key wins");
}

#[test]
fn lww_picks_later_hlc() {
    let ours_hlc = Hlc {
        wall_ms: 100,
        counter: 0,
        node: "aaaaaaaa".into(),
    };
    let theirs_hlc = Hlc {
        wall_ms: 200,
        counter: 0,
        node: "bbbbbbbb".into(),
    };
    let conflicts = vec![ConflictFile {
        path: "ddb/test.md".into(),
        ancestor: None,
        ours: "---\ntitle: Ours\n---\nOurs body.".into(),
        theirs: "---\ntitle: Theirs\n---\nTheirs body.".into(),
        ours_hlc: Some(ours_hlc),
        theirs_hlc: Some(theirs_hlc),
        ours_blob_oid: None,
        theirs_blob_oid: None,
    }];

    let resolved = resolve_lww(conflicts).unwrap();
    assert!(resolved[0].content.contains("Theirs"));
}

#[test]
fn lww_equal_wall_ms_orders_by_hlc_node_not_content() {
    // Same wall_ms AND same counter: the HLC `node` component must break the
    // tie, NOT the content key. To tell the two rules apart, the higher-`node`
    // side is given the LEXICALLY-LOWER content, so "order by HLC node" and
    // "fall back to content key" DISAGREE on the winner.
    let ours_hlc = Hlc {
        wall_ms: 100,
        counter: 0,
        node: "aaaaaaaa".into(), // lower node
    };
    let theirs_hlc = Hlc {
        wall_ms: 100,
        counter: 0,
        node: "zzzzzzzz".into(), // higher node
    };
    let conflicts = vec![ConflictFile {
        path: "ddb/test.md".into(),
        ancestor: None,
        ours: "zzz body".into(),   // higher content key, but LOWER HLC node
        theirs: "aaa body".into(), // lower content key, but HIGHER HLC node
        ours_hlc: Some(ours_hlc),
        theirs_hlc: Some(theirs_hlc),
        ours_blob_oid: None,
        theirs_blob_oid: None,
    }];

    let resolved = resolve_lww(conflicts).unwrap();
    // Higher HLC node (theirs) wins even though its content key is lower.
    // A wall_ms-only impl would treat this as a tie, fall back to the content
    // key, and return "zzz body" (ours) — which is WRONG.
    assert_eq!(resolved[0].content, "aaa body");
}

#[test]
fn lww_missing_hlc_picks_higher_content_key() {
    // resolve_lww's content key is the side's full text. With both HLCs
    // absent, the higher text (str `>`) wins: "theirs content" > "ours content".
    let conflicts = vec![ConflictFile {
        path: "ddb/test.md".into(),
        ancestor: None,
        ours: "ours content".into(),
        theirs: "theirs content".into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: None,
        theirs_blob_oid: None,
    }];

    let resolved = resolve_lww(conflicts).unwrap();
    assert_eq!(resolved[0].content, "theirs content");

    // Role-swap convergence: swap ours/theirs → same winning content.
    let swapped = vec![ConflictFile {
        path: "ddb/test.md".into(),
        ancestor: None,
        ours: "theirs content".into(),
        theirs: "ours content".into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: None,
        theirs_blob_oid: None,
    }];

    let resolved_swapped = resolve_lww(swapped).unwrap();
    assert_eq!(resolved_swapped[0].content, "theirs content");
}

#[test]
fn append_log_different_entries_both_survive() {
    let ancestor =
        "---\ntitle: Project\ntype: project\n---\n## Log\n- [x] 2026-01-01 Setup project";
    let ours = "---\ntitle: Project\ntype: project\n---\n## Log\n- [x] 2026-01-01 Setup project\n- [x] 2026-01-05 Laptop entry";
    let theirs = "---\ntitle: Project\ntype: project\n---\n## Log\n- [x] 2026-01-01 Setup project\n- [x] 2026-01-03 Desktop entry";

    let conflicts = vec![ConflictFile {
        path: "ddb/project/test.md".into(),
        ancestor: Some(ancestor.into()),
        ours: ours.into(),
        theirs: theirs.into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: None,
        theirs_blob_oid: None,
    }];

    let resolved = resolve_append_log(conflicts).unwrap();
    let content = &resolved[0].content;
    assert!(content.contains("Laptop entry"), "missing laptop entry");
    assert!(content.contains("Desktop entry"), "missing desktop entry");
    // Sorted chronologically: desktop (01-03) before laptop (01-05)
    let desktop_pos = content.find("Desktop entry").unwrap();
    let laptop_pos = content.find("Laptop entry").unwrap();
    assert!(
        desktop_pos < laptop_pos,
        "entries not sorted chronologically"
    );
}

#[test]
fn append_log_dedup_same_entry() {
    let ancestor = "---\ntitle: P\n---\n## Log\n- [x] 2026-01-01 Init";
    let ours = "---\ntitle: P\n---\n## Log\n- [x] 2026-01-01 Init\n- [x] 2026-02-01 Same";
    let theirs = "---\ntitle: P\n---\n## Log\n- [x] 2026-01-01 Init\n- [x] 2026-02-01 Same";

    let conflicts = vec![ConflictFile {
        path: "ddb/test.md".into(),
        ancestor: Some(ancestor.into()),
        ours: ours.into(),
        theirs: theirs.into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: None,
        theirs_blob_oid: None,
    }];

    let resolved = resolve_append_log(conflicts).unwrap();
    let content = &resolved[0].content;
    // "Same" should appear only once
    assert_eq!(content.matches("Same").count(), 1);
}

#[test]
fn append_log_non_log_sections_use_text_crdt() {
    let ancestor = "---\ntitle: P\n---\n## Description\nOriginal description.\n## Log\n- [x] 2026-01-01 Init";
    let ours = "---\ntitle: P\n---\n## Description\nUpdated description from laptop.\n## Log\n- [x] 2026-01-01 Init";
    let theirs = "---\ntitle: P\n---\n## Description\nOriginal description.\n## Log\n- [x] 2026-01-01 Init\n- [x] 2026-02-01 New entry";

    let conflicts = vec![ConflictFile {
        path: "ddb/test.md".into(),
        ancestor: Some(ancestor.into()),
        ours: ours.into(),
        theirs: theirs.into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: None,
        theirs_blob_oid: None,
    }];

    let resolved = resolve_append_log(conflicts).unwrap();
    let content = &resolved[0].content;
    assert!(content.contains("Updated description from laptop"));
    assert!(content.contains("New entry"));
}

#[test]
fn append_log_empty_log_section() {
    let ancestor = "---\ntitle: P\n---\n## Log\n";
    let ours = "---\ntitle: P\n---\n## Log\n- [x] 2026-01-01 First entry";
    let theirs = "---\ntitle: P\n---\n## Log\n";

    let conflicts = vec![ConflictFile {
        path: "ddb/test.md".into(),
        ancestor: Some(ancestor.into()),
        ours: ours.into(),
        theirs: theirs.into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: None,
        theirs_blob_oid: None,
    }];

    let resolved = resolve_append_log(conflicts).unwrap();
    assert!(resolved[0].content.contains("First entry"));
}

#[test]
fn resolve_with_non_default_strategy_still_works() {
    let ancestor = "---\ntitle: Base\n---\nBody.";
    let ours = "---\ntitle: Ours\n---\nBody.";
    let theirs = "---\ntitle: Base\n---\nBody changed.";

    let conflicts = vec![ConflictFile {
        path: "ddb/20260226120000.md".into(),
        ancestor: Some(ancestor.into()),
        ours: ours.into(),
        theirs: theirs.into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: None,
        theirs_blob_oid: None,
    }];

    // Should succeed (using default strategy) and log a warning
    let resolved = resolve_conflicts(conflicts, Some("preset:append-log")).unwrap();
    assert_eq!(resolved.len(), 1);
    let parsed = parser::parse(&resolved[0].content, &resolved[0].path).unwrap();
    assert!(parsed.meta.title.is_some());
}

#[test]
fn resolve_errors_on_unparseable_content() {
    // Content without frontmatter markers → parse_zones fails
    // This proves the LWW fallback path in sync_manager is reachable
    let conflicts = vec![ConflictFile {
        path: "ddb/broken.md".into(),
        ancestor: Some("no frontmatter here".into()),
        ours: "also no frontmatter".into(),
        theirs: "still no frontmatter".into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: None,
        theirs_blob_oid: None,
    }];

    let result = resolve_conflicts(conflicts, None);
    assert!(result.is_err());
}

#[test]
fn derive_actor_same_content_same_actor() {
    let content = "title: A\nx: 1";
    let a = derive_actor(content);
    let b = derive_actor(content);
    assert_eq!(a, b);
}

#[test]
fn derive_actor_different_content_different_actor() {
    let a = derive_actor("title: A\nx: 1");
    let b = derive_actor("title: B\nx: 2");
    assert_ne!(a, b);
}

#[test]
fn derive_actor_produces_valid_actor_id() {
    let actor = derive_actor("title: A\nx: 1");
    assert_eq!(actor.to_bytes().len(), 16);
}

#[test]
fn merge_frontmatter_is_byte_identical_on_re_resolution() {
    let ancestor = "title: Base\nstatus: open";
    let ours = "title: Base\nstatus: ours";
    let theirs = "title: Base\nstatus: theirs";

    let (yaml_a, bytes_a) = merge_frontmatter(ancestor, ours, theirs).unwrap();
    let (yaml_b, bytes_b) = merge_frontmatter(ancestor, ours, theirs).unwrap();

    assert_eq!(yaml_a, yaml_b);
    assert_eq!(bytes_a, bytes_b);
}
