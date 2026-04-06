
use super::*;
use crate::types::{DoogatMeta, InlineField, Section, Severity, TitleSource, Zone};
use std::collections::BTreeMap;

fn empty_parsed() -> ParsedDoogat {
    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(crate::types::DoogatId("20260315120000".to_string())),
            title: Some("Test note".to_string()),
            date: Some("2026-03-15".to_string()),
            doogat_type: Some("note".to_string()),
            tags: vec!["alpha".to_string(), "beta".to_string()],
            extra: BTreeMap::new(),
        },
        body: String::new(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: "ddb/20260315120000.md".to_string(),
        updated_at: None,
    }
}

#[test]
fn no_fixes_clean_doogat() {
    let parsed = empty_parsed();
    let fixes = detect_fixes(&parsed, None);
    assert!(
        fixes.is_empty(),
        "clean doogat should need no fixes: {fixes:?}"
    );
}

#[test]
fn detect_duplicate_tags() {
    let mut parsed = empty_parsed();
    parsed.meta.tags = vec!["a".into(), "b".into(), "a".into()];
    let fixes = detect_fixes(&parsed, None);
    assert!(
        fixes
            .iter()
            .any(|f| matches!(f, Fix::TagsDeduped { removed } if removed == &["a"])),
        "should detect duplicate tag: {fixes:?}"
    );
}

#[test]
fn detect_hash_tags() {
    let mut parsed = empty_parsed();
    parsed.meta.tags = vec!["#gtd".into(), "work".into()];
    let fixes = detect_fixes(&parsed, None);
    assert!(
        fixes
            .iter()
            .any(|f| matches!(f, Fix::TagsStrippedHash { tags } if tags == &["#gtd"])),
        "should detect #-prefixed tag: {fixes:?}"
    );
}

#[test]
fn detect_unsorted_tags() {
    let mut parsed = empty_parsed();
    parsed.meta.tags = vec!["zebra".into(), "apple".into()];
    let fixes = detect_fixes(&parsed, None);
    assert!(
        fixes.iter().any(|f| matches!(f, Fix::TagsSorted)),
        "should detect unsorted tags: {fixes:?}"
    );
}

#[test]
fn detect_missing_type() {
    let mut parsed = empty_parsed();
    parsed.meta.doogat_type = None;
    let fixes = detect_fixes(&parsed, None);
    assert!(
        fixes.iter().any(
            |f| matches!(f, Fix::DefaultSet { field, value } if field == "type" && value == "note")
        ),
        "should detect missing type: {fixes:?}"
    );
}

#[test]
fn detect_missing_title_from_h1() {
    let mut parsed = empty_parsed();
    parsed.meta.title = None;
    parsed.sections = vec![Section {
        heading: "My Heading".to_string(),
        level: 1,
        content: String::new(),
    }];
    let fixes = detect_fixes(&parsed, None);
    assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::TitleDerived { source: TitleSource::FirstH1(h) } if h == "My Heading")
            ),
            "should derive title from H1: {fixes:?}"
        );
}

#[test]
fn detect_missing_title_from_filename() {
    let mut parsed = empty_parsed();
    parsed.meta.title = None;
    parsed.sections = vec![];
    parsed.path = "ddb/20260315120000-project-plan.md".to_string();
    let fixes = detect_fixes(&parsed, None);
    assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::TitleDerived { source: TitleSource::Filename(n) } if n == "Project plan")
            ),
            "should derive title from filename: {fixes:?}"
        );
}

#[test]
fn detect_camel_case_key() {
    let mut parsed = empty_parsed();
    parsed
        .meta
        .extra
        .insert("CamelKey".into(), crate::types::Value::String("val".into()));
    let fixes = detect_fixes(&parsed, None);
    assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::KeyNormalized { old, new } if old == "CamelKey" && new == "camel-key")
            ),
            "should detect CamelCase key: {fixes:?}"
        );
}

#[test]
fn detect_snake_case_key() {
    let mut parsed = empty_parsed();
    parsed.meta.extra.insert(
        "snake_key".into(),
        crate::types::Value::String("val".into()),
    );
    let fixes = detect_fixes(&parsed, None);
    assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::KeyNormalized { old, new } if old == "snake_key" && new == "snake-key")
            ),
            "should detect snake_case key: {fixes:?}"
        );
}

