use super::*;

#[test]
fn strip_wikilink_cases() {
    assert_eq!(strip_wikilink("[[20260226120000]]"), "20260226120000");
    assert_eq!(strip_wikilink("[[people/jane|Jane Doe]]"), "people/jane");
    assert_eq!(strip_wikilink("Wikipedia"), "Wikipedia");
    assert_eq!(strip_wikilink(""), "");
    assert_eq!(strip_wikilink("  [[spaced]]  "), "spaced");
    // Malformed: only opening bracket
    assert_eq!(strip_wikilink("[[broken"), "[[broken");
}

#[test]
fn basic_three_zone_split() {
    let content = "\
---
title: Test
---
Body content here.

Some more body.
---
- source:: Wikipedia
- tags:: test";

    let z = split_zones(content).unwrap();
    assert_eq!(z.raw_frontmatter, "title: Test");
    assert!(z.body.contains("Body content here."));
    assert!(z.body.contains("Some more body."));
    assert!(z.reference_section.contains("- source:: Wikipedia"));
    assert!(z.reference_section.contains("- tags:: test"));
}

#[test]
fn no_reference_section() {
    let content = "\
---
title: Test
---
Just body content.";

    let z = split_zones(content).unwrap();
    assert_eq!(z.raw_frontmatter, "title: Test");
    assert_eq!(z.body, "Just body content.");
    assert!(z.reference_section.is_empty());
}

#[test]
fn code_block_with_separator() {
    let content = "\
---
title: Test
---
Before code.

```
---
this is not a separator
---
```

After code.";

    let z = split_zones(content).unwrap();
    assert!(z.body.contains("---"));
    assert!(z.body.contains("this is not a separator"));
    assert!(z.reference_section.is_empty());
}

#[test]
fn thematic_break_not_reference_boundary() {
    let content = "\
---
title: Test
---
Paragraph one.

---

Paragraph two with no reference fields.";

    let z = split_zones(content).unwrap();
    // The `---` in the body is a thematic break, not a reference boundary
    // because the content after it doesn't match `- key:: value`
    assert!(z.body.contains("Paragraph one."));
    assert!(z.body.contains("Paragraph two"));
    assert!(z.reference_section.is_empty());
}

#[test]
fn trailing_separator_after_reference_backtracks() {
    let content = "\
---
title: Test
---
Body here.
---
- source:: Wikipedia
---
";

    let z = split_zones(content).unwrap();
    // Last `---` has only whitespace/empty after it → backtrack to previous `---`
    assert_eq!(z.body, "Body here.");
    assert!(z.reference_section.contains("- source:: Wikipedia"));
}

#[test]
fn empty_after_last_separator_backtracks() {
    let content = "\
---
title: Test
---
Body here.
---
";

    let z = split_zones(content).unwrap();
    // Last `---` has nothing after it → backtrack, no valid reference boundary
    assert!(z.body.contains("Body here."));
    assert!(z.reference_section.is_empty());
}

// -- frontmatter parsing tests --

use crate::types::Zone;

#[test]
fn frontmatter_all_fields() {
    let yaml = "id: 20260226120000\ntitle: My Note\ndate: 2026-02-26\ntype: permanent\ntags:\n  - test\n  - demo";
    let meta = parse_frontmatter(yaml, "20260226120000.md").unwrap();
    assert_eq!(meta.id, Some(DoogatId("20260226120000".into())));
    assert_eq!(meta.title.as_deref(), Some("My Note"));
    assert_eq!(meta.date.as_deref(), Some("2026-02-26"));
    assert_eq!(meta.doogat_type.as_deref(), Some("permanent"));
    assert_eq!(meta.tags, vec!["test", "demo"]);
}

#[test]
fn frontmatter_empty() {
    let meta = parse_frontmatter("", "20260226120000.md").unwrap();
    assert_eq!(meta.id, Some(DoogatId("20260226120000".into())));
    assert!(meta.title.is_none());
    assert!(meta.tags.is_empty());
}

#[test]
fn frontmatter_extra_fields_preserved() {
    let yaml = "title: Test\ncustom_field: hello\nanother: 42";
    let meta = parse_frontmatter(yaml, "note.md").unwrap();
    assert_eq!(meta.title.as_deref(), Some("Test"));
    assert!(meta.extra.contains_key("custom_field"));
    assert!(meta.extra.contains_key("another"));
}

#[test]
fn frontmatter_id_fallback_from_filename() {
    let yaml = "title: No ID here";
    let meta = parse_frontmatter(yaml, "ddb/20260226130000.md").unwrap();
    assert_eq!(meta.id, Some(DoogatId("20260226130000".into())));
}

#[test]
fn frontmatter_id_fallback_rejects_short_numeric_stem() {
    let meta = parse_frontmatter("", "ddb/123.md").unwrap();
    assert!(meta.id.is_none());
}

