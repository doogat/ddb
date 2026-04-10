//! Property-based tests for parser, CRDT resolver, indexer, and SQL engine.
//!
//! Uses proptest to systematically explore the input space.
//! Default case counts are smoke budgets tuned for a fast local `cargo test`.
//! Regressions are saved in `property_tests.proptest-regressions` and
//! always replayed regardless of case count.
//!
//! For thorough runs (pre-release, post-refactor, new generator changes):
//!
//! ```sh
//! PROPTEST_CASES=5000 cargo test -p ddb-core --test property_tests -- --nocapture
//! ```
//!
//! This bumps all blocks uniformly. Expect a long soak run at 5000 cases.
//! Local defaults intentionally stay small; CI overrides them with
//! `PROPTEST_CASES=50` for broader coverage.
//! The parser blocks are CPU-only and scale linearly; SQL/CRDT/indexer
//! blocks do real SQLite/Automerge I/O per case and dominate runtime.

use proptest::prelude::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use ddb_core::crdt_resolver;
use ddb_core::indexer::Index;
use ddb_core::parser;
use ddb_core::sql_engine::SqlEngine;
use ddb_core::traits::{DoogatSource, DoogatStore};
use ddb_core::types::{CommitHash, ConflictFile, DiffKind, Value, DoogatId, DoogatMeta};

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

/// Safe ASCII alphanumeric string (no YAML-special chars).
fn safe_word() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9]{0,15}".prop_map(|s| s)
}

/// Generate a well-formed search query that should parse successfully.
/// Mixes bare words, explicit AND/OR/NOT, and field filters.
fn arb_search_query_valid() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z]{2,8}".prop_map(|w| w),
        ("[a-z]{2,6}", "[a-z]{2,6}").prop_map(|(f, v)| format!("{f}={v}")),
        ("[a-z]{2,6}", "[a-z]{2,6}").prop_map(|(a, b)| format!("{a} AND {b}")),
        ("[a-z]{2,6}", "[a-z]{2,6}").prop_map(|(a, b)| format!("{a} OR {b}")),
        ("tag", "[a-z]{2,6}").prop_map(|(f, v)| format!("NOT {f}={v}")),
    ]
}

/// Safe sentence for body text (no `---` on its own line, no `::` to avoid inline fields).
fn safe_sentence() -> impl Strategy<Value = String> {
    prop::collection::vec("[a-zA-Z0-9 ,\\.!?]{1,60}", 1..=3).prop_map(|parts| parts.join(" "))
}

/// Generate a 14-digit timestamp ID.
fn arb_doogat_id() -> impl Strategy<Value = String> {
    // YYYYMMDDHHmmss — constrain to valid-ish ranges
    (
        2020u32..2030,
        1u32..=12,
        1u32..=28,
        0u32..=23,
        0u32..=59,
        0u32..=59,
    )
        .prop_map(|(y, mo, d, h, mi, s)| format!("{y:04}{mo:02}{d:02}{h:02}{mi:02}{s:02}"))
}

/// Generate a Value for extra frontmatter fields (leaf types only for safety).
fn arb_value_leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        safe_word().prop_map(Value::String),
        (0.0f64..1000.0).prop_map(|n| Value::Number((n * 100.0).round() / 100.0)),
        any::<bool>().prop_map(Value::Bool),
    ]
}

/// Generate a tag (simple alphanumeric).
fn arb_tag() -> impl Strategy<Value = String> {
    "[a-z]{2,10}"
}

/// Generate an extra-field key that won't collide with reserved frontmatter keys.
fn arb_extra_key() -> impl Strategy<Value = String> {
    // Prefix with "x" to avoid any collision with reserved keys
    safe_word().prop_map(|w| format!("x{w}"))
}

/// Generate a safe doogat type name (prefixed to avoid SQL reserved words).
fn arb_doogat_type() -> impl Strategy<Value = String> {
    safe_word().prop_map(|w| format!("zt{w}"))
}

/// Generate DoogatMeta.
fn arb_doogat_meta() -> impl Strategy<Value = DoogatMeta> {
    (
        arb_doogat_id(),
        prop::option::of(safe_word()),
        prop::option::of("[0-9]{4}-[0-9]{2}-[0-9]{2}"),
        prop::option::of(arb_doogat_type()),
        prop::collection::vec(arb_tag(), 0..=4),
        prop::collection::btree_map(arb_extra_key(), arb_value_leaf(), 0..=3),
    )
        .prop_map(|(id, title, date, ztype, tags, extra)| DoogatMeta {
            id: Some(DoogatId(id)),
            title,
            date,
            doogat_type: ztype,
            tags,
            extra,
        })
}

/// Generate a body paragraph (safe text, no zone-boundary triggers).
fn arb_paragraph() -> impl Strategy<Value = String> {
    safe_sentence()
}

/// Generate a multi-paragraph body.
fn arb_body() -> impl Strategy<Value = String> {
    prop::collection::vec(arb_paragraph(), 1..=5).prop_map(|paras| paras.join("\n\n"))
}

/// Generate a reference section line: `- key:: value`.
fn arb_ref_line() -> impl Strategy<Value = String> {
    (safe_word(), safe_word()).prop_map(|(k, v)| format!("- {k}:: {v}"))
}

/// Generate a reference section (0 or more ref lines).
fn arb_reference_section() -> impl Strategy<Value = String> {
    prop::collection::vec(arb_ref_line(), 0..=4).prop_map(|lines| lines.join("\n"))
}

/// Generate a complete doogat Markdown string from parts.
fn arb_doogat_markdown() -> impl Strategy<Value = String> {
    (arb_doogat_meta(), arb_body(), arb_reference_section())
        .prop_map(|(meta, body, ref_section)| build_doogat_markdown(&meta, &body, &ref_section))
}

// ---------------------------------------------------------------------------
// Parser properties
// ---------------------------------------------------------------------------

/// Helper: build markdown from meta + body + ref_section (shared by generators and tests).
fn build_doogat_markdown(meta: &DoogatMeta, body: &str, ref_section: &str) -> String {
    let mut out = String::from("---\n");

    if let Some(ref id) = meta.id {
        out.push_str(&format!("id: {}\n", id.0));
    }
    if let Some(ref title) = meta.title {
        out.push_str(&format!("title: {title}\n"));
    }
    if let Some(ref date) = meta.date {
        out.push_str(&format!("date: {date}\n"));
    }
    if !meta.tags.is_empty() {
        out.push_str("tags:\n");
        for tag in &meta.tags {
            out.push_str(&format!("  - {tag}\n"));
        }
    }
    if let Some(ref t) = meta.doogat_type {
        out.push_str(&format!("type: {t}\n"));
    }
    for (key, value) in &meta.extra {
        match value {
            Value::String(s) => out.push_str(&format!("{key}: {s}\n")),
            Value::Number(n) => out.push_str(&format!("{key}: {n}\n")),
            Value::Bool(b) => out.push_str(&format!("{key}: {b}\n")),
            _ => {}
        }
    }

    out.push_str("---\n");
    out.push_str(body);

    if !ref_section.is_empty() {
        out.push_str("\n---\n");
        out.push_str(ref_section);
    }

    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// Smoke: generated doogats are parseable.
    #[test]
    fn generated_doogats_parse(md in arb_doogat_markdown()) {
        let result = parser::parse(&md, "test.md");
        prop_assert!(result.is_ok(), "parse failed: {:?}\ninput:\n{}", result.err(), md);
    }

    /// Parse-serialize-parse idempotency: parse(serialize(parse(x))) == parse(x).
    #[test]
    fn parser_idempotency(md in arb_doogat_markdown()) {
        let parsed = parser::parse(&md, "test.md").unwrap();
        let serialized = parser::serialize(&parsed);
        let reparsed = parser::parse(&serialized, "test.md").unwrap();

        // Compare meta fields
        prop_assert_eq!(&parsed.meta.id, &reparsed.meta.id);
        prop_assert_eq!(&parsed.meta.title, &reparsed.meta.title);
        prop_assert_eq!(&parsed.meta.date, &reparsed.meta.date);
        prop_assert_eq!(&parsed.meta.doogat_type, &reparsed.meta.doogat_type);
        prop_assert_eq!(&parsed.meta.tags, &reparsed.meta.tags);
        prop_assert_eq!(&parsed.meta.extra, &reparsed.meta.extra);
        // Compare body and reference section
        prop_assert_eq!(&parsed.body, &reparsed.body);
        prop_assert_eq!(&parsed.reference_section, &reparsed.reference_section);
        // Second round-trip should be stable too
        let serialized2 = parser::serialize(&reparsed);
        prop_assert_eq!(&serialized, &serialized2);
    }

    /// Zone isolation: modifying body doesn't change frontmatter or references.
    #[test]
    fn parser_zone_isolation(
        md in arb_doogat_markdown(),
        extra_text in "[a-zA-Z ]{5,30}",
    ) {
        let parsed = parser::parse(&md, "test.md").unwrap();
        let mut mutated = parsed.clone();
        mutated.body.push_str(&format!("\n\n{extra_text}"));

        let serialized = parser::serialize(&mutated);
        let reparsed = parser::parse(&serialized, "test.md").unwrap();

        // Frontmatter unchanged
        prop_assert_eq!(&parsed.meta.id, &reparsed.meta.id);
        prop_assert_eq!(&parsed.meta.title, &reparsed.meta.title);
        prop_assert_eq!(&parsed.meta.date, &reparsed.meta.date);
        prop_assert_eq!(&parsed.meta.doogat_type, &reparsed.meta.doogat_type);
        prop_assert_eq!(&parsed.meta.tags, &reparsed.meta.tags);
        prop_assert_eq!(&parsed.meta.extra, &reparsed.meta.extra);
        // Reference section unchanged
        prop_assert_eq!(&parsed.reference_section, &reparsed.reference_section);
        // Body contains the appended text
        prop_assert!(reparsed.body.contains(&extra_text));
    }

    /// Reference section detection: `---` thematic breaks in body don't become ref boundaries.
    #[test]
    fn parser_no_false_ref_boundary(
        meta in arb_doogat_meta(),
        body_before in arb_paragraph(),
        body_after in arb_paragraph(),
    ) {
        // Build a doogat with `---` in the body (thematic break) but NO reference section
        let body = format!("{body_before}\n\nSome text here.\n\n---\n\n{body_after}");
        let md = build_doogat_markdown(&meta, &body, "");
        let parsed = parser::parse(&md, "test.md").unwrap();

        // The `---` in the body should NOT create a reference section because
        // the content after it doesn't match `- key:: value` pattern
        prop_assert!(
            parsed.reference_section.is_empty(),
            "false positive ref section detected: {:?}\ninput:\n{}",
            parsed.reference_section,
            md,
        );
        // Body should contain both parts
        prop_assert!(parsed.body.contains(&body_before));
        prop_assert!(parsed.body.contains(&body_after));
    }
}

