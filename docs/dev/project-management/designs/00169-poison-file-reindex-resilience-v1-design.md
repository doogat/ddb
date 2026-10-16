# Design: Indexer rebuild resilience (poison-file skip + serialized cold-start rebuild)

PRD: `dev/local/prds/wip/00169-poison-file-reindex-resilience-v1.md`

## Architecture fit

Two independent availability fixes, both inside the indexer layer (`ddb-core/src/indexer/`), below `DoogatService` and above the six-transport surface. Neither touches git write paths, CRDT resolution, or the write-path data-loss class (memory `project_data_loss_write_path`) — the index is a derived, rebuildable cache, and this PRD keeps that invariant.

- **Lenient incremental indexing**: `Index::batch_index_changes` (private, called only from `Index::incremental_reindex`) currently propagates the first read/parse error via `?` and aborts the whole change-set. `Index::parallel_parse` (used by the full-rebuild path) already does collect-and-skip. This design brings the incremental path to parity with the pattern the full-rebuild path already established — no new pattern invented.
- **Skip surfacing**: `DoogatService::ensure_fresh` (`service/mod.rs:116-124`) is the single choke-point every one of ~40 verb functions calls before touching the index, but it discards `Index::rebuild_if_stale`'s `Option<RebuildReport>` entirely. This design makes it capture warnings without changing its signature or any of those ~40 call sites (see Interfaces & contracts — Q1 in the reuse inventory explains why).
- **Serialized cold-start rebuild**: reuses the 00162 `git_ops::write_lock` primitive (already proven: `git_ops/mod.rs:310-321` wraps every git commit path in it) for a second, independently-scoped lock protecting the SQLite index's destructive drop-and-recreate path, keyed on the `.ddb/` directory rather than `.git/` — the index's own home, not the git repo's.

## Module placement

New code, in existing files (no new files):

- `ddb-core/src/types/schema.rs` — add `ConsistencyWarning::UnreadableFile { path: String, error: String }` variant (alongside the existing `MalformedYaml`/`CrossZoneDuplicate`/`MissingRequired`).
- `ddb-core/src/indexer/rebuild.rs` — rewrite `batch_index_changes` to collect-and-skip + gain a `strict: bool` param; `incremental_reindex` gains a `strict: bool` param (always called with `false` from production); `parallel_parse` splits its `Err` handling into `UnreadableFile` (read stage) vs `MalformedYaml` (parse stage) for consistency with the newly-lenient incremental path; new private `Index::locked_rebuild` wraps the destructive `rebuild()` call with the rebuild lock + a double-checked re-verify; `rebuild_if_stale`'s two destructive branches call `locked_rebuild` instead of `rebuild` directly; new `pub fn rebuild_strict` sibling to `rebuild` for the explicit hard-fail path.
- `ddb-core/src/indexer/mod.rs` — `Index` gains a `db_dir: Option<PathBuf>` field (set in `open()`, `None` in `open_in_memory()`); `configure_connection` takes the dir and locks the schema-version-triggered drop+recreate branch the same way; `needs_schema_upgrade`'s FTS-column-text probe is demoted to a fallback behind a new `PRAGMA user_version` check.
- `ddb-core/src/parser/mod.rs` — add `MAX_FRONTMATTER_BYTES: usize` constant and a length check in `parse_frontmatter` before `serde_yaml::from_str`.
- `ddb-core/src/git_ops/write_lock.rs` — generalize `acquire` to take the lock directory and file name as explicit params (currently hardcodes `.git/ddb-write.lock`), so the indexer can reuse the exact same primitive for a differently-named, differently-located lock file. One call site (`git_ops/mod.rs:315`) updates to pass `.git` + the existing name explicitly; no behavior change there.
- `ddb-core/src/service/mod.rs` — `ensure_fresh`'s return type changes from `Result<()>` to `Result<Vec<ConsistencyWarning>>` (body change only; no new struct field).
- `ddb-core/src/service/create.rs`, `update.rs`, `schema_apply.rs` — the three `AppOutput`-returning facades (`create`, `update`, `apply_schema`) each gain one explicit `self.ensure_fresh()?` call at the top and merge the result into their existing `warnings` vec.
- `ddb-core/src/app_contract/output.rs` — new `summarize_reindex_warnings(Vec<ConsistencyWarning>) -> Option<AppWarning>` function (the ConsistencyWarning → AppWarning mapping, summarized to avoid flooding the envelope on a large corrupt corpus).
- `ddb-core/src/app_contract/mod.rs` — one new `pub(crate) use output::{describe_consistency_warning, describe_consistency_warning_code};` re-export line, since `output` is a private module and `ffi/driver.rs` needs to reach both helpers (see Interfaces & contracts).
- `ddb-core/src/service/search.rs` — `reindex()` routes through `locked_rebuild` instead of `rebuild` directly; new `reindex_strict()` sibling.
- `ddb-core/src/ffi/records.rs` — `RebuildReport` FFI record gains a `warnings: Vec<RebuildWarningRecord>` field; new `RebuildWarningRecord { code: String, message: String }` (mirrors the existing `SchemaWarningRecord` shape at `records.rs:302-306`).
- `ddb-core/src/ffi/driver.rs` — `reindex()` (`:117-126`) fills the new field instead of dropping `report.warnings`.
- `ddb-cli/src/main.rs`, `ddb-cli/src/commands/maintenance.rs` — `Reindex` command gains a `--strict` flag, dispatching to `reindex_strict()`.
- `ddb-cli/src/commands/crud.rs` — remove the redundant `svc.rebuild_if_stale()?;` pre-call in `create` (`:35`, predates this PRD, already documented in its own comment as redundant for freshness) — see Interfaces & contracts for why keeping it under this design would silently swallow the new warnings.
- `ddb-core/src/git_ops/tests.rs` — two existing direct `write_lock::acquire(&dir, Duration::from_secs(10))` calls (`:1130`, `:1310`) gain the new lock-name argument (`"ddb-write.lock"`).
- `ddb-core/src/indexer/tests/{materialize_tests.rs,mod.rs}`, `ddb-core/benches/search.rs` — direct `incremental_reindex(repo, old_head)` callers gain the new `strict` argument (`false`).
- `docs/src/technical/indexer.md`, `docs/src/technical/invariants.md`, CHANGELOG — doc updates (Phase 2, per PRD).