#[test]
fn frontmatter_id_fallback_rejects_long_numeric_stem() {
    let meta = parse_frontmatter("", "ddb/202602261300009.md").unwrap();
    assert!(meta.id.is_none());
}

#[test]
fn frontmatter_id_fallback_rejects_non_numeric_stem() {
    let meta = parse_frontmatter("", "note.md").unwrap();
    assert!(meta.id.is_none());
}

#[test]
fn frontmatter_explicit_id_overrides_stem_fallback() {
    let yaml = "id: 20260226120000";
    let meta = parse_frontmatter(yaml, "ddb/123.md").unwrap();
    assert_eq!(meta.id, Some(DoogatId("20260226120000".into())));
}

// -- frontmatter byte cap tests --

#[test]
fn frontmatter_at_exactly_the_byte_cap_still_parses() {
    let prefix = "title: ";
    let pad_len = MAX_FRONTMATTER_BYTES - prefix.len();
    let yaml = format!("{prefix}{}", "a".repeat(pad_len));
    assert_eq!(yaml.len(), MAX_FRONTMATTER_BYTES);

    let meta = parse_frontmatter(&yaml, "note.md")
        .expect("frontmatter exactly at the cap must still parse");
    assert_eq!(meta.title.as_deref(), Some("a".repeat(pad_len).as_str()));
}

#[test]
fn frontmatter_one_byte_over_the_cap_fails() {
    let prefix = "title: ";
    let pad_len = MAX_FRONTMATTER_BYTES - prefix.len() + 1;
    let yaml = format!("{prefix}{}", "a".repeat(pad_len));
    assert_eq!(yaml.len(), MAX_FRONTMATTER_BYTES + 1);

    let result = parse_frontmatter(&yaml, "note.md");
    assert!(
        result.is_err(),
        "frontmatter one byte over the cap must be rejected"
    );
}

#[test]
fn frontmatter_over_cap_error_states_cap_and_actual_size() {
    let prefix = "title: ";
    let pad_len = MAX_FRONTMATTER_BYTES - prefix.len() + 1;
    let yaml = format!("{prefix}{}", "a".repeat(pad_len));
    let actual_len = yaml.len();
    assert_eq!(actual_len, MAX_FRONTMATTER_BYTES + 1);

    let err = parse_frontmatter(&yaml, "note.md")
        .expect_err("over-cap frontmatter must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains(&MAX_FRONTMATTER_BYTES.to_string()),
        "error must state the configured cap ({MAX_FRONTMATTER_BYTES}), got: {msg}"
    );
    assert!(
        msg.contains(&actual_len.to_string()),
        "error must state the actual frontmatter size ({actual_len}), got: {msg}"
    );
}

// -- inline field extraction tests --

#[test]
fn inline_fields_body_only() {
    let fields = extract_inline_fields("source:: Wikipedia\nstatus:: draft", "").unwrap();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].key, "source");
    assert_eq!(fields[0].value, "Wikipedia");
    assert_eq!(fields[0].zone, Zone::Body);
}

#[test]
fn inline_fields_reference_only() {
    let fields = extract_inline_fields("", "- source:: Wikipedia\n- tags:: test").unwrap();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].zone, Zone::Reference);
    assert_eq!(fields[1].key, "tags");
}

#[test]
fn inline_fields_mixed() {
    let fields = extract_inline_fields("status:: draft", "- source:: Wikipedia").unwrap();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].zone, Zone::Body);
    assert_eq!(fields[1].zone, Zone::Reference);
}

#[test]
fn inline_fields_empty_reference_value() {
    let fields = extract_inline_fields("", "- emptykey::").unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].key, "emptykey");
    assert_eq!(fields[0].value, "");
}

#[test]
fn inline_fields_reference_strips_wikilinks() {
    let fields = extract_inline_fields(
        "",
        "- related:: [[20260226120000]]\n- author:: [[people/jane|Jane Doe]]\n- source:: Wikipedia",
    )
    .unwrap();
    assert_eq!(fields.len(), 3);
    assert_eq!(fields[0].key, "related");
    assert_eq!(fields[0].value, "20260226120000");
    assert_eq!(fields[1].key, "author");
    assert_eq!(fields[1].value, "people/jane");
    assert_eq!(fields[2].key, "source");
    assert_eq!(fields[2].value, "Wikipedia");
}

#[test]
fn inline_fields_cross_zone_duplicate_errors() {
    let result = extract_inline_fields("source:: Body Version", "- source:: Ref Version");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("duplicate inline field 'source'"));
}

#[test]
fn inline_fields_same_zone_duplicate_first_wins() {
    let fields = extract_inline_fields("source:: First\nsource:: Second", "").unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].value, "First");
}

#[test]
fn inline_fields_skip_fenced_code_block() {
    let body = "status:: draft\n```\nsource:: Wikipedia\n```\nvisible:: yes";
    let fields = extract_inline_fields(body, "").unwrap();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].key, "status");
    assert_eq!(fields[1].key, "visible");
}

