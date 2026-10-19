# Design: Index-health quarantine listing surface

## Architecture fit

This is a new read-only app-contract verb, not a new layer. It slots into the
existing `AppCommand → DoogatService → AppOutput` pipeline that `SearchCommand`
and `ApplySchemaCommand` already use (`ddb-core/src/app_contract/`,
`ddb-core/src/service/utility.rs`). The scan it reuses — `Index::parallel_parse`
(`ddb-core/src/indexer/rebuild.rs:397`) — already exists and is already called
by the lenient `rebuild`/`reindex` path; this PRD adds a sibling call site that
stops after parsing instead of indexing, so no indexer code changes. A comment
already anchors this intent: `ddb-core/src/app_contract/output.rs:39` says the
full per-file skip list "will be the 00181 doctor surface's job to list."

Transport exposure follows the `search_command`/`reindex` precedent exactly:
CLI calls `DoogatService` directly (`ddb-cli/src/commands/maintenance.rs`
pattern), GraphQL and REST go through `ReadPool::with_service` (read-only, no
actor write-lock needed — this verb never mutates `_ddb_meta` or any table),
and FFI calls `DoogatDriver::with_service`.

## Module placement

- **New file: none.** All changes are additions to existing files.
- `ddb-core/src/app_contract/commands.rs` — add `IndexHealthCommand` (unit-like
  struct, no fields; a side-effect-free full-repo scan takes no parameters).
- `ddb-core/src/app_contract/results.rs` — add `IndexHealthEntry` and
  `IndexHealthResult`.
- `ddb-core/src/app_contract/mod.rs` — re-export the three new types alongside
  the existing `SearchCommand`/`AppSearchResult` re-exports.
- `ddb-core/src/service/utility.rs` — add
  `DoogatService::index_health(&self, cmd: IndexHealthCommand) -> Result<AppOutput<IndexHealthResult>>`.
- `ddb-cli/src/commands/maintenance.rs` — add `pub(crate) fn doctor(repo: &Path) -> ddb_core::error::Result<()>`.
- `ddb-cli/src/main.rs` — add a `Doctor` variant to the `Commands` enum (next to
  `Status`/`Reindex`/`Fix`, ~line 355) and its dispatch arm.
- `ddb-server/src/read_pool.rs` — add `ReadPool::index_health(&self) -> Result<AppOutput<IndexHealthResult>>`
  using the existing `with_service` helper (no `_mut` needed).
- `ddb-server/src/schema/discovery_queries.rs` — add the `indexHealth` GraphQL
  query field (this file already hosts read-only discovery queries like
  `staleDoogats`/`orphanDoogats`, which this verb's shape matches more closely
  than `queries.rs`'s `search` — same read-only, zero-argument, whole-repo-scan
  shape).