## Interfaces & contracts

### `types/schema.rs`

```rust
pub enum ConsistencyWarning {
    MalformedYaml { path: String, error: String },
    UnreadableFile { path: String, error: String },   // NEW
    CrossZoneDuplicate { path: String, key: String },
    MissingRequired { path: String, type_name: String, field: String },
}
```
Additive variant. No exhaustive `match` over `ConsistencyWarning` exists anywhere in the crate today (`materialize.rs`'s three sites are variant *constructions*, not matches; test sites use single-arm `if let`) — the new variant needs no existing-call-site updates at all. The only exhaustive match on this enum is the new `describe_consistency_warning` helper this design itself adds (below).

### `indexer/rebuild.rs`

```rust
/// Read + parse a batch of changed paths, collecting per-file read/parse
/// failures as warnings instead of aborting (mirrors `parallel_parse`).
/// `strict: true` restores the old fatal-on-first-error behavior.
fn batch_index_changes(
    &self,
    repo: &impl DoogatSource,
    paths: &[String],
    strict: bool,
) -> Result<(usize, Vec<ParsedDoogat>, Vec<ConsistencyWarning>)>

pub fn incremental_reindex(
    &self,
    repo: &impl DoogatSource,
    old_head: &str,
    strict: bool,
) -> Result<crate::types::RebuildReport>

/// Rebuild lock file name, placed under the index's own `.ddb/` directory
/// (NOT `.git/` — this lock protects the SQLite index, not the git repo).
const REBUILD_LOCK_FILE_NAME: &str = "ddb-rebuild.lock";
const REBUILD_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Acquire the rebuild lock (skipped when `db_dir` is `None`, e.g.
/// `open_in_memory`), re-verify the destructive rebuild is still needed
/// (another process may have finished it while this one waited), and only
/// then run the unconditional `rebuild`. `rebuild()` itself is UNCHANGED —
/// callers that want an unconditional rebuild (bundle import, the
/// consistency sweep) keep calling it directly and are NOT covered by this
/// lock; documented as an explicit residual gap (see Risks).
fn locked_rebuild(&self, repo: &impl DoogatSource) -> Result<crate::types::RebuildReport> {
    let _guard = match &self.db_dir {
        Some(dir) => Some(write_lock::acquire(dir, REBUILD_LOCK_FILE_NAME, REBUILD_LOCK_TIMEOUT)?),
        None => None,
    };
    if self.check_integrity()? && !self.is_stale(repo)? {
        return Ok(crate::types::RebuildReport::default());
    }
    self.rebuild(repo)
}

/// Strict sibling of `rebuild()`: first read/parse error aborts instead of
/// being collected. `rebuild()`'s own signature and lenient behavior are
/// UNCHANGED (PRD's own "public API unchanged" commitment).
pub fn rebuild_strict(&self, repo: &impl DoogatSource) -> Result<crate::types::RebuildReport>
```

`rebuild_if_stale`'s two `self.rebuild(repo)?` call sites (today's `rebuild.rs:433` and `:450`) become `self.locked_rebuild(repo)?`. **A third site also changes**: `incremental_reindex`'s `diff_paths` failure fallback (today's `rebuild.rs:83`, `return self.rebuild(repo);`, reached when the stored HEAD is unreachable — e.g. after a history rewrite, force-push, or `git gc --prune`, proven reachable by the existing test `diff_paths_unreachable_oid_returns_error`) becomes `return self.locked_rebuild(repo);` too — two concurrent processes hitting a diff failure at the same time would otherwise race on the exact same unlocked `drop_all_tables()`/FTS-bootstrap defect this PRD closes. All three sites that can reach the destructive `rebuild()` from the implicit/background path now go through `locked_rebuild`; only the explicit callers named in Risks (`bundle::mod.rs:229`) remain a documented, deliberate gap. Its own signature (`pub fn rebuild_if_stale(&self, repo: &impl DoogatSource) -> Result<Option<RebuildReport>>`) is UNCHANGED — it is part of the `IndexPort` trait (`ports.rs:131`) and has 4+ callers across the crate that must not break. Internally it always calls `incremental_reindex(repo, old_head, false)` — strict mode is never triggered by the implicit/background path, only by explicit `ddb reindex --strict`.

`parallel_parse`'s signature is unchanged (`pub fn parallel_parse(repo: &impl DoogatSource, paths: &[String]) -> Result<(Vec<ParsedDoogat>, Vec<ConsistencyWarning>)>` — no external callers besides `rebuild()` itself, confirmed by repo-wide search); only its body changes to distinguish `UnreadableFile` (the `content_result: Err` arm) from `MalformedYaml` (the `crate::parser::parse: Err` arm).

### `indexer/mod.rs`

```rust
pub struct Index {
    pub(crate) conn: Connection,
    db_dir: Option<PathBuf>,   // NEW — None for open_in_memory(), Some(path.parent()) for open()
}

pub fn open(path: &Path) -> Result<Self> {
    let conn = Connection::open(path)?;
    // `Path::parent()` returns `Some("")` (not `None`) for a bare relative
    // filename like `":memory:"` — filter that out explicitly so a bare
    // filename degrades to unlocked (matching open_in_memory), never to a
    // lock file created at a bogus cwd-relative path.
    let db_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf());
    Self::configure_connection(conn, db_dir)
}

pub fn open_in_memory() -> Result<Self> {
    let conn = Connection::open_in_memory()?;
    Self::configure_connection(conn, None)
}

fn configure_connection(conn: Connection, db_dir: Option<PathBuf>) -> Result<Self>

const SCHEMA_VERSION: i64 = 1;
```