#[test]
fn inline_fields_skip_tilde_fenced_code_block() {
    let body = "status:: draft\n~~~\nsource:: Wikipedia\n~~~\nvisible:: yes";
    let fields = extract_inline_fields(body, "").unwrap();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].key, "status");
    assert_eq!(fields[1].key, "visible");
}

#[test]
fn inline_fields_skip_inline_code() {
    let body = "some `key:: value` text\nreal:: field";
    let fields = extract_inline_fields(body, "").unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].key, "real");
}

#[test]
fn inline_fields_normal_next_to_inline_code() {
    let body = "status:: draft with `some code` here";
    let fields = extract_inline_fields(body, "").unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].key, "status");
}

// -- wikilink extraction tests --

#[test]
fn extract_all_link_types() {
    let body = "See [[wiki]] and [md link](path.md). Also ![[embed#sec|alt]]. Visit https://example.com for info.";
    let links = extract_links("", body, "");
    assert_eq!(links.len(), 4);

    let kinds: Vec<_> = links.iter().map(|l| &l.kind).collect();
    assert!(kinds.contains(&&crate::types::LinkKind::Embed));
    assert!(kinds.contains(&&crate::types::LinkKind::WikiLink));
    assert!(kinds.contains(&&crate::types::LinkKind::MarkdownLink));
    assert!(kinds.contains(&&crate::types::LinkKind::BareUrl));
}

#[test]
fn embed_not_double_counted_as_wikilink() {
    let body = "![[embed_target]]";
    let links = extract_links("", body, "");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].kind, crate::types::LinkKind::Embed);
    assert_eq!(links[0].target, "embed_target");
}

#[test]
fn extract_bare_url_basic() {
    let links = extract_bare_urls("Visit https://example.com for info", Zone::Body, &[]);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "https://example.com");
    assert_eq!(links[0].kind, crate::types::LinkKind::BareUrl);
    assert!(links[0].display.is_none());
}

#[test]
fn bare_url_trailing_punct_trimmed() {
    let links = extract_bare_urls("See https://example.com.", Zone::Body, &[]);
    assert_eq!(links[0].target, "https://example.com");

    let links = extract_bare_urls("See https://example.com,", Zone::Body, &[]);
    assert_eq!(links[0].target, "https://example.com");
}

#[test]
fn bare_url_not_double_counted() {
    let links = extract_bare_urls(
        "Check https://example.com for details",
        Zone::Body,
        &["https://example.com"],
    );
    assert!(links.is_empty());
}

#[test]
fn bare_url_in_code_block_skipped() {
    let text = "```\nhttps://hidden.com\n```\nhttps://visible.com";
    let links = extract_bare_urls(text, Zone::Body, &[]);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "https://visible.com");
}

#[test]
fn extract_markdown_link_basic() {
    let links = extract_markdown_links("[My Page](some/page.md)", Zone::Body);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "some/page.md");
    assert_eq!(links[0].display.as_deref(), Some("My Page"));
    assert_eq!(links[0].kind, crate::types::LinkKind::MarkdownLink);
}

#[test]
fn extract_markdown_link_external() {
    let links = extract_markdown_links("[Google](https://google.com)", Zone::Body);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "https://google.com");
}

#[test]
fn markdown_link_in_code_block_skipped() {
    let text = "```\n[hidden](path)\n```\n[visible](other)";
    let links = extract_markdown_links(text, Zone::Body);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "other");
}

#[test]
fn extract_embed_basic() {
    let links = extract_embeds("![[myfile]]", Zone::Body);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "myfile");
    assert!(links[0].section.is_none());
    assert!(links[0].display.is_none());
    assert_eq!(links[0].kind, crate::types::LinkKind::Embed);
}

#[test]
fn extract_embed_with_section() {
    let links = extract_embeds("![[file#heading]]", Zone::Body);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "file");
    assert_eq!(links[0].section.as_deref(), Some("heading"));
    assert!(links[0].display.is_none());
}

#[test]
fn extract_embed_with_display() {
    let links = extract_embeds("![[file|alt text]]", Zone::Body);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "file");
    assert_eq!(links[0].display.as_deref(), Some("alt text"));
}

#[test]
fn extract_embed_full() {
    let links = extract_embeds("![[file#section|display]]", Zone::Body);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "file");
    assert_eq!(links[0].section.as_deref(), Some("section"));
    assert_eq!(links[0].display.as_deref(), Some("display"));
}

#[test]
fn embed_in_code_block_skipped() {
    let text = "before\n```\n![[inside]]\n```\n![[outside]]";
    let links = extract_embeds(text, Zone::Body);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "outside");
}

#[test]
fn wikilinks_body() {
    let links = extract_wikilinks("", "See [[some/note]] and [[other|Other Note]].", "");
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].target, "some/note");
    assert!(links[0].display.is_none());
    assert_eq!(links[1].target, "other");
    assert_eq!(links[1].display.as_deref(), Some("Other Note"));
}