// ---------------------------------------------------------------------------
// Extended parser generators
// ---------------------------------------------------------------------------

/// Strings containing YAML-special characters.
fn arb_yaml_special_string() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            Just(":".to_string()),
            Just("-".to_string()),
            Just("[".to_string()),
            Just("]".to_string()),
            Just("{".to_string()),
            Just("}".to_string()),
            Just("#".to_string()),
            Just("&".to_string()),
            Just("*".to_string()),
            Just("!".to_string()),
            Just("|".to_string()),
            Just(">".to_string()),
            Just("%".to_string()),
            Just("@".to_string()),
            Just("`".to_string()),
            "[a-zA-Z0-9 ]{1,10}".prop_map(|s| s),
        ],
        1..=20,
    )
    .prop_map(|parts| parts.join(""))
}

/// Unicode chars safe for YAML values (no leading `:`, `---`, or `#` at start).
fn arb_unicode_safe_string() -> impl Strategy<Value = String> {
    let safe_chars = vec![
        'á', 'é', 'í', 'ó', 'ú', 'ü', 'ñ', 'ç', 'ø', 'å', 'ß', 'œ', 'æ', 'ğ', 'ş', 'č', 'ř', 'ž',
        'ł', 'ń', 'Γ', 'δ', 'λ', 'Ω', 'π', 'Д', 'Ж', 'й', 'я', 'ё', 'א', 'ב', 'ג', 'ד', 'ה', 'ו',
        'ז', 'ا', 'ب', 'ت', 'ث', 'ح', 'خ', 'د', 'क', 'ख', 'ग', 'न', 'म', 'य', 'あ', 'い', 'う',
        'え', 'お', 'カ', 'キ', 'ク', 'ケ', 'コ', '你', '好', '世', '界', '漢', '字', '한', '글',
        '서', '울',
    ];

    prop::collection::vec(
        prop_oneof![8 => prop::sample::select(safe_chars), 1 => Just(' ')],
        1..=50,
    )
    .prop_map(|chars| {
        let s: String = chars.into_iter().collect();
        // Collapse whitespace so tags/frontmatter values stay YAML-friendly.
        let normalized = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.is_empty() {
            "unicode".to_string()
        } else {
            normalized
        }
    })
}

/// Single string of 10K+ chars (no newlines).
fn arb_long_line() -> impl Strategy<Value = String> {
    prop::collection::vec("[a-zA-Z0-9 ]{100,200}", 50..=120).prop_map(|parts| parts.join(" "))
}

/// Vec of 100+ tags.
fn arb_many_tags() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(arb_tag(), 100..=150)
}

/// Vec of 500+ ref lines.
fn arb_many_refs() -> impl Strategy<Value = String> {
    prop::collection::vec(arb_ref_line(), 500..=600).prop_map(|lines| lines.join("\n"))
}

/// BTreeMap of 50+ extra fields.
fn arb_many_extras() -> impl Strategy<Value = std::collections::BTreeMap<String, Value>> {
    prop::collection::btree_map(arb_extra_key(), arb_value_leaf(), 50..=80)
}

// ---------------------------------------------------------------------------
// Extended parser properties
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(25))]

    // -- Category 1: Malformed frontmatter --

    #[test]
    fn malformed_yaml_special_chars_no_panic(content in arb_yaml_special_string()) {
        let input = format!("---\n{content}\n---\n");
        let result = std::panic::catch_unwind(|| parser::parse(&input, "test.md"));
        prop_assert!(result.is_ok(), "panicked on input: {}", input);
    }

    #[test]
    fn malformed_random_bytes_no_panic(bytes in prop::collection::vec(0x20u8..0x7E, 10..500)) {
        let content = String::from_utf8_lossy(&bytes).to_string();
        let input = format!("---\n{content}\n---\n");
        let result = std::panic::catch_unwind(|| parser::parse(&input, "test.md"));
        prop_assert!(result.is_ok(), "panicked on input: {}", input);
    }

    #[test]
    fn malformed_truncated_frontmatter_no_panic(content in arb_yaml_special_string()) {
        let input = format!("---\n{content}\nsome body text");
        let result = std::panic::catch_unwind(|| parser::parse(&input, "test.md"));
        prop_assert!(result.is_ok(), "panicked on input: {}", input);
    }

    #[test]
    fn malformed_nested_yaml_no_panic(depth in 10u32..30, word in safe_word()) {
        let mut input = String::from("---\n");
        for i in 0..depth {
            let indent = "  ".repeat(i as usize);
            input.push_str(&format!("{indent}level{i}:\n"));
        }
        let deep_indent = "  ".repeat(depth as usize);
        input.push_str(&format!("{deep_indent}{word}\n---\n"));
        let result = std::panic::catch_unwind(|| parser::parse(&input, "test.md"));
        prop_assert!(result.is_ok(), "panicked on input: {}", input);
    }

    #[test]
    fn malformed_mixed_valid_invalid_no_panic(
        id in arb_doogat_id(),
        garbage in arb_yaml_special_string(),
        title in safe_word(),
    ) {
        let input = format!("---\nid: {id}\n{garbage}\ntitle: {title}\n---\n");
        let result = std::panic::catch_unwind(|| parser::parse(&input, "test.md"));
        prop_assert!(result.is_ok(), "panicked on input: {}", input);
    }

    // -- Category 2: Unicode --

    #[test]
    fn unicode_title_roundtrip(
        meta in arb_doogat_meta(),
        uni_title in arb_unicode_safe_string(),
        body in arb_body(),
    ) {
        let mut m = meta;
        m.title = Some(uni_title.clone());
        let md = build_doogat_markdown(&m, &body, "");
        let parsed = parser::parse(&md, "test.md");
        if let Ok(p) = parsed {
            let serialized = parser::serialize(&p);
            let reparsed = parser::parse(&serialized, "test.md");
            if let Ok(rp) = reparsed {
                prop_assert_eq!(&p.meta.title, &rp.meta.title);
            }
        }
    }

    #[test]
    fn unicode_tags_preserved(
        meta in arb_doogat_meta(),
        uni_tags in prop::collection::vec(arb_unicode_safe_string(), 1..=4),
        body in arb_body(),
    ) {
        let mut m = meta;
        m.tags = uni_tags;
        let md = build_doogat_markdown(&m, &body, "");
        let parsed = parser::parse(&md, "test.md");
        if let Ok(p) = parsed {
            let serialized = parser::serialize(&p);
            let reparsed = parser::parse(&serialized, "test.md");
            if let Ok(rp) = reparsed {
                prop_assert_eq!(&p.meta.tags, &rp.meta.tags);
            }
        }
    }

    #[test]
    fn unicode_body_roundtrip(
        meta in arb_doogat_meta(),
        uni_body in arb_unicode_safe_string(),
    ) {
        let md = build_doogat_markdown(&meta, &uni_body, "");
        let parsed = parser::parse(&md, "test.md");
        if let Ok(p) = parsed {
            let serialized = parser::serialize(&p);
            let reparsed = parser::parse(&serialized, "test.md");
            if let Ok(rp) = reparsed {
                prop_assert_eq!(&p.body, &rp.body);
            }
        }
    }

    #[test]
    fn unicode_extra_values_roundtrip(
        meta in arb_doogat_meta(),
        key in arb_extra_key(),
        uni_val in arb_unicode_safe_string(),
        body in arb_body(),
    ) {
        let mut m = meta;
        m.extra.insert(key, Value::String(uni_val));
        let md = build_doogat_markdown(&m, &body, "");
        let parsed = parser::parse(&md, "test.md");
        if let Ok(p) = parsed {
            let serialized = parser::serialize(&p);
            let reparsed = parser::parse(&serialized, "test.md");
            if let Ok(rp) = reparsed {
                prop_assert_eq!(&p.meta.extra, &rp.meta.extra);
            }
        }
    }

    // -- Category 3: Empty/missing zones --

    #[test]
    fn empty_body_roundtrip(
        meta in arb_doogat_meta(),
        refs in arb_reference_section(),
    ) {
        let md = build_doogat_markdown(&meta, "", &refs);
        let parsed = parser::parse(&md, "test.md").unwrap();
        let serialized = parser::serialize(&parsed);
        let reparsed = parser::parse(&serialized, "test.md").unwrap();
        prop_assert_eq!(&parsed.meta.id, &reparsed.meta.id);
        prop_assert_eq!(&parsed.meta.title, &reparsed.meta.title);
        prop_assert_eq!(&parsed.meta.tags, &reparsed.meta.tags);
        prop_assert_eq!(&parsed.body, &reparsed.body);
    }

    #[test]
    fn empty_refs_roundtrip(
        meta in arb_doogat_meta(),
        body in arb_body(),
    ) {
        let md = build_doogat_markdown(&meta, &body, "");
        let parsed = parser::parse(&md, "test.md").unwrap();
        let serialized = parser::serialize(&parsed);
        let reparsed = parser::parse(&serialized, "test.md").unwrap();
        prop_assert_eq!(&parsed.meta.id, &reparsed.meta.id);
        prop_assert_eq!(&parsed.body, &reparsed.body);
        prop_assert!(reparsed.reference_section.is_empty());
    }

    #[test]
    fn minimal_frontmatter_no_panic(body in prop::option::of(arb_body())) {
        let input = match body {
            Some(b) => format!("---\n---\n{b}"),
            None => "---\n---\n".to_string(),
        };
        let result = std::panic::catch_unwind(|| parser::parse(&input, "test.md"));
        prop_assert!(result.is_ok(), "panicked on input: {}", input);
    }

    // -- Category 4: Boundary cases --

    #[test]
    fn boundary_long_line_body(
        meta in arb_doogat_meta(),
        long_line in arb_long_line(),
    ) {
        let md = build_doogat_markdown(&meta, &long_line, "");
        let parsed = parser::parse(&md, "test.md").unwrap();
        let serialized = parser::serialize(&parsed);
        let reparsed = parser::parse(&serialized, "test.md").unwrap();
        prop_assert_eq!(&parsed.body, &reparsed.body);
    }

    #[test]
    fn boundary_many_tags(
        meta in arb_doogat_meta(),
        tags in arb_many_tags(),
        body in arb_body(),
    ) {
        let mut m = meta;
        m.tags = tags;
        let md = build_doogat_markdown(&m, &body, "");
        let parsed = parser::parse(&md, "test.md").unwrap();
        let serialized = parser::serialize(&parsed);
        let reparsed = parser::parse(&serialized, "test.md").unwrap();
        prop_assert_eq!(&parsed.meta.tags, &reparsed.meta.tags);
    }

    #[test]
    fn boundary_many_refs(
        meta in arb_doogat_meta(),
        body in arb_body(),
        refs in arb_many_refs(),
    ) {
        let md = build_doogat_markdown(&meta, &body, &refs);
        let parsed = parser::parse(&md, "test.md").unwrap();
        let serialized = parser::serialize(&parsed);
        let reparsed = parser::parse(&serialized, "test.md").unwrap();
        prop_assert_eq!(
            sorted_lines(&parsed.reference_section),
            sorted_lines(&reparsed.reference_section),
        );
    }

    #[test]
    fn boundary_many_extras(
        meta in arb_doogat_meta(),
        extras in arb_many_extras(),
        body in arb_body(),
    ) {
        let mut m = meta;
        m.extra = extras;
        let md = build_doogat_markdown(&m, &body, "");
        let parsed = parser::parse(&md, "test.md").unwrap();
        let serialized = parser::serialize(&parsed);
        let reparsed = parser::parse(&serialized, "test.md").unwrap();
        prop_assert_eq!(&parsed.meta.extra, &reparsed.meta.extra);
    }
}