#[test]
fn detect_cross_zone_duplicate() {
    let mut parsed = empty_parsed();
    parsed
        .meta
        .extra
        .insert("project".into(), crate::types::Value::String("foo".into()));
    parsed.inline_fields.push(InlineField {
        key: "project".into(),
        value: "bar".into(),
        zone: Zone::Body,
    });
    let fixes = detect_fixes(&parsed, None);
    assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::CrossZoneResolved { key, kept_zone } if key == "project" && *kept_zone == Zone::Frontmatter)
            ),
            "should detect cross-zone duplicate: {fixes:?}"
        );
}

#[test]
fn severity_classification() {
    assert_eq!(
        Fix::CrossZoneResolved {
            key: "k".into(),
            kept_zone: Zone::Frontmatter
        }
        .severity(),
        Severity::Error
    );
    assert_eq!(
        Fix::DefaultSet {
            field: "type".into(),
            value: "note".into()
        }
        .severity(),
        Severity::Warning
    );
    assert_eq!(
        Fix::TitleDerived {
            source: TitleSource::Filename("t".into())
        }
        .severity(),
        Severity::Warning
    );
    assert_eq!(Fix::TagsSorted.severity(), Severity::Info);
    assert_eq!(Fix::TitleTrimmed.severity(), Severity::Info);
    assert_eq!(Fix::TitleCapitalized.severity(), Severity::Info);
    assert_eq!(
        Fix::TagsDeduped { removed: vec![] }.severity(),
        Severity::Info
    );
    assert_eq!(
        Fix::TagsStrippedHash { tags: vec![] }.severity(),
        Severity::Info
    );
    assert_eq!(
        Fix::KeyNormalized {
            old: "a".into(),
            new: "b".into()
        }
        .severity(),
        Severity::Info
    );
    assert_eq!(
        Fix::H1Aligned {
            old_h1: "a".into(),
            new_h1: "b".into()
        }
        .severity(),
        Severity::Info
    );
    assert_eq!(
        Fix::FieldRenamed {
            old: "a".into(),
            new: "b".into()
        }
        .severity(),
        Severity::Warning
    );
    assert_eq!(
        Fix::TypeNormalized {
            old: "loop".into(),
            new: "project".into()
        }
        .severity(),
        Severity::Info
    );
}

#[test]
fn to_kebab_case_camel() {
    assert_eq!(to_kebab_case("CamelCase"), "camel-case");
}

#[test]
fn to_kebab_case_snake() {
    assert_eq!(to_kebab_case("snake_case"), "snake-case");
}

#[test]
fn to_kebab_case_acronym() {
    assert_eq!(to_kebab_case("XMLParser"), "xml-parser");
}

#[test]
fn to_kebab_case_already_kebab() {
    assert_eq!(to_kebab_case("already-kebab"), "already-kebab");
}

#[test]
fn to_kebab_case_mixed() {
    assert_eq!(to_kebab_case("myField_Name"), "my-field-name");
}

#[test]
fn title_from_path_with_id_and_slug() {
    assert_eq!(
        title_from_path("ddb/20260315120000-project-plan.md"),
        "Project plan"
    );
}

#[test]
fn title_from_path_id_only() {
    assert_eq!(title_from_path("ddb/20260315120000.md"), "");
}

#[test]
fn title_from_path_underscore_slug() {
    assert_eq!(title_from_path("ddb/20260315120000_my_note.md"), "My note");
}

#[test]
fn detect_title_trim() {
    let mut parsed = empty_parsed();
    parsed.meta.title = Some("  spaced  ".to_string());
    let fixes = detect_fixes(&parsed, None);
    assert!(
        fixes.iter().any(|f| matches!(f, Fix::TitleTrimmed)),
        "should detect untrimmed title: {fixes:?}"
    );
}

#[test]
fn detect_title_capitalize() {
    let mut parsed = empty_parsed();
    parsed.meta.title = Some("lowercase start".to_string());
    let fixes = detect_fixes(&parsed, None);
    assert!(
        fixes.iter().any(|f| matches!(f, Fix::TitleCapitalized)),
        "should detect uncapitalized title: {fixes:?}"
    );
}

#[test]
fn typedef_skips_title_fixes() {
    let mut parsed = empty_parsed();
    parsed.meta.doogat_type = Some("_typedef".to_string());
    parsed.meta.title = Some("lowercase".to_string());
    let fixes = detect_fixes(&parsed, None);
    assert!(
        !fixes.iter().any(|f| matches!(
            f,
            Fix::TitleCapitalized
                | Fix::TitleTrimmed
                | Fix::TitleDerived { .. }
                | Fix::H1Aligned { .. }
        )),
        "typedef titles should not be modified: {fixes:?}"
    );
}

