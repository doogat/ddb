# Indexer rebuild resilience: poison-file skip and serialized cold-start rebuild

**Priority:** P1 (availability) · **Roadmap:** Phase 1 (availability) · **Depends on:** none (00162 write-lock helper shipped)

> **Merge note (2026-07-13, backlog review):** absorbed 00169b `serialize-cold-start-index-rebuild` — one PRD now owns indexer rebuild resilience: the lenient incremental path (poison files) AND the serialized destructive full-rebuild path (concurrent cold starts).

> **Provenance:** the index schema-version stamp and the frontmatter-size cap were added by the 2026-07-16 assessment (evidence: evolution-assessment-2026-07-16.md L2/L3) and folded into the body's Features/tasks at the 2026-08-14 backlog review. Probe location re-verified at `d77ad58`: `needs_schema_upgrade` at `indexer/mod.rs:159-180`.

## Overview

### Problem Statement
One malformed markdown file makes the whole database unusable through the service. The incremental reindex path is fatal on the first unreadable/unparsable file, and that path gates almost every read and write:

- **The hot path aborts the whole batch**: `batch_index_changes` (`ddb-core/src/indexer/rebuild.rs:176-198`) does `let content = content_result?;` (`:185`) then `crate::parser::parse(&content, &path)?` (`:186`); the single-file branch propagates the same way (`:191-192`). A single bad file aborts the entire change-set, so the *other* valid files changed in that same sync are also not indexed.
- **The full-rebuild path is already lenient — the hot path is the inconsistent one**: `parallel_parse` (`ddb-core/src/indexer/rebuild.rs:320-357`) is documented "Parse errors are collected, not propagated — one bad doogat doesn't block the rest" (`:322-323`) and partitions failures into `ConsistencyWarning::MalformedYaml` (`:349-353`). Full `rebuild` extends `report.warnings` from it (`:375-376`). Only the incremental path is fatal.
- **The fatal path gates ~40 entry points**: `incremental_reindex` calls `batch_index_changes(...)?` (`:118`), reached from `rebuild_if_stale` (`:420-452`, incremental branch `:443`), reached from `ensure_fresh` (`ddb-core/src/service/mod.rs:116-119`). `ensure_fresh` runs at the top of every public verb module (search / read / create / update / delete / sql / discovery / batch / utility / schema_apply). So one stray file turns every read AND write into an error until a manual full rebuild.
- **The parser is strict on user-editable input**: `parse_frontmatter` rejects missing delimiters (`ddb-core/src/parser/mod.rs:184,190`) and malformed YAML via `serde_yaml::from_str(yaml)?` (`:220`). Users edit `.md` files with other tools and sync brings untrusted content, so malformed input is expected, not exceptional.
- **This violates the AGENTS.md guardrail**: "User-facing read/write/list paths must not silently drop rows, schemas, or parse failures; return errors or structured warnings." Today the path neither indexes the good files nor returns a structured warning — it hard-fails the whole verb.

Note: `ensure_fresh` (`:118`) already discards the `Option<RebuildReport>` returned by `rebuild_if_stale`, so even the lenient full-rebuild warnings never reach a caller today.

A second, independent availability gap sits on the **destructive full-rebuild path**: concurrent writers on a cold repo (no stored HEAD in `_ddb_meta`) each take the destructive branch of `rebuild_if_stale` (`rebuild.rs:449-451` → `rebuild` → `drop_all_tables()` at `:368` + FTS recreation `:369`). Nothing serializes this across processes, so concurrent cold starts stomp on each other's FTS bootstrap and fail with `sql: no such table: _ddb_fts` / `_ddb_boost`. Found while writing the PRD 00162 concurrent-write e2e (8 concurrent `ddb create` on a cold repo); the 00162 git write lock serializes git commits, not the SQLite rebuild. Not data loss — the index is a derived, rebuildable cache — but concurrent cold starts can error. The 00162 e2e currently masks it by warming the index with one sequential create before going concurrent (`tests/e2e/concurrent_writes.rs:102-117`).