// ---------------------------------------------------------------------------
// SQL engine mock store + generators
// ---------------------------------------------------------------------------

/// In-memory mock implementing DoogatStore for SQL engine tests.
struct MockStore {
    files: RefCell<HashMap<String, String>>,
    head: String,
}

impl MockStore {
    fn new() -> Self {
        Self {
            files: RefCell::new(HashMap::new()),
            head: "abc123".to_string(),
        }
    }
}

fn open_test_index() -> Index {
    Index::open_in_memory().unwrap()
}

impl DoogatSource for MockStore {
    fn list_doogats(&self) -> ddb_core::error::Result<Vec<String>> {
        let mut paths: Vec<String> = self
            .files
            .borrow()
            .keys()
            .filter(|p| p.starts_with("ddb/") && p.ends_with(".md"))
            .cloned()
            .collect();
        paths.sort();
        Ok(paths)
    }

    fn read_file(&self, path: &str) -> ddb_core::error::Result<String> {
        self.files
            .borrow()
            .get(path)
            .cloned()
            .ok_or_else(|| ddb_core::error::DoogatError::NotFound(path.to_string()))
    }

    fn head_oid(&self) -> ddb_core::error::Result<CommitHash> {
        Ok(CommitHash(self.head.clone()))
    }

    fn diff_paths(
        &self,
        _old_oid: &str,
        _new_oid: &str,
    ) -> ddb_core::error::Result<Vec<(DiffKind, String)>> {
        Ok(Vec::new())
    }
}

impl DoogatStore for MockStore {
    fn commit_file(
        &self,
        path: &str,
        content: &str,
        _msg: &str,
    ) -> ddb_core::error::Result<CommitHash> {
        self.files
            .borrow_mut()
            .insert(path.to_string(), content.to_string());
        Ok(CommitHash("mock".into()))
    }

    fn commit_files(
        &self,
        files: &[(&str, &str)],
        _msg: &str,
    ) -> ddb_core::error::Result<CommitHash> {
        let mut map = self.files.borrow_mut();
        for (path, content) in files {
            map.insert(path.to_string(), content.to_string());
        }
        Ok(CommitHash("mock".into()))
    }

    fn delete_file(&self, path: &str, _msg: &str) -> ddb_core::error::Result<CommitHash> {
        self.files.borrow_mut().remove(path);
        Ok(CommitHash("mock".into()))
    }

    fn delete_files(&self, paths: &[&str], _msg: &str) -> ddb_core::error::Result<CommitHash> {
        let mut map = self.files.borrow_mut();
        for path in paths {
            map.remove(*path);
        }
        Ok(CommitHash("mock".into()))
    }

    fn commit_batch(
        &self,
        writes: &[(&str, &str)],
        deletes: &[&str],
        _msg: &str,
    ) -> ddb_core::error::Result<CommitHash> {
        let mut map = self.files.borrow_mut();
        for (path, content) in writes {
            map.insert(path.to_string(), content.to_string());
        }
        for path in deletes {
            map.remove(*path);
        }
        Ok(CommitHash("mock".into()))
    }
}

/// Safe SQL column name (prefixed to avoid reserved words).
fn arb_column_name() -> impl Strategy<Value = String> {
    safe_word().prop_map(|w| format!("col_{}", w.to_lowercase()))
}

/// Safe SQL table name (prefixed, not "doogats" or "_ddb_*").
fn arb_table_name() -> impl Strategy<Value = String> {
    safe_word().prop_map(|w| format!("tbl_{}", w.to_lowercase()))
}

/// Random SQL column type.
fn arb_column_type() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("TEXT".to_string()),
        Just("INTEGER".to_string()),
        Just("REAL".to_string()),
        Just("BOOLEAN".to_string()),
    ]
}

/// Generate a valid CREATE TABLE statement with 1-5 columns.
fn arb_create_table_sql() -> impl Strategy<Value = (String, String, Vec<(String, String)>)> {
    (
        arb_table_name(),
        prop::collection::vec((arb_column_name(), arb_column_type()), 1..=5),
    )
        .prop_map(|(tbl, cols)| {
            // Deduplicate column names
            let mut seen = std::collections::HashSet::new();
            let cols: Vec<(String, String)> = cols
                .into_iter()
                .filter(|(name, _)| seen.insert(name.to_lowercase()))
                .collect();
            let col_defs: Vec<String> = cols
                .iter()
                .map(|(name, typ)| format!("{name} {typ}"))
                .collect();
            let sql = format!("CREATE TABLE {tbl} ({})", col_defs.join(", "));
            (sql, tbl, cols)
        })
}

/// Safe string value for SQL INSERT/UPDATE (alphanumeric, no quotes).
fn arb_sql_string_value() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{1,20}"
}

/// Injection strings with SQL-special characters.
fn arb_injection_string() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("'; DROP TABLE doogats; --".to_string()),
        Just("\" OR 1=1 --".to_string()),
        Just("/**/".to_string()),
        Just("\\".to_string()),
        Just("\0".to_string()),
        Just("'; DELETE FROM doogats WHERE '1'='1".to_string()),
        Just("value'); INSERT INTO doogats VALUES('hack".to_string()),
        prop::collection::vec(
            prop_oneof![
                Just("'".to_string()),
                Just("\"".to_string()),
                Just(";".to_string()),
                Just("--".to_string()),
                Just("/*".to_string()),
                Just("*/".to_string()),
                Just("\\".to_string()),
                "[a-zA-Z0-9]{1,5}".prop_map(|s| s),
            ],
            2..=10,
        )
        .prop_map(|parts| parts.join("")),
    ]
}