#[test]
fn empty_tags_no_fix() {
    let mut parsed = empty_parsed();
    parsed.meta.tags = vec![];
    let fixes = detect_fixes(&parsed, None);
    assert!(
        !fixes
            .iter()
            .any(|f| matches!(f, Fix::DefaultSet { field, .. } if field == "tags")),
        "empty tags should not trigger a fix: {fixes:?}"
    );
}

#[test]
fn detect_schema_required_field_with_default() {
    let parsed = empty_parsed();
    let schema = crate::types::TableSchema {
        table_name: "note".into(),
        columns: vec![crate::types::ColumnDef {
            name: "priority".into(),
            data_type: "INTEGER".into(),
            references: None,
            zone: None,
            required: true,
            search_boost: None,
            allowed_values: None,
            default_value: Some("0".into()),
        }],
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
    };
    let fixes = detect_fixes(&parsed, Some(&schema));
    assert!(
        fixes.iter().any(
            |f| matches!(f, Fix::DefaultSet { field, value } if field == "priority" && value == "0")
        ),
        "should detect missing required field from schema: {fixes:?}"
    );
}

#[test]
fn detect_schema_required_field_present_no_fix() {
    let mut parsed = empty_parsed();
    parsed
        .meta
        .extra
        .insert("priority".into(), crate::types::Value::Number(5.0));
    let schema = crate::types::TableSchema {
        table_name: "note".into(),
        columns: vec![crate::types::ColumnDef {
            name: "priority".into(),
            data_type: "INTEGER".into(),
            references: None,
            zone: None,
            required: true,
            search_boost: None,
            allowed_values: None,
            default_value: Some("0".into()),
        }],
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
    };
    let fixes = detect_fixes(&parsed, Some(&schema));
    assert!(
        !fixes
            .iter()
            .any(|f| matches!(f, Fix::DefaultSet { field, .. } if field == "priority")),
        "present field should not trigger schema fix: {fixes:?}"
    );
}

#[test]
fn detect_hash_apple_plus_apple_dedup() {
    let mut parsed = empty_parsed();
    parsed.meta.tags = vec!["#apple".into(), "apple".into()];
    let fixes = detect_fixes(&parsed, None);
    assert!(
        fixes
            .iter()
            .any(|f| matches!(f, Fix::TagsDeduped { removed } if !removed.is_empty())),
        "should detect post-strip duplicate: {fixes:?}"
    );
}

#[test]
fn apply_hash_apple_plus_apple_roundtrip() {
    let mut parsed = empty_parsed();
    parsed.meta.tags = vec!["#apple".into(), "apple".into()];
    let fixes = detect_fixes(&parsed, None);
    let content = apply_fixes(&mut parsed, &fixes).unwrap();
    let reparsed = crate::parser::parse(&content, &parsed.path).unwrap();
    assert_eq!(reparsed.meta.tags, vec!["apple".to_string()]);
    // Idempotent
    let second_fixes = detect_fixes(&reparsed, None);
    assert!(
        !second_fixes
            .iter()
            .any(|f| matches!(f, Fix::TagsDeduped { .. } | Fix::TagsStrippedHash { .. })),
        "should be idempotent: {second_fixes:?}"
    );
}

// ── H1 alignment tests ──────────────────────────────────────────

#[test]
fn detect_h1_mismatch() {
    let mut parsed = empty_parsed();
    parsed.meta.title = Some("My Note".to_string());
    parsed.sections = vec![Section {
        heading: "Old Title".to_string(),
        level: 1,
        content: String::new(),
    }];
    let fixes = detect_fixes(&parsed, None);
    assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::H1Aligned { old_h1, new_h1 } if old_h1 == "Old Title" && new_h1 == "My Note")
            ),
            "should detect H1 mismatch: {fixes:?}"
        );
}

#[test]
fn detect_h1_match_no_fix() {
    let mut parsed = empty_parsed();
    parsed.meta.title = Some("Same Title".to_string());
    parsed.sections = vec![Section {
        heading: "Same Title".to_string(),
        level: 1,
        content: String::new(),
    }];
    let fixes = detect_fixes(&parsed, None);
    assert!(
        !fixes.iter().any(|f| matches!(f, Fix::H1Aligned { .. })),
        "matching H1 should not produce fix: {fixes:?}"
    );
}

#[test]
fn detect_no_h1_no_fix() {
    let mut parsed = empty_parsed();
    parsed.meta.title = Some("My Note".to_string());
    parsed.sections = vec![Section {
        heading: "Sub heading".to_string(),
        level: 2,
        content: String::new(),
    }];
    let fixes = detect_fixes(&parsed, None);
    assert!(
        !fixes.iter().any(|f| matches!(f, Fix::H1Aligned { .. })),
        "no H1 should not produce alignment fix: {fixes:?}"
    );
}

