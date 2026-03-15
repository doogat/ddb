# Parser

**Source**: `zdb-core/src/parser.rs` (726 lines)

The parser handles splitting Markdown into three zones, extracting metadata, and serializing back to Markdown. It's the largest module because the zettel format has several edge cases.

## Three-Zone Splitting

`split_zones(content) -> Result<Zettel>`

### Algorithm

1. Find the first `---` pair for frontmatter boundaries
2. Collect all `---` positions after frontmatter, skipping those inside fenced code blocks (`` ``` `` or `~~~`)
3. Try separators from last to first, looking for a valid reference boundary:
   - If all non-empty lines after a `---` match `- key:: value` pattern → reference boundary found
   - If only whitespace/empty lines follow → backtrack to previous `---`
   - If content doesn't match reference pattern → stop searching (it's a thematic break)

### Edge Cases

- **Code blocks**: `---` inside fenced code blocks is ignored
- **Thematic breaks**: A `---` in the body followed by prose is not a reference boundary
- **Trailing separators**: A `---` at the end with only whitespace after it is skipped
- **No reference section**: If no valid boundary found, the entire post-frontmatter content is body

## Frontmatter Parsing

`parse_frontmatter(yaml, path) -> Result<ZettelMeta>`

Deserializes YAML into `ZettelMeta`. If the `id` field is missing, falls back to extracting a numeric ID from the filename stem (e.g., `zettelkasten/20260226130000.md` → `ZettelId("20260226130000")`).

## Inline Field Extraction

`extract_inline_fields(body, reference) -> Result<Vec<InlineField>>`

### Patterns

- **Body fields**: `^([\w][\w\s-]*):: (.+)$` — one per line
- **Reference fields**: `^- ([\w][\w\s-]*):: ?(.*)$` — list-item format, value can be empty

### Exclusions

- Lines inside fenced code blocks (`` ``` `` toggle) are skipped
- Inline code (`` `...` ``) is stripped before regex matching, so `key:: value` inside backticks is not extracted

### Duplicate Handling

- **Cross-zone duplicate** (same key in body and reference): returns `Err(Validation(...))`
- **Same-zone duplicate**: first occurrence wins silently

## Wikilink Extraction

`extract_wikilinks(frontmatter, body, reference) -> Vec<WikiLink>`

Pattern: `\[\[([^\]|]+)(?:\|([^\]]+))?\]\]`

Extracts from all three zones. In frontmatter, wikilinks appear inside quoted YAML values (e.g., `related: "[[20260226120000|My Note]]"`).

## Hashtag Extraction

`extract_hashtags(body) -> Vec<String>`

Extracts `#tag` tokens from body text. Respects the same exclusion zones as inline fields (fenced code blocks, inline code spans) plus wikilinks. URL fragments (`https://example.com#section`) are excluded by checking for `://` before `#`. Returns unique tags without `#` prefix.

Pattern: `(?:^|\s)#([\w][\w/-]*)` — matches hierarchical tags like `#gtd/act/next` and `#client/100-acme-corp`.

Results stored in `ParsedZettel.body_tags` and indexed in `_zdb_tags` with `source = 'body'`.

## Checkbox Extraction

`extract_checkboxes(body) -> Vec<CheckboxItem>`

Extracts `- [ ]` (open), `- [x]` (done), `- [i]` (info) items from body text. Fenced code blocks are excluded. Each item captures:

- **state**: Open, Done, or Info
- **content**: text after the checkbox marker
- **date**: optional `YYYY-MM-DD HH:MM` prefix (separated by ` - ` or ` – `)
- **due_date**: optional `⏳ YYYY-MM-DD` within content (stripped from content when extracted)
- **line_number**: 1-indexed position within the body
- **indent_level**: number of leading spaces (0 for top-level, 2+ for sub-items)

Results stored in `ParsedZettel.checkboxes` and indexed in `_zdb_checkboxes`.

## Serialization

`serialize(zettel: &ParsedZettel) -> String`

Produces Markdown with canonical field ordering in frontmatter:

1. `id` (always unquoted)
2. `title`
3. `date`
4. `tags` (as YAML list)
5. `type`
6. `publish` (promoted from extras)
7. `processed` (promoted from extras)
8. Remaining extras (alphabetically, from `BTreeMap`)

### Quoting Rules

Values containing `:`, `[`, `]`, `{`, `}`, `#`, or `[[` are double-quoted with proper escaping.

### Reference Section

If `reference_section` is non-empty, appended after a `---` separator.

## ID Generation

`generate_id() -> ZettelId`

Returns a `ZettelId` from the current local timestamp: `chrono::Local::now().format("%Y%m%d%H%M%S")`.

## Test Coverage

50+ tests covering:
- Three-zone splits (basic, no ref, code blocks, thematic breaks, backtracking, trailing separators)
- Frontmatter parsing (all fields, partial, extras, filename fallback)
- Inline fields (body, reference, mixed, empty values, cross-zone duplication, same-zone duplication, fenced code block exclusion, inline code exclusion)
- Wikilink extraction (body, reference, frontmatter with quoted YAML)
- Checkbox extraction (all states, date prefix, due date, code block exclusion, line numbers, indentation)
- Hashtag extraction (basic, hierarchical, code/wikilink exclusion, dedup)
- Serialization round-trip
- Obsidian syntax passthrough (dataview blocks, Templater)
- ID generation format