`configure_connection`'s upgrade branch: read `PRAGMA user_version`. `0` (SQLite's default — every DB created before this PRD) is treated as "unstamped, check the legacy probe" and falls back to the existing `needs_schema_upgrade` FTS-column-text check (preserves upgrade behavior for existing on-disk indexes with no version stamp yet); any non-zero value `!= SCHEMA_VERSION` triggers the drop+recreate unconditionally. The destructive branch (whichever check fired) is wrapped in the SAME `write_lock::acquire(db_dir, REBUILD_LOCK_FILE_NAME, REBUILD_LOCK_TIMEOUT)` guard as `locked_rebuild`, skipped when `db_dir` is `None`. After `SCHEMA_DDL` runs, `PRAGMA user_version = SCHEMA_VERSION` is written unconditionally (covers both the fresh-DB path and the just-upgraded path).

### `parser/mod.rs`

```rust
/// Generous cap on a single doogat's frontmatter block, checked before
/// `serde_yaml::from_str` runs. Bounds parse cost against an adversarial or
/// corrupt synced file; degrades into the standard MalformedYaml skip path
/// rather than a fatal error (no new ConsistencyWarning variant needed —
/// this reuses the existing parse-error → MalformedYaml classification).
const MAX_FRONTMATTER_BYTES: usize = 256 * 1024; // 256 KiB

pub fn parse_frontmatter(yaml: &str, path: &str) -> Result<DoogatMeta> {
    if yaml.len() > MAX_FRONTMATTER_BYTES {
        return Err(DoogatError::Parse(format!(
            "frontmatter exceeds {MAX_FRONTMATTER_BYTES} byte cap ({} bytes)",
            yaml.len()
        )));
    }
    // ...unchanged from here
}
```

### `git_ops/write_lock.rs`

```rust
/// Acquire an exclusive advisory lock on `<lock_dir>/<lock_name>`, creating
/// `lock_dir` if absent. Blocks up to `timeout`.
pub fn acquire(lock_dir: &Path, lock_name: &str, timeout: Duration) -> Result<WriteLockGuard>
```
(today: `pub fn acquire(repo_root: &Path, timeout: Duration) -> Result<WriteLockGuard>`, internally joining `repo_root.join(".git").join(LOCK_FILE_NAME)`). The `.git` join moves to the one call site: `git_ops/mod.rs:315` becomes `write_lock::acquire(&self.path.join(".git"), "ddb-write.lock", WRITE_LOCK_TIMEOUT)`. `WriteLockGuard` and its `Drop` impl are unchanged. The module doc comment gains the lock-ordering rule (see Risks & edge cases).

### `service/mod.rs`

```rust
// DoogatService's fields are UNCHANGED — no new field.

fn ensure_fresh(&self) -> Result<Vec<ConsistencyWarning>> {
    if !self.skip_stale_check {
        if let Some(report) = self.index.rebuild_if_stale(&self.repo)? {
            return Ok(report.warnings);
        }
    }
    Ok(Vec::new())
}
```
Return type changes from `Result<()>` to `Result<Vec<ConsistencyWarning>>`; no new struct field (see Alternatives — a `RefCell` side-channel was considered and rejected for a real cross-request leakage risk). **This does not require touching any of the ~40 existing `self.ensure_fresh()?;` call sites** — Rust permits discarding a non-`#[must_use]` value in statement position, and `Vec<T>` carries no `#[must_use]`, so `self.ensure_fresh()?;` continues to compile unchanged everywhere it already appears (`search.rs` ×12, `sql.rs` ×2, `read.rs` ×3, `update.rs` ×3, `create.rs` ×4, `discovery.rs` ×13, `delete.rs` ×1, `batch.rs` ×1, `utility.rs` ×1, `schema_apply.rs` ×1 — verified exhaustively, see Reuse inventory). Only the 3 `AppOutput` facades below capture the return value explicitly.

### `service/create.rs`, `update.rs`, `schema_apply.rs` — the 3 `AppOutput` facades

```rust
// create.rs — DoogatService::create
pub fn create(&self, cmd: CreateCommand) -> Result<AppOutput<ParsedDoogat>> {
    let reindex_warnings = self.ensure_fresh()?;                     // NEW — explicit, up front
    if let Some(mut output) = self.try_baseonly_unregistered(&cmd)? {
        output.warnings.extend(summarize_reindex_warnings(reindex_warnings));
        return Ok(output);
    }
    // ...unchanged batch_create_with_message + title_from_template_warning...
    let mut warnings: Vec<AppWarning> = self.title_from_template_warning(...).into_iter().collect();
    warnings.extend(summarize_reindex_warnings(reindex_warnings));
    Ok(AppOutput { value, warnings })
}
```
```rust
// update.rs — DoogatService::update
pub fn update(&self, cmd: UpdateCommand) -> Result<AppOutput<ParsedDoogat>> {
    let reindex_warnings = self.ensure_fresh()?;                     // NEW — explicit, up front
    let extra = ExtraFieldUpdates { set: &cmd.fields, unset: &cmd.unset_fields };
    let value = self.update_doogat_parsed(&cmd.id, ..., &extra)?;
    Ok(AppOutput { value, warnings: summarize_reindex_warnings(reindex_warnings).into_iter().collect() })
}
```
```rust
// schema_apply.rs — DoogatService::apply_schema
pub fn apply_schema(&mut self, cmd: ApplySchemaCommand) -> Result<AppOutput<SchemaApplyReport>> {
    let reindex_warnings = self.ensure_fresh()?;                     // NEW — explicit, up front
    let doc = schema_diff::desired::SchemaDoc::from_yaml(&cmd.schema_doc)?;
    let live: Vec<Option<TableSchema>> = doc.types.iter().map(|t| self.describe_type(&t.table_name)).collect::<Result<Vec<_>>>()?;
    let plan = schema_diff::diff(&doc, &live);
    let mut warnings: Vec<AppWarning> = plan.unsupported.iter().map(|m| AppWarning { code: SCHEMA_UNSUPPORTED_CHANGE, message: m.clone() }).collect();
    warnings.extend(summarize_reindex_warnings(reindex_warnings));
    // ...every existing return point (dry_run, plan.is_empty(), main path) uses this `warnings` unchanged...
}
```
Every OTHER `self.ensure_fresh()?;` call reached transitively from these 3 functions (`try_baseonly_unregistered` → `create_doogat_with_extra` at `create.rs:188`; `batch_create_with_message` at `batch.rs:130`; `update_doogat_parsed` at `update.rs:36`; `describe_type`'s loop calls at `schema_apply.rs:26`) stays a bare, unchanged `self.ensure_fresh()?;` — by the time those run, `is_stale()` is already `false` (the explicit up-front call just synced `_ddb_meta`'s stored HEAD), so they resolve to cheap no-ops and return an empty `Vec` that is correctly discarded.