// ---------------------------------------------------------------------------
// SQL engine: valid DDL/DML properties (Task #5)
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1))]

    /// CREATE TABLE produces Ok result and typedef is queryable.
    #[test]
    fn sql_create_table_succeeds((sql, tbl, _cols) in arb_create_table_sql()) {
        let idx = open_test_index();
        let store = MockStore::new();
        let mut engine = SqlEngine::new(&idx, &store);

        let result = engine.execute(&sql);
        prop_assert!(result.is_ok(), "CREATE TABLE failed: {:?}\nsql: {}", result.err(), sql);

        // Verify typedef was created by querying _typedef table
        let check_sql = format!(
            "SELECT id FROM doogats WHERE type = '_typedef' AND title = '{}'",
            tbl.to_lowercase()
        );
        let check = idx.query_raw(&check_sql);
        prop_assert!(check.is_ok(), "typedef query failed: {:?}", check.err());
    }

    /// INSERT after CREATE TABLE returns Affected(1).
    #[test]
    fn sql_insert_succeeds(
        (create_sql, tbl, cols) in arb_create_table_sql(),
        values in prop::collection::vec(arb_sql_string_value(), 5),
    ) {
        let idx = open_test_index();
        let store = MockStore::new();
        let mut engine = SqlEngine::new(&idx, &store);

        engine.execute(&create_sql).unwrap();

        let col_names: Vec<&str> = cols.iter().map(|(n, _)| n.as_str()).collect();
        let val_strs: Vec<String> = cols.iter().enumerate().map(|(i, _)| {
            format!("'{}'", values.get(i).map(|s| s.as_str()).unwrap_or("val"))
        }).collect();
        let insert_sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            tbl,
            col_names.join(", "),
            val_strs.join(", ")
        );

        let result = engine.execute(&insert_sql);
        prop_assert!(result.is_ok(), "INSERT failed: {:?}\nsql: {}", result.err(), insert_sql);
        if let Ok(ddb_core::sql_engine::SqlResult::Affected(n)) = &result {
            prop_assert_eq!(*n, 1);
        }
    }

    /// SELECT after INSERT returns the inserted row.
    #[test]
    fn sql_select_after_insert(
        (create_sql, tbl, cols) in arb_create_table_sql(),
        values in prop::collection::vec(arb_sql_string_value(), 5),
    ) {
        let idx = open_test_index();
        let store = MockStore::new();
        let mut engine = SqlEngine::new(&idx, &store);

        engine.execute(&create_sql).unwrap();

        let col_names: Vec<&str> = cols.iter().map(|(n, _)| n.as_str()).collect();
        let val_strs: Vec<String> = cols.iter().enumerate().map(|(i, _)| {
            format!("'{}'", values.get(i).map(|s| s.as_str()).unwrap_or("val"))
        }).collect();
        let insert_sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            tbl, col_names.join(", "), val_strs.join(", ")
        );
        engine.execute(&insert_sql).unwrap();

        let select_sql = format!("SELECT {} FROM {}", col_names.join(", "), tbl);
        let result = engine.execute(&select_sql);
        prop_assert!(result.is_ok(), "SELECT failed: {:?}", result.err());
        if let Ok(ddb_core::sql_engine::SqlResult::Rows { rows, .. }) = &result {
            prop_assert!(!rows.is_empty(), "SELECT returned no rows after INSERT");
        }
    }

    /// UPDATE modifies the inserted value.
    #[test]
    fn sql_update_modifies(
        (create_sql, tbl, cols) in arb_create_table_sql(),
        values in prop::collection::vec(arb_sql_string_value(), 5),
        new_val in arb_sql_string_value(),
    ) {
        let idx = open_test_index();
        let store = MockStore::new();
        let mut engine = SqlEngine::new(&idx, &store);

        engine.execute(&create_sql).unwrap();

        let col_names: Vec<&str> = cols.iter().map(|(n, _)| n.as_str()).collect();
        let val_strs: Vec<String> = cols.iter().enumerate().map(|(i, _)| {
            format!("'{}'", values.get(i).map(|s| s.as_str()).unwrap_or("val"))
        }).collect();
        let insert_sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            tbl, col_names.join(", "), val_strs.join(", ")
        );
        engine.execute(&insert_sql).unwrap();

        let first_col = &cols[0].0;
        let update_sql = format!("UPDATE {} SET {} = '{}'", tbl, first_col, new_val);
        let result = engine.execute(&update_sql);
        prop_assert!(result.is_ok(), "UPDATE failed: {:?}\nsql: {}", result.err(), update_sql);

        let select_sql = format!("SELECT {} FROM {}", first_col, tbl);
        let sel = engine.execute(&select_sql);
        prop_assert!(sel.is_ok());
        if let Ok(ddb_core::sql_engine::SqlResult::Rows { rows, .. }) = &sel {
            prop_assert!(!rows.is_empty());
            // SQLite type affinity may normalize values (e.g. '00' → '0' for INTEGER).
            // BOOLEAN columns also coerce stored "0"/"1" to "false"/"true" on SELECT.
            // Compare with normalization: match exact, numeric equivalence, or boolean equivalence.
            let got = &rows[0][0];
            let as_bool = |s: &str| -> Option<bool> {
                match s.to_lowercase().as_str() {
                    "true" | "1" | "yes" => Some(true),
                    "false" | "0" | "no" => Some(false),
                    _ => None,
                }
            };
            let col_type = cols[0].1.to_uppercase();
            let matches = got.contains(&new_val)
                || got.parse::<f64>().ok() == new_val.parse::<f64>().ok()
                || (col_type == "BOOLEAN"
                    && as_bool(got).is_some()
                    && as_bool(got) == as_bool(&new_val));
            prop_assert!(
                matches,
                "expected updated value '{}' in row, got '{}'",
                new_val, got
            );
        }
    }

    /// DELETE removes inserted rows.
    #[test]
    fn sql_delete_removes(
        (create_sql, tbl, cols) in arb_create_table_sql(),
        values in prop::collection::vec(arb_sql_string_value(), 5),
    ) {
        let idx = open_test_index();
        let store = MockStore::new();
        let mut engine = SqlEngine::new(&idx, &store);

        engine.execute(&create_sql).unwrap();

        let col_names: Vec<&str> = cols.iter().map(|(n, _)| n.as_str()).collect();
        let val_strs: Vec<String> = cols.iter().enumerate().map(|(i, _)| {
            format!("'{}'", values.get(i).map(|s| s.as_str()).unwrap_or("val"))
        }).collect();
        let insert_sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            tbl, col_names.join(", "), val_strs.join(", ")
        );
        engine.execute(&insert_sql).unwrap();

        let delete_sql = format!("DELETE FROM {}", tbl);
        let result = engine.execute(&delete_sql);
        prop_assert!(result.is_ok(), "DELETE failed: {:?}", result.err());

        let select_sql = format!("SELECT * FROM {}", tbl);
        let sel = engine.execute(&select_sql);
        prop_assert!(sel.is_ok());
        if let Ok(ddb_core::sql_engine::SqlResult::Rows { rows, .. }) = &sel {
            prop_assert!(rows.is_empty(), "rows remain after DELETE: {:?}", rows);
        }
    }

    /// DDL roundtrip: CREATE TABLE columns match typedef query.
    #[test]
    fn sql_ddl_roundtrip((create_sql, _, cols) in arb_create_table_sql()) {
        let idx = open_test_index();
        let store = MockStore::new();
        let mut engine = SqlEngine::new(&idx, &store);

        let result = engine.execute(&create_sql);
        prop_assert!(result.is_ok(), "CREATE TABLE failed: {:?}", result.err());

        // The typedef doogat should be stored in MockStore
        let files = store.files.borrow();
        let typedef_file = files.iter().find(|(path, _)| {
            path.starts_with("ddb/_typedef/") && path.ends_with(".md")
        });
        prop_assert!(typedef_file.is_some(), "no typedef file found in store");

        let (_, content) = typedef_file.unwrap();
        // Verify each column name appears in the typedef content
        for (col_name, _col_type) in &cols {
            prop_assert!(
                content.to_lowercase().contains(&col_name.to_lowercase()),
                "column '{}' not found in typedef:\n{}", col_name, content
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SQL engine: invalid input properties (Task #6)
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2))]

    /// Random ASCII strings don't cause panics.
    #[test]
    fn sql_random_strings_no_panic(input in "[\\x20-\\x7E]{0,200}") {
        let idx = open_test_index();
        let store = MockStore::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut engine = SqlEngine::new(&idx, &store);
            let _ = engine.execute_batch(&input);
        }));
        prop_assert!(result.is_ok(), "panicked on input: {:?}", input);
    }

    /// Truncated SQL keywords return Err, not panic.
    #[test]
    fn sql_partial_fragments_no_panic(
        fragment in prop_oneof![
            Just("SELE".to_string()),
            Just("CREA".to_string()),
            Just("INS".to_string()),
            Just("UPDA".to_string()),
            Just("DELE".to_string()),
            Just("DROP".to_string()),
            Just("ALTER".to_string()),
            Just("CREATE TABLE".to_string()),
            Just("INSERT INTO".to_string()),
            Just("SELECT FROM".to_string()),
            Just("DELETE WHERE".to_string()),
        ],
    ) {
        let idx = open_test_index();
        let store = MockStore::new();
        let mut engine = SqlEngine::new(&idx, &store);
        let result = engine.execute_batch(&fragment);
        prop_assert!(result.is_err(), "expected Err for fragment '{}', got: {:?}", fragment, result);
    }

    /// Unsupported SQL operations return Err.
    #[test]
    fn sql_unsupported_ops_return_err(
        stmt in prop_oneof![
            Just("CREATE INDEX idx ON tbl_test (col_a)".to_string()),
            Just("CREATE VIEW v AS SELECT 1".to_string()),
            Just("CREATE VIRTUAL TABLE vt USING fts5(content)".to_string()),
            Just("CREATE TRIGGER tr AFTER INSERT ON tbl_test BEGIN SELECT 1; END".to_string()),
            Just("DROP INDEX idx".to_string()),
            Just("DROP VIEW v".to_string()),
        ],
    ) {
        let idx = open_test_index();
        let store = MockStore::new();
        let mut engine = SqlEngine::new(&idx, &store);
        let result = engine.execute(&stmt);
        prop_assert!(result.is_err(), "expected Err for unsupported '{}', got: {:?}", stmt, result);
    }

    /// Empty and whitespace-only strings return Err.
    #[test]
    fn sql_empty_input_returns_err(
        input in prop_oneof![
            Just("".to_string()),
            Just(" ".to_string()),
            Just("  \t  ".to_string()),
            Just("\n".to_string()),
            Just("   \n   \n   ".to_string()),
        ],
    ) {
        let idx = open_test_index();
        let store = MockStore::new();
        let mut engine = SqlEngine::new(&idx, &store);
        let result = engine.execute_batch(&input);
        prop_assert!(result.is_err(), "expected Err for empty/whitespace input '{:?}'", input);
    }
}

