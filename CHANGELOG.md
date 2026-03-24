# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Junction tables for multi-value references: `REFERENCES` columns auto-create `{type}_{col}` junction tables with INSERT/DELETE write-through and DROP CASCADE support
- Pluralized GraphQL list fields and REST `references` JSON object for multi-value reference columns
- Cross-device ID collision detection: when two devices create a zettel in the same second, both survive sync with distinct IDs
- `SyncReport.collisions_reassigned` field and CLI display
- `collisionsReassigned` field in GraphQL `SyncResult`
- Title template compliance checking in `zdb fix --verbose` — flags zettels whose title doesn't match their typedef's `title_template`
- Zone migration via `zdb fix --migrate` — moves zettel data between frontmatter/body/reference zones to match typedef schema
- `zdb help` subcommand with in-depth guides (start with `zdb help create-app` for data modeling, zones, title resolution, and API access)
- Contextual hints on `zdb query --help`, `zdb type --help`, `zdb create --help` pointing to the create-app guide
- GUIDES section in `zdb --help` output

### Fixed

- Merge commits now include non-conflicting Added and Modified files from the remote side (previously silently dropped during conflict resolution)
- Wikilink rewriting during collision resolution skips references from the winner's side
- `update_frontmatter_id` returns an error on parse failure instead of silently preserving the old ID
- Collision loser ID generation checks both flat and folder-typed paths
- Scalar GraphQL field for REFERENCES columns returns first value instead of all comma-joined

## [0.1.0] - 2026-03-19

### Added

- CLI (`zdb`) with CRUD operations: `init`, `create`, `read`, `update`, `delete`, `list`, `status`
- Git-backed storage layer with full version history
- Three-zone Markdown parsing: YAML frontmatter, body, references
- FTS5 full-text search with ranked results and snippets
- SQL engine: DDL (`CREATE TABLE`, `DROP TABLE`, `ALTER TABLE`) and DML (`INSERT`, `SELECT`, `UPDATE`, `DELETE`) translated to zettel operations
- Multi-row `INSERT` with single git commit
- Quoted identifier support for hyphenated type names
- Multi-device sync with CRDT conflict resolution (Automerge)
- Hybrid Logical Clock for causal ordering
- Compaction: CRDT temp cleanup, git gc, byte-level reporting
- Type system: `_typedef` zettels, bundled types (project, contact), folder-aware namespaces
- Link extraction: wikilinks, embeds, Markdown links, bare URLs with kind tracking
- Hashtag extraction from body text with `source` column in `_zdb_tags`
- Checkbox extraction with state tracking (`_zdb_checkboxes` table)
- Section parsing (`extract_sections()`) for structured heading navigation
- Sequence navigation: tree, breadcrumb, broken chain detection
- Path navigation: dot/bracket notation for nested frontmatter values (`get_path`, `set_path`, `remove_path`)
- Discovery queries: orphans, stale zettels, recent changes, link density
- Consistency auto-fix: H1 alignment, tag dedup, migration framework
- Broken backlink detection and reporting on delete
- Resurrection tracking for deleted-then-recreated zettels
- Wikilink resolution: path → ID → alias precedence
- Post-rename unresolvable link detection
- Parallel rebuild pipeline with rayon for multi-core parsing
- Batch indexing with single transaction and error resilience
- Incremental reindex on multi-change diffs
- Criterion benchmarks: CRUD, search, sync, growth simulation at 1K–50K zettels
- Performance threshold tests (NFR-01 query latency, NFR-02 growth, NFR-03 sync)
- Property-based tests across parser, SQL engine, indexer, and sync subsystems
- Cross-platform smoke tests (bash + PowerShell)
- E2E test suite via `assert_cmd`
- BSL-1.1 license
- SECURITY.md vulnerability reporting policy
- CONTRIBUTING.md with dev setup and PR guidelines
- mdBook documentation site (architecture, technical guides, API reference)
- **Experimental:** GraphQL server with dynamic schema from `_typedef` zettels
- **Experimental:** REST API with field-level filtering (`field.*` query params)
- **Experimental:** PgWire protocol for SQL client access
- **Experimental:** WebSocket subscriptions with dual-path auth (header + payload)
- **Experimental:** NoSQL storage backend (redb)
- **Experimental:** UniFFI bindings for Swift and Kotlin (ZettelDriver facade)
- **Experimental:** Delta bundle export/import for offline sync
- **Experimental:** Tar-based bundle export/import
- **Experimental:** Attachment support
- **Experimental:** Auto-update mechanism
- **Experimental:** Concurrent read path via ReadPool with semaphore
- **Experimental:** Background maintenance with auto-trigger on high-write sessions
- **Experimental:** Stability tier markers (`X-Experimental` header, CLI `--help` annotations)

### Fixed

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

[Unreleased]: https://github.com/doogat/zdb/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/doogat/zdb/releases/tag/v0.1.0
