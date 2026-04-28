# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **sql**: `ALTER TABLE foo RENAME TO bar` across GraphQL `executeSql`, raw SQL via `ddb query`, and PgWire. One git commit covers the typedef name change, the data folder rename (folder-typed) or `type:` frontmatter rewrite (flat-layout), an auto-rewrite of every other typedef whose `columns:` lists `references: foo`, and any path-based wikilinks pointing into `ddb/foo/`. The materialized SQLite table is renamed via native `ALTER TABLE` after the git commit lands, and `materialize_all_types` now drops orphan tables on rebuild so a kill between commit and SQLite rename is recovered automatically. Validation rejects empty / non-identifier / reserved (`doogats`, `_typedef`, `_ddb_*`, `sqlite_*`) target names up front. The MySQL alias `RENAME TABLE foo TO bar` is rejected with an explicit hint pointing at the supported syntax instead of leaking an internal error. (#14, PRD 00132)

### Fixed

- **server**: GraphQL error envelopes now actually carry `extensions.code` (and the structured-context fields documented in v0.2.5) on every resolver path — typed mutations (`createDoogat`, `createMany`, `updateDoogat`, `batchUpdate`, `deleteDoogat`) and `executeSql` alike. Two regressions were stacked. First, the service-layer `batch_create` path flattened structured errors to `DoogatError::Validation(String)` at the cross-batch (`validation.rs`) and intra-batch (`crud.rs`) conflict sites; both now return `DoogatError::unique_violation` instead. Second, the resolver-layer `to_server_error` returned `async_graphql::ServerError`, which then went through async-graphql v7's blanket `impl<T: Display> From<T> for Error` when propagated by `?` inside `FieldFuture::new` closures, silently dropping `extensions`. The new `to_graphql_error` returns `async_graphql::Error` directly so the extensions survive. Affects every code defined in PRD 00129 §6 (`UNIQUE_VIOLATION`, `NOT_NULL_VIOLATION`, `REFERENCES_VIOLATION`, `UNKNOWN_FIELD`, `TYPE_NOT_REGISTERED`, `CASCADE_CYCLE`). (jink PRD 00022 blocker, PRD 00131)
- **server**: `createDoogat` accepts an omitted `title` (the `CreateDoogatInput.title` field is now nullable). Typedefs that declare a `title_template` render it server-side, matching the SQL `INSERT` path. Typedefs without a template, or untyped creates, still reject the missing title with `NOT_NULL_VIOLATION`. Same change on `createMany` items. (#13)
- **server**: `createMany(onConflict: IGNORE)` returns the surviving row's ID for skipped rows in both cross-batch and intra-batch duplicate scenarios. Earlier the bulk path returned the rejected/rolled-back ID for intra-batch duplicates, so callers using the response IDs for follow-up reads silently missed. (#12)
- **server**: `TagsFilter` operators `containsAll` and `containsAny` are now nullable (`[String!]` instead of `[String!]!`), so `where: { tags: { contains: "rust" } }` no longer requires every caller to also pass empty `containsAll` / `containsAny` arrays. Empty filter (zero operators) and empty arrays are rejected at resolve time with a clear error instead of silently matching nothing. (#11)

## [0.2.5] - 2026-04-27

### Added

- **server**: structured error code vocabulary on GraphQL `errors[].extensions`. `NOT NULL` violations now carry `code: "NOT_NULL_VIOLATION"` plus `table` / `column` fields; unknown-column rejections carry `code: "UNKNOWN_FIELD"` plus `table` / `unknown_field`. Future codes (`UNIQUE_VIOLATION`, `REFERENCES_VIOLATION`, `TYPE_NOT_REGISTERED`, `CASCADE_CYCLE`) light up as their respective enforcement paths are wired. The legacy `message` text is unchanged for every code, so callers still string-matching `"NOT NULL constraint violated"` / `"unknown column"` keep working. (jink feedback, PRD 00129 §6)
- **server**: every registered typedef gets a matching nested accessor on the base `Doogat` GraphQL type. e.g. `mutation { createDoogat(input:{type:"link", ...}) { id link { url description } } }` returns the typed payload populated in a single round trip; the same selection on a quote-typed row returns `link: null`. Available on every mutation response (`createDoogat`, `updateDoogat`, `createMany`, `batchUpdate`) and every read path (`doogat(id:)`, `doogats`, nested references). Field name is the camelCased table name (`category-membership` -> `categoryMembership`); collisions with reserved Doogat fields (`id`, `title`, `type`, etc.) are skipped. (jink feedback, PRD 00129 §4 Option B)
- **server**: per-type GraphQL queries accept a `tags` filter on their `where` input — `where: { tags: { contains: "rust" } }` returns only doogats tagged `rust`. Operators: `contains` (single tag), `containsAll` (every listed tag), `containsAny` (at least one listed tag). Composes with column filters via the existing AND conjunction. Backed by `EXISTS` against the `_ddb_tags` index, no new storage. Hidden when the typedef declares its own `tags` column. (jink feedback, PRD 00129 §5)
- **sql**: REFERENCES columns can opt into `ON DELETE CASCADE` at typedef declaration time. Deleting the referenced parent removes every row that references it through a CASCADE-marked column, recursing through chains of CASCADE references in a single git commit. RESTRICT remains the default (PRD #10 behavior, now using a structured `REFERENCES_VIOLATION` code). Mixed RESTRICT + CASCADE on one table behaves per-column. Cycles are detected and rejected with `CASCADE_CYCLE` rather than looping. (jink feedback, PRD 00129 §2)
- **sql**: typedef-declared `UNIQUE(...)` violations now carry `extensions.code = "UNIQUE_VIOLATION"` plus `table` / `columns` / `values` fields on the GraphQL surface. The `message` text continues to mirror SQLite's `"UNIQUE constraint failed: <table>.<col>[, <table>.<col>]..."` format so callers like jink that match on the substring keep working. (jink feedback, PRD 00129 §3a + §6)
- **sql**: `CREATE INDEX IF NOT EXISTS ...` and `CREATE UNIQUE INDEX IF NOT EXISTS ...` are accepted as no-ops with an info-level log line, so apps with legacy startup migrations keep booting after upgrade. The intended uniqueness path is `UNIQUE(...)` in the typedef, enforced at write time. Plain `CREATE INDEX` (no `IF NOT EXISTS`) continues to reject — that's an intentional declaration the caller should drop. (jink feedback, PRD 00129 §3b)
- **server**: `createDoogat` and `createMany` now populate the type-specific materialized table, not just the base `doogats` row. A typed create that supplies `fields` writes to the matching type table atomically with the base row; subsequent `SELECT` against the type table (or the per-type GraphQL query) sees the row immediately. The PRD 00122 validator now also runs at create time: `type` referencing an unregistered typedef rejects with `TYPE_NOT_REGISTERED`, unknown fields with `UNKNOWN_FIELD`, missing `NOT NULL` columns with `NOT_NULL_VIOLATION`. The CLI `ddb create` path is unchanged — silent base-only creation for unregistered types is preserved there. (jink feedback, PRD 00129 §1)
- **sql**: `ALTER TABLE t ALTER COLUMN c TYPE <new_type>` on materialized type tables. Widening (`VARCHAR(N)` → wider `VARCHAR`, `VARCHAR`/`CHAR` → `TEXT`) is metadata-only. Narrowing (`VARCHAR(N)` → smaller `VARCHAR`, `TEXT` → `VARCHAR(N)`) runs a pre-flight scan and rejects with a row-count message when existing data exceeds the new limit. `INTEGER` ↔ `REAL` conversions scan existing values the same way. REFERENCES columns only accept widening. Accepts both the standard `SET DATA TYPE` syntax and the PostgreSQL-style `TYPE` shorthand. (jink feedback, PRD 00128)
- **sql**: `title_template` placeholders can dereference `REFERENCES` columns via the dotted form `{col.field}`. On INSERT and UPDATE, `{link.title}` pulls the `title` off the referenced link doogat and composes it into the junction row's title — any typed column on the target type works, not just `title`. Bad paths (missing column, non-`REFERENCES` column, missing target field, multi-hop `{a.b.c}`) are rejected at `ALTER TABLE ... SET TITLE TEMPLATE` time. Missing target row or NULL field renders empty at write time. Updates recompute the title automatically when the SET list touches any template-referenced column. Cascading re-title when the referenced doogat changes is out of scope. (jink feedback, PRD 00127)

### Fixed

- **sql**: deleting a doogat that is referenced by a typed-table row through a `NOT NULL REFERENCES` column now fails with `cannot delete '<id>': NOT NULL REFERENCES from <table>.<column> in row '<blocker>'` instead of silently stripping the wikilink and leaving the row with `NULL` in a `NOT NULL` column. Enforced on every delete entry point: SQL `DELETE FROM`, the `deleteDoogat` GraphQL mutation, and `ddb delete <id>` on the CLI. Bulk SQL deletes are atomic — if any matched id has a required-FK dependent, no rows are deleted. Nullable `REFERENCES` columns are unaffected (the wikilink-strip cascade still applies). (#10)

### Changed

- **build**: bump direct `rand` dependency from 0.8 to 0.9 and ignore RUSTSEC-2026-0097 for the transitive `rand 0.8.5` pulled by `pgwire 0.25` (advisory unsoundness path requires a custom `log` logger that calls `rand::rng()` inside its own output; ddb uses `tracing` and has no such logger)

## [0.2.4] - 2026-04-13

## [0.2.3] - 2026-04-13

## [0.2.2] - 2026-04-13

### Fixed

- **sql**: materialized table constraint errors (UNIQUE violations, etc.) now surface to the client instead of being redacted to "internal error"
- **sql**: failed INSERTs (e.g. UNIQUE constraint violations) no longer leave a ghost row in the internal `doogats` index table; the index write and the materialized typed-table write are now atomic via a SQLite savepoint, so a rejected row is fully rolled back and subsequent mutations are unaffected (#4)
- **sql**: `UPDATE`/`DELETE FROM t WHERE id = 'X'` on a non-existent `X` now returns `0 rows affected` instead of throwing "doogat X" not-found, matching standard SQL semantics and the behavior of compound (`AND`) and `IN (...)` predicates (#5)
- **search**: accept in-query field filter syntax (e.g. `tag=rust`, `category=work.dev`) — `search()` now agrees with `normalizeSearchQuery()` on what is a valid query and routes in-query filters through the same filter SQL as the `tag`/`where` arguments (#6). Negated field filters (`NOT field=X`) are only supported for the `tag` field; other fields return a BadRequest instead of silently dropping the negation. `search(query: "")` with no filters now returns `invalid search query` to match the other malformed-input cases; the filter-only pattern (`search(query: "", where: [...])` or `tag:` arg) still works.
- **search**: return `invalid search query: <q>` for all malformed input (bare wildcards `*`/`**`/`.*`, unbalanced parens, bare operators, empty) instead of surfacing as `internal error`; SQLite backend errors on the search path now default-classify as `BadRequest` with genuine backend failures (corrupt DB, disk full, etc.) still surfaced as internal errors. `normalizeSearchQuery` now rejects the same malformed inputs as `search()` so the two endpoints agree on the set of valid queries (#9)
- **sql**: enforce `NOT NULL` on `INSERT` and `UPDATE` — INSERT with `NULL` or absent-no-default on a `NOT NULL` column is now rejected with `NOT NULL constraint violated: <table>.<column>`. Pre-existing rows that violate a newly-added constraint are not retroactively validated (#7)
- **sql**: enforce `VARCHAR(N)` and `CHAR(N)` length on `INSERT` and `UPDATE` — values exceeding the declared length are rejected with `value too long for <table>.<column>: <actual> chars exceeds limit <limit>` and no silent truncation (#7)
- **sql**: enforce `INTEGER`, `REAL`/`FLOAT`/`DOUBLE`, and `BOOLEAN` types at write time — non-parseable values are rejected with `type mismatch for <table>.<column>: expected <TYPE>, got '<value>'`. `BOOLEAN` accepts `0`, `1`, `true`, `false`, `TRUE`, `FALSE` (#7)
- **sql**: reject unknown columns in `INSERT` and `UPDATE` — previously silently dropped (`affected: 1` despite no column existing), now rejected with `unknown column: <table>.<column>`. The SQL path now matches the GraphQL `Where` input typing behavior (#7)

### Changed

- **cli**: expand `ddb help create-app` guide to cover strict type/constraint enforcement, title NOT NULL fallback removal, multi-row INSERT atomicity, and `ALTER TABLE ADD/DROP/RENAME COLUMN`
- **docs**: note title NOT NULL + no-title_template breaking change in `guide/building-apps.md`
- **sql**: **breaking** — remove silent title fallback. INSERT into a table whose `title` is `NOT NULL` and which has no `title_template` no longer coerces `url`, `description`, or any other body/frontmatter column into the title slot. Such INSERTs now fail with `NOT NULL constraint violated: <table>.title`. Clients that relied on the fallback should provide an explicit `title`, declare a `title_template` on the typedef, or make `title` nullable (#7)

### Added

- **test**: comprehensive SQL correctness regression suite covering groups A-G from issue #9: cross-mutation parity after UNIQUE rollback (#4 A1), server-restart persistence (#4 A2), cross-table isolation (#4 A3), UPDATE/DELETE no-match GraphQL parity (#5 B1-B5), normalize/search round-trip (#6 C1), JOIN pinned as working plus CTE/subquery/UNION/window audit (#8 E), composite UNIQUE clear-error (#9 F1), executeBatch atomicity (#9 F4), updateDoogat tag semantics (#9 F5/F6/F7), SQL feature smoke (#9 F9), search limit boundaries (#9 F10), ALTER TABLE in typeDefs (#9 F11), GraphQL schema introspection contract (#9 G1/G2), and the jink full-sweep port distributed across integration sections 17 and 18. Adds four SQL engine invariant property tests (P1-P4) to `ddb-core/tests/property_tests.rs`

## [0.2.1] - 2026-04-09

### Fixed

- **server**: `SqlEngine` error messages now surface to the client instead of being redacted to generic "query failed"
- **sql**: `UNIQUE()` table constraints in `CREATE TABLE` are now parsed and enforced on the materialized SQLite table

## [0.2.0] - 2026-04-09

### Fixed

- **sql**: `INSERT ... ON CONFLICT DO NOTHING` now returns the existing row ID for duplicates (was returning affected count)
- **server**: `deleteDoogat` now removes orphaned rows from materialized type tables

### Changed

- **server**: remove `X-Experimental: true` header from all HTTP responses
- **cli**: `ddb serve` no longer marked as experimental in `--help` output
- `SearchHit.fields` is now a `JSON` scalar (object) instead of a JSON string - access `hit.fields.url` directly instead of parsing `JSON.parse(hit.fields)` (breaking change for GraphQL search clients)

### Added

- **server**: `fields` (JSON string) and `unsetFields` ([String!]) on `updateDoogat` and `batchUpdate` mutations for type-specific field updates with allowed_values and FK validation; materialized type table rows are now updated in place after field changes
- Upsert support: `unique_together` in typedef frontmatter creates composite unique constraints; `onConflict: IGNORE` argument on `createDoogat`/`createMany` mutations skips creation on conflict and returns the existing doogat; `createDoogat` now accepts `fields` JSON input for typed columns (matching `createMany` behavior); `INSERT ... ON CONFLICT DO NOTHING` in SQL engine

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
- `createDoogat(onConflict: IGNORE)` now correctly detects duplicates by passing typed fields to the pre-check (previously always passed empty fields, making deduplication a no-op)
- Unique constraint pre-check in `batch_create` now uses parameterized SQL for column name lookups, preventing malformed SQL from `unique_together` column names containing special characters
- Non-string `Value` variants (numbers, booleans) in unique constraint pre-check now use proper string coercion instead of debug representation

## [0.1.0] - 2026-03-26

### Added

- Upsert support: `unique_together` in typedef frontmatter creates composite unique constraints; `onConflict: IGNORE` argument on `createDoogat`/`createMany` mutations skips creation on conflict and returns the existing doogat

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
- GraphQL server with dynamic schema from `_typedef` doogats
- REST API with field-level filtering (`field.*` query params)
- PgWire protocol for SQL client access
- WebSocket subscriptions with dual-path auth (header + payload)
- **Experimental:** NoSQL storage backend (redb)
- **Experimental:** UniFFI bindings for Swift and Kotlin (DoogatDriver facade)
- **Experimental:** Delta bundle export/import for offline sync
- **Experimental:** Tar-based bundle export/import
- **Experimental:** Attachment support
- **Experimental:** Auto-update mechanism
- Concurrent read path via ReadPool with semaphore
- Background maintenance with auto-trigger on high-write sessions
- Stability tier markers (CLI `--help` annotations)

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

[Unreleased]: https://github.com/doogat/ddb/compare/v0.2.5...HEAD
[0.2.5]: https://github.com/doogat/ddb/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/doogat/ddb/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/doogat/ddb/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/doogat/ddb/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/doogat/ddb/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/doogat/ddb/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/doogat/ddb/releases/tag/v0.1.0