// ---------------------------------------------------------------------------
// SQL engine: injection/edge-case properties (Task #7)
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2))]

    /// Injection strings in VALUES are treated as data, not code.
    #[test]
    fn sql_injection_in_values_no_escape(injection in arb_injection_string()) {
        let idx = open_test_index();
        let store = MockStore::new();
        let mut engine = SqlEngine::new(&idx, &store);

        engine.execute("CREATE TABLE tbl_inject (col_data TEXT)").unwrap();

        // Escape single quotes for SQL string literal
        let escaped = injection.replace('\'', "''");
        let insert_sql = format!("INSERT INTO tbl_inject (col_data) VALUES ('{escaped}')");
        let result = engine.execute(&insert_sql);
        // Either Err (parse failure) or the literal string is stored — never executed as SQL
        if result.is_ok() {
            let sel = engine.execute("SELECT col_data FROM tbl_inject");
            if let Ok(ddb_core::sql_engine::SqlResult::Rows { rows, .. }) = sel {
                if !rows.is_empty() {
                    // The stored value should be the literal injection string
                    prop_assert!(
                        rows[0][0].contains(&injection) || rows[0][0].contains(&escaped),
                        "injection string not stored literally: got '{}', expected '{}'",
                        rows[0][0], injection
                    );
                }
            }
        }
    }

    /// SQL reserved words as double-quoted column identifiers succeed.
    #[test]
    fn sql_reserved_word_identifiers(
        word in prop_oneof![
            Just("select"),
            Just("table"),
            Just("index"),
            Just("where"),
            Just("from"),
            Just("order"),
            Just("group"),
            Just("insert"),
            Just("update"),
            Just("delete"),
            Just("create"),
            Just("drop"),
        ],
    ) {
        let idx = open_test_index();
        let store = MockStore::new();
        let mut engine = SqlEngine::new(&idx, &store);

        let sql = format!("CREATE TABLE tbl_reserved (\"{word}\" TEXT)");
        let result = engine.execute(&sql);
        prop_assert!(
            result.is_ok(),
            "CREATE TABLE with reserved word '{}' failed: {:?}", word, result.err()
        );
    }

    /// Hyphenated identifiers in double quotes succeed.
    #[test]
    fn sql_hyphenated_identifiers(
        prefix in safe_word(),
        suffix in safe_word(),
    ) {
        let idx = open_test_index();
        let store = MockStore::new();
        let mut engine = SqlEngine::new(&idx, &store);

        let tbl_name = format!("tbl_{prefix}-{suffix}");
        let sql = format!("CREATE TABLE \"{tbl_name}\" (col_a TEXT)");
        let result = engine.execute(&sql);
        prop_assert!(
            result.is_ok(),
            "CREATE TABLE with hyphen '{}' failed: {:?}", tbl_name, result.err()
        );
    }

    /// Unicode identifiers in double quotes don't panic.
    #[test]
    fn sql_unicode_identifiers(name in arb_unicode_safe_string()) {
        let idx = open_test_index();
        let store = MockStore::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut engine = SqlEngine::new(&idx, &store);
            let sql = format!("CREATE TABLE \"tbl_{}\" (col_a TEXT)", name);
            let _ = engine.execute(&sql);
        }));
        prop_assert!(result.is_ok(), "panicked on unicode identifier: {:?}", name);
    }

    /// Edge-case SQL structures don't cause panics.
    #[test]
    fn sql_edge_case_structures_no_panic(
        input in prop_oneof![
            Just(";;;".to_string()),
            Just("SELECT 1; ; SELECT 2".to_string()),
            Just("SELECT 1 WHERE (((((1=1)))))".to_string()),
            Just("SELECT 1 WHERE ((((((((((1=1))))))))))".to_string()),
            Just("SELECT ''; SELECT '';".to_string()),
            prop::collection::vec(Just(";"), 1..=50).prop_map(|v| v.join("")),
            prop::collection::vec(Just("()"), 1..=30).prop_map(|v| format!("SELECT {}", v.join(""))),
        ],
    ) {
        let idx = open_test_index();
        let store = MockStore::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut engine = SqlEngine::new(&idx, &store);
            let _ = engine.execute_batch(&input);
        }));
        prop_assert!(result.is_ok(), "panicked on edge-case input: {:?}", input);
    }
}

/// Compare reference sections as sorted line sets (CRDT merge may reorder).
fn sorted_lines(s: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines.sort();
    lines
}

// ---------------------------------------------------------------------------
// CRDT generators
// ---------------------------------------------------------------------------

/// Generate a ConflictFile with non-overlapping frontmatter edits (different fields).
fn arb_conflict_divergent_fm() -> impl Strategy<Value = ConflictFile> {
    // Ours changes title, theirs adds a new extra field — no overlap
    (arb_doogat_meta(), safe_word(), safe_word(), safe_word()).prop_map(
        |(meta, new_title, extra_key, extra_val)| {
            let ancestor_md = build_doogat_markdown(&meta, "Body text.", "");
            let mut meta_a = meta.clone();
            meta_a.title = Some(new_title);
            let mut meta_b = meta;
            let extra_key = format!("xConflict{extra_key}");
            meta_b.extra.insert(extra_key, Value::String(extra_val));
            let ours = build_doogat_markdown(&meta_a, "Body text.", "");
            let theirs = build_doogat_markdown(&meta_b, "Body text.", "");

            ConflictFile {
                path: "ddb/test.md".into(),
                ancestor: Some(ancestor_md),
                ours,
                theirs,
                ours_hlc: None,
                theirs_hlc: None,
                ours_blob_oid: None,
                theirs_blob_oid: None,
            }
        },
    )
}

/// Generate a ConflictFile with non-overlapping body edits.
fn arb_conflict_body_edits() -> impl Strategy<Value = ConflictFile> {
    (
        arb_doogat_meta(),
        arb_paragraph(),
        arb_paragraph(),
        arb_paragraph(),
        safe_sentence(),
        safe_sentence(),
    )
        .prop_map(|(meta, para1, para2, para3, edit_first, edit_last)| {
            let body = format!("{para1}\n\n{para2}\n\n{para3}");
            let ancestor_md = build_doogat_markdown(&meta, &body, "");

            let body_ours = format!("{edit_first}\n\n{para2}\n\n{para3}");
            let body_theirs = format!("{para1}\n\n{para2}\n\n{edit_last}");
            let ours = build_doogat_markdown(&meta, &body_ours, "");
            let theirs = build_doogat_markdown(&meta, &body_theirs, "");

            ConflictFile {
                path: "ddb/test.md".into(),
                ancestor: Some(ancestor_md),
                ours,
                theirs,
                ours_hlc: None,
                theirs_hlc: None,
                ours_blob_oid: None,
                theirs_blob_oid: None,
            }
        })
}

/// Generate a ConflictFile with concurrent reference additions.
fn arb_conflict_ref_additions() -> impl Strategy<Value = ConflictFile> {
    (
        arb_doogat_meta(),
        arb_body(),
        safe_word(),
        safe_word(),
        safe_word(),
        safe_word(),
    )
        .prop_map(|(meta, body, key_a, val_a, key_b, val_b)| {
            let ancestor_md = build_doogat_markdown(&meta, &body, "");

            // Use prefixed keys to avoid cross-zone inline field duplicates
            let ref_ours = format!("- rOurs{key_a}:: {val_a}");
            let ref_theirs = format!("- rTheirs{key_b}:: {val_b}");
            let ours = build_doogat_markdown(&meta, &body, &ref_ours);
            let theirs = build_doogat_markdown(&meta, &body, &ref_theirs);

            ConflictFile {
                path: "ddb/test.md".into(),
                ancestor: Some(ancestor_md),
                ours,
                theirs,
                ours_hlc: None,
                theirs_hlc: None,
                ours_blob_oid: None,
                theirs_blob_oid: None,
            }
        })
}