### `app_contract/output.rs`

```rust
pub const REINDEX_SKIPPED_FILES: &str = "REINDEX_SKIPPED_FILES";

/// Summarize skipped/consistency warnings from an implicit background
/// reindex into at most ONE AppWarning — an incidental discovery during an
/// unrelated write should not flood the verb's response envelope on a
/// large corrupt corpus. The full per-file list is not lost: it is logged
/// (`tracing::warn!` per file, already emitted by `batch_index_changes`/
/// `parallel_parse`) and will be the 00181 doctor surface's job to list.
pub(crate) fn summarize_reindex_warnings(warnings: Vec<ConsistencyWarning>) -> Option<AppWarning> {
    if warnings.is_empty() {
        return None;
    }
    let first = describe_consistency_warning(&warnings[0]);
    let message = if warnings.len() == 1 {
        first
    } else {
        format!("{first} (+{} more, see ddb doctor)", warnings.len() - 1)
    };
    Some(AppWarning { code: REINDEX_SKIPPED_FILES, message })
}

pub(crate) fn describe_consistency_warning(w: &ConsistencyWarning) -> String {
    match w {
        ConsistencyWarning::UnreadableFile { path, error } => format!("{path}: unreadable ({error})"),
        ConsistencyWarning::MalformedYaml { path, error } => format!("{path}: malformed ({error})"),
        ConsistencyWarning::CrossZoneDuplicate { path, key } => format!("{path}: duplicate key '{key}'"),
        ConsistencyWarning::MissingRequired { path, field, .. } => format!("{path}: missing required field '{field}'"),
    }
}

/// Stable per-variant code for the FFI `RebuildWarningRecord` (`ffi/driver.rs`
/// needs a `&'static str` code alongside `describe_consistency_warning`'s
/// human message; `pub(crate)` on both so the FFI module can call them).
pub(crate) fn describe_consistency_warning_code(w: &ConsistencyWarning) -> &'static str {
    match w {
        ConsistencyWarning::UnreadableFile { .. } => "UNREADABLE_FILE",
        ConsistencyWarning::MalformedYaml { .. } => "MALFORMED_YAML",
        ConsistencyWarning::CrossZoneDuplicate { .. } => "CROSS_ZONE_DUPLICATE",
        ConsistencyWarning::MissingRequired { .. } => "MISSING_REQUIRED",
    }
}
```
`describe_consistency_warning`/`describe_consistency_warning_code` are the first `AppWarning`-construction helpers in the codebase not inlined at the call site — deliberate, since both the 3-facade summarized path and the FFI full-list path need identical per-variant logic. `pub(crate)` alone is NOT enough for `ffi/driver.rs` to reach them: `app_contract::output` itself is a private module (`mod output;`, `app_contract/mod.rs:13`, only `AppOutput`/`AppWarning` are re-exported via the existing `pub use output::{AppOutput, AppWarning};` at `:14`), so a sibling module cannot name a path through it regardless of the items' own visibility. **`app_contract/mod.rs` gains one more re-export line**, matching the existing pattern exactly: `pub(crate) use output::{describe_consistency_warning, describe_consistency_warning_code};` (`pub(crate)`, not `pub`, since these are internal helpers, not part of `app_contract`'s public API). `ffi/driver.rs` then calls them as `crate::app_contract::describe_consistency_warning(&w)` / `crate::app_contract::describe_consistency_warning_code(&w)`.

### `service/search.rs`

```rust
pub fn reindex(&self) -> Result<RebuildReport> {
    self.index.locked_rebuild(&self.repo)          // was: self.index.rebuild(&self.repo)
}

pub fn reindex_strict(&self) -> Result<RebuildReport> {
    self.index.rebuild_strict(&self.repo)
}
```

### `ffi/records.rs` / `ffi/driver.rs`

```rust
#[derive(uniffi::Record)]
pub struct RebuildWarningRecord {   // mirrors SchemaWarningRecord's shape (records.rs:302-306)
    pub code: String,
    pub message: String,
}

#[derive(uniffi::Record)]
pub struct RebuildReport {
    pub indexed: u64,
    pub tables_materialized: u64,
    pub types_inferred: Vec<String>,
    pub warnings: Vec<RebuildWarningRecord>,   // NEW
}
```
`ffi/driver.rs:117-126`'s inline construction gains `warnings: report.warnings.into_iter().map(|w| RebuildWarningRecord { code: describe_consistency_warning_code(&w).to_string(), message: describe_consistency_warning(&w) }).collect()` — note the `.to_string()`: `describe_consistency_warning_code` returns `&'static str` but `RebuildWarningRecord.code` is `String` (matching the existing `SchemaWarningRecord` precedent's own `code: w.code.to_string()` at `ffi/records.rs:335-338`). The explicit `ddb reindex`/FFI path keeps the FULL per-file list (unlike the summarized AppOutput path above), since this IS the dedicated reindex-report surface, not an incidental one.