#[test]
fn wikilinks_reference() {
    let links = extract_wikilinks("", "", "- related:: [[20260226120000]]");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].zone, Zone::Reference);
}

#[test]
fn wikilinks_frontmatter() {
    let links = extract_wikilinks("related: \"[[20260226120000|My Note]]\"", "", "");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].zone, Zone::Frontmatter);
    assert_eq!(links[0].display.as_deref(), Some("My Note"));
}

// -- wikilink rewriting tests --

#[test]
fn rewrite_wikilinks_bare() {
    let content = "See [[old_target]] here.";
    let result = rewrite_wikilinks(content, "old_target", "new_target");
    assert_eq!(result, "See [[new_target]] here.");
}

#[test]
fn rewrite_wikilinks_with_display() {
    let content = "Link: [[old_target|Display Name]]";
    let result = rewrite_wikilinks(content, "old_target", "new_target");
    assert_eq!(result, "Link: [[new_target|Display Name]]");
}

#[test]
fn rewrite_wikilinks_yaml_quoted() {
    let content = "related: \"[[old_target|Name]]\"";
    let result = rewrite_wikilinks(content, "old_target", "new_target");
    assert_eq!(result, "related: \"[[new_target|Name]]\"");
}

#[test]
fn rewrite_wikilinks_reference_section() {
    let content = "- related:: [[old_target]]";
    let result = rewrite_wikilinks(content, "old_target", "new_target");
    assert_eq!(result, "- related:: [[new_target]]");
}

#[test]
fn rewrite_wikilinks_multiple_occurrences() {
    let content = "First [[old_target]] then [[old_target|Alt]] and [[other]]";
    let result = rewrite_wikilinks(content, "old_target", "new_target");
    assert_eq!(
        result,
        "First [[new_target]] then [[new_target|Alt]] and [[other]]"
    );
}

#[test]
fn rewrite_wikilinks_no_match() {
    let content = "Nothing to change [[unrelated]]";
    let result = rewrite_wikilinks(content, "old_target", "new_target");
    assert_eq!(result, "Nothing to change [[unrelated]]");
}

#[test]
fn rewrite_wikilinks_path_qualified() {
    let content = "See [[ddb/20260301120000]]";
    let result = rewrite_wikilinks(content, "ddb/20260301120000", "ddb/contact/20260301120000");
    assert_eq!(result, "See [[ddb/contact/20260301120000]]");
}

#[test]
fn rewrite_links_markdown() {
    let content = "See [my page](old_path) for details";
    let result = rewrite_links(content, "old_path", "new_path");
    assert_eq!(result, "See [my page](new_path) for details");
}

#[test]
fn rewrite_links_embeds() {
    let content = "![[old_target]] and ![[old_target#section|display]]";
    let result = rewrite_links(content, "old_target", "new_target");
    assert_eq!(
        result,
        "![[new_target]] and ![[new_target#section|display]]"
    );
}

#[test]
fn rewrite_links_skips_bare_urls() {
    let content = "See https://example.com and [[old_target]]";
    let result = rewrite_links(content, "https://example.com", "https://other.com");
    // Bare URLs are never rewritten (external), but wikilink matching the URL would be
    // In practice URLs never appear as wikilink targets
    assert!(result.contains("https://example.com"));
}

#[test]
fn rewrite_links_mixed() {
    let content = "[[old]] and [title](old) and ![[old]]";
    let result = rewrite_links(content, "old", "new");
    assert_eq!(result, "[[new]] and [title](new) and ![[new]]");
}

// -- checkbox tests --

#[test]
fn sections_basic() {
    let body = "## A\ncontent a\n## B\ncontent b";
    let secs = extract_sections(body);
    assert_eq!(secs.len(), 2);
    assert_eq!(secs[0].heading, "A");
    assert_eq!(secs[0].level, 2);
    assert_eq!(secs[0].content, "content a");
    assert_eq!(secs[1].heading, "B");
    assert_eq!(secs[1].content, "content b");
}

#[test]
fn sections_nested_levels() {
    let body = "# H1\n## H2\n### H3";
    let secs = extract_sections(body);
    assert_eq!(secs.len(), 3);
    assert_eq!(secs[0].level, 1);
    assert_eq!(secs[1].level, 2);
    assert_eq!(secs[2].level, 3);
}

#[test]
fn sections_pre_heading_content() {
    let body = "intro text\n\n## Section\ncontent";
    let secs = extract_sections(body);
    assert_eq!(secs.len(), 2);
    assert_eq!(secs[0].level, 0);
    assert_eq!(secs[0].heading, "");
    assert_eq!(secs[0].content, "intro text\n");
    assert_eq!(secs[1].heading, "Section");
}

#[test]
fn sections_skip_fenced_code() {
    let body = "## Real\ncontent\n```\n## Fake\n```\n## Also Real\nmore";
    let secs = extract_sections(body);
    assert_eq!(secs.len(), 2);
    assert_eq!(secs[0].heading, "Real");
    assert!(secs[0].content.contains("## Fake"));
    assert_eq!(secs[1].heading, "Also Real");
}