- `ddb-server/src/rest.rs` — add a `GET /health/index` handler (no `/api` prefix exists anywhere in this router — `rest.rs:272-280` registers `/doogats`, `/schema/apply`, etc. with no prefix).
- `ddb-core/src/ffi/driver.rs` — add `DoogatDriver::index_health(&self) -> Result<IndexHealthReport, DdbError>`.
- `ddb-core/src/ffi/records.rs` — add `IndexHealthEntryRecord { path: String, reason: String }` and
  `IndexHealthReport { entries: Vec<IndexHealthEntryRecord> }` (new types; `RebuildReport`/`RebuildWarningRecord`
  at `records.rs:181` stay as shipped, per the PRD's "Two FFI warning shapes" risk).

## Interfaces & contracts

```rust
// ddb-core/src/app_contract/commands.rs
/// Command to list currently-unindexable (quarantined) files without
/// triggering a rebuild. No fields: the scan always covers the whole repo.
#[derive(Debug, Clone, Default)]
pub struct IndexHealthCommand {}

// ddb-core/src/app_contract/results.rs
/// One quarantined file: its repo-relative path and a human-readable reason
/// it could not be indexed.
#[derive(Debug, Clone)]
pub struct IndexHealthEntry {
    pub path: String,
    pub reason: String,
}

/// Returned by `IndexHealthCommand`: every currently-quarantined file.
#[derive(Debug, Clone)]
pub struct IndexHealthResult {
    pub entries: Vec<IndexHealthEntry>,
}
```

```rust
// ddb-core/src/app_contract/mod.rs — add to the existing re-export list
pub use commands::IndexHealthCommand;
pub use results::{IndexHealthEntry, IndexHealthResult};
```

```rust
// ddb-core/src/service/utility.rs
impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    /// List currently-unindexable files without mutating the index or
    /// touching `_ddb_meta`. Reuses the lenient `Index::parallel_parse` scan
    /// `rebuild()` already runs; `parallel_parse` itself is confirmed
    /// read+parse-only (no `index_doogat`/materialize calls — rebuild.rs:397-438).
    /// `list_doogats()` is a repo listing call with no write semantics in any
    /// other read verb that calls it (e.g. `service::utility::list_doogats`
    /// itself). Deliberately skips `ensure_fresh()` (unlike
    /// `search.rs`'s index-backed reads): this verb reads git directly, not
    /// the SQLite index, so there is no index staleness to reconcile.
    pub fn index_health(
        &self,
        _cmd: crate::app_contract::IndexHealthCommand,
    ) -> Result<crate::app_contract::AppOutput<crate::app_contract::IndexHealthResult>> {
        let paths = self.repo.list_doogats()?;
        let (_parsed, warnings) = crate::indexer::Index::parallel_parse(&self.repo, &paths)?;
        let entries = warnings
            .iter()
            .map(|w| crate::app_contract::IndexHealthEntry {
                path: match w {
                    crate::types::ConsistencyWarning::UnreadableFile { path, .. }
                    | crate::types::ConsistencyWarning::MalformedYaml { path, .. } => path.clone(),
                    // parallel_parse never produces these two variants; kept
                    // exhaustive so a future variant added to the enum fails
                    // this match instead of silently dropping an entry.
                    crate::types::ConsistencyWarning::CrossZoneDuplicate { path, .. } => path.clone(),
                    crate::types::ConsistencyWarning::MissingRequired { path, .. } => path.clone(),
                },
                reason: crate::app_contract::describe_consistency_warning(w),
            })
            .collect();
        Ok(crate::app_contract::AppOutput {
            value: crate::app_contract::IndexHealthResult { entries },
            warnings: Vec::new(),
        })
    }
}
```

`describe_consistency_warning` is `pub(crate)` in `app_contract::output` and
already re-exported as `pub fn describe_consistency_warning` from
`app_contract/mod.rs:24` — callable as-is, no visibility change needed.

```rust
// ddb-server/src/read_pool.rs
impl ReadPool {
    pub async fn index_health(&self) -> Result<ddb_core::app_contract::AppOutput<ddb_core::app_contract::IndexHealthResult>> {
        self.with_service(|svc| svc.index_health(ddb_core::app_contract::IndexHealthCommand::default()))
            .await
    }
}
```

```graphql
# ddb-server/src/schema/discovery_queries.rs — new field on Query
type IndexHealthEntry {
  path: String!
  reason: String!
}

# indexHealth: [IndexHealthEntry!]!
```

Resolver body (mirrors the existing `staleDoogats`/`orphanDoogats` field
builders in this same file — `FieldFuture::new(async move { ... })`, no
arguments, `pool.<verb>().await.map_err(to_graphql_error)?`, then map each
`IndexHealthEntry` into a GraphQL object with `path`/`reason` string fields):

```rust
let output = pool.index_health().await.map_err(to_graphql_error)?;
// output.warnings is always empty for this verb (see Risks); no forward_warnings call needed.
let list = output.value.entries.iter().map(|e| {
    let mut obj = IndexMap::new();
    obj.insert(Name::new("path"), GqlValue::from(e.path.clone()));
    obj.insert(Name::new("reason"), GqlValue::from(e.reason.clone()));
    GqlValue::Object(obj)
}).collect();
```

```rust
// ddb-server/src/rest.rs — new handler, registered as GET /health/index
#[derive(serde::Serialize)]
struct IndexHealthEntryJson {
    path: String,
    reason: String,
}

async fn index_health(
    Extension(read_pool): Extension<ReadPool>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorBody>)> {
    let output = read_pool.index_health().await.map_err(rest_error)?;
    Ok(Json(serde_json::to_value(
        output.value.entries.into_iter()
            .map(|e| IndexHealthEntryJson { path: e.path, reason: e.reason })
            .collect::<Vec<_>>(),
    ).unwrap()))
}
```

Route registration: add `.route("/health/index", routing::get(index_health))`
in the same `Router::new()` chain as the existing `.route("/doogats", ...)`
registration (`rest.rs:272-280`).

```rust
// ddb-core/src/ffi/records.rs — new records (RebuildReport/RebuildWarningRecord unchanged)
pub struct IndexHealthEntryRecord {
    pub path: String,
    pub reason: String,
}

pub struct IndexHealthReport {
    pub entries: Vec<IndexHealthEntryRecord>,
}
```

```rust
// ddb-core/src/ffi/driver.rs — new method, alongside search_paginated/reindex
pub fn index_health(&self) -> Result<IndexHealthReport, DdbError> {
    self.with_service(|svc| {
        let output = svc.index_health(crate::app_contract::IndexHealthCommand::default())?;
        Ok(IndexHealthReport {
            entries: output.value.entries.into_iter()
                .map(|e| IndexHealthEntryRecord { path: e.path, reason: e.reason })
                .collect(),
        })
    })
}
```

```rust
// ddb-cli/src/commands/maintenance.rs — new function, alongside reindex/fix/status
pub(crate) fn doctor(repo: &std::path::Path) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let output = svc.index_health(ddb_core::app_contract::IndexHealthCommand::default())?;
    if output.value.entries.is_empty() {
        outln!("no quarantined files")?;
    } else {
        for entry in &output.value.entries {
            outln!("{}: {}", entry.path, entry.reason)?;
        }
    }
    Ok(())
}
```

```rust
// ddb-cli/src/main.rs — new Commands variant, next to Status/Reindex/Fix
/// List quarantined (unindexable) files without rebuilding
Doctor,
```

Dispatch arm (alongside the existing `Commands::Status => maintenance::status(&repo)?` arm):
`Commands::Doctor => maintenance::doctor(&repo)?,`

## Data flow

1. Transport receives the request (CLI arg parse, GraphQL field resolve, REST
   route, or FFI call) with no caller-supplied parameters.
2. Transport builds `IndexHealthCommand::default()` (or calls the FFI/CLI
   wrapper that does) and calls `DoogatService::index_health` directly (CLI,
   FFI) or via `ReadPool::with_service` (GraphQL, REST) — the same split every
   other read verb in this codebase uses.
3. `index_health` calls `self.repo.list_doogats()` (the same `DoogatSource`
   method `rebuild_inner` calls) then `Index::parallel_parse(&self.repo,
   &paths)`, discarding the `Vec<ParsedDoogat>` half of the return and keeping
   only `Vec<ConsistencyWarning>`.
4. Each warning becomes one `IndexHealthEntry { path, reason }`, `reason` built
   by the existing `describe_consistency_warning` formatter — the same text
   `ddb reindex` already prints per skipped path, so an operator sees an
   identical reason string whether they ran `reindex` earlier or `doctor` now.
5. Wrapped in `AppOutput { value: IndexHealthResult { entries }, warnings: vec![] }`
   and returned up through whichever transport called it, serialized to that
   transport's native shape (CLI lines, GraphQL list of objects, REST JSON
   array, FFI record).

