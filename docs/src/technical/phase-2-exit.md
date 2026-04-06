# Phase 1 & 2 Exit Gate

Combined exit gate for Phases 1 and 2. Phase 1 had no formal exit document.

Source: Initial System Specification, Section 7 (Implementation Roadmap).

## Phase 1 Deliverable Checklist

Phase 1 scope: Core Driver + Primary Interfaces.

| # | Deliverable | Status | Evidence |
|---|---|---|---|
| 1 | Rust project with libgit2 + automerge-rs | Done | `git2` 0.19 and `automerge` 0.7 in ddb-core/Cargo.toml |
| 2 | GitBackend trait abstraction | Not implemented | Spec called for a trait to enable gitoxide swap. Git ops use `git2` directly via `git_ops/mod.rs`. See deviation log. |
| 3 | Generic three-zone parser | Done | `parser/mod.rs` - frontmatter (YAML), body (Markdown + inline fields), reference section |
| 4 | Structured Automerge document schema (Map + Text + List) | Done | `crdt_resolver.rs` - frontmatter as Map, body as Text, reference as key-value set |
| 5 | HLC implementation | Done | `hlc.rs` - wall_ms + counter + node_id, monotonic per-node, commit trailer embedding |
| 6 | Separate frontmatter CRDT tracking | Done | `crdt_resolver.rs` - `merge_frontmatter()` with dedicated CRDT bytes (`fm_crdt_bytes`) |
| 7 | Commit-graph integration | Done | `git_ops/mod.rs` - `write_commit_graph()` called after commits. Read via libgit2, write via git CLI. |
| 8 | Index-as-read-cache (SQLite) with `_ddb_` prefixed tables | Done | `indexer/mod.rs` - `_ddb_tags`, `_ddb_fields`, `_ddb_links`, `_ddb_aliases`, `_ddb_meta`, `_ddb_attachments` |
| 9 | SQL engine: CREATE TABLE, INSERT, UPDATE, DELETE, SELECT | Done | `sql_engine/mod.rs` - DDL/DML with git write-through, SELECT from materialized tables |
| 10 | Type definition system with implicit inference | Done | `indexer/` - `_typedef` doogats + inference from frontmatter keys, body headings, reference fields. Merged schema. |
| 11 | 2 bundled type definitions (project, contact) | Done | `bundled_types.rs` - PROJECT_TYPEDEF, CONTACT_TYPEDEF, installed via `ddb type install` |
| 12 | 3 CRDT presets (default, append-log, LWW) | Done | `crdt_resolver.rs` - `preset:default`, `preset:append-log`, `preset:last-writer-wins` |
| 13 | Node registration | Done | `sync_manager/mod.rs` - `.nodes/{uuid}.toml`, auto-registration, known_heads, HLC state |
| 14 | GraphQL interface | Done | `ddb-server/src/schema/` - async-graphql dynamic schema from type definitions |
| 15 | SQL/pgwire interface | Done | `ddb-server/src/pgwire.rs` - PostgreSQL wire protocol, simple query mode, MD5 auth |
| 16 | Token auth | Done | `ddb-server/src/auth.rs` - UUID token, 0600 permissions, Bearer header |
| 17 | `ddb serve` command | Done | `ddb-cli/src/main.rs` - HTTP + pgwire ports, bind address config |
| 18 | Core API with full CRUD + error types | Done | `service/mod.rs` (DoogatService), `error.rs` (DoogatError enum) |
| 19 | Configuration system | Done | `.ddb.toml` (repo-wide), `~/.config/ddb/config.toml` (node-local) |
| 20 | UniFFI bindings for Swift + Kotlin | Done | `ffi.rs`, `ddb.udl`, `tests/swift/`, `tests/kotlin/`, `dev/bin/build-xcframework`, `dev/bin/build-android` |
| 21 | Unit tests | Done | `#[cfg(test)]` modules in crdt_resolver, sync_manager, consistency, parser, sql_engine, and others |
| 22 | Cross-device ID collision detection | Done | `sync_manager/mod.rs` - `resolve_add_add_collision()`, CollisionLoser tracking, ID reassignment |

21 of 22 deliverables complete. See deviation log for the GitBackend trait.

## Phase 2 Deliverable Checklist

Phase 2 scope: Scalability + Storage + Remaining Interfaces.