// ---------------------------------------------------------------------------
// CRDT properties
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5))]

    /// Merge commutativity: swapping ours/theirs produces same result.
    #[test]
    fn crdt_commutativity(conflict in arb_conflict_divergent_fm()) {
        let swapped = ConflictFile {
            path: conflict.path.clone(),
            ancestor: conflict.ancestor.clone(),
            ours: conflict.theirs.clone(),
            theirs: conflict.ours.clone(),
            ours_hlc: conflict.theirs_hlc.clone(),
            theirs_hlc: conflict.ours_hlc.clone(),
            ours_blob_oid: None,
            theirs_blob_oid: None,
        };

        let result_ab = crdt_resolver::resolve_conflicts(vec![conflict], None).unwrap();
        let result_ba = crdt_resolver::resolve_conflicts(vec![swapped], None).unwrap();

        prop_assert_eq!(result_ab.len(), 1);
        prop_assert_eq!(result_ba.len(), 1);
        prop_assert_eq!(&result_ab[0].content, &result_ba[0].content);
    }

    /// Body-edit commutativity: swapping ours/theirs body edits produces same result.
    #[test]
    fn crdt_commutativity_body(conflict in arb_conflict_body_edits()) {
        let swapped = ConflictFile {
            path: conflict.path.clone(),
            ancestor: conflict.ancestor.clone(),
            ours: conflict.theirs.clone(),
            theirs: conflict.ours.clone(),
            ours_hlc: conflict.theirs_hlc.clone(),
            theirs_hlc: conflict.ours_hlc.clone(),
            ours_blob_oid: None,
            theirs_blob_oid: None,
        };

        let result_ab = crdt_resolver::resolve_conflicts(vec![conflict], None).unwrap();
        let result_ba = crdt_resolver::resolve_conflicts(vec![swapped], None).unwrap();

        prop_assert_eq!(result_ab.len(), 1);
        prop_assert_eq!(result_ba.len(), 1);
        prop_assert_eq!(&result_ab[0].content, &result_ba[0].content);
    }

    /// Reference-addition commutativity: swapping ours/theirs ref additions produces same result.
    #[test]
    fn crdt_commutativity_refs(conflict in arb_conflict_ref_additions()) {
        let swapped = ConflictFile {
            path: conflict.path.clone(),
            ancestor: conflict.ancestor.clone(),
            ours: conflict.theirs.clone(),
            theirs: conflict.ours.clone(),
            ours_hlc: conflict.theirs_hlc.clone(),
            theirs_hlc: conflict.ours_hlc.clone(),
            ours_blob_oid: None,
            theirs_blob_oid: None,
        };

        let result_ab = crdt_resolver::resolve_conflicts(vec![conflict], None).unwrap();
        let result_ba = crdt_resolver::resolve_conflicts(vec![swapped], None).unwrap();

        prop_assert_eq!(result_ab.len(), 1);
        prop_assert_eq!(result_ba.len(), 1);
        // Compare structurally — ref line order may differ
        let merged_ab = parser::parse(&result_ab[0].content, "test.md").unwrap();
        let merged_ba = parser::parse(&result_ba[0].content, "test.md").unwrap();
        prop_assert_eq!(&merged_ab.meta.id, &merged_ba.meta.id);
        prop_assert_eq!(&merged_ab.meta.title, &merged_ba.meta.title);
        prop_assert_eq!(&merged_ab.meta.date, &merged_ba.meta.date);
        prop_assert_eq!(&merged_ab.meta.doogat_type, &merged_ba.meta.doogat_type);
        prop_assert_eq!(&merged_ab.meta.tags, &merged_ba.meta.tags);
        prop_assert_eq!(&merged_ab.meta.extra, &merged_ba.meta.extra);
        prop_assert_eq!(&merged_ab.body, &merged_ba.body);
        prop_assert_eq!(sorted_lines(&merged_ab.reference_section),
                        sorted_lines(&merged_ba.reference_section));
    }

    /// Merge idempotency: merging identical ours/theirs returns original.
    #[test]
    fn crdt_idempotency(md in arb_doogat_markdown()) {
        let conflict = ConflictFile {
            path: "ddb/test.md".into(),
            ancestor: Some(md.clone()),
            ours: md.clone(),
            theirs: md.clone(),
            ours_hlc: None,
            theirs_hlc: None,
            ours_blob_oid: None,
            theirs_blob_oid: None,
        };

        let result = crdt_resolver::resolve_conflicts(vec![conflict], None).unwrap();
        prop_assert_eq!(result.len(), 1);

        // Re-parse both and compare structurally (serialization may normalize)
        let original = parser::parse(&md, "test.md").unwrap();
        let merged = parser::parse(&result[0].content, "test.md").unwrap();

        prop_assert_eq!(&original.meta.id, &merged.meta.id);
        prop_assert_eq!(&original.meta.title, &merged.meta.title);
        prop_assert_eq!(&original.meta.tags, &merged.meta.tags);
        prop_assert_eq!(&original.body, &merged.body);
        // Reference lines may be reordered by set merge; compare as sorted sets
        let orig_refs = sorted_lines(&original.reference_section);
        let merged_refs = sorted_lines(&merged.reference_section);
        prop_assert_eq!(&orig_refs, &merged_refs);
    }

    /// Frontmatter field independence: changing field X doesn't affect field Y.
    #[test]
    fn crdt_field_independence(conflict in arb_conflict_divergent_fm()) {
        let ancestor = parser::parse(conflict.ancestor.as_ref().unwrap(), "test.md").unwrap();
        let ours = parser::parse(&conflict.ours, "test.md").unwrap();
        let theirs = parser::parse(&conflict.theirs, "test.md").unwrap();

        let result = crdt_resolver::resolve_conflicts(vec![conflict], None).unwrap();
        prop_assert_eq!(result.len(), 1);
        let merged = parser::parse(&result[0].content, "test.md").unwrap();

        // Ours changed title — merged should have ours' title
        prop_assert_eq!(&ours.meta.title, &merged.meta.title);
        // Theirs added extra field — merged should have it
        for (k, v) in &theirs.meta.extra {
            if !ancestor.meta.extra.contains_key(k) {
                prop_assert_eq!(merged.meta.extra.get(k), Some(v),
                    "theirs extra field {} missing in merge", k);
            }
        }
        // Fields unchanged by either side should match ancestor
        prop_assert_eq!(&ancestor.meta.id, &merged.meta.id);
        prop_assert_eq!(&ancestor.meta.date, &merged.meta.date);
        prop_assert_eq!(&ancestor.meta.doogat_type, &merged.meta.doogat_type);
        prop_assert_eq!(&ancestor.meta.tags, &merged.meta.tags);
        // Ancestor's original extra fields should survive
        for (k, v) in &ancestor.meta.extra {
            prop_assert_eq!(merged.meta.extra.get(k), Some(v),
                "ancestor extra field {} lost in merge", k);
        }
    }

    /// Non-overlapping body edits both survive merge.
    #[test]
    fn crdt_non_overlapping_body(conflict in arb_conflict_body_edits()) {
        // Extract what the edits were
        let ours_parsed = parser::parse(&conflict.ours, "test.md").unwrap();
        let theirs_parsed = parser::parse(&conflict.theirs, "test.md").unwrap();
        let ours_first_para = ours_parsed.body.lines().next().unwrap_or("").to_string();
        let theirs_last_para = theirs_parsed.body.lines().last().unwrap_or("").to_string();

        let result = crdt_resolver::resolve_conflicts(vec![conflict], None).unwrap();
        prop_assert_eq!(result.len(), 1);

        let merged = parser::parse(&result[0].content, "test.md").unwrap();
        // Both edits should be present
        prop_assert!(
            merged.body.contains(&ours_first_para),
            "ours edit missing: {:?}\nmerged body: {:?}",
            ours_first_para,
            merged.body,
        );
        prop_assert!(
            merged.body.contains(&theirs_last_para),
            "theirs edit missing: {:?}\nmerged body: {:?}",
            theirs_last_para,
            merged.body,
        );
    }

    /// Concurrent reference additions both survive merge.
    #[test]
    fn crdt_reference_union(conflict in arb_conflict_ref_additions()) {
        // Extract added ref keys
        let ours_parsed = parser::parse(&conflict.ours, "test.md").unwrap();
        let theirs_parsed = parser::parse(&conflict.theirs, "test.md").unwrap();

        let result = crdt_resolver::resolve_conflicts(vec![conflict], None).unwrap();
        prop_assert_eq!(result.len(), 1);

        let merged = parser::parse(&result[0].content, "test.md").unwrap();
        // Both ref sections should be present in merged
        for line in ours_parsed.reference_section.lines() {
            if !line.trim().is_empty() {
                prop_assert!(
                    merged.reference_section.contains(line),
                    "ours ref line missing: {:?}\nmerged ref: {:?}",
                    line,
                    merged.reference_section,
                );
            }
        }
        for line in theirs_parsed.reference_section.lines() {
            if !line.trim().is_empty() {
                prop_assert!(
                    merged.reference_section.contains(line),
                    "theirs ref line missing: {:?}\nmerged ref: {:?}",
                    line,
                    merged.reference_section,
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Indexer mock + generators
// ---------------------------------------------------------------------------

/// In-memory mock DoogatSource for integration test use (mirrors traits::mock::MockSource).
struct MockSource {
    files: HashMap<String, String>,
    head: String,
}

impl MockSource {
    fn new() -> Self {
        Self {
            files: HashMap::new(),
            head: "abc123".to_string(),
        }
    }
}

impl DoogatSource for MockSource {
    fn list_doogats(&self) -> ddb_core::error::Result<Vec<String>> {
        let mut paths: Vec<String> = self
            .files
            .keys()
            .filter(|p| p.starts_with("ddb/") && p.ends_with(".md"))
            .cloned()
            .collect();
        paths.sort();
        Ok(paths)
    }

    fn read_file(&self, path: &str) -> ddb_core::error::Result<String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| ddb_core::error::DoogatError::NotFound(path.to_string()))
    }

    fn head_oid(&self) -> ddb_core::error::Result<CommitHash> {
        Ok(CommitHash(self.head.clone()))
    }

    fn diff_paths(
        &self,
        _old_oid: &str,
        _new_oid: &str,
    ) -> ddb_core::error::Result<Vec<(ddb_core::types::DiffKind, String)>> {
        Ok(Vec::new())
    }
}

/// Generate a set of random doogats as (path, content) pairs with unique IDs.
fn arb_doogat_set(count: std::ops::Range<usize>) -> impl Strategy<Value = Vec<(String, String)>> {
    prop::collection::vec((arb_doogat_meta(), arb_body()), count).prop_map(|items| {
        items
            .into_iter()
            .enumerate()
            .map(|(i, (mut meta, body))| {
                // Ensure unique IDs by appending index
                let id = format!("2025010100{:04}", i);
                meta.id = Some(DoogatId(id.clone()));
                let md = build_doogat_markdown(&meta, &body, "");
                (format!("ddb/{id}.md"), md)
            })
            .collect()
    })
}

// ---------------------------------------------------------------------------
// Indexer properties
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5))]

    /// Index-rebuild equivalence: sequential index_doogat == full rebuild.
    #[test]
    fn indexer_equivalence(doogats in arb_doogat_set(3..10)) {
        // Build a MockSource
        let mut source = MockSource::new();
        for (path, content) in &doogats {
            source.files.insert(path.clone(), content.clone());
        }

        // Index A: sequential index_doogat calls
        let idx_a = open_test_index();
        for (path, content) in &doogats {
            let parsed = parser::parse(content, path).unwrap();
            idx_a.index_doogat(&parsed).unwrap();
        }

        // Index B: full rebuild
        let idx_b = open_test_index();
        idx_b.rebuild(&source).unwrap();

        // Compare: query each doogat by ID
        for (path, _) in &doogats {
            let id = path
                .strip_prefix("ddb/")
                .unwrap()
                .strip_suffix(".md")
                .unwrap();
            let sql = format!("SELECT id, title, type, body FROM doogats WHERE id = '{id}'");
            let rows_a = idx_a.query_raw(&sql).unwrap();
            let rows_b = idx_b.query_raw(&sql).unwrap();
            prop_assert_eq!(&rows_a, &rows_b, "mismatch for doogat {}", id);
        }
    }

    /// Staleness detection: index at commit X is stale after commit Y.
    #[test]
    fn indexer_staleness(
        doogats in arb_doogat_set(1..5),
        commit_x in "[0-9a-f]{40}",
        commit_y in "[0-9a-f]{40}",
    ) {
        let mut source = MockSource::new();
        source.head = commit_x.clone();
        for (path, content) in &doogats {
            source.files.insert(path.clone(), content.clone());
        }

        let idx = open_test_index();
        idx.rebuild(&source).unwrap();

        // Same commit → not stale
        prop_assert!(!idx.is_stale(&source).unwrap());

        // Different commit → stale
        if commit_x != commit_y {
            source.head = commit_y;
            prop_assert!(idx.is_stale(&source).unwrap());
        }
    }
}

