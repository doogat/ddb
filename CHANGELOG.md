# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `ddb discover recent` command: list recently modified doogats with `--days N` (default 7) and `--type` filters, sorted by recency
- `ddb discover link-density` command: show inbound/outbound link counts and density score per doogat with `--type` filter, sorted by density descending
- Raw ID scalar field on REFERENCES columns: `category_id` returns the raw reference ID alongside the existing `category` resolved object; for `_id` suffix columns (e.g., `link_id`), the scalar keeps the original name and a stripped resolver (`link`) is added
- `orderBy`, `orderDir`, and `limit` arguments on plural reference resolvers for sorting and limiting nested collections (e.g., `categories(orderBy: "label", limit: 5)`)
- `createMany` GraphQL mutation for atomic bulk record creation in a single git commit; accepts `[CreateDoogatInput!]!` with optional `fields` JSON for typed columns, resolves `DEFAULT NEXT` across the batch, validates constraints
- `id` (IDFilter) and `title` (StringFilter) base field filters on all typed GraphQL Where inputs, enabling single-record lookups and title searches on any typed query
- `--set key=value` (repeatable) on `ddb create` and `ddb update` for setting arbitrary frontmatter fields; `--unset key` on `ddb update` for removing fields
- SQL expression support in INSERT/UPDATE: COALESCE, IFNULL, NULLIF, ABS, LENGTH, LOWER, UPPER, TRIM, TYPEOF, MIN, MAX scalar functions, subqueries, and arithmetic operators
- Hyphenated type names now generate typed GraphQL queries and subscriptions (e.g. `category-membership` becomes type `CategoryMembership`, query `categoryMemberships`, subscription `categoryMembershipChanged`); colliding names are detected at schema build time
- FTS5 search boost weighting - typedef columns with `search_boost` now influence search ranking via bm25()
- `DEFAULT NEXT` and `DEFAULT NEXT(partition_col)` expressions for auto-incrementing INTEGER columns; resolves to `MAX(col) + 1` at insert time, with optional per-partition scoping
- `batchUpdate` GraphQL mutation for atomic multi-doogat updates in a single git commit; accepts `[UpdateDoogatInput!]!`, returns `[Doogat!]!`
- `updated_at` and `created_at` fields in all GraphQL doogat responses (generic, typed, and search hits); `updated_at` also available as a sort field
- Cascade delete: deleting a doogat automatically removes junction table rows referencing it and cleans up dangling wikilinks in other doogats' reference sections, all in a single atomic git commit
- `distinct` argument on typed connection queries for deduplicating results by a column (e.g. `categories(distinct: "space")`)
- `groupBy` argument on typed aggregate queries returning per-group counts and numeric aggregates (e.g. `linksAggregate(groupBy: "status") { groups { key count } }`)
- `executeBatch` GraphQL mutation for atomic multi-statement SQL execution with per-statement results
- REST list endpoint `sort` query parameter: `sort=-field` for descending; supports id, title, date, type; date/id default descending, title/type default ascending; omitting sort defaults to date descending (FR-50)
- Auto-register node on first sync using system hostname (FR-41)
- Binary asset conflict resolution via LWW for `reference/` files during sync
- Search `where` filters now resolve against materialized type columns and `_ddb_tags`, falling back to `_ddb_fields` for unrecognized fields
- `tagEntries` GraphQL query exposing individual tag-doogat associations from `_ddb_tags` with `where` filters (`doogatId`: `eq`/`in`, `tag`: `eq`/`contains`/`in`); returns `TagEntryConnection` with `items` and `totalCount`
- FTS negation support: queries like `important NOT meeting` and `NOT archive` now work transparently by splitting into compound SQL (FTS MATCH for positive terms, NOT IN subqueries for negated terms); negated tag filters (`NOT tag=archive`) use `_ddb_tags` directly
- Search query filters: `types`, `tag`, and `where` (field predicates) on GraphQL `search` query
- GraphQL `tags` query returning all tags with usage counts (`TagInfo` type)
- Body hashtags now included in GraphQL doogat `tags` field (merged with frontmatter, deduplicated)
- `columns` field in GraphQL `SqlResult` type returns column names alongside row data
- Optional `format: "objects"` argument on `sql`, `executeSql`, and `executeBatch` returns rows as JSON objects keyed by column name instead of positional arrays
- Core doogat fields (`title`, `date`, `updated_at`) in materialized type tables, removing need for `JOIN doogats`
- PgWire `pg_catalog` support: psql `\dt` and tab-completion show only user type tables, hiding internal `_ddb_*` tables
- Enriched search results: `tags`, `type`, `fields` (JSON), and `created_at` on GraphQL SearchHit, eliminating need for follow-up queries to get type-specific data from search results
- Search query normalization: `queryNormalized` field on SearchConnection and `normalizeSearchQuery` standalone GraphQL query for canonical comparison of semantically equivalent queries (sorts AND operands, lowercases, collapses whitespace, makes implicit AND explicit)
- `DoogatError` now includes `Conflict`, `Sync`, `Index`, `BadRequest` variants for precise error discrimination; server returns 409 for conflicts, 400 for bad requests
- GraphQL schema introspection hints: every query, mutation, input, and object field now has a description visible via standard introspection queries
- Search `in` operator for set membership filtering (`where: [{field: "tag", in: ["a", "b"]}]`)
- Search `where` filters now resolve REFERENCES columns via junction tables (`{type}_{col}`): `eq` matches the referenced doogat ID, `contains` matches the referenced doogat title, `in` matches a set of IDs; UNION is used when multiple types share the same field name

