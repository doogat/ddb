# Phase 2 Exit Gate

## Exit Summary

> **Status**: go (with accepted deferrals)

**Tally**: 19 done, 1 partial, 1 fail, 1 deferred

### Blockers

- **NFR-03 (sync time)**: 12.6s at 5K vs 2s target. 6x over. Needs profiling of `SyncManager::sync` hot path before Phase 3 can claim sync readiness. Accepted as known debt — sync optimization is a Phase 3 concern.

### Accepted deferrals

- **Sparse index**: dropped — DDB indexes all doogats, sparse checkout adds no value
- **fsmonitor**: not supported in libgit2/gitoxide, deferred to Phase 3+

### Partial items

- **FR-33 (delta export)**: implemented with unit + smoke coverage. FFI delta export deferred until mobile needs it

### Recommendation

Phase 2 is complete for practical purposes. Core functionality (bundles, REST, NoSQL, compaction, types, multi-device, background maintenance, pre-compaction backup) is implemented and tested. The two deferred items (sparse index, fsmonitor) were formally evaluated and documented as not applicable or Phase 3+ candidates. NFR-03 is the only measurable miss — accept as known debt and track as Phase 3 optimization work.

## Checklist

### Air-Gapped Bundle Protocol (FR-30–FR-33)

| Req | Description | Status | Source | Tests | Docs |
|-----|-------------|--------|--------|-------|------|
| FR-30 | Air-gapped bundle protocol for offline nodes | done | `ddb-core/src/bundle.rs`, `ddb-cli/src/main.rs`, `ffi.rs` | unit: `full_bundle_export_and_verify`, `checksum_verification_catches_tampering`, `full_bundle_import_on_new_repo`; e2e: `bundle_full_bootstrap`, `bundle_recovery_after_compaction`; smoke: sh/ps1 §27-28 | `technical/bundle.md` |
| FR-31 | Bundles contain: Git bundle, node registrations, Git objects | done | `bundle.rs:build_tar_bundle` (objects.bundle, nodes/*.toml, manifest.toml, checksum.sha256) | unit: `full_bundle_export_and_verify` | `technical/bundle.md` |
| FR-32 | Bundle import triggers standard merge protocol | done | `bundle.rs:import_bundle` (unbundle → fetch → merge → CRDT resolve → reindex) | unit+e2e+smoke (see FR-30) | `technical/bundle.md` |
| FR-33 | Bundle export targets specific nodes (delta); `--full` for bootstrapping | partial | `bundle.rs:export_bundle` (delta via known_heads), `export_full_bundle`; FFI: full only, no delta | smoke: sh/ps1 §28 (delta CLI); unit: `delta_export_known_heads`, `delta_export_no_known_heads`. FFI missing delta export | `technical/bundle.md` |

### REST API (FR-50)

| Req | Description | Status | Source | Tests | Docs |
|-----|-------------|--------|--------|-------|------|
| FR-50 | REST API: /rest/doogats CRUD + query, pagination, FTS, type filtering | done | `ddb-server/src/rest.rs`, `actor.rs` | e2e: `serve::rest_crud_lifecycle`, `rest_pagination`, `rest_filter_by_tag`, `rest_search`, `rest_auth_required`; smoke: sh §8, ps1 §8 | `technical/rest-api.md` |

### NoSQL Interface (FR-53)

| Req | Description | Status | Source | Tests | Docs |
|-----|-------------|--------|--------|-------|------|
| FR-53 | NoSQL (redb) key-value interface with prefix scans, type-extended schema | done | `ddb-core/src/nosql.rs` (`RedbIndex`), `ddb-server/src/nosql_api.rs`, `actor.rs` | unit: `nosql::tests::crud_and_prefix_scan`, `upsert_updates_secondary_indices`, `get_missing_returns_none`; smoke: sh §13/§27, ps1 §13/§27 | `technical/nosql.md` |

### Storage and Compaction (FR-60–FR-65)

| Req | Description | Status | Source | Tests | Docs |
|-----|-------------|--------|--------|-------|------|
| FR-60 | Selective compaction of CRDT history | done | `compaction.rs:cleanup_crdt_temp`, `compact_crdt_docs`, `compact_doogat`, `compact` | unit: 5 tests; e2e: 5 multi_device + 2 server_mutations; smoke: sh/ps1 §14 | `storage-budget.md`, `sync.md` |
| FR-61 | Compaction boundary: hash present in ALL active nodes' known_heads | done | `compaction.rs:shared_head` (filters Active nodes, merge-base across known_heads) | unit: `cleanup_removes_temp_files`; e2e: multi_device compaction tests | `sync.md` |
| FR-63 | Annual growth within NFR-02 targets with compaction | done | `benches/growth.rs` (5K, 365 days, 10 edits/day) | bench: `repo_size_after_1yr_with_compaction` (1.2 MB/yr) | `storage-budget.md` |
| FR-64 | git gc after pruning (not --aggressive) | done | `compaction.rs:run_gc` (just `["gc"]`, no --aggressive) | unit: `gc_runs_on_test_repo`, `full_compact_pipeline` | `sync.md` |
| FR-64a | Export git bundle backup before compaction | done | `compaction.rs:backup_before_compact`, `main.rs` `--no-backup`/`--backup-path` flags | unit: `backup_before_compact_writes_file`, `backup_before_compact_default_path`, `compact_skip_backup`; smoke: sh/ps1 §14 | walkthrough |
| FR-64b | Dry-run mode: report what would be compacted | done | `main.rs` Compact `--dry-run` flag | smoke: sh/ps1 §16 | walkthrough |
| FR-65 | Frontmatter CRDT separate compaction when all nodes sync past commit | done | `compaction.rs:compact_crdt_docs` groups by `(doogat_id, is_frontmatter)` | unit: `compact_crdt_docs_separates_fm_and_body`, `parse_crdt_temp_name_formats`, `cleanup_handles_fm_naming_format` | walkthrough |

### Performance Targets (NFR-01–NFR-03)

| Req | Metric | 5K target | 50K target | Status | Evidence |
|-----|--------|-----------|------------|--------|----------|
| NFR-01 | Query latency (indexed) | < 10 ms | < 50 ms | done (5K), not measured (50K) | 5K: FTS ~3.0ms, SQL ~6.1µs. Benches: `search.rs` (5K), `large_scale.rs` (50K). Thresholds: `query_thresholds.rs` (50K ignored). Docs: `performance.md`, `benchmarks.md` |
| NFR-02 | Repository growth | < 50 MB/year | < 200 MB/year | done (5K), no test (50K) | 5K: 43.7 MB/yr without compaction, 1.2 MB/yr with. Bench: `growth.rs`. Threshold: `growth_thresholds.rs`. Docs: `storage-budget.md` |
| NFR-03 | Sync time (batch) | < 2 s | < 10 s | **fail** (5K: ~12.6s vs 2s target) | Bench: `sync.rs`. Threshold: `sync_thresholds.rs` (ignored). Docs: `performance.md` |

### Additional Phase 2 Roadmap Items

| Item | Description | Status | Evidence |
|------|-------------|--------|----------|
| Bundled type: literature-note | Type definition via `ddb type install` | done | `bundled_types.rs:LITERATURE_NOTE_TYPEDEF`; unit: `get_literature_note_bundled_type`; e2e: `install_literature_note_type` |
| Bundled type: meeting-minutes | Type definition via `ddb type install` | done | `bundled_types.rs:MEETING_MINUTES_TYPEDEF`; unit: `get_meeting_minutes_bundled_type`; e2e: `install_meeting_minutes_type` |
| Bundled type: kanban | Type definition via `ddb type install` | done | `bundled_types.rs:KANBAN_TYPEDEF`; unit+e2e: `sql_lifecycle`, `hlc_lww_picks_later_writer` |
| Sparse index | Audit libgit2/gitoxide coverage for sparse checkout | deferred | `git-scalability-audit.md`: "Not applicable — DDB indexes all doogats, sparse checkout adds no value" |
| fsmonitor | OS file watchers (FSEvents, inotify) for O(changed) status | deferred | `git-scalability-audit.md`: not supported in libgit2/gitoxide. Deferred to Phase 3+ |
| Background maintenance | Scheduled `git maintenance run --auto` | done | `maintenance.rs`: `run()`, `check_write_threshold()`, `maybe_auto_run()`; integrated in `git_ops.rs`, `compaction.rs`, `sync_manager.rs`; config: `MaintenanceConfig { auto_enabled, write_threshold }` | unit: 9 tests in `maintenance.rs` | `git-scalability-audit.md` |
| Multi-device simulation | Automated multi-node sync testing | done | PRD 00004 complete. `tests/e2e/multi_device.rs`: 18 tests (3-node convergence, chaos, partition, stale-node, bundle bootstrap, etc.) |

## Deviation Log

_Items that diverge from the initial spec. Each entry: what changed, why, replacement strategy._

| Req/Item | Deviation | Rationale | Replacement |
|----------|-----------|-----------|-------------|
| FR-33 | Delta export implemented; FFI exposes full export only | Delta path works with unit + smoke coverage. Mobile use case only needs full export for now | FFI delta export deferred until mobile clients need node-targeted sync |
| ~~FR-64a~~ | ~~Pre-compaction bundle backup not wired into compact pipeline~~ Resolved | Implemented: `backup_before_compact()` wired into `compact()` pipeline; CLI flags `--no-backup`, `--backup-path` | No action needed |
| NFR-03 | Sync time 12.6s at 5K vs 2s target (6x over) | Git fetch+merge dominates. Optimization not yet attempted | Profile `SyncManager::sync` hot path. Likely needs shallow fetch or incremental approach. Formally deferred to Phase 3. Evidence: `sync_thresholds.rs` (ignored, annotated with current measurement) |
| NFR-01 (50K) | Benchmarks exist but results not measured/recorded | 50K threshold tests are `#[ignore]`. Need dedicated run on representative hardware | Run `large_scale.rs` benchmarks, record in `performance.md` |
| NFR-02 (50K) | No 50K growth benchmark | Only 5K measured. Linear extrapolation suggests ~8.7 MB/yr with compaction | Add `growth_50k` benchmark or document extrapolation as sufficient |
| Sparse index | Dropped entirely | DDB indexes all doogats — sparse checkout adds no value when full index is required | No replacement needed; architecture eliminates the use case |
| fsmonitor | Deferred to Phase 3+ | Not supported in libgit2 or gitoxide; requires external watchman daemon | Phase 3 candidate when gitoxide adds support or DDB migrates git backend |
| ~~Background maintenance~~ | ~~Deferred to Phase 3+~~ Resolved | Implemented: `maintenance.rs` with auto/manual modes, write-threshold triggers, 9 unit tests | No action needed |
| meeting-minutes | ~~Implemented but weak test coverage~~ Resolved | Unit + e2e test now cover install+use (`install_meeting_minutes_type`) | No action needed |

## Phase 3 Entry Criteria

Phase 3 work may begin when all of these hold:

1. This exit checklist is reviewed and accepted
2. ~~FR-64a (pre-compaction backup) is either implemented or formally deferred with a backlog item~~ ✓ Implemented: `backup_before_compact()` in compact pipeline, `--no-backup`/`--backup-path` CLI flags
3. ~~NFR-03 sync regression is tracked as a concrete backlog item with profiling data~~ ✓ Formally deferred in deviation log; baseline measurement in `sync_thresholds.rs`; profiling deferred to Phase 3 start
4. All `cargo test` and `cargo clippy --workspace` pass on master
5. No Phase 2 items remain marked "not started" without an explicit deferral rationale