#[test]
fn apply_h1_alignment() {
    let mut parsed = empty_parsed();
    parsed.meta.title = Some("My Note".to_string());
    parsed.body = "# Old Title\n\nSome content.\n".to_string();
    parsed.sections = vec![Section {
        heading: "Old Title".to_string(),
        level: 1,
        content: "\nSome content.\n".to_string(),
    }];
    let fixes = vec![Fix::H1Aligned {
        old_h1: "Old Title".into(),
        new_h1: "My Note".into(),
    }];
    let result = apply_fixes(&mut parsed, &fixes).unwrap();
    assert!(result.contains("# My Note\n"));
    assert!(!result.contains("# Old Title"));
}

// ── apply_fixes tests ──────────────────────────────────────────

#[test]
fn apply_tag_dedup() {
    let mut parsed = empty_parsed();
    parsed.meta.tags = vec!["a".into(), "b".into(), "a".into()];
    let fixes = vec![Fix::TagsDeduped {
        removed: vec!["a".into()],
    }];
    let result = apply_fixes(&mut parsed, &fixes).unwrap();
    assert!(result.contains("  - a\n  - b\n"));
    assert_eq!(result.matches("  - a").count(), 1);
}

#[test]
fn apply_tag_sort() {
    let mut parsed = empty_parsed();
    parsed.meta.tags = vec!["zebra".into(), "apple".into()];
    let fixes = vec![Fix::TagsSorted];
    let result = apply_fixes(&mut parsed, &fixes).unwrap();
    let tag_section = result
        .lines()
        .skip_while(|l| !l.starts_with("tags:"))
        .skip(1)
        .take_while(|l| l.starts_with("  - "))
        .collect::<Vec<_>>();
    assert_eq!(tag_section, vec!["  - apple", "  - zebra"]);
}

#[test]
fn apply_tag_strip_hash() {
    let mut parsed = empty_parsed();
    parsed.meta.tags = vec!["#gtd".into(), "work".into()];
    let fixes = vec![Fix::TagsStrippedHash {
        tags: vec!["#gtd".into()],
    }];
    let result = apply_fixes(&mut parsed, &fixes).unwrap();
    assert!(result.contains("  - gtd\n"));
    assert!(!result.contains("#gtd"));
}

#[test]
fn apply_default_type() {
    let mut parsed = empty_parsed();
    parsed.meta.doogat_type = None;
    let fixes = vec![Fix::DefaultSet {
        field: "type".into(),
        value: "note".into(),
    }];
    let result = apply_fixes(&mut parsed, &fixes).unwrap();
    assert!(result.contains("type: note"));
}

#[test]
fn apply_title_derived() {
    let mut parsed = empty_parsed();
    parsed.meta.title = None;
    let fixes = vec![Fix::TitleDerived {
        source: TitleSource::FirstH1("Derived Title".into()),
    }];
    let result = apply_fixes(&mut parsed, &fixes).unwrap();
    assert!(result.contains("title: Derived Title"));
}

#[test]
fn apply_key_normalize() {
    let mut parsed = empty_parsed();
    parsed
        .meta
        .extra
        .insert("CamelKey".into(), crate::types::Value::String("val".into()));
    let fixes = vec![Fix::KeyNormalized {
        old: "CamelKey".into(),
        new: "camel-key".into(),
    }];
    let result = apply_fixes(&mut parsed, &fixes).unwrap();
    assert!(result.contains("camel-key: val"));
    assert!(!result.contains("CamelKey"));
}

#[test]
fn apply_cross_zone_resolved() {
    let mut parsed = empty_parsed();
    parsed.body = "Some text.\nproject:: bar\nMore text.\n".to_string();
    parsed
        .meta
        .extra
        .insert("project".into(), crate::types::Value::String("foo".into()));
    let fixes = vec![Fix::CrossZoneResolved {
        key: "project".into(),
        kept_zone: Zone::Frontmatter,
    }];
    let result = apply_fixes(&mut parsed, &fixes).unwrap();
    assert!(!result.contains("project:: bar"));
    assert!(result.contains("project: foo"));
}

#[test]
fn round_trip_fidelity() {
    let mut parsed = empty_parsed();
    parsed.meta.tags = vec![
        "#gtd".into(),
        "zebra".into(),
        "apple".into(),
        "apple".into(),
    ];
    parsed.meta.doogat_type = None;
    parsed.meta.title = Some("  lowercase  ".into());
    parsed
        .meta
        .extra
        .insert("CamelKey".into(), crate::types::Value::String("val".into()));

    let fixes = detect_fixes(&parsed, None);
    assert!(!fixes.is_empty());

    let content = apply_fixes(&mut parsed, &fixes).unwrap();

    // Re-parse and detect again — should find no fixes
    let reparsed = crate::parser::parse(&content, &parsed.path).unwrap();
    let second_fixes = detect_fixes(&reparsed, None);
    assert!(
        second_fixes.is_empty(),
        "round-trip should produce clean doogat, but found: {second_fixes:?}"
    );
}