No write occurs anywhere in this path: `parallel_parse` only reads and parses;
nothing calls `index_doogat`, `materialize_*`, or touches `_ddb_meta`.

## Reuse inventory

- `Index::parallel_parse` (`ddb-core/src/indexer/rebuild.rs:397`) — the exact
  lenient scan the PRD asks to reuse; already returns `Vec<ConsistencyWarning>`
  with no side effects.
- `describe_consistency_warning` / `describe_consistency_warning_code`
  (`ddb-core/src/app_contract/output.rs:66,78`, re-exported from
  `app_contract/mod.rs:24`) — the existing path+reason formatter and FFI code
  mapper; reused verbatim rather than writing a second formatter.
- `DoogatService::list_doogats` (`ddb-core/src/service/utility.rs:24`) — not
  used directly (it calls `self.repo.list_doogats()` with no extra value;
  `index_health` calls `self.repo.list_doogats()` itself to avoid an
  unnecessary indirection) — noted here because the search confirmed no
  existing higher-level helper already combines list+parse without indexing.
- `ReadPool::with_service` (`ddb-server/src/read_pool.rs:249`) — the existing
  read-only dispatch helper every other GraphQL/REST read verb
  (`search_command`, `list_tags`, `query_tags`) uses; reused as-is, no new
  pooling logic.