#[test]
fn sections_trailing_hashes() {
    let body = "## Title ##\ncontent";
    let secs = extract_sections(body);
    assert_eq!(secs.len(), 1);
    assert_eq!(secs[0].heading, "Title");
}

#[test]
fn sections_empty_body() {
    let secs = extract_sections("");
    assert!(secs.is_empty());
}

#[test]
fn sections_heading_only() {
    let body = "## Heading";
    let secs = extract_sections(body);
    assert_eq!(secs.len(), 1);
    assert_eq!(secs[0].heading, "Heading");
    assert_eq!(secs[0].content, "");
}

#[test]
fn sections_preserves_content_whitespace() {
    let body = "## Section\n\nline 1\n\nline 2";
    let secs = extract_sections(body);
    assert_eq!(secs.len(), 1);
    assert!(secs[0].content.contains("line 1\n\nline 2"));
}

#[test]
fn checkboxes_all_states() {
    let body = "- [ ] open item\n- [x] done item\n- [i] info item";
    let cbs = extract_checkboxes(body);
    assert_eq!(cbs.len(), 3);
    assert_eq!(cbs[0].state, crate::types::CheckboxState::Open);
    assert_eq!(cbs[0].content, "open item");
    assert_eq!(cbs[1].state, crate::types::CheckboxState::Done);
    assert_eq!(cbs[1].content, "done item");
    assert_eq!(cbs[2].state, crate::types::CheckboxState::Info);
    assert_eq!(cbs[2].content, "info item");
}

#[test]
fn checkboxes_date_prefix() {
    let body = "- [i] 2026-02-20 20:54 - Issue created: ENG-1234";
    let cbs = extract_checkboxes(body);
    assert_eq!(cbs.len(), 1);
    assert_eq!(cbs[0].date, Some("2026-02-20 20:54".into()));
    assert_eq!(cbs[0].content, "Issue created: ENG-1234");
}

#[test]
fn checkboxes_due_date() {
    let body = "- [ ] Do the thing ⏳ 2026-03-20";
    let cbs = extract_checkboxes(body);
    assert_eq!(cbs.len(), 1);
    assert_eq!(cbs[0].due_date, Some("2026-03-20".into()));
    assert_eq!(cbs[0].content, "Do the thing");
}

#[test]
fn checkboxes_skip_code_block() {
    let body = "- [ ] real\n```\n- [ ] fake\n```\n- [x] also real";
    let cbs = extract_checkboxes(body);
    assert_eq!(cbs.len(), 2);
    assert_eq!(cbs[0].content, "real");
    assert_eq!(cbs[1].content, "also real");
}

#[test]
fn checkboxes_line_numbers() {
    let body = "Some text\n- [ ] first\nmore text\n- [x] second";
    let cbs = extract_checkboxes(body);
    assert_eq!(cbs.len(), 2);
    assert_eq!(cbs[0].line_number, 2);
    assert_eq!(cbs[1].line_number, 4);
}

#[test]
fn checkboxes_indent_level() {
    let body = "- [ ] parent\n  - [ ] child\n    - [x] grandchild";
    let cbs = extract_checkboxes(body);
    assert_eq!(cbs.len(), 3);
    assert_eq!(cbs[0].indent_level, 0);
    assert_eq!(cbs[0].content, "parent");
    assert_eq!(cbs[1].indent_level, 2);
    assert_eq!(cbs[1].content, "child");
    assert_eq!(cbs[2].indent_level, 4);
    assert_eq!(cbs[2].content, "grandchild");
}

// -- hashtag tests --

#[test]
fn hashtags_basic() {
    let body = "Some text #gtd/act/next and more";
    let tags = extract_hashtags(body);
    assert_eq!(tags, vec!["gtd/act/next"]);
}

#[test]
fn hashtags_hierarchical() {
    let body = "Tagged #client/100-acme-corp here";
    let tags = extract_hashtags(body);
    assert_eq!(tags, vec!["client/100-acme-corp"]);
}

#[test]
fn hashtags_skip_fenced_code() {
    let body = "Before\n```\n#not-a-tag\n```\nAfter #real-tag";
    let tags = extract_hashtags(body);
    assert_eq!(tags, vec!["real-tag"]);
}

#[test]
fn hashtags_skip_inline_code() {
    let body = "See `#not-a-tag` but #real-tag";
    let tags = extract_hashtags(body);
    assert_eq!(tags, vec!["real-tag"]);
}

#[test]
fn hashtags_skip_wikilinks() {
    let body = "Link [[#heading]] and #real-tag";
    let tags = extract_hashtags(body);
    assert_eq!(tags, vec!["real-tag"]);
}

#[test]
fn hashtags_dedup() {
    let body = "#duplicate and #duplicate again";
    let tags = extract_hashtags(body);
    assert_eq!(tags, vec!["duplicate"]);
}