// ── migration tests ──────────────────────────────────────────

#[test]
fn migrate_zkn_id_to_id() {
    let mut parsed = empty_parsed();
    parsed.meta.id = None;
    parsed.meta.extra.insert(
        "zkn-id".into(),
        crate::types::Value::String("20260101120000".into()),
    );

    let migrations = super::built_in_migrations();
    let m = &migrations[0]; // v1: zkn-id-to-id
    let fixes = (m.apply)(&mut parsed);

    assert_eq!(fixes.len(), 1);
    assert!(matches!(&fixes[0], Fix::FieldRenamed { old, new } if old == "zkn-id" && new == "id"));
    assert_eq!(
        parsed.meta.id,
        Some(crate::types::DoogatId("20260101120000".into()))
    );
}

#[test]
fn migrate_tag_singular_to_tags() {
    let mut parsed = empty_parsed();
    parsed.meta.tags = vec![];
    parsed
        .meta
        .extra
        .insert("tag".into(), crate::types::Value::String("gtd".into()));

    let migrations = super::built_in_migrations();
    let m = &migrations[1]; // v2: tag-to-tags
    let fixes = (m.apply)(&mut parsed);

    assert_eq!(fixes.len(), 1);
    assert!(matches!(&fixes[0], Fix::FieldRenamed { old, new } if old == "tag" && new == "tags"));
    assert_eq!(parsed.meta.tags, vec!["gtd".to_string()]);
}

#[test]
fn migrate_type_loop_to_project() {
    let mut parsed = empty_parsed();
    parsed.meta.doogat_type = Some("loop".into());

    let migrations = super::built_in_migrations();
    let m = &migrations[2]; // v3: type-normalize
    let fixes = (m.apply)(&mut parsed);

    assert_eq!(fixes.len(), 1);
    assert!(
        matches!(&fixes[0], Fix::TypeNormalized { old, new } if old == "loop" && new == "project")
    );
    assert_eq!(parsed.meta.doogat_type, Some("project".into()));
}

#[test]
fn migrate_type_doogat_to_note() {
    let mut parsed = empty_parsed();
    parsed.meta.doogat_type = Some("doogat".into());

    let migrations = super::built_in_migrations();
    let m = &migrations[2];
    let fixes = (m.apply)(&mut parsed);

    assert_eq!(fixes.len(), 1);
    assert!(
        matches!(&fixes[0], Fix::TypeNormalized { old, new } if old == "doogat" && new == "note")
    );
}

#[test]
fn migrate_type_normal_no_change() {
    let mut parsed = empty_parsed();
    parsed.meta.doogat_type = Some("project".into());

    let migrations = super::built_in_migrations();
    let m = &migrations[2];
    let fixes = (m.apply)(&mut parsed);
    assert!(fixes.is_empty());
}

#[test]
fn migration_version_tracking() {
    // read_migration_version returns 0 when file doesn't exist
    // (can't test file I/O without a repo, but we test the function logic)
    assert_eq!(super::read_migration_version(&MockSource), 0);
}

/// Minimal mock for testing read_migration_version when file doesn't exist.
struct MockSource;

impl crate::traits::DoogatSource for MockSource {
    fn list_doogats(&self) -> crate::error::Result<Vec<String>> {
        Ok(vec![])
    }
    fn read_file(&self, _path: &str) -> crate::error::Result<String> {
        Err(crate::error::DoogatError::Git("not found".into()))
    }
    fn head_oid(&self) -> crate::error::Result<crate::types::CommitHash> {
        Ok(crate::types::CommitHash("abc".into()))
    }
    fn diff_paths(
        &self,
        _old: &str,
        _new: &str,
    ) -> crate::error::Result<Vec<(crate::types::DiffKind, String)>> {
        Ok(vec![])
    }
    fn read_files_batch(
        &self,
        _paths: &[String],
    ) -> crate::error::Result<Vec<(String, crate::error::Result<String>)>> {
        Ok(vec![])
    }
}

fn make_schema_with_template(template: &str, columns: Vec<ColumnDef>) -> TableSchema {
    TableSchema {
        table_name: "widget".into(),
        columns,
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: Some(template.into()),
        origin: Some("ddl".into()),
        unique_together: None,
    }
}

