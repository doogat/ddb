# Doogat Format Reference

Every doogat is a Markdown file with three zones separated by `---`.

## Structure

```markdown
---
id: 20260226120000
title: Note Title
date: 2026-02-26
tags:
  - tag1
  - tag2
type: permanent
---
Body content goes here.

Inline fields in body: source:: Wikipedia

Wikilinks: [[20260101000000|Related Note]]
---
- reference-field:: value
- another-field:: another value
```

## Zone 1: Frontmatter (YAML)

Delimited by the first pair of `---` lines.

### Core Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | String | 14-digit timestamp (`YYYYMMDDHHmmss`). Auto-generated. |
| `title` | String | Note title |
| `date` | String | Creation date (`YYYY-MM-DD`) |
| `tags` | List | YAML list of tag strings |
| `type` | String | Doogat type (e.g., `permanent`, `literature`, `fleeting`) |

### Extra Fields

Any additional YAML fields are preserved through parse/serialize round-trips. Custom fields like `publish`, `processed`, `status`, etc. are all supported.

### Quoting

Values containing `:`, `[`, `]`, `{`, `}`, `#`, or `[[` are automatically double-quoted during serialization.

## Zone 2: Body (Markdown)

Everything between the frontmatter closing `---` and the reference section `---`.

Supports standard Markdown including:
- Headings, paragraphs, lists
- Code blocks (fenced with `` ``` `` or `~~~`)
- Links, images, emphasis
- Dataview-style inline fields
- Wikilinks
- Obsidian-specific syntax (dataview queries, Templater) — preserved verbatim

### Inline Fields

```text
key:: value
```

Must be at the start of a line. Lines inside fenced code blocks are ignored. Inline code (`` `key:: value` ``) is not extracted.

## Zone 3: Reference Section

Optional. Starts at the last `---` separator where all subsequent non-empty lines match the `- key:: value` pattern.

```markdown
---
- source:: Wikipedia
- author:: Bob
- related:: [[20260101000000]]
```

Values can be empty: `- emptykey::`.

### Detection Heuristic

The parser finds the reference boundary by:
1. Collecting all `---` positions after frontmatter (excluding those in code blocks)
2. Checking from last to first whether all non-empty lines match `- key:: value`
3. Backtracking past trailing `---` with only whitespace after them

A `---` followed by non-reference prose is treated as a thematic break in the body.

## Wikilinks

```text
[[target]]
[[target|Display Text]]
```

- `target` is typically a doogat ID (e.g., `20260226120000`)
- `display` is optional alternative text
- Work in all three zones (in frontmatter, must be inside quoted strings)
- Indexed for backlink queries

## Duplicate Field Rules

| Scenario | Behavior |
|----------|----------|
| Same key in body and reference | Error (cross-zone duplicate) |
| Same key twice in body | First occurrence wins |
| Same key twice in reference | First occurrence wins |

## ID Format

14-digit timestamp: `YYYYMMDDHHmmss`

Generated from the local system clock at creation time. Serves as both the unique identifier and the filename (e.g., `ddb/20260226120000.md`).

## Filename Convention

```text
ddb/{id}.md
```

ID-only filenames ensure wikilinks remain stable when titles change.