### `ddb-cli`

```rust
// main.rs
/// Rebuild the search index
Reindex {
    /// Fail on the first unreadable/malformed file instead of skipping it
    #[arg(long)]
    strict: bool,
},

// commands/maintenance.rs
pub(crate) fn reindex(repo: &std::path::Path, strict: bool) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let report = if strict { svc.reindex_strict()? } else { svc.reindex()? };
    outln!("indexed {} doogats", report.indexed)?;
    if !report.warnings.is_empty() {
        outln!("{} warning(s)", report.warnings.len())?;
    }
    Ok(())
}
```

### `ddb-cli/src/commands/crud.rs`

`create` (`:16-83`) calls `svc.rebuild_if_stale()?;` at `:35` — a pre-existing, PRD-00136-era "defence-in-depth" pre-warm, predating this design, whose own comment already says it is *"redundant today"* for freshness (the service-layer `create_doogat_with_extra` already calls `ensure_fresh`). Under this design it becomes actively harmful, not just redundant: it runs the real reindex and discards `DoogatService::rebuild_if_stale()`'s `Vec<ConsistencyWarning>` (that wrapper's own `Result<()>` signature is unchanged by this design — see below), which leaves the index already fresh by the time `svc.create(cmd)` runs its own explicit `ensure_fresh()?` a few lines later — so that call's `is_stale()` check is `false` and `reindex_warnings` comes back empty. The skip warning would be silently lost specifically on `ddb create`, one of the two transports (CLI + GraphQL) the PRD's own conformance plan names. **Fix: delete the `svc.rebuild_if_stale()?;` line at `crud.rs:35`.** Nothing else in `create` depends on it — `svc.create(cmd)` performs the real freshness check and reindex itself immediately after. `DoogatService::rebuild_if_stale()` (`search.rs:46-49`, `pub fn rebuild_if_stale(&self) -> Result<()>`) itself is NOT changed by this design — it is called by 9 other CLI/actor sites (`attach`/`detach`/`query`/`discover`/`fix`, actor compact/sync) that never build an `AppOutput` and were never in scope for warning surfacing; only its `create.rs:35` caller interacted badly with this design, and removing that one call site is the fix, not touching the shared helper.

## Data flow

**Implicit skip-and-surface (e.g. `ddb create` hits a poison file left by a sync):**
1. CLI/GraphQL/REST/actor calls `DoogatService::create`/`update`/`apply_schema`.
2. Facade calls `self.ensure_fresh()?` explicitly → `Index::rebuild_if_stale` → (stale + stored HEAD) → `Index::incremental_reindex(repo, old_head, strict=false)` → `batch_index_changes` reads+parses each changed path, collects `ConsistencyWarning`s for failures instead of aborting, indexes the survivors.
3. `RebuildReport.warnings` flows back up through `rebuild_if_stale` → `ensure_fresh`'s `Vec<ConsistencyWarning>` return.
4. Facade calls `summarize_reindex_warnings`, gets `Option<AppWarning>`, merges into its `AppOutput.warnings`.
5. CLI: `write_warnings` prints it to stderr (already wired, `crud.rs:79,129` — zero CLI code change). GraphQL: `forward_warnings` puts it on `extensions.warnings` (already wired, `operations.rs:40,82,572` — zero GraphQL code change).
6. The rest of the create/update proceeds against the now-fresh, poison-file-skipped index.

**Serialized cold-start rebuild (N concurrent `ddb create` on a cold repo):**
1. Each process opens the repo, `ensure_fresh` → `rebuild_if_stale` → no stored HEAD → `locked_rebuild(repo)`.
2. Each attempts `write_lock::acquire(db_dir, "ddb-rebuild.lock", 30s)`. One wins; the rest block.
3. Winner: `check_integrity() && !is_stale()` is false (genuinely needs it) → runs `rebuild()` (drop_all_tables + SCHEMA_DDL + parse + index + materialize + store_head) → releases the lock on drop.
4. Each waiter, once it acquires the lock, re-checks `check_integrity() && !is_stale()` — now true (winner already rebuilt and stored HEAD) — returns `RebuildReport::default()` without touching the tables again.
5. All N processes proceed with a correct, un-corrupted index; zero `no such table: _ddb_fts` errors.

**Schema-version-triggered rebuild:** `Index::open` → `configure_connection` → `PRAGMA user_version` mismatch (or `0` + legacy FTS-probe hit) → same `write_lock::acquire(db_dir, ...)` guard around the drop+recreate → stamps `user_version = SCHEMA_VERSION` on exit.

## Reuse inventory