// ---------------------------------------------------------------------------
// FTS5 + type-inference generators
// ---------------------------------------------------------------------------

/// Generate strings with FTS5-special characters and operators.
fn arb_fts5_query() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("*".to_string()),
        Just("\"".to_string()),
        Just("NEAR".to_string()),
        Just("AND".to_string()),
        Just("OR".to_string()),
        Just("NOT".to_string()),
        Just("(".to_string()),
        Just(")".to_string()),
        Just("^".to_string()),
        Just("+".to_string()),
        Just("-".to_string()),
        Just("NEAR(a b, 2)".to_string()),
        Just("\"unterminated".to_string()),
        Just("a AND OR b".to_string()),
        Just("((())".to_string()),
        Just("a* OR b*".to_string()),
        Just("NOT NOT NOT a".to_string()),
        // Random mixes of FTS5 operators
        (safe_word(), safe_word()).prop_map(|(a, b)| format!("{a} AND {b}")),
        (safe_word(), safe_word()).prop_map(|(a, b)| format!("{a} OR {b}")),
        (safe_word(), safe_word()).prop_map(|(a, b)| format!("{a} NOT {b}")),
        (safe_word(), safe_word()).prop_map(|(a, b)| format!("NEAR({a} {b})")),
        (safe_word(),).prop_map(|(a,)| format!("\"{a}\"*")),
        (safe_word(),).prop_map(|(a,)| format!("^{a}")),
    ]
}

/// Generate long query strings (1K+ characters).
fn arb_long_query() -> impl Strategy<Value = String> {
    prop::collection::vec("[a-zA-Z0-9 *\"()]{1,20}", 60..=120).prop_map(|parts| parts.join(" "))
}

/// Generate a vec of (key, Value) pairs where the same key maps to different
/// Value types across items — used to test type widening.
fn arb_mixed_type_extras() -> impl Strategy<Value = Vec<Vec<(String, Value)>>> {
    let key = prop_oneof![Just("xMixed".to_string()), Just("xField".to_string()),];
    let values = prop::collection::vec((key.clone(), arb_value_leaf()), 3..=6);
    // Return a vec of single-entry extra maps, each with the same key but
    // potentially different Value types.
    values.prop_map(|pairs| pairs.into_iter().map(|(k, v)| vec![(k, v)]).collect())
}

/// Create a seeded index with 3 hardcoded doogats for FTS5 fuzzing.
fn seed_index_with_sample_data() -> Index {
    let mut source = MockSource::new();
    source.files.insert(
        "ddb/20250101000000.md".into(),
        "---\nid: \"20250101000000\"\ntitle: Alpha note\ntags: [rust, testing]\n---\nThis is the first doogat about Rust programming.\n".into(),
    );
    source.files.insert(
        "ddb/20250101000001.md".into(),
        "---\nid: \"20250101000001\"\ntitle: Beta note\ntags: [python, data]\n---\nSecond doogat covers Python data analysis.\n".into(),
    );
    source.files.insert(
        "ddb/20250101000002.md".into(),
        "---\nid: \"20250101000002\"\ntitle: Gamma note\ntags: [rust, wasm]\n---\nThird doogat about WebAssembly and Rust.\n".into(),
    );

    let idx = open_test_index();
    idx.rebuild(&source).unwrap();
    idx
}

// ---------------------------------------------------------------------------
// FTS5 query fuzzing (Task #8)
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5))]

    /// Random ASCII strings as search queries never panic.
    #[test]
    fn fts5_random_query_no_crash(query in "[\\x20-\\x7E]{0,200}") {
        let idx = seed_index_with_sample_data();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = idx.search(&query);
        }));
        prop_assert!(result.is_ok(), "panicked on query: {:?}", query);
    }

    /// FTS5 operator strings never panic.
    #[test]
    fn fts5_special_operators_no_crash(query in arb_fts5_query()) {
        let idx = seed_index_with_sample_data();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = idx.search(&query);
        }));
        prop_assert!(result.is_ok(), "panicked on FTS5 query: {:?}", query);
    }

    /// Long queries (1K+ chars) never panic.
    #[test]
    fn fts5_long_query_no_crash(query in arb_long_query()) {
        let idx = seed_index_with_sample_data();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = idx.search(&query);
        }));
        prop_assert!(result.is_ok(), "panicked on long query (len={})", query.len());
    }

    /// Empty and whitespace-only queries never panic.
    #[test]
    fn fts5_empty_query_no_crash(
        query in prop_oneof![
            Just("".to_string()),
            Just(" ".to_string()),
            Just("   ".to_string()),
            Just("\t".to_string()),
            Just("\n".to_string()),
            Just("  \t\n  ".to_string()),
            prop::collection::vec(Just(" "), 1..=50).prop_map(|v| v.join("")),
        ],
    ) {
        let idx = seed_index_with_sample_data();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = idx.search(&query);
        }));
        prop_assert!(result.is_ok(), "panicked on empty/whitespace query: {:?}", query);
    }
}

// ---------------------------------------------------------------------------
// Type inference properties (Task #9)
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5))]

    /// Same-type extra fields are preserved after indexing.
    #[test]
    fn indexer_type_inference_consistent(
        value_type in 0u8..3,
        count in 2usize..=5,
    ) {
        let idx = open_test_index();

        let key = "xConsistent";
        for i in 0..count {
            let id = format!("2025020100{:04}", i);
            let val_str = match value_type {
                0 => format!("word{i}"),
                1 => format!("{}", 42 + i),
                _ => if i % 2 == 0 { "true".to_string() } else { "false".to_string() },
            };
            let value = match value_type {
                0 => Value::String(val_str.clone()),
                1 => Value::Number((42 + i) as f64),
                _ => Value::Bool(i % 2 == 0),
            };
            let mut extra = BTreeMap::new();
            extra.insert(key.to_string(), value);

            let doogat = ddb_core::types::ParsedDoogat {
                meta: DoogatMeta {
                    id: Some(DoogatId(id.clone())),
                    title: Some(format!("Test {i}")),
                    date: None,
                    doogat_type: None,
                    tags: vec![],
                    extra,
                },
                body: format!("Body {i}"),
                sections: vec![],
                reference_section: String::new(),
                inline_fields: vec![],
                links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
                path: format!("ddb/{id}.md"),
                updated_at: None,
            };
            idx.index_doogat(&doogat).unwrap();

            // Verify value stored correctly
            let rows = idx.query_raw(&format!(
                "SELECT value FROM _ddb_fields WHERE doogat_id = '{}' AND key = '{}'", id, key
            )).unwrap();
            prop_assert!(!rows.is_empty(), "no field row for doogat {}", id);
            prop_assert_eq!(&rows[0][0], &val_str, "value mismatch for doogat {}", id);
        }
    }

    /// Indexing the same doogats into two separate DBs produces identical results.
    #[test]
    fn indexer_type_inference_deterministic(doogats in arb_doogat_set(2..6)) {
        let idx_a = open_test_index();
        let idx_b = open_test_index();

        for (path, content) in &doogats {
            let parsed = parser::parse(content, path).unwrap();
            idx_a.index_doogat(&parsed).unwrap();
            idx_b.index_doogat(&parsed).unwrap();
        }

        // Compare all fields
        let rows_a = idx_a.query_raw(
            "SELECT doogat_id, key, value, zone FROM _ddb_fields ORDER BY doogat_id, key"
        ).unwrap();
        let rows_b = idx_b.query_raw(
            "SELECT doogat_id, key, value, zone FROM _ddb_fields ORDER BY doogat_id, key"
        ).unwrap();
        prop_assert_eq!(&rows_a, &rows_b, "field rows differ between identical indexes");

        // Compare doogats table
        let z_a = idx_a.query_raw(
            "SELECT id, title, type, body FROM doogats ORDER BY id"
        ).unwrap();
        let z_b = idx_b.query_raw(
            "SELECT id, title, type, body FROM doogats ORDER BY id"
        ).unwrap();
        prop_assert_eq!(&z_a, &z_b, "doogat rows differ between identical indexes");
    }

    /// Mixed-type extra fields on the same key don't panic.
    #[test]
    fn indexer_type_widening_no_panic(extras in arb_mixed_type_extras()) {
        let idx = open_test_index();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for (i, extra_pairs) in extras.iter().enumerate() {
                let id = format!("2025030100{:04}", i);
                let mut extra = BTreeMap::new();
                for (k, v) in extra_pairs {
                    extra.insert(k.clone(), v.clone());
                }
                let doogat = ddb_core::types::ParsedDoogat {
                    meta: DoogatMeta {
                        id: Some(DoogatId(id.clone())),
                        title: Some(format!("Widen {i}")),
                        date: None,
                        doogat_type: None,
                        tags: vec![],
                        extra,
                    },
                    body: format!("Body {i}"),
                    sections: vec![],
                    reference_section: String::new(),
                    inline_fields: vec![],
                    links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
                    path: format!("ddb/{id}.md"),
                    updated_at: None,
                };
                let _ = idx.index_doogat(&doogat);
            }
        }));
        prop_assert!(result.is_ok(), "panicked on mixed-type extras");
    }
}