### Changed

- Plural REFERENCES fields on typed GraphQL queries use batch loading per parent item (reduces per-reference overhead)
- 50K query threshold tests (NFR-01/AC-19) validated and enabled in release builds and nightly CI
- 50K repo growth threshold test (NFR-02, < 200MB/yr) added to nightly CI

- **Breaking:** REFERENCES columns in typed GraphQL queries now resolve as nested objects instead of ID strings. Singular fields return the referenced typed object (or null), plural fields return `[TargetType!]!` (or empty list). Clients requesting REFERENCES fields as scalars must update to sub-selections (e.g., `category { id title }`)
- Boolean values in materialized tables stored as `1`/`0` integers instead of `"true"`/`"false"` strings (requires `ddb reindex` after upgrade)

### Fixed

- `ddb fix` no longer modifies typedef titles (table names), which previously caused tables to become inaccessible after title capitalization
- SQL `SELECT` responses now return `"true"`/`"false"` for BOOLEAN columns instead of `"1"`/`"0"` (applies to materialized type tables with typedefs)
- PgWire responses use proper BOOL type for BOOLEAN columns (psql shows `t`/`f`)
- Malformed FTS5 search queries (e.g., `AND AND`) now return `BAD_REQUEST` instead of `INTERNAL_ERROR`
- SQL INSERT without explicit `date` column now defaults to the date derived from the doogat ID, so `created_at` is non-null in GraphQL responses
- DDL responses (CREATE/ALTER/DROP TABLE) via `executeSql` and `executeBatch` no longer emit spurious GraphQL errors; `columns` and `rows` return empty arrays instead of null

## [0.1.0] - 2026-03-26

### Added