The net-new quarantine/index-health *listing* surface (a `ddb doctor` listing, a GraphQL `indexHealth` query, a REST endpoint, and an FFI method) is a follow-up owned by **00181**; this PRD delivers the core lenient-skip behavior, surfaces it on the triggering verb, and serializes the destructive rebuild.

### Target Users
Downstream apps (jink) whose backend syncs untrusted, externally-edited `.md` content; a single bad synced file must not brick reads and writes. Also any deployment where a fresh clone is hit concurrently (serve + CLI): cold starts must not error on FTS bootstrap.

### Success Metrics
- One malformed `.md` in an incremental change-set: the other changed files still index; the bad file is skipped, not fatal; a structured warning names the bad file and reason.
- Subsequent reads AND writes succeed after a poison file lands (no manual full rebuild required to unblock the service).
- The skip warning surfaces on the triggering verb through the existing `AppWarning` channel, asserted present on at least CLI + GraphQL.
- An explicit `--strict` mode preserves the old hard-fail behavior for callers that want it.
- The FFI `RebuildReport` record carries a `warnings` field so FFI callers see the skip.
- N concurrent `ddb create` processes on a cold repo (no stored HEAD) all succeed: zero `no such table: _ddb_fts` / `_ddb_boost` errors; final index row count matches git.
- Double-checked rebuild: a waiter that blocked behind another process's rebuild finds a fresh index and returns without dropping tables; lock acquisition failure/timeout surfaces as a structured error, never a panic.
- The 00162 concurrent-write e2e's warm-up workaround is removed (or demoted to a comment), so that scenario also exercises the cold path.
- An index stamped with an older schema version drop-rebuilds exactly once (version-triggered, under the rebuild lock); a current version never does.
- An oversized frontmatter block degrades into the standard skip warning; reindex cost stays bounded.
- No regression: existing incremental-reindex, full-rebuild, and `AppWarning` tests stay green; no behavior change on the warm incremental path.

## Functional Decomposition

### Capability: Lenient incremental indexing
#### Feature: Collect-and-skip per-file failures
- **Description**: `batch_index_changes` collects per-file read/parse failures into the `RebuildReport` warning list and skips the bad file instead of aborting the batch, matching `parallel_parse`.
- **Inputs**: the change-set paths; the batch read results (`Vec<(path, Result<content>)>`).
- **Outputs**: `(indexed_count, Vec<ParsedDoogat>)` for the good files plus a `Vec<ConsistencyWarning>` for the skipped ones.
- **Behavior**: iterate results; on a read error emit `ConsistencyWarning::UnreadableFile`, on a parse error emit `ConsistencyWarning::MalformedYaml`; push both onto the report; continue with the remaining files; index the survivors.

#### Feature: Strict-mode opt-in
- **Description**: an explicit strict flag restores the hard-fail behavior (first read/parse error aborts).
- **Inputs**: a `strict: bool` on the reindex entry point + a `--strict` CLI flag on `reindex`.
- **Outputs**: on a bad file under strict mode, a `DoogatError` (unchanged old behavior); otherwise collect-and-skip.
- **Behavior**: thread `strict` into `incremental_reindex`/`batch_index_changes`; default is lenient.

### Capability: Skip surfacing on verbs
#### Feature: Warnings ride the verb output
- **Description**: when `ensure_fresh` skips files, the triggering read/write verb returns a structured warning rather than silently continuing.
- **Inputs**: the `RebuildReport` now captured from `rebuild_if_stale`.
- **Outputs**: one or more `AppWarning`s merged into the verb's `AppOutput.warnings`; the FFI `RebuildReport` record gains a `warnings` field.
- **Behavior**: `ensure_fresh` captures the report; map `ConsistencyWarning → AppWarning` (new `From` impl); the app-contract facade merges the skip warnings into the result envelope so the existing per-transport channels forward them.