- **`Index::parallel_parse`** (`indexer/rebuild.rs:324-358`) — the collect-and-skip pattern `batch_index_changes` needs to match already exists here; reused as the template, not reinvented. Greps tried: `collect.*warning`, `ConsistencyWarning::`, `parallel_parse` (all confirmed the pattern's one existing home).
- **`git_ops::write_lock::acquire`** (`git_ops/write_lock.rs:51-84`) — the exact primitive the PRD's own Feature list names for reuse ("reusing the lock pattern shipped in 00162"). Generalized (lock dir + name as params) rather than duplicated; `WriteLockGuard`'s RAII `Drop` unlock logic is reused as-is. **Three call sites today, not one** (corrected after dispatch 2 caught the undercount): the production site `git_ops/mod.rs:315`, plus two direct test call sites exercising the write-lock contract itself — `git_ops/tests.rs:1130` (`merge_blocks_while_write_lock_held`) and `:1310` — both gain the new `"ddb-write.lock"` third argument (see Module placement).
- **`SchemaWarningRecord` / `SchemaApplyReportRecord::from_output`** (`ffi/records.rs:301-341`) — the one existing precedent in the codebase for an `AppWarning`-derived FFI record; `RebuildWarningRecord` copies its exact `{code, message}` shape rather than inventing a richer one. Greps tried: `uniffi::Record`, `AppWarning` in `ffi/`.
- **`ddb-cli::warnings::write_warnings`** (`ddb-cli/src/warnings.rs:4`) and **`forward_warnings`** (`ddb-server/src/warning_extension.rs:39-58`) — both already wired to the exact 3 `AppOutput`-producing functions this design extends (`create`, `update`, `apply_schema`); zero new CLI or GraphQL plumbing needed. Greps tried: `write_warnings`, `forward_warnings`, `AppOutput` (confirmed exactly 3 producers exist in the whole crate).
- **`update()`'s own doc comment** (`update.rs:122-124`): *"Update emits no warnings today, so `warnings` is always empty; the envelope keeps the warning channel available for callers."* — this is a pre-existing, explicit invitation to extend exactly this path; confirms the facade-merge design rather than a coincidence.
- Nothing found for: a schema-version pragma (`rg -n "user_version"` → zero hits, confirms `needs_schema_upgrade`'s ad hoc FTS-text-probe is genuinely the only mechanism today), a frontmatter size guard (`rg -ni "MAX|cap|size"` near `parse_frontmatter` → zero hits), a `--strict`-shaped CLI flag anywhere (`rg -in strict ddb-cli/src` → doc-comment prose only, no clap flag).

## Alternatives considered

1. **(Smallest diff) Change `ensure_fresh`'s return type only; do nothing else.** Every call site keeps compiling (as established), but no facade ever reads the new `Vec<ConsistencyWarning>`, so warnings still never reach a transport — the PRD's core success metric ("skip warning surfaces on the triggering verb") goes unmet. Rejected: doesn't deliver the PRD.
2. **RefCell side-channel on `DoogatService`** (`pending_reindex_warnings: RefCell<Vec<ConsistencyWarning>>`, written by `ensure_fresh` unconditionally, drained by the 3 facades). Zero of the ~40 call sites OR the 3 facades need an explicit `ensure_fresh()` call at all — even less code than the chosen design. **Rejected**: the actor pattern (`ddb-server/src/actor/`) keeps ONE `DoogatService` instance alive across many sequential commands. A leaf command (e.g. `upsert_singleton`, never drained) would leave stale warnings sitting in the `RefCell`, silently attached to the NEXT unrelated `create`/`update` call's `AppOutput` — a real cross-request correctness bug, not just an edge case. The explicit-call design (chosen) has no shared mutable state and therefore no leakage window: each facade's warnings come only from the `ensure_fresh()` call *it itself* made, milliseconds earlier, in the same call stack.
3. **(Chosen) Explicit up-front `ensure_fresh()` call in each of the 3 `AppOutput` facades, return-type change on `ensure_fresh` only.** No RefCell, no leakage, no signature change anywhere except `ensure_fresh` itself (backward-compatible). Costs a few extra lines in 3 files vs. option 2, and one redundant (but cheap — `is_stale()` short-circuits) nested `ensure_fresh` call per facade invocation. This is the smallest diff that is also correct.

## Risks & edge cases

- **Reindex-skip warnings are lost when the facade's own operation subsequently errors.** All three facades capture `reindex_warnings` up front, but `apply_schema` in particular has two `Err` exit points after that point (`SCHEMA_DESTRUCTIVE_BLOCKED`, `SCHEMA_APPLY_PARTIAL`) — `Result::Err` has no warnings channel in this codebase, so a captured skip warning never reaches a transport if the verb itself then fails for an unrelated reason. Not a regression (nothing surfaces skip warnings today either) and not silent (the per-file `tracing::warn!` inside `batch_index_changes` still fires), but it narrows the PRD's success metric to "surfaces on a triggering verb that also succeeds." Accepted: matches this codebase's existing warnings-only-ride-`Ok` convention; not fixed here.
- **`REBUILD_LOCK_TIMEOUT = 30s` is an unvalidated starting point, not sized against a real corpus.** A legitimately large repo's full rebuild could plausibly exceed it under correct (non-buggy) contention, producing a spurious `Conflict` timeout for a waiter that should have gotten the fresh index. No corpus-size ceiling is defined by the PRD or this design; `/plan-tasks` should either pick a number justified against a realistic ceiling or make it configurable — flagged here rather than silently asserted as correct.
- **Lock ordering (git write lock vs. rebuild lock): they must never nest.** Verified today: `with_write_lock` (git commits) lives entirely in `git_ops/{mod,merge,remote}.rs`; nothing in that call chain touches `Index::rebuild`/`locked_rebuild`, and nothing in the indexer touches `with_write_lock`. **Rule, documented in `write_lock.rs`'s module doc and `docs/src/technical/invariants.md`: the git write lock and the index rebuild lock are never held simultaneously by the same call stack. If a future change needs both, fully release one before acquiring the other — never acquire the second while the first is still held.** This is the "fix ONE acquisition-order rule at design time" the PRD asks for; today's answer is "don't nest them at all," which is stricter and simpler than an ordering rule and holds because the two lock scopes (`.git/` for commits, `.ddb/` for the index) protect genuinely independent resources.
- **`bundle::mod.rs:229`'s `index.rebuild(repo)?` is NOT covered by `locked_rebuild`.** Left as a direct, unlocked call — in scope only for `rebuild_if_stale` (both destructive branches) and the new `reindex()`/`reindex_strict()` pair. Bundle import already serializes its own git operations under the 00168 write lock and is not part of this PRD's concurrency test matrix (N concurrent `ddb create`, not N concurrent bundle imports); documented here rather than silently claimed as covered.
- **`ddb-server`'s `ReadPool` readers bypass the rebuild lock entirely.** `read_pool.rs:253,270` call `svc.set_skip_stale_check(true)` — with that flag set, `ensure_fresh` never calls `rebuild_if_stale`/`locked_rebuild` at all (readers trust WAL visibility instead). If another connection is concurrently mid-`drop_all_tables()` inside `locked_rebuild`, a ReadPool reader issuing a raw query at that exact moment is not part of the lock protocol and could still hit `no such table: _ddb_fts` — the PRD's own named symptom, via the read-pool path rather than the N-concurrent-`ddb create` path this PRD's test matrix covers. Parallel to the `bundle::mod.rs` gap above; not fixed here, documented as a second accepted residual gap.
- **Warning message strings have no length cap.** `ConsistencyWarning::{UnreadableFile,MalformedYaml}.error` is an unbounded `String` (a `serde_yaml`/IO error's `Display`, which can echo a snippet of the offending — externally-synced, untrusted — content). `summarize_reindex_warnings` caps the warning *count* to one but not that one warning's *size*. Not fixed here (non-blocking); a future pass could truncate `error` when formatting.
- **Warning volume**: `summarize_reindex_warnings` caps the `AppOutput` envelope at one `AppWarning` regardless of corpus size, per the PRD's own stated mitigation. The full per-file list still reaches `tracing::warn!` (already emitted today) and will be queryable via 00181's doctor surface — this design does not lose it, only doesn't repeat it 1:1 into the response body.
- **`ConsistencyWarning` is a public, non-`#[non_exhaustive]` enum** (confirmed: no `#[non_exhaustive]` attribute in `types/schema.rs`) — adding `UnreadableFile` is source-breaking for any exhaustive `match` outside the crate. No external consumers exist yet (ddb-client, 00185, is still backlog), so this is safe now and should be flagged to the eventual 00185 design as a precedent for marking future public enums `#[non_exhaustive]`.
- **`Index::open`'s new `db_dir` field**: `path.parent()` alone is NOT sufficient — it returns `Some("")` for a bare relative filename, not `None` (verified empirically). Two existing test-only call sites open a real `Index` directly against the literal path `":memory:"` (`ddb-core/src/attachments.rs:209`, `ddb-core/src/id_minting.rs:180` — both `#[cfg(test)]`, bypassing `open_in_memory()`); without the `.filter(|p| !p.as_os_str().is_empty())` guard above, these would silently compute `db_dir = Some("")` and try to lock a relative `ddb-rebuild.lock` file in the test process's cwd (cross-test contention + a stray untracked file). The filter fixes this for `Index::open` generally; flag those two call sites for migration to `open_in_memory()` as a cheap, independent cleanup (matches their own intent — they want an in-memory index, not a `":memory:"`-named file on disk). `DoogatService::open`'s own production path (`db_dir.join("index.db")`) always has a real parent and is unaffected either way.
- **Frontmatter cap (256 KiB) is a guess** — the PRD asks for "generous"; 256 KiB comfortably exceeds any real doogat's frontmatter (typedefs and data files alike) while still bounding `serde_yaml` parse cost against an adversarial payload. Flagged as a tunable constant, not a load-bearing number; `/plan-tasks` may adjust with justification.
- **Next 2-3 likely follow-ups this design should not box in**: (1) 00181's doctor listing will want the FULL per-file skip list somewhere queryable — `tracing::warn!` today is not queryable; 00181's design should decide whether that means a new `_ddb_meta`-adjacent quarantine table or a re-scan, not constrained by this PRD. (2) A future `#[non_exhaustive]` sweep across public enums (prompted by the `ConsistencyWarning` addition above). (3) If a 5th `AppOutput`-producing facade is added later (a new verb), it should follow the same "explicit `ensure_fresh()` up front, `summarize_reindex_warnings` merge" pattern established here rather than reaching for the RefCell alternative rejected above.

## Test strategy outline

- **Unit (`indexer/rebuild.rs`)**: `batch_index_changes`/`incremental_reindex` — one malformed + one unreadable + two valid files → 2 survivors, 2 warnings (correct variants); `strict=true` on the same input → `Err`; all-good input → 0 warnings. `locked_rebuild` — two handles serialize (mirrors `write_lock.rs`'s existing `second_acquire_blocks_until_first_guard_released` test shape); a waiter that blocks behind a completed rebuild returns without dropping tables (assert table row counts unchanged post-wait).
- **Unit (`indexer/mod.rs`)**: a DB stamped with `user_version = 0` (or absent) triggers exactly one drop-rebuild on open; a DB stamped with the current `SCHEMA_VERSION` triggers none.
- **Unit (`parser/mod.rs`)**: a frontmatter block over `MAX_FRONTMATTER_BYTES` returns `Err` (caught by the caller's lenient path, not a panic); normal-sized frontmatter unaffected.
- **Unit (`app_contract/output.rs`)**: `summarize_reindex_warnings` — empty → `None`; one → unsummarized single message; many → count-suffixed summary.
- **Integration**: after a poison file lands (via a raw commit bypassing validation, matching the PRD's synced-untrusted-content scenario), a `create` AND a `search` both succeed and `create`'s `AppOutput.warnings` carries exactly one `REINDEX_SKIPPED_FILES` warning; GraphQL `createDoogat` surfaces it via `extensions.warnings` (mirrors the existing `graphql_create_doogat_surfaces_title_from_template_warning` test shape at `ddb-server/tests/graphql_response_warnings_t154.rs:17`).
- **e2e (`tests/e2e/concurrent_writes.rs`)**: remove the warm-up workaround (`:102-117`); assert 8 concurrent `ddb create` on a truly cold repo (no stored HEAD, no warm-up) all succeed with zero FTS bootstrap errors and a row count matching git. Per CLAUDE.md, `cargo test -p ddb-e2e` runs after this task since it replaces an existing code path (the warm-up workaround).
- **Smoke/integration scenarios**: `tests/smoke.sh`/`.ps1` — `ddb reindex --strict` on a corpus with a bad file exits non-zero naming it; `ddb reindex` (lenient) on the same corpus exits 0 with a warning count. `tests/integration.sh`/`.ps1` — a synced poison file surfaces its skip warning via a GraphQL `createDoogat` mutation's `extensions.warnings`.

## Review log

- Dispatch 1 (claude) findings, fixed in this doc:
  - BLOCKING: `incremental_reindex`'s `diff_paths`-failure fallback (`rebuild.rs:83`) also called `rebuild()` unlocked, a third racy site missed by the original two-site `locked_rebuild` rerouting. Fixed: now also routed through `locked_rebuild`.
  - BLOCKING: `db_dir` derivation via bare `path.parent()` returns `Some("")` (not `None`) for a relative filename like `":memory:"`, which two existing test call sites (`attachments.rs:209`, `id_minting.rs:180`) use directly — would have silently locked a cwd-relative file instead of skipping the lock. Fixed: added an empty-parent filter and documented the two test sites for follow-up migration to `open_in_memory()`.
  - NON-BLOCKING (documented, not code-fixed): reindex-skip warnings captured up front are lost if the facade's own operation subsequently returns `Err` (no warnings channel on the error path in this codebase). Documented in Risks.
  - NON-BLOCKING (documented, not code-fixed): the original doc overstated the `ConsistencyWarning` match-site burden — no exhaustive match over the enum exists in the crate today, so the new variant needs no existing-call-site updates. Corrected in Interfaces & contracts.
  - QUESTION (documented, not resolved): `REBUILD_LOCK_TIMEOUT = 30s` is an unvalidated guess against no stated corpus-size ceiling. Documented in Risks as a `/plan-tasks` decision point.
dispatch 1 (claude): cardinal-sin 0, blocker 2, non-blocker 2, question 1
- Dispatch 2 (claude-fallback — codex ran an extensive source investigation but its session terminated without ever emitting a final structured answer; treated as a codex outage per the skill's fallback rule, raw transcript at `dev/local/tmp/design-codex-output-00169.txt`) findings, fixed in this doc:
  - BLOCKING: CLI `ddb create`'s pre-existing redundant `svc.rebuild_if_stale()?` call (`crud.rs:35`) pre-warms the index and swallows `ensure_fresh`'s warnings before `svc.create(cmd)`'s own explicit call ever sees them — the PRD's own CLI conformance case would have silently failed. Fixed: delete that call site.
  - BLOCKING: `write_lock::acquire`'s call-site count was undercounted (claimed one, actually three — two direct test call sites in `git_ops/tests.rs` also use the old 2-arg signature). Fixed: added to Module placement, corrected in Reuse inventory.
  - BLOCKING: the FFI warnings mapping referenced an undefined `describe_consistency_warning_code` and called `describe_consistency_warning` across a module boundary without `pub(crate)` visibility — would not compile. Fixed: both functions defined and made `pub(crate)`.
  - NON-BLOCKING (documented, not code-fixed): `incremental_reindex`'s new `strict` param breaks ~9 more test/bench call sites not originally listed. Added to Module placement.
  - NON-BLOCKING (documented, not code-fixed): `ReadPool`'s `skip_stale_check` readers bypass the rebuild lock entirely — a second accepted residual gap, parallel to the bundle-import one. Documented in Risks.
  - NON-BLOCKING (documented, not code-fixed): warning message strings (the `error` field) have no length cap, undercutting the "avoid flooding the envelope" rationale on the count side only. Documented in Risks.
  - QUESTION (documented, not resolved): whether `DoogatService::rebuild_if_stale()`'s other 9 non-`AppOutput` callers silently discarding warnings is deliberate or an oversight. Resolved as deliberate (out of scope) while fixing blocker 1.
dispatch 2 (claude-fallback): cardinal-sin 0, blocker 3, non-blocker 3, question 1
- Dispatch 3 (claude-fallback — a fresh `codex` attempt was run with a deliberately narrowed, bounded verification prompt (see `dev/local/tmp/design-codex-output-00169-v2.txt`); it again terminated mid-investigation without a final structured answer, a second consecutive outage, but its partial transcript surfaced one real, independently-verified lead before dying, which the orchestrator confirmed directly against source and fixed; a fresh Claude reviewer subagent then completed the mandatory conditional dispatch — required because dispatch 2 found blockers) findings, fixed in this doc:
  - BLOCKING (surfaced by the failed codex session's partial transcript, confirmed by the orchestrator directly against `app_contract/mod.rs`, fixed before the Claude dispatch-3 subagent ran): `pub(crate)` on `describe_consistency_warning`/`describe_consistency_warning_code` was insufficient — `app_contract::output` is itself a private module (`mod output;`), so a sibling module (`ffi::driver`) could not reach either function regardless of their own visibility. Fixed: added `pub(crate) use output::{describe_consistency_warning, describe_consistency_warning_code};` to `app_contract/mod.rs`, matching the existing `pub use output::{AppOutput, AppWarning};` re-export pattern.
  - BLOCKING (found by the Claude dispatch-3 subagent, independently confirmed by the orchestrator via a standalone `rustc` type check before this edit): the FFI construction assigned `describe_consistency_warning_code`'s `&'static str` return directly into `RebuildWarningRecord.code: String` — a type mismatch (`E0308`). Fixed: added `.to_string()`, matching the exact precedent (`SchemaWarningRecord`'s own `code: w.code.to_string()`) this design already claimed to mirror.
  - Items 1-3 of the dispatch-3 verification (CLI create-path fix, `write_lock::acquire`/`incremental_reindex` call-site completeness) all independently re-verified PASS, no further findings.
dispatch 3 (claude-fallback): cardinal-sin 0, blocker 2, non-blocker 0, question 0