#[test]
fn hashtags_line_start_and_mid() {
    let body = "#start-tag\nsome #mid-tag text";
    let tags = extract_hashtags(body);
    assert_eq!(tags, vec!["start-tag", "mid-tag"]);
}

#[test]
fn hashtags_not_in_urls() {
    let body = "Visit https://example.com#section for info";
    let tags = extract_hashtags(body);
    assert!(
        tags.is_empty(),
        "URL fragments should not be extracted: {tags:?}"
    );
}

#[test]
fn hashtags_whitespace_required() {
    let body = "word#not-a-tag but #real-tag";
    let tags = extract_hashtags(body);
    assert_eq!(tags, vec!["real-tag"]);
}

// -- serialization tests --

#[test]
fn serialize_round_trip() {
    let content = "\
---
id: 20260226120000
title: Test Note
date: 2026-02-26
type: permanent
tags:
  - test
  - demo
---
Body content here.

Some more body.
---
- source:: Wikipedia
- tags:: test";

    let z = split_zones(content).unwrap();
    let meta = parse_frontmatter(&z.raw_frontmatter, "20260226120000.md").unwrap();
    let inline_fields = extract_inline_fields(&z.body, &z.reference_section).unwrap();
    let wikilinks = extract_wikilinks(&z.raw_frontmatter, &z.body, &z.reference_section);

    let parsed = crate::types::ParsedDoogat {
        meta,
        body: z.body.clone(),
        sections: vec![],
        reference_section: z.reference_section.clone(),
        inline_fields,
        links: wikilinks,
        body_tags: vec![],
        checkboxes: vec![],
        path: "20260226120000.md".into(),
        updated_at: None,
    };

    let serialized = serialize(&parsed);

    // Re-parse and verify equivalence
    let z2 = split_zones(&serialized).unwrap();
    let meta2 = parse_frontmatter(&z2.raw_frontmatter, "20260226120000.md").unwrap();
    assert_eq!(meta2.id, parsed.meta.id);
    assert_eq!(meta2.title, parsed.meta.title);
    assert_eq!(meta2.tags, parsed.meta.tags);
    assert!(z2.body.contains("Body content here."));
    assert!(z2.reference_section.contains("- source:: Wikipedia"));
}