### Capability: Serialized cold-start rebuild
#### Feature: Cross-process rebuild lock
- **Description**: the destructive full-rebuild path holds a repo-scoped cross-process advisory lock, reusing the lock pattern shipped in 00162 (`ddb-core/src/git_ops/write_lock.rs::acquire(repo_root, timeout)` → RAII guard).
- **Inputs**: the repo root; a rebuild-lock timeout.
- **Outputs**: at most one process executing `drop_all_tables()` + FTS bootstrap at a time; a structured error (never a panic) on acquisition timeout.
- **Behavior**: design decides whether to generalize the existing helper (parameterized lock-file name) or add a sibling index-rebuild lock, and fixes the lock-ordering rule relative to the git write lock so the two can never deadlock. The same guard covers concurrent explicit full-rebuild invocations (`ddb reindex`-style maintenance) — same code path, confirm rather than build.

#### Feature: Double-checked locked rebuild
- **Description**: after acquiring the lock, re-check staleness/integrity before dropping tables; waiters must not redo a rebuild another process just completed.
- **Inputs**: the post-acquisition staleness/integrity state.
- **Outputs**: a fresh index served without a second destructive rebuild; public API unchanged (`rebuild_if_stale()`, `rebuild()` signatures unchanged).
- **Behavior**: acquire → re-verify the rebuild is still needed → rebuild only if still stale/broken → release. Optionally trace/log when a process waits on the rebuild lock, for support diagnostics.