// ---------------------------------------------------------------------------
// Collision detection properties (sync_manager)
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3))]

    /// Two nodes each create 3-5 doogats with identical IDs. After sync,
    /// no duplicate IDs exist — every doogat has a unique ID.
    #[test]
    fn concurrent_creation_no_duplicate_ids(count in 3usize..=5) {
        use ddb_core::git_ops::GitRepo;
        use ddb_core::indexer::Index;
        use ddb_core::sync_manager::{self, SyncManager};
        use std::collections::HashSet;

        // Bare remote
        let bare_dir = tempfile::TempDir::new().unwrap();
        git2::Repository::init_bare(bare_dir.path()).unwrap();

        // Node A
        let dir_a = tempfile::TempDir::new().unwrap();
        let repo_a = GitRepo::init(dir_a.path()).unwrap();
        repo_a.add_remote("origin", bare_dir.path().to_str().unwrap()).unwrap();
        repo_a.push("origin", "master").unwrap();
        sync_manager::register_node(&repo_a, "NodeA").unwrap();
        repo_a.push("origin", "master").unwrap();

        // Node B (clone)
        let dir_b = tempfile::TempDir::new().unwrap();
        git2::Repository::clone(bare_dir.path().to_str().unwrap(), dir_b.path()).unwrap();
        let repo_b = GitRepo::open(dir_b.path()).unwrap();
        sync_manager::register_node(&repo_b, "NodeB").unwrap();
        repo_b.push("origin", "master").unwrap();

        // Sync A to get B's node file
        repo_a.fetch("origin", "master").unwrap();
        repo_a.merge_remote("origin", "master").unwrap();

        // Both nodes create `count` doogats with identical IDs
        let base_ids: Vec<String> = (0..count)
            .map(|i| format!("2026010112{:04}", i))
            .collect();

        for id in &base_ids {
            let content_a = format!(
                "---\nid: {id}\ntitle: From A {id}\ndate: 2026-01-01\n---\nBody A {id}\n"
            );
            repo_a.commit_file(
                &format!("ddb/{id}.md"),
                &content_a,
                &format!("A creates {id}"),
            ).unwrap();

            let content_b = format!(
                "---\nid: {id}\ntitle: From B {id}\ndate: 2026-01-01\n---\nBody B {id}\n"
            );
            repo_b.commit_file(
                &format!("ddb/{id}.md"),
                &content_b,
                &format!("B creates {id}"),
            ).unwrap();
        }

        // A pushes
        repo_a.push("origin", "master").unwrap();

        // B syncs (triggers collision detection)
        let db_b = dir_b.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
        let index_b = Index::open(&db_b).unwrap();
        let mut mgr_b = SyncManager::open(&repo_b).unwrap();
        let report = mgr_b.sync("origin", "master", &index_b).unwrap();

        prop_assert_eq!(
            report.collisions_reassigned, count,
            "should reassign all {} collisions", count
        );

        // Collect all doogat IDs from B's repo
        let zk_dir = dir_b.path().join("ddb");
        let files: Vec<String> = std::fs::read_dir(&zk_dir)
            .unwrap()
            .filter_map(|e| {
                let e = e.unwrap();
                let name = e.file_name().to_string_lossy().to_string();
                if name.ends_with(".md") && !name.starts_with('_') {
                    Some(name.trim_end_matches(".md").to_string())
                } else {
                    None
                }
            })
            .collect();

        // Should have 2 * count doogats (each original ID + reassigned)
        prop_assert_eq!(
            files.len(), count * 2,
            "expected {} doogats, got {}: {:?}", count * 2, files.len(), files
        );

        // No duplicate IDs
        let unique: HashSet<&String> = files.iter().collect();
        prop_assert_eq!(
            unique.len(), files.len(),
            "duplicate IDs found: {:?}", files
        );

        // Verify each doogat file has a valid id in frontmatter matching its filename
        for filename_id in &files {
            let path = format!("ddb/{filename_id}.md");
            let content = repo_b.read_file(&path).unwrap();
            prop_assert!(
                content.contains(&format!("id: {filename_id}")),
                "doogat {filename_id} frontmatter id mismatch: {content}"
            );
        }

        // Sync back to A and verify convergence
        repo_b.push("origin", "master").unwrap();
        let db_a = dir_a.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_a.parent().unwrap()).unwrap();
        let index_a = Index::open(&db_a).unwrap();
        let mut mgr_a = SyncManager::open(&repo_a).unwrap();
        mgr_a.sync("origin", "master", &index_a).unwrap();

        // A should have same files as B
        let files_a: Vec<String> = std::fs::read_dir(dir_a.path().join("ddb"))
            .unwrap()
            .filter_map(|e| {
                let e = e.unwrap();
                let name = e.file_name().to_string_lossy().to_string();
                if name.ends_with(".md") && !name.starts_with('_') {
                    Some(name.trim_end_matches(".md").to_string())
                } else {
                    None
                }
            })
            .collect();

        let mut sorted_a = files_a.clone();
        sorted_a.sort();
        let mut sorted_b = files.clone();
        sorted_b.sort();
        prop_assert_eq!(
            sorted_a, sorted_b,
            "nodes A and B should converge to same doogat set"
        );
    }
}

// ---------------------------------------------------------------------------
// PRD 00121: search query parse/normalize/search alignment
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// Strong property: if a query parses cleanly, search() accepts both the
    /// query and its normalized form. Locks in PRD 00121 alignment.
    #[test]
    fn search_accepts_parseable_queries(q in arb_search_query_valid()) {
        prop_assume!(ddb_core::search_query::parse(&q).is_some());

        let idx = Index::open_in_memory().unwrap();
        let titles = [
            "meeting notes rust",
            "python experiments",
            "crdt merge strategy",
            "svelte bindings demo",
            "ddb roadmap",
        ];
        for (i, title) in titles.iter().enumerate() {
            let id = format!("2026040112{i:04}");
            let doogat = ddb_core::types::ParsedDoogat {
                meta: ddb_core::types::DoogatMeta {
                    id: Some(ddb_core::types::DoogatId(id.clone())),
                    title: Some((*title).to_string()),
                    date: Some("2026-04-01".into()),
                    doogat_type: Some("note".into()),
                    tags: vec!["rust".into(), "python".into()],
                    extra: Default::default(),
                },
                body: format!("Body of {title}"),
                sections: vec![],
                reference_section: String::new(),
                inline_fields: vec![],
                links: vec![],
                body_tags: vec![],
                checkboxes: vec![],
                path: format!("ddb/{id}.md"),
                updated_at: None,
            };
            idx.index_doogat(&doogat).unwrap();
        }

        let res_raw = idx.search(&q);
        prop_assert!(
            res_raw.is_ok(),
            "search({q:?}) returned Err: {:?}",
            res_raw.err()
        );

        let normalized = ddb_core::search_query::normalize(&q);
        let res_norm = idx.search(&normalized);
        prop_assert!(
            res_norm.is_ok(),
            "search(normalized={normalized:?}) returned Err: {:?} (original: {q:?})",
            res_norm.err()
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// Weak property: for any input, search() never returns DoogatError::Sql
    /// (which the server redacts to "internal error"). User input must always
    /// be classified as BadRequest on failure.
    #[test]
    fn search_never_returns_sql_error_for_arbitrary_input(q in ".{0,60}") {
        let idx = Index::open_in_memory().unwrap();
        let doogat = ddb_core::types::ParsedDoogat {
            meta: ddb_core::types::DoogatMeta {
                id: Some(ddb_core::types::DoogatId("20260401120000".into())),
                title: Some("probe".into()),
                date: Some("2026-04-01".into()),
                doogat_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: "probe body".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "ddb/20260401120000.md".into(),
            updated_at: None,
        };
        idx.index_doogat(&doogat).unwrap();

        match idx.search(&q) {
            Ok(_) => {}
            Err(ddb_core::error::DoogatError::BadRequest(_)) => {}
            Err(other) => prop_assert!(
                false,
                "search({q:?}) returned unexpected error variant: {other:?}"
            ),
        }
    }
}