- CLI (`ddb`) with CRUD operations: `init`, `create`, `read`, `update`, `delete`, `list`, `status`
- Git-backed storage layer with full version history
- Three-zone Markdown parsing: YAML frontmatter, body, references
- FTS5 full-text search with ranked results and snippets
- SQL engine: DDL (`CREATE TABLE`, `DROP TABLE`, `ALTER TABLE`) and DML (`INSERT`, `SELECT`, `UPDATE`, `DELETE`) translated to doogat operations
- Multi-row `INSERT` with single git commit
- Quoted identifier support for hyphenated type names
- Multi-device sync with CRDT conflict resolution (Automerge)
- Hybrid Logical Clock for causal ordering
- Compaction: CRDT temp cleanup, git gc, byte-level reporting
- Type system: `_typedef` doogats, bundled types (project, contact), folder-aware namespaces
- Link extraction: wikilinks, embeds, Markdown links, bare URLs with kind tracking
- Hashtag extraction from body text with `source` column in `_ddb_tags`
- Checkbox extraction with state tracking (`_ddb_checkboxes` table)
- Section parsing (`extract_sections()`) for structured heading navigation
- Sequence navigation: tree, breadcrumb, broken chain detection
- Path navigation: dot/bracket notation for nested frontmatter values (`get_path`, `set_path`, `remove_path`)
- Discovery queries: orphans, stale doogats, unlinked mentions, suggest links
- Consistency auto-fix: H1 alignment, tag dedup, migration framework
- Broken backlink detection and reporting on delete
- Resurrection tracking for deleted-then-recreated doogats
- Wikilink resolution: path → ID → alias precedence
- Post-rename unresolvable link detection
- Parallel rebuild pipeline with rayon for multi-core parsing
- Batch indexing with single transaction and error resilience
- Incremental reindex on multi-change diffs
- Junction tables for multi-value references: `REFERENCES` columns auto-create `{type}_{col}` junction tables with INSERT/DELETE write-through and DROP CASCADE support
- Pluralized GraphQL list fields and REST `references` JSON object for multi-value reference columns
- Cross-device ID collision detection: when two devices create a doogat in the same second, both survive sync with distinct IDs
- `SyncReport.collisions_reassigned` field and CLI display
- `collisionsReassigned` field in GraphQL `SyncResult`
- Title template compliance checking in `ddb fix --verbose` - flags doogats whose title doesn't match their typedef's `title_template`
- Zone migration via `ddb fix --migrate` - moves doogat data between frontmatter/body/reference zones to match typedef schema
- `ddb help` subcommand with in-depth guides (start with `ddb help create-app` for data modeling, zones, title resolution, and API access)
- Contextual hints on `ddb query --help`, `ddb type --help`, `ddb create --help` pointing to the create-app guide
- GUIDES section in `ddb --help` output
- Type-aware zone inference: SQL column types determine default zone placement (VARCHAR/CHAR to frontmatter, TEXT to body, REFERENCES to reference)
- ENUM/SET column types map to `allowed_values` constraints in typedef YAML
- `ALTER TABLE ... SET ZONE` for column zone overrides after type creation
- `ALTER TABLE ... SET/DROP TITLE TEMPLATE` for title derivation from column values
- Title resolution cascade: explicit title > template > body H1 > frontmatter > fallback
- `origin` field on typedefs (`ddl` for SQL-created, `manual` for hand-created) with info-level warning on manual creation
- Criterion benchmarks: CRUD, search, sync, growth simulation at 1K-50K doogats
- Performance threshold tests (NFR-01 query latency, NFR-02 growth, NFR-03 sync)
- Property-based tests across parser, SQL engine, indexer, and sync subsystems
- Cross-platform smoke tests (bash + PowerShell)
- E2E test suite via `assert_cmd`
- BSL-1.1 license
- SECURITY.md vulnerability reporting policy
- CONTRIBUTING.md with dev setup and PR guidelines
- mdBook documentation site (architecture, technical guides, API reference)
- **Experimental:** GraphQL server with dynamic schema from `_typedef` doogats
- **Experimental:** REST API with field-level filtering (`field.*` query params)
- **Experimental:** PgWire protocol for SQL client access
- **Experimental:** WebSocket subscriptions with dual-path auth (header + payload)
- **Experimental:** NoSQL storage backend (redb)
- **Experimental:** UniFFI bindings for Swift and Kotlin (DoogatDriver facade)
- **Experimental:** Delta bundle export/import for offline sync
- **Experimental:** Tar-based bundle export/import
- **Experimental:** Attachment support
- **Experimental:** Auto-update mechanism
- **Experimental:** Concurrent read path via ReadPool with semaphore
- **Experimental:** Background maintenance with auto-trigger on high-write sessions
- **Experimental:** Stability tier markers (`X-Experimental` header, CLI `--help` annotations)

### Fixed

- Merge commits now include non-conflicting Added and Modified files from the remote side (previously silently dropped during conflict resolution)
- Wikilink rewriting during collision resolution skips references from the winner's side
- `update_frontmatter_id` returns an error on parse failure instead of silently preserving the old ID
- Collision loser ID generation checks both flat and folder-typed paths
- Scalar GraphQL field for REFERENCES columns returns first value instead of all comma-joined
- SQL INSERT respects explicit `title` column instead of always generating from template
- Body section removal no longer leaves orphan blank lines between remaining sections
- Zone migration preserves Map and List frontmatter values instead of silently dropping them
- Error sanitization: internal details redacted from all API responses
- Proper HTTP status codes for NoSQL error variants
- Exhaustive error classification across REST, GraphQL, and NoSQL handlers
- Windows compatibility: `USERPROFILE` fallback for config dir, file lock handling in smoke tests
- SQLite cross-process safety via `busy_timeout`
- Sync performance: deferred commit-graph writes, incremental reindex, single push

### Security

- cargo-deny checks for license compliance and dependency advisories
- Error responses never leak internal paths, stack traces, or SQL details
- quinn-proto upgraded to 0.11.14 (RUSTSEC-2026-0037)

[Unreleased]: https://github.com/doogat/ddb/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/doogat/ddb/releases/tag/v0.1.0