### Capability: Rebuild-adjacent guards
#### Feature: Index schema-version stamp
- **Description**: the index carries a schema version; a mismatch triggers the drop-rebuild path instead of running new `SCHEMA_DDL` code against an old DB.
- **Inputs**: a `SCHEMA_VERSION` constant; the stored version (`PRAGMA user_version` or a `_ddb_meta` key).
- **Outputs**: version-triggered drop-rebuild; the ad-hoc FTS-column probe (`needs_schema_upgrade`, `indexer/mod.rs:159-180`) retired or demoted to a fallback.
- **Behavior**: stamp on every rebuild; compare at connection/configure; mismatch takes the locked destructive-rebuild path (this PRD's rebuild lock), so upgrades and cold starts share one owner.

#### Feature: Frontmatter size cap at the parse boundary
- **Description**: one huge or adversarial synced file cannot blow reindex cost; parse cost is bounded before YAML runs.
- **Inputs**: a generous frontmatter byte-size cap (constant).
- **Outputs**: an over-cap file degrades into the same per-file skip warning path — never fatal, consistent with this PRD's lenient core.
- **Behavior**: check the frontmatter block size before `serde_yaml` parses it; over-cap emits a `ConsistencyWarning` and skips; `--strict` hard-fails it like other poison files.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/indexer/
├── rebuild.rs           # Maps to: Collect-and-skip per-file failures; Strict-mode opt-in; Double-checked locked rebuild (rebuild_if_stale/rebuild take the lock)
├── mod.rs               # Maps to: Index schema-version stamp (retire/demote needs_schema_upgrade :159-180)
ddb-core/src/parser/
└── mod.rs               # Maps to: Frontmatter size cap (checked before serde_yaml parse)
ddb-core/src/git_ops/
└── write_lock.rs        # Maps to: Cross-process rebuild lock (generalize the 00162 helper or add a sibling lock file; lock ordering documented)
ddb-core/src/types/
└── schema.rs            # Maps to: ConsistencyWarning::UnreadableFile variant
ddb-core/src/service/
└── mod.rs               # Maps to: Warnings ride the verb output (ensure_fresh captures report)
ddb-core/src/app_contract/
└── output.rs            # Maps to: ConsistencyWarning → AppWarning mapping
ddb-core/src/ffi/
└── records.rs           # Maps to: FFI RebuildReport gains a warnings field (:173-177)
ddb-cli/src/commands/
└── maintenance.rs       # Maps to: reindex --strict flag
tests/e2e/
└── concurrent_writes.rs # Maps to: cold-start e2e; warm-up workaround removal (:102-117)
```

### Module: rebuild (ddb-core/src/indexer/rebuild.rs)
- **Maps to capability**: Lenient incremental indexing; Serialized cold-start rebuild
- **Responsibility**: collect-and-skip per-file failures in the incremental batch; thread `strict`; take the rebuild lock around the destructive full-rebuild path with a post-acquisition re-check.
- **Exports**:
  - `batch_index_changes(...)` - now returns survivors + collected warnings
  - `incremental_reindex(...)` / `rebuild_if_stale(...)` / `rebuild(...)` - warnings propagated into `RebuildReport`; destructive path lock-guarded; signatures unchanged

### Module: write_lock (ddb-core/src/git_ops/write_lock.rs)
- **Maps to capability**: Serialized cold-start rebuild
- **Responsibility**: repo-scoped advisory file lock with timeout and RAII guard; extended/reused for the index-rebuild lock file.
- **Exports**: `acquire()` (existing), plus whatever parameterization design lands on

### Module: service (ddb-core/src/service/mod.rs)
- **Maps to capability**: Skip surfacing on verbs
- **Responsibility**: capture the reindex report in `ensure_fresh` and merge skip warnings into the verb envelope.
- **Exports**: `ensure_fresh(...)` - returns captured skip warnings for the facade to merge

### Module: app_contract (ddb-core/src/app_contract/output.rs)
- **Maps to capability**: Skip surfacing on verbs
- **Responsibility**: adapter-neutral mapping from `ConsistencyWarning` to `AppWarning`.
- **Exports**: `From<ConsistencyWarning> for AppWarning` (stable `code`, human `message`)

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **rebuild (collect-and-skip + strict)**: makes the incremental path non-fatal; independent.
- **schema (UnreadableFile variant)**: additive enum variant the skip path needs.
- **rebuild-lock helper**: extends the shipped 00162 `write_lock.rs`; independent of the leniency work.

### Core Layer (Phase 1)
- **app_contract mapping + ensure_fresh capture + FFI warnings field**: Depends on [rebuild, schema] - surfaces the collected warnings on the verb envelope and the FFI record.
- **locked destructive rebuild**: Depends on [rebuild-lock helper] - wraps `rebuild_if_stale`/`rebuild` with double-checked re-check.
- **rebuild-adjacent guards (schema-version stamp + frontmatter cap)**: Depend on [rebuild, rebuild-lock helper] - the stamp reuses the locked destructive path; the cap feeds the skip warning path.

### Integration Layer (Phase 2)
- **docs + scenarios + e2e**: Depends on [rebuild, service, locked destructive rebuild].

## Implementation Phases

### Phase 0: Foundation
**Goal**: make the incremental path lenient like the full path; a rebuild lock exists.

**Tasks**:
- [ ] Add `ConsistencyWarning::UnreadableFile { path, error }` to `ddb-core/src/types/schema.rs` (no deps) - Acceptance: variant compiles; existing `MalformedYaml` matches unaffected.
- [ ] Make `batch_index_changes` collect-and-skip read/parse failures into the report and thread a `strict` flag through `incremental_reindex` (depends on: UnreadableFile) - Acceptance: a change-set with one malformed and two valid files indexes the two valid files and returns two survivors + one warning naming the bad path; under `strict=true` the same input returns a `DoogatError`; pure-indexer unit tests cover read-fail, parse-fail, strict, and all-good.
- [ ] Extend/reuse the 00162 lock helper (`git_ops/write_lock.rs`) for an index-rebuild lock file; define and document the lock-ordering rule vs the git write lock (no deps) - Acceptance: a unit test proves two handles serialize and the guard releases on drop; the ordering rule is written down (module doc).

**Exit Criteria**: incremental reindex skips bad files and reports them; strict mode preserved; a rebuild lock helper exists with a documented ordering rule; unit-tested.

### Phase 1: Core
**Goal**: surface the skips on the triggering read/write verbs and the FFI record; serialize the destructive rebuild.

**Tasks**:
- [ ] Add `From<ConsistencyWarning> for AppWarning` in `ddb-core/src/app_contract/output.rs` (depends on: Phase 0) - Acceptance: each variant maps to a stable `code` + human `message`; unit-tested.
- [ ] Capture the `RebuildReport` in `ensure_fresh` and merge its skip warnings into the verb `AppOutput.warnings` via the facade; add a `warnings` field to the FFI `RebuildReport` record (`ddb-core/src/ffi/records.rs:173-177`) (depends on: mapping) - Acceptance: after a poison file lands, a create AND a search both succeed and each carries a skip `AppWarning`; the warning reaches CLI stderr (`write_warnings`) and the GraphQL warnings extension (`forward_warnings`); the FFI reindex report exposes the skip.
- [ ] Wrap the destructive full-rebuild path (`rebuild_if_stale`/`rebuild`) in the rebuild lock with a post-acquisition staleness/integrity re-check; surface lock timeout as a structured error, never a panic (depends on: Phase 0 lock helper) - Acceptance: concurrent cold-start unit/integration coverage passes; a waiter that blocked behind another process's rebuild finds a fresh index and does not drop tables; no behavior change on the warm incremental path.
- [ ] Stamp an index schema version (`PRAGMA user_version` or a `_ddb_meta` key) written on every rebuild and checked at connection/configure; a mismatch takes the locked destructive-rebuild path; retire or demote the FTS-column probe (`needs_schema_upgrade`, `indexer/mod.rs:159-180`) (depends on: locked destructive rebuild) - Acceptance: opening an index stamped with an older version triggers exactly one locked drop-rebuild and a current version triggers none; unit-tested.
- [ ] Cap frontmatter size at the parse boundary so an oversized synced file degrades into the lenient skip path, never a fatal error or unbounded parse (depends on: Phase 0 collect-and-skip) - Acceptance: a file with an over-cap frontmatter block is skipped with a warning naming the path and reason; normal files unaffected; `--strict` hard-fails it; unit-tested.

**Exit Criteria**: reads and writes succeed and surface a structured skip warning across CLI + GraphQL + FFI; only one process at a time executes the destructive rebuild.

### Phase 2: Integration
**Goal**: docs and scenarios for the skip/strict behavior and the cold-start guarantee.

**Tasks**:
- [ ] Update `docs/src/technical/indexer.md` (poison-file resilience, `--strict`) + CHANGELOG; add smoke/integration scenarios (`tests/smoke.sh` + `.ps1` for `reindex --strict`; `tests/integration.sh` + `.ps1` for a synced poison file surfacing via GraphQL) and an e2e test under `tests/e2e/` for the skip + `--strict` behavior (depends on: Phase 1) - Acceptance: docs match; scenarios authored and green locally under Tier 1.
- [ ] Author the concurrent cold-start e2e (N concurrent `ddb create` on a cold repo, NO warm-up; verified to fail against pre-change code once); remove (or demote to a comment) the warm-up workaround in the 00162 concurrent-write e2e (`tests/e2e/concurrent_writes.rs:102-117`) so that scenario also exercises the cold path; add the rebuild-lock invariant (with its lock-ordering rule) to `docs/src/technical/invariants.md` alongside the indexer doc updates (depends on: Phase 1) - Acceptance: the e2e fails against pre-change code and passes after; the warm-up is gone; the invariants page lists the rebuild lock and its ordering relative to the git write lock. (Rebuild path is replaced, so run `cargo test -p ddb-e2e` per CLAUDE.md.)

**Exit Criteria**: skip + `--strict` + cold-start behavior documented and scenario-covered.

## Test Strategy

### Critical Scenarios
- **Happy path**: incremental sync brings three changed files, one malformed → Expected: two valid files indexed, one skipped, a warning names the bad path; a subsequent `ddb query` returns the two valid rows.
- **Edge case**: `reindex --strict` over the same change-set → Expected: a `DoogatError` naming the bad file (old fatal behavior preserved on demand).
- **Error case**: after the poison file lands, run a create then a search → Expected: both succeed; each result carries a skip `AppWarning`.
- **Concurrency case**: 8 concurrent `ddb create` on a cold repo (no stored HEAD, no warm-up) → Expected: all succeed; zero FTS bootstrap errors; final index row count matches git; only one destructive rebuild ran.

## Risks

- **Silently masking genuinely broken data**: skipping a bad file means its rows are absent from query results. Mitigation: always emit the structured warning (never silent) and surface it on every triggering verb; `--strict` remains for callers that must fail hard; the 00181 doctor listing (follow-up) enumerates the quarantined files.
- **Warning volume on a large corrupt corpus**: many bad files could flood the verb output. Mitigation: attach a single summary `AppWarning` (count + "see ddb doctor") on the verb envelope; the full per-file list lives in the 00181 doctor surface.
- **Enum variant is a public shape change**: adding `ConsistencyWarning::UnreadableFile` touches every exhaustive match. Mitigation: additive variant; update matches; the `AppWarning` mapping keeps transport codes stable.
- **`ensure_fresh` signature change fans out**: returning warnings touches the facade. Mitigation: keep `ensure_fresh` returning the warnings and merge only at the facade boundary that already builds `AppOutput`; verbs that do not build an envelope are unaffected.
- **Lock ordering vs the git write lock**: a path holding both locks in opposite orders could deadlock. Mitigation: fix ONE acquisition-order rule at design time, document it in `invariants.md`, and enforce it (the rebuild path and the write path never interleave acquisitions in the other order).
- **Waiter staleness**: a waiter serving immediately after another process's rebuild must not see a half-built index. Mitigation: the double-checked re-check validates integrity (tables present) as well as staleness before returning.

## Transport Impact

- **CLI**: Guaranteed — skip warnings print to the stderr warning stream (`ddb-cli/src/warnings.rs::write_warnings`); `reindex --strict` opt-in.
- **GraphQL**: Guaranteed — skip warnings ride the warnings extension (`ddb-server/src/warning_extension.rs::forward_warnings`).
- **REST**: Guaranteed — skip warnings ride the `warnings` field (`ddb-server/src/rest.rs::warnings_to_json`).
- **FFI**: Guaranteed — skip warnings ride the verb-output warning records; the FFI `RebuildReport` record (`ddb-core/src/ffi/records.rs:173-177`) gains a `warnings` field.
- **PgWire**: Specialized — SQL surface returns SQL errors only; malformed-file skips surface through the same index the SQL reads hit, but PgWire has no structured-warning channel. Document the boundary.
- **NoSQL HTTP**: Specialized — read-only document surface; a skipped file is simply absent from `get`/`scan`, consistent with the query index. The structured warning is surfaced via GraphQL/REST/CLI, not the NoSQL read.
- **Cold-start serialization**: no public API, response-shape, or error-code change on any transport — the rebuild lock sits below `DoogatService` in the indexer, so all six transports gain the same cold-start robustness through the shared code path; the CLI-driven e2e is the conformance vehicle.
- **Cross-interface conformance plan**: one poison file is synced into a repo; an integration test asserts (a) a GraphQL mutation + query both succeed and carry the skip warning via the extension, and (b) the CLI verb prints the same skip via stderr; smoke/integration + e2e scenarios pin the collect-and-skip and `--strict` behaviors; the concurrent cold-start e2e pins the rebuild serialization. The net-new quarantine listing and its cross-interface parity are owned by 00181.