#[test]
fn title_noncompliant_detected() {
    let mut parsed = empty_parsed();
    parsed.meta.title = Some("Foo".into());
    parsed.meta.doogat_type = Some("widget".into());
    parsed
        .meta
        .extra
        .insert("name".into(), crate::types::Value::String("Bar".into()));
    let schema = make_schema_with_template(
        "{name} Widget",
        vec![ColumnDef {
            name: "name".into(),
            data_type: "TEXT".into(),
            references: None,
            zone: Some(Zone::Frontmatter),
            required: false,
            search_boost: None,
            allowed_values: None,
            default_value: None,
        }],
    );
    let fixes = detect_fixes(&parsed, Some(&schema));
    assert!(
        fixes
            .iter()
            .any(|f| matches!(f, Fix::TitleNonCompliant { expected } if expected == "Bar Widget")),
        "should detect title mismatch: {fixes:?}"
    );
}

#[test]
fn title_compliant_not_flagged() {
    let mut parsed = empty_parsed();
    parsed.meta.title = Some("Bar Widget".into());
    parsed.meta.doogat_type = Some("widget".into());
    parsed
        .meta
        .extra
        .insert("name".into(), crate::types::Value::String("Bar".into()));
    let schema = make_schema_with_template(
        "{name} Widget",
        vec![ColumnDef {
            name: "name".into(),
            data_type: "TEXT".into(),
            references: None,
            zone: Some(Zone::Frontmatter),
            required: false,
            search_boost: None,
            allowed_values: None,
            default_value: None,
        }],
    );
    let fixes = detect_fixes(&parsed, Some(&schema));
    assert!(
        !fixes
            .iter()
            .any(|f| matches!(f, Fix::TitleNonCompliant { .. })),
        "compliant title should not be flagged: {fixes:?}"
    );
}

#[test]
fn title_template_unfilled_placeholders_stripped() {
    let mut parsed = empty_parsed();
    parsed.meta.title = Some("Wrong".into());
    parsed.meta.doogat_type = Some("widget".into());
    parsed
        .meta
        .extra
        .insert("name".into(), crate::types::Value::String("Bar".into()));
    let schema = make_schema_with_template(
        "{name} {missing}",
        vec![
            ColumnDef {
                name: "name".into(),
                data_type: "TEXT".into(),
                references: None,
                zone: Some(Zone::Frontmatter),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
            },
            ColumnDef {
                name: "missing".into(),
                data_type: "TEXT".into(),
                references: None,
                zone: Some(Zone::Frontmatter),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
            },
        ],
    );
    let fixes = detect_fixes(&parsed, Some(&schema));
    assert!(
        fixes
            .iter()
            .any(|f| matches!(f, Fix::TitleNonCompliant { expected } if expected == "Bar")),
        "unfilled placeholders should be stripped: {fixes:?}"
    );
}

#[test]
fn title_compliance_no_template_skipped() {
    let mut parsed = empty_parsed();
    parsed.meta.doogat_type = Some("widget".into());
    let schema = TableSchema {
        table_name: "widget".into(),
        columns: vec![],
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
    };
    let fixes = detect_fixes(&parsed, Some(&schema));
    assert!(
        !fixes
            .iter()
            .any(|f| matches!(f, Fix::TitleNonCompliant { .. })),
        "no template means no compliance check: {fixes:?}"
    );
}

#[test]
fn title_noncompliant_applied() {
    let mut parsed = empty_parsed();
    parsed.meta.title = Some("Old Title".into());
    parsed.body = "# Old Title\n\nSome content".into();
    let fixes = vec![Fix::TitleNonCompliant {
        expected: "New Title".into(),
    }];
    let result = apply_fixes(&mut parsed, &fixes).unwrap();
    assert_eq!(parsed.meta.title.as_deref(), Some("New Title"));
    assert!(
        result.contains("# New Title"),
        "H1 should be updated in body"
    );
}

// ── Zone migration tests ────────────────────────────────────────

#[test]
fn migrate_body_to_frontmatter() {
    let mut parsed = empty_parsed();
    parsed.body = "## description\n\nSome content here\n".into();
    let col = ColumnDef {
        name: "description".into(),
        data_type: "TEXT".into(),
        references: None,
        zone: Some(Zone::Frontmatter),
        required: false,
        search_boost: None,
        allowed_values: None,
        default_value: None,
    };

    let current = detect_current_zone(&parsed, "description");
    assert_eq!(current, Some(Zone::Body));

    let value = extract_from_zone(&parsed, "description", &Zone::Body).unwrap();
    assert_eq!(value, "Some content here");

    remove_from_zone(&mut parsed, "description", &Zone::Body);
    insert_into_zone(&mut parsed, "description", &value, &col.effective_zone());

    assert_eq!(
        parsed.meta.extra.get("description"),
        Some(&crate::types::Value::String("Some content here".into()))
    );
    assert!(!parsed.body.contains("## description"));
}