- `DoogatDriver::with_service` (FFI) — same role as `ReadPool::with_service`,
  reused the same way `search_paginated`/`reindex` do.
- Searches tried for an existing "list without rebuild" verb before
  concluding none exists: `rg "fn.*quarantine|fn.*unindexable|fn.*doctor|fn
  list_index"` (nothing), `rg "parallel_parse"` (only the two call sites in
  `rebuild_inner` and tests — confirms no other read-only wrapper exists yet),
  `rg "describe_consistency_warning"` (confirms it is already reused across
  CLI reindex, FFI reindex, and tests — safe to reuse again here).

## Alternatives considered

1. **Chosen: new `IndexHealthCommand`/`IndexHealthResult` app-contract pair,
   one service verb, four thin transport adapters.** Matches the PRD's
   explicit Transport Impact table and the project's "app-contract-first"
   coherence invariant; keeps the formatter and scan logic in one place.
2. **Smallest-diff alternative: skip the `AppCommand` wrapper — add a bare
   `DoogatService::list_index_warnings(&self) -> Result<Vec<ConsistencyWarning>>`
   (mirroring `discovery.rs`'s plain methods like `stale_doogats`), and have
   each transport call `describe_consistency_warning` itself.** Smaller diff
   (no new command/result types), but it duplicates the
   warning→(path,reason) mapping across four transports instead of once, and
   breaks the "App-contract-first" coherence invariant (`AGENTS.md`: "Every
   verb flows through `AppCommand → DoogatService → AppOutput`"). Rejected:
   the PRD's own Structural Decomposition names `app_contract/commands.rs` and
   `results.rs` as touched files, and a future transport (PgWire, NoSQL HTTP —
   both intentionally excluded here, but a future read surface) would need the
   mapping redone rather than reused.
3. **Rejected: extend `RebuildReport`/`RebuildWarningRecord` with a
   "dry-run" flag so `reindex --dry-run` doubles as the doctor surface.**
   Rejected per the PRD's own stated risk ("Two FFI warning shapes... do not
   retrofit the rebuild report") — conflating a rebuild-shaped report with a
   side-effect-free listing muddies both callers' expectations about whether
   the call touched the index.

## Risks & edge cases

- **`AppOutput.warnings` is always empty for this verb.** Unlike
  `search_command`, there is no secondary signal to attach as an `AppWarning`
  here — the quarantine entries ARE the primary payload, not an aside. GraphQL
  resolvers for this field should not call `forward_warnings` (there is
  nothing to forward); REST's `warnings_to_json` helper, likewise, is not
  needed in the REST handler above.
- **Scan cost on large corpora** (PRD's own risk): `parallel_parse` reads and
  parses every doogat on every `doctor`/`indexHealth` call. This is the same
  cost `ddb reindex` already pays per invocation; no caching is added here
  because the PRD scopes this as "on-demand", not a hot-path check.
- **Likely next changes this design should not box in:**
  1. **00205 index-eviction-by-indexed-identity** (already in the backlog) —
     today's quarantine key is the file path (`ConsistencyWarning`'s own
     `path` field); if 00205 changes how poison files are tracked/evicted by
     indexed identity rather than path, this verb's `IndexHealthEntry.path`
     stays valid (it is a direct scan result, not derived from the eviction
     table), so no coupling to revisit.
  2. **A future `--strict`-style severity split** (e.g. distinguishing
     `UnreadableFile` from `MalformedYaml` in the CLI/GraphQL output) — the
     current design keeps `reason` as a formatted string rather than a typed
     enum on the wire, which is deliberately the same choice `reindex`
     already made; if a future PRD wants typed severity, `describe_consistency_warning_code`
     already exists and could be added as a second field without changing
     `path`/`reason`.
  3. **PgWire/NoSQL HTTP exposure** — intentionally out of scope per the PRD's
     Transport Impact table; if a future PRD reverses that, the single
     `index_health` service verb and `IndexHealthResult` already provide
     everything a fifth adapter would need — no redesign required.
- **`IndexHealthCommand` has no fields today.** Kept as a struct (not a unit
  type like `()`) specifically so a future filter (e.g. "only files under
  `ddb/<type>/`") can be added as an `Option<String>` field without changing
  every call site's signature — matches the PRD's explicit non-goal of
  parameters but avoids a breaking signature change later.

## Test strategy outline

- **Core unit test** (`ddb-core/src/service/tests.rs` or a new
  `utility_tests.rs` module): a repo with one unreadable/malformed doogat
  present → `index_health` returns exactly one `IndexHealthEntry` with the
  expected path and a reason string matching `describe_consistency_warning`'s
  output for that warning variant; assert `_ddb_meta`'s stored HEAD is
  unchanged before/after the call (the PRD's explicit acceptance criterion).
- **Core unit test**: a clean repo (no poison files) → `index_health` returns
  an empty `entries` list.
- **Core unit test**: the poison file's underlying issue is fixed and
  re-synced → a second `index_health` call on the same service instance
  returns an empty list again (no stale quarantine entry carried over,
  because the scan is always fresh, never cached).
- **Cross-interface parity e2e scenario** (`tests/e2e/`, Rust harness per
  00171 — no `.sh`/`.ps1`): one poison file synced into a repo; assert `ddb
  doctor`'s printed line and the GraphQL `indexHealth` query's returned
  `{path, reason}` are equal (REST/FFI asserted too where easily wired into
  the same scenario file).
- **FFI test**: `DoogatDriver::index_health` on a repo with a poison file
  returns one `IndexHealthEntryRecord`; `RebuildReport.warnings`'s existing
  tests are unaffected (no shared code path beyond the formatter functions).

## Review log

dispatch 1 (claude): cardinal-sin 0, blocker 0, non-blocker 2, question 2
- non-blocker: "`describe_consistency_warning`... already re-exported" (Interfaces & contracts) mischaracterizes the mechanism — `app_contract/mod.rs:24-26` is a hand-written passthrough `pub fn`, not a `pub use` re-export. Functionally equivalent, callable exactly as assumed; wording only.
- non-blocker: the exhaustive-match comment on the `CrossZoneDuplicate`/`MissingRequired` arms (Interfaces & contracts) reads as explaining dead code rather than why those arms are deliberately kept for future-proofing; cosmetic only, logic is sound.
- question: the GraphQL snippet (Interfaces & contracts) is illustrative SDL+resolver pseudocode, not literal async-graphql dynamic-schema builder code (`discovery_queries.rs` uses `TypeRef`/`Field`/`Object` registration, not SDL text) — flagged for task-planning to say "mirror staleDoogats' builder pattern" rather than transcribe the snippet.
- question: the FFI record snippets for `IndexHealthEntryRecord`/`IndexHealthReport` omit the `#[derive(uniffi::Record)]` attribute every existing record in `ffi/records.rs` carries — flagged for the implementation task, not a blocker since "no new file" claim stands and the attribute is a one-line addition at implementation time.

dispatch 2 (codex unavailable, Claude fallback): cardinal-sin 0, blocker 1, non-blocker 1, question 1
- blocker (FIXED): the REST section claimed `GET /api/health/index` registered "beside the existing `/api/doogats` route group," but `rest.rs:272-280` registers `/doogats`, `/schema/apply`, etc. with no `/api` prefix anywhere in the router. Corrected the route to `GET /health/index` and the registration note to point at the real `.route("/doogats", ...)` chain (Module placement, Interfaces & contracts, Risks sections updated).
- non-blocker: `index_health` skips `ensure_fresh()` (used by `search.rs`'s index-backed reads) with no stated reason, which a planner implementing by analogy could misread as an omission. Fixed by adding one sentence to the service-verb doc comment explaining the deliberate skip (reads git directly, not the index).
- question: "`_ddb_meta` HEAD is provably unchanged" overstated what was actually traced (only `parallel_parse`, not `list_doogats()`/service construction). Softened the doc comment to name exactly what was checked instead of claiming a full proof.

dispatch 3 (codex unavailable, Claude fallback, verification): cardinal-sin 0, blocker 0, non-blocker 2, question 0
- verified: the REST route fix is accurate against `rest.rs:272-280` and the dispatch-2 blocker is fully corrected, not partially.
- verified: `parallel_parse` (rebuild.rs:397-438) is confirmed read+parse-only (sequential read, parallel parse, partition into parsed/warnings — no `index_doogat`/materialize calls).
- non-blocker: the `ensure_fresh()` doc-comment contrasts only against `search.rs`'s index-backed reads; it could also note `rebuild_inner`/`parallel_parse` itself never calls `ensure_fresh()` either (consistent, not contradictory — an incompleteness in the explanation, not an error). Not fixed; recorded only.

result: ok