#[test]
fn serialize_no_reference_section() {
    let parsed = crate::types::ParsedDoogat {
        meta: DoogatMeta {
            id: Some(DoogatId("20260226120000".into())),
            title: Some("Test".into()),
            ..Default::default()
        },
        body: "Just body.".into(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: "test.md".into(),
        updated_at: None,
    };

    let serialized = serialize(&parsed);
    assert!(serialized.contains("title: Test"));
    assert!(serialized.contains("Just body."));
    // Should not have trailing ---
    assert_eq!(serialized.matches("---").count(), 2);
}

#[test]
fn serialize_canonical_yaml_key_ordering() {
    let mut extra = std::collections::BTreeMap::new();
    extra.insert("zeta".to_string(), Value::String("last".into()));
    extra.insert("publish".to_string(), Value::Bool(true));
    extra.insert("alpha".to_string(), Value::String("first".into()));
    extra.insert("processed".to_string(), Value::Bool(false));

    let parsed = crate::types::ParsedDoogat {
        meta: DoogatMeta {
            id: Some(DoogatId("20260226120000".into())),
            title: Some("Ordering Test".into()),
            date: Some("2026-02-26".into()),
            tags: vec!["test".into(), "order".into()],
            doogat_type: Some("permanent".into()),
            extra,
        },
        body: "Body.".into(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: "test.md".into(),
        updated_at: None,
    };

    let serialized = serialize(&parsed);
    let lines: Vec<&str> = serialized.lines().collect();

    let tags_idx = lines.iter().position(|l| *l == "tags:").unwrap();
    let type_idx = lines.iter().position(|l| *l == "type: permanent").unwrap();
    let publish_idx = lines.iter().position(|l| *l == "publish: true").unwrap();
    let processed_idx = lines.iter().position(|l| *l == "processed: false").unwrap();
    let alpha_idx = lines.iter().position(|l| *l == "alpha: first").unwrap();
    let zeta_idx = lines.iter().position(|l| *l == "zeta: last").unwrap();

    assert!(tags_idx < type_idx);
    assert!(type_idx < publish_idx);
    assert!(type_idx < processed_idx);
    assert!(publish_idx < processed_idx);
    assert!(processed_idx < alpha_idx);
    assert!(alpha_idx < zeta_idx);
}

// -- rewrite_id_field tests --

#[test]
fn rewrite_id_field_replaces_existing_id() {
    let content = "\
---
id: 20260226120000
title: Test Note
---
Body content.";

    let result = rewrite_id_field(content, "20260301120000").unwrap();

    let reparsed = parse(&result, "collision-loser").unwrap();
    assert_eq!(reparsed.meta.id, Some(DoogatId("20260301120000".into())));
}

#[test]
fn rewrite_id_field_sets_id_when_absent() {
    let content = "\
---
title: No Id Note
---
Body content.";

    let result = rewrite_id_field(content, "20260301120000").unwrap();

    let reparsed = parse(&result, "collision-loser").unwrap();
    assert_eq!(reparsed.meta.id, Some(DoogatId("20260301120000".into())));
}

#[test]
fn rewrite_id_field_preserves_everything_else() {
    let content = "\
---
id: 20260226120000
title: Keep Me
date: 2026-02-26
type: permanent
tags:
  - test
  - demo
custom_field: hello
---
Body content here.

Some more body.
---
- source:: Wikipedia
- tags:: test";

    let result = rewrite_id_field(content, "20260301120000").unwrap();

    let reparsed = parse(&result, "collision-loser").unwrap();
    assert_eq!(reparsed.meta.id, Some(DoogatId("20260301120000".into())));
    assert_eq!(reparsed.meta.title.as_deref(), Some("Keep Me"));
    assert_eq!(reparsed.meta.date.as_deref(), Some("2026-02-26"));
    assert_eq!(reparsed.meta.doogat_type.as_deref(), Some("permanent"));
    assert_eq!(reparsed.meta.tags, vec!["test", "demo"]);
    assert!(reparsed.body.contains("Body content here."));
    assert!(reparsed.body.contains("Some more body."));
    assert!(reparsed.reference_section.contains("- source:: Wikipedia"));
    assert!(reparsed.reference_section.contains("- tags:: test"));
    assert!(
        result.contains("custom_field: hello"),
        "custom extra field should survive unchanged: {result}"
    );
}

#[test]
fn rewrite_id_field_propagates_parse_error() {
    let content = "No frontmatter delimiters at all, just plain text.";
    let result = rewrite_id_field(content, "20260301120000");
    assert!(result.is_err());
}

// -- top-level parse tests --

#[test]
fn parse_full_doogat() {
    let content = "\
---
id: 20260226120000
title: Full Note
date: 2026-02-26
type: permanent
tags:
  - test
---
Body with [[some/link|Link]] and source:: Wikipedia
---
- related:: [[20260101000000]]";

    let p = parse(content, "ddb/20260226120000.md").unwrap();
    assert_eq!(p.meta.id, Some(DoogatId("20260226120000".into())));
    assert_eq!(p.meta.title.as_deref(), Some("Full Note"));
    assert_eq!(p.inline_fields.len(), 1); // related from ref (source:: not at line start)
    assert_eq!(p.links.len(), 2); // one in body, one in ref
}

#[test]
fn parse_minimal_doogat() {
    let content = "\
---
title: Minimal
---
Just body.";

    let p = parse(content, "ddb/20260226130000.md").unwrap();
    assert_eq!(p.meta.id, Some(DoogatId("20260226130000".into()))); // from filename
    assert!(p.reference_section.is_empty());
}

#[test]
fn parse_obsidian_passthrough() {
    let content = "\
---
title: Obsidian Test
---
Some text.

```dataview
TABLE file.ctime AS Created
FROM #notes
```

<% tp.date.now() %>

Body continues.";

    let p = parse(content, "test.md").unwrap();
    // Obsidian-specific syntax preserved verbatim
    let rt = serialize(&p);
    assert!(rt.contains("```dataview"));
    assert!(rt.contains("<% tp.date.now() %>"));
}

// -- ID generation tests --

#[test]
fn id_generation_14_digits() {
    let id = generate_id();
    let s = id.0.to_string();
    assert_eq!(s.len(), 14);
}

#[test]
fn id_generation_no_duplicates() {
    let a = generate_id();
    let b = generate_id();
    assert_ne!(a, b, "rapid consecutive calls must produce unique IDs");
    assert_eq!(b.0.len(), 14);
}

#[test]
fn yaml_canonical_special_chars_quoted() {
    let content = "\
---
id: 20260301120000
title: \"ticket: ENG-1234\"
date: 2026-03-01
tags:
  - alpha
  - beta
---
Body content.";

    let parsed = parse(content, "ddb/20260301120000.md").unwrap();
    let serialized = serialize(&parsed);

    // Title with colons must be double-quoted (core field uses yaml_quote directly)
    assert!(
        serialized.contains("title: \"ticket: ENG-1234\""),
        "colon-containing title should be double-quoted: {serialized}"
    );
    // Tags should be block-style list
    assert!(
        serialized.contains("tags:\n  - alpha\n  - beta"),
        "tags should use block-style list: {serialized}"
    );
    // Closing --- should be followed only by body content
    let fm_end = serialized.rfind("---\n").expect("should have closing ---");
    let after_fm = &serialized[fm_end + 4..];
    assert!(
        after_fm.starts_with("Body content."),
        "body should follow closing --- directly: {serialized}"
    );

    // Re-parse to verify the round-trip produces equivalent data
    let reparsed = parse(&serialized, "ddb/20260301120000.md").unwrap();
    assert_eq!(reparsed.meta.title, parsed.meta.title);
    assert_eq!(reparsed.meta.tags, parsed.meta.tags);

    // Also test wikilinks in title (another special-char scenario)
    let wikilink_content = "\
---
id: 20260301120001
title: \"See [[ddb/20260101120000|Foo]]\"
date: 2026-03-01
tags:
  - test
---
Body.";

    let parsed2 = parse(wikilink_content, "ddb/20260301120001.md").unwrap();
    let serialized2 = serialize(&parsed2);

    // Wikilink-containing title must be double-quoted
    assert!(
        serialized2.contains("\"See [[ddb/20260101120000|Foo]]\""),
        "wikilink title should be double-quoted: {serialized2}"
    );

    // Round-trip preserves title value
    let reparsed2 = parse(&serialized2, "ddb/20260301120001.md").unwrap();
    assert_eq!(reparsed2.meta.title, parsed2.meta.title);
}

#[test]
fn yaml_user_key_order_preserved_on_body_edit() {
    let content = "\
---
title: My Note
id: 20260301120000
type: permanent
date: 2026-03-01
---
Original body.
---
- source:: Wikipedia";

    let z = split_zones(content).unwrap();
    // Verify original key order: title before id, type before date
    let fm_lines: Vec<&str> = z.raw_frontmatter.lines().collect();
    let title_pos = fm_lines
        .iter()
        .position(|l| l.starts_with("title:"))
        .unwrap();
    let id_pos = fm_lines.iter().position(|l| l.starts_with("id:")).unwrap();
    let type_pos = fm_lines
        .iter()
        .position(|l| l.starts_with("type:"))
        .unwrap();
    let date_pos = fm_lines
        .iter()
        .position(|l| l.starts_with("date:"))
        .unwrap();
    assert!(
        title_pos < id_pos,
        "title should come before id in original"
    );
    assert!(
        type_pos < date_pos,
        "type should come before date in original"
    );

    // Modify body, keep frontmatter and references unchanged
    let modified_body = "Modified body content.";

    // Reassemble using original frontmatter (unchanged) + modified body + original references
    let reassembled = if z.reference_section.is_empty() {
        format!("---\n{}\n---\n{}", z.raw_frontmatter, modified_body)
    } else {
        format!(
            "---\n{}\n---\n{}\n---\n{}",
            z.raw_frontmatter, modified_body, z.reference_section
        )
    };

    // Re-split and verify frontmatter key order is preserved
    let z2 = split_zones(&reassembled).unwrap();
    let fm_lines2: Vec<&str> = z2.raw_frontmatter.lines().collect();
    let title_pos2 = fm_lines2
        .iter()
        .position(|l| l.starts_with("title:"))
        .unwrap();
    let id_pos2 = fm_lines2.iter().position(|l| l.starts_with("id:")).unwrap();
    let type_pos2 = fm_lines2
        .iter()
        .position(|l| l.starts_with("type:"))
        .unwrap();
    let date_pos2 = fm_lines2
        .iter()
        .position(|l| l.starts_with("date:"))
        .unwrap();
    assert!(
        title_pos2 < id_pos2,
        "title should still come before id after body edit"
    );
    assert!(
        type_pos2 < date_pos2,
        "type should still come before date after body edit"
    );

    // Verify body was actually modified
    assert_eq!(z2.body, modified_body);
    // Verify references survived
    assert!(z2.reference_section.contains("- source:: Wikipedia"));
}

#[test]
fn multi_value_reference_fields_preserved() {
    let content = "---\nid: '20260301120000'\ntitle: test\ntype: bookmark\n---\n\n---\n- category:: [[20260301120100]]\n- category:: [[20260301120101]]\n- category:: [[20260301120102]]\n";
    let parsed = parse(content, "ddb/20260301120000.md").unwrap();
    let cat_fields: Vec<_> = parsed
        .inline_fields
        .iter()
        .filter(|f| f.key == "category")
        .collect();
    assert_eq!(cat_fields.len(), 3);
    assert_eq!(cat_fields[0].value, "20260301120100");
    assert_eq!(cat_fields[1].value, "20260301120101");
    assert_eq!(cat_fields[2].value, "20260301120102");
    assert!(cat_fields.iter().all(|f| f.zone == Zone::Reference));
}

#[test]
fn body_zone_dedup_unchanged() {
    let body = "status:: active\nstatus:: inactive\n";
    let fields = extract_inline_fields(body, "").unwrap();
    let status_fields: Vec<_> = fields.iter().filter(|f| f.key == "status").collect();
    assert_eq!(status_fields.len(), 1, "body zone should still first-wins");
    assert_eq!(status_fields[0].value, "active");
}

#[test]
fn cross_zone_error_unchanged() {
    let body = "category:: something\n";
    let reference = "- category:: [[20260301120100]]\n";
    let result = extract_inline_fields(body, reference);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("duplicate inline field"));
}