#[test]
fn migrate_frontmatter_to_body() {
    let mut parsed = empty_parsed();
    parsed.meta.extra.insert(
        "notes".into(),
        crate::types::Value::String("Important stuff".into()),
    );

    let current = detect_current_zone(&parsed, "notes");
    assert_eq!(current, Some(Zone::Frontmatter));

    let value = extract_from_zone(&parsed, "notes", &Zone::Frontmatter).unwrap();
    remove_from_zone(&mut parsed, "notes", &Zone::Frontmatter);
    insert_into_zone(&mut parsed, "notes", &value, &Zone::Body);

    assert!(!parsed.meta.extra.contains_key("notes"));
    assert!(parsed.body.contains("## notes"));
    assert!(parsed.body.contains("Important stuff"));
}

#[test]
fn migrate_to_reference() {
    let mut parsed = empty_parsed();
    parsed.meta.extra.insert(
        "category".into(),
        crate::types::Value::String("20260301120000".into()),
    );

    let value = extract_from_zone(&parsed, "category", &Zone::Frontmatter).unwrap();
    remove_from_zone(&mut parsed, "category", &Zone::Frontmatter);
    insert_into_zone(&mut parsed, "category", &value, &Zone::Reference);

    assert!(!parsed.meta.extra.contains_key("category"));
    assert!(parsed
        .reference_section
        .contains("- category:: [[20260301120000]]"));
    assert!(parsed
        .inline_fields
        .iter()
        .any(|f| f.key == "category" && f.value == "20260301120000"));
}

#[test]
fn migrate_from_reference() {
    let mut parsed = empty_parsed();
    parsed.reference_section = "- source:: [[20260301120000]]\n".into();
    parsed.inline_fields.push(InlineField {
        key: "source".into(),
        value: "20260301120000".into(),
        zone: Zone::Reference,
    });

    let current = detect_current_zone(&parsed, "source");
    assert_eq!(current, Some(Zone::Reference));

    let value = extract_from_zone(&parsed, "source", &Zone::Reference).unwrap();
    remove_from_zone(&mut parsed, "source", &Zone::Reference);
    insert_into_zone(&mut parsed, "source", &value, &Zone::Frontmatter);

    assert!(
        parsed.reference_section.trim().is_empty() || !parsed.reference_section.contains("source")
    );
    assert_eq!(
        parsed.meta.extra.get("source"),
        Some(&crate::types::Value::String("20260301120000".into()))
    );
}

#[test]
fn migrate_preserves_subheadings() {
    let mut parsed = empty_parsed();
    parsed.body =
        "## notes\n\nTop content\n\n### Sub-heading\n\nSub content\n\n## other\n\nOther stuff\n"
            .into();

    let value = extract_from_zone(&parsed, "notes", &Zone::Body).unwrap();
    assert!(
        value.contains("### Sub-heading"),
        "sub-headings preserved in extraction"
    );
    assert!(value.contains("Sub content"));

    remove_from_zone(&mut parsed, "notes", &Zone::Body);
    assert!(!parsed.body.contains("## notes"));
    assert!(parsed.body.contains("## other"), "other section preserved");
}

#[test]
fn migrate_multiline_body_to_frontmatter() {
    let mut parsed = empty_parsed();
    parsed.body = "## bio\n\nLine one\nLine two\nLine three\n".into();

    let value = extract_from_zone(&parsed, "bio", &Zone::Body).unwrap();
    assert!(value.contains("Line one\nLine two\nLine three"));

    remove_from_zone(&mut parsed, "bio", &Zone::Body);
    insert_into_zone(&mut parsed, "bio", &value, &Zone::Frontmatter);

    let stored = parsed.meta.extra.get("bio").unwrap();
    match stored {
        crate::types::Value::String(s) => {
            assert!(s.contains("Line one"));
            assert!(s.contains("Line three"));
        }
        _ => panic!("expected String value"),
    }
}

#[test]
fn migrate_idempotent() {
    let mut parsed = empty_parsed();
    parsed.meta.extra.insert(
        "status".into(),
        crate::types::Value::String("active".into()),
    );
    let col = ColumnDef {
        name: "status".into(),
        data_type: "TEXT".into(),
        references: None,
        zone: Some(Zone::Frontmatter),
        required: false,
        search_boost: None,
        allowed_values: None,
        default_value: None,
    };

    // Data already in correct zone
    let current = detect_current_zone(&parsed, "status").unwrap();
    assert_eq!(current, col.effective_zone(), "already in correct zone");
}