| # | Deliverable | Spec Refs | Status | Evidence |
|---|---|---|---|---|
| 1 | Selective compaction (incl. frontmatter CRDT) | FR-60 to FR-65, AC-05, AC-21 | Done | `compaction/mod.rs` - shared-head boundary, frontmatter CRDT cleanup, pre-compaction backup, dry-run, git gc |
| 2 | Bundle export/import (incl. full export for bootstrapping) | FR-30 to FR-34, AC-03 | Done | `bundle.rs` - delta and full export, SHA-256 checksum, node-targeted bundles |
| 3 | Background maintenance | Scalability strategy table | Done | `maintenance.rs` (core + server) - auto/manual modes, write-threshold triggers |
| 4 | Git gc | FR-64 | Done | Integrated into compaction path (not --aggressive by default) |
| 5 | REST API | FR-50, AC-01 | Done | `ddb-server/src/rest.rs` - full CRUD, pagination, tag/field filters, sort, auth |
| 6 | NoSQL (redb) interface | FR-53, DD-15 | Done | `ddb-core/src/nosql.rs`, `ddb-server/src/nosql_api.rs` - optional `nosql` feature flag, redb 2.x |
| 7 | 3 more bundled type definitions | Success Criteria | Done | `bundled_types.rs` - literature-note, meeting-minutes, kanban (all via `ddb type install`) |
| 8 | Multi-device simulation | Testing Strategy | Done | `tests/e2e/multi_device.rs` - 15 tests: 3-4 node convergence, concurrent edits, stale node return, compaction recovery, HLC LWW, delete-vs-edit, chaos convergence |
| 9 | Sparse index evaluation | Scalability Strategy | Done (dropped) | Not applicable - DDB indexes all doogats, sparse checkout adds no value |
| 10 | fsmonitor evaluation | Scalability Strategy | Done (deferred) | Not supported in libgit2 or gitoxide. Deferred to Phase 3+ as candidate if gitoxide adds support |

All 10 deliverables complete.

## Deviation Log

### Phase 1 deviations

| Item | Original Plan | Actual Outcome | Rationale |
|---|---|---|---|
| GitBackend trait | Trait abstraction over libgit2 to enable per-feature gitoxide swap (DD-17) | No trait. `git_ops/mod.rs` uses git2 directly. | Trait was speculative - gitoxide was not mature enough to swap any feature during Phase 1. Direct git2 usage is simpler. **Blocks Phase 3.** PRD 00099 created to address before mobile work begins. |

### Phase 2 deviations

| Item | Original Plan | Actual Outcome | Rationale |
|---|---|---|---|
| Sparse index | Evaluate for selective directory loading | Dropped | DDB indexes all doogats on rebuild. Sparse checkout provides no benefit for this access pattern. |
| fsmonitor | Evaluate OS file watchers for incremental indexing | Deferred as future enhancement | Neither libgit2 nor gitoxide supports fsmonitor. Revisit if gitoxide adds support. Not tied to any phase. |
| Commit-graph write | libgit2 read + write | libgit2 read only; write via git CLI shell-out | libgit2 supports reading commit-graph but not writing. Desktop/server shells out to `git commit-graph write`. No impact on desktop. Mobile impact noted in Phase 3 risks. |

### Items shipped beyond Phase 2 scope

Future enhancements shipped early:

- FE-17: ALTER TABLE (ADD COLUMN, SET ZONE, SET/DROP TITLE TEMPLATE), DROP TABLE (CASCADE), multi-row INSERT
- FE-22: Property-based testing (`cargo test -p ddb-core --test property_tests` - parser, SQL engine, indexer, sync)
- FE-23: Symlink/path traversal hardening (`git_ops::validate_repo_path`, `attachments.rs`)

PRDs completed beyond the original roadmap:

- 00045: Aggregation batch queries
- 00046: Cascade deletes
- 00047: FTS5 search boost
- 00077-00091: GraphQL relation queries, SQL column metadata, search API, boolean consistency, transparent fields, batch updates, auto-increment defaults, hyphenated type support, SQL expressions, create flags, typed query filters, default dates, enriched search results, search query normalization

These are recorded in `dev/local/prds/done/`.

## Deferred Future Enhancements

The following Future Enhancements (spec Section 13) have PRDs in `dev/local/prds/deferred/before-closing-phase-2/`. They are NOT Phase 2 requirements - they are post-roadmap items flagged for evaluation before formally closing Phase 2:

| PRD | Enhancement | Spec Ref | Disposition |
|---|---|---|---|
| 00058 | Table access across protocols (GraphQL/REST/NoSQL) | FE-18 | Deferred. Phase 2 shipped REST and NoSQL APIs for doogats. Typed table exposure through all protocols is additive. |
| 00059 | Schema versioning and migrations | FE-21 | Deferred. Append-only schema evolution (DD-06) remains sufficient. Breaking changes still require new type name + migration. |
| 00061 | Write-batch debouncing | FE-20 | Deferred. Explicit transactions cover current use cases. Revisit when mobile keystroke-level writes create commit churn. |
| 00062 | Bundle signing | FE-10 | Deferred. Checksums cover corruption detection. Cryptographic signing adds value for higher-threat environments only. |
| 00063 | Bundle encryption | FE-05 | Deferred. DD-04: encryption deferred to avoid premature complexity. |
| 00064 | Remote API TLS | FE-09 | Deferred. Server is localhost-only by design. Revisit when LAN/remote client access is needed. |
| 00065 | GPG commit signing | FE-04 | Deferred. Not critical for solo user. Useful for audit trails in regulated environments. |
| 00066 | SQLCipher index encryption | FE-14 | Deferred. Index is derived and rebuildable. Relevant primarily for mobile at-rest protection. |
| 00067 | Repository encryption | FE-01 | Deferred. DD-04: core sync and merge must be solid first. |
| 00069 | Browser client SDK | FE-11 | Deferred. Browser apps can use raw GraphQL/WebSocket. SDK is convenience. |
| 00071 | Admin frontend | FE-02 | Deferred. CLI and API cover all admin operations. Visual dashboard is quality-of-life. |
| 00073 | Git LFS for reference/ scaling | FE-07 | Deferred. reference/ folder is small in practice. Monitor size; migrate if needed. |
| 00074 | Hot-reloadable CRDT strategies | FE-06 | Deferred. DD-08: driver re-initialization is acceptable. Hot reload adds complexity to merge state management. |

None of these block Phase 2 closure. They remain in the deferred backlog for future phases.

## Phase 3 Entry Criteria

Phase 3 scope: iOS + Android mobile apps with embedded driver.

All criteria must be met before starting Phase 3 work:

| # | Criterion | Status | Notes |
|---|---|---|---|
| 1 | All Phase 2 deliverables passing | Met | See checklist above |
| 2 | No known data-loss bugs in core | Verify | Review open issues and `ddb status` edge cases |
| 3 | UniFFI bindings compile for iOS targets (aarch64-apple-ios, aarch64-apple-ios-sim) | Verify | `dev/bin/build-xcframework` exists; run on CI |
| 4 | UniFFI bindings compile for Android targets (aarch64-linux-android, armv7-linux-androideabi, x86_64-linux-android) | Verify | `dev/bin/build-android` exists; run on CI |
| 5 | XCFramework and AAR build scripts produce working artifacts | Verify | Build scripts exist but need validation on target hardware |
| 6 | Desktop performance baselines established | Verify | Criterion benchmarks exist (`ddb-core/benches/`). Capture baseline numbers for comparison with mobile. |
| 7 | Git scalability audit complete | Met | `docs/src/technical/git-scalability-audit.md` covers libgit2/gitoxide feature matrix, desktop vs mobile capabilities |
| 8 | Phase 2 exit gate signed off | Pending | This document |
| 9 | Core API stable for FFI consumers | Verify | `ddb.udl` and `ffi.rs` define the FFI surface. Confirm no planned breaking changes before mobile work begins. |
| 10 | Compaction tested under mobile-realistic constraints | Verify | Multi-device tests use 2-4 nodes. Add test scenario with constrained storage budget. |

### Phase 3 risks to monitor

- **libgit2 on mobile**: iOS sandbox restricts git CLI fallback. No GitBackend trait exists yet (Phase 1 deviation, PRD 00099 created) - introducing it is a prerequisite for mobile. See `git-scalability-audit.md` for the feature matrix.
- **Commit-graph write on mobile**: currently shells out to `git commit-graph write`, unavailable in mobile sandbox. Likely acceptable to skip on mobile (performance optimization, not correctness; small repos; desktop nodes generate it on sync). If needed, gitoxide has partial write support - evaluate as part of PRD 00099.
- **Battery and storage**: full clone is ~5-25 MB for 5K doogats (acceptable). Foreground sync preferred; background sync via BackgroundTasks (iOS) / WorkManager (Android) is best-effort.
- **No embedded server on mobile**: driver accessed via UniFFI directly (DD-14). All server-dependent features (GraphQL, REST, NoSQL, pgwire) are desktop/server only.