#[test]
fn migrate_no_data_no_changes() {
    let parsed = empty_parsed();
    // Column exists in schema but doogat has no data for it
    let current = detect_current_zone(&parsed, "nonexistent");
    assert_eq!(current, None);
}

#[test]
fn migrate_multi_value_reference_preserves_all() {
    let mut parsed = empty_parsed();
    parsed.reference_section = "- tag:: [[20260301120000]]\n- tag:: [[20260301120001]]\n".into();
    parsed.inline_fields.push(InlineField {
        key: "tag".into(),
        value: "20260301120000".into(),
        zone: Zone::Reference,
    });
    parsed.inline_fields.push(InlineField {
        key: "tag".into(),
        value: "20260301120001".into(),
        zone: Zone::Reference,
    });

    let value = extract_from_zone(&parsed, "tag", &Zone::Reference).unwrap();
    assert_eq!(value, "20260301120000,20260301120001");

    remove_from_zone(&mut parsed, "tag", &Zone::Reference);
    assert!(parsed.inline_fields.is_empty());

    // Re-insert into Reference zone — should produce two lines
    insert_into_zone(&mut parsed, "tag", &value, &Zone::Reference);
    assert_eq!(parsed.inline_fields.len(), 2);
    assert!(parsed
        .reference_section
        .contains("- tag:: [[20260301120000]]"));
    assert!(parsed
        .reference_section
        .contains("- tag:: [[20260301120001]]"));
}

#[test]
fn migrate_body_inline_field_to_frontmatter() {
    let mut parsed = empty_parsed();
    parsed.body = "Some text\nstatus:: active\nMore text".into();
    parsed.inline_fields.push(InlineField {
        key: "status".into(),
        value: "active".into(),
        zone: Zone::Body,
    });

    let current = detect_current_zone(&parsed, "status");
    assert_eq!(current, Some(Zone::Body));

    let value = extract_from_zone(&parsed, "status", &Zone::Body).unwrap();
    assert_eq!(value, "active");

    remove_from_zone(&mut parsed, "status", &Zone::Body);
    insert_into_zone(&mut parsed, "status", &value, &Zone::Frontmatter);

    assert_eq!(
        parsed.meta.extra.get("status"),
        Some(&crate::types::Value::String("active".into()))
    );
    assert!(!parsed.body.contains("status::"));
}

#[test]
fn remove_body_section_collapses_blank_lines() {
    let mut body = "## A\ncontent a\n\n## B\ncontent b\n\n## C\ncontent c\n".to_string();
    remove_body_section(&mut body, "B");
    assert!(
        !body.contains("\n\n\n"),
        "body should not contain triple+ newlines: {body:?}"
    );
    assert!(body.contains("## A\ncontent a\n"));
    assert!(body.contains("## C\ncontent c\n"));
}

#[test]
fn extract_from_zone_map_value() {
    let mut p = empty_parsed();
    let mut inner = BTreeMap::new();
    inner.insert(
        "k1".to_string(),
        crate::types::Value::String("v1".to_string()),
    );
    inner.insert("k2".to_string(), crate::types::Value::Number(42.0));
    p.meta
        .extra
        .insert("nested".to_string(), crate::types::Value::Map(inner));
    let result = extract_from_zone(&p, "nested", &Zone::Frontmatter);
    assert!(result.is_some(), "Map value should be extracted");
    let yaml = result.unwrap();
    assert!(yaml.contains("k1:"), "YAML should contain key k1: {yaml:?}");
    assert!(yaml.contains("k2:"), "YAML should contain key k2: {yaml:?}");
}

#[test]
fn extract_from_zone_list_value() {
    let mut p = empty_parsed();
    let items = vec![
        crate::types::Value::String("a".to_string()),
        crate::types::Value::String("b".to_string()),
        crate::types::Value::Number(3.0),
    ];
    p.meta
        .extra
        .insert("items".to_string(), crate::types::Value::List(items));
    let result = extract_from_zone(&p, "items", &Zone::Frontmatter);
    assert!(result.is_some(), "List value should be extracted");
    let yaml = result.unwrap();
    assert!(yaml.contains("a"), "YAML should contain 'a': {yaml:?}");
    assert!(yaml.contains("b"), "YAML should contain 'b': {yaml:?}");
    assert!(yaml.contains("3.0"), "YAML should contain '3.0': {yaml:?}");
}
