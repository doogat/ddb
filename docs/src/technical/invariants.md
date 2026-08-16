# System Invariants

Cross-cutting rules that keep ddb safe and coherent. Each is a contract the whole codebase depends on; breaking one is a defect even when the compiler and tests are green, because the failure mode is silent (lost data, divergent nodes, drifted transports). AGENTS.md carries the short enforceable list; this page carries the rationale, the evidence, and — honestly — where the codebase does not yet hold the invariant and which PRD closes the gap.

Status legend: **HOLDS** (enforced today) · **PARTIAL** (part shipped, remainder tracked) · **GAP** (invariant defined, code not yet compliant, PRD tracked). PRD numbers refer to `dev/local/prds/`; re-synced to the renumbered backlog on 2026-07-16 (harness-migration PRD inserted at 00171; former 00171-00198 shifted +1).

## Data-safety invariants

### I1 — Every git write holds the repo write lock · HOLDS (commit/merge/ref-delete paths; 00162, 00163, 00200, 00168)
All commit/delete/rename paths must run inside one repo-scoped, cross-process advisory lock, and must build their tree from `fresh_index()` (never the cached `repo.index()`).

**Why**: without a lock, two processes (a downstream app's `ddb serve` and a user's CLI, or two scripted `ddb create`s) build trees from stale HEADs and overwrite each other's commits with no error; `git maintenance --auto` then prunes the orphaned commit permanently. This is the highest-severity failure class in the system.

**Current state**: the cross-process advisory lock SHIPPED 2026-07 as **00162** (cross-process-write-lock): `ddb-core/src/git_ops/write_lock.rs`, the eight CRUD/rename write functions wrapped in `with_write_lock`. **00163** (git-tree-construction-correctness) then closed two more legs: (a) `delete_file`/`delete_files` now build from `fresh_index()` instead of the stale cached index; (b) the merge/sync write path — `git_ops/merge.rs` `commit_merge`, `merge_remote`'s normal-merge commit + forced checkout (`perform_normal_merge`), and the fast-forward `set_target` + forced checkout — now runs inside `with_write_lock` (`git commit-graph write` relocated outside the locked section). As of **00200** the conflicted-merge path builds its tree from libgit2's `merge_commits` result, not a hand-staged `fresh_index()` (see I2 and `git-ops.md` § "Write serialization"); the normal 3-way merge commits from libgit2's merge-result index and the fast-forward path stages no tree. **00168** (bundle-import-conflict-recovery) closed the merge leg: `merge_bundle_and_resolve` no longer shells out to a CLI `git merge` — it calls `GitRepo::merge_remote` + `SyncManager::apply_merge_result`, the same write-locked libgit2 merge engine `sync()` uses — and `import_bundle` deletes `refs/remotes/bundle/master` via the new write-locked `GitRepo::delete_remote_ref`, only after a successful merge commit. Every git write that commits, merges, moves a ref, or moves the checkout now holds the repo write lock. One gap remains: bundle import's object-preparation leg (`unbundle_git_objects` in `ddb-core/src/bundle.rs`, running `git bundle unbundle` and `git fetch --no-prune ... refs/heads/*:refs/remotes/bundle/*`) still runs as an unlocked subprocess, before the locked merge. That leg only populates the object store and the side `refs/remotes/bundle/*` namespace; it never moves `HEAD` or the working tree, which is why the design has accepted it so far.

**Rule for new code**: any new write path goes through `with_write_lock`. If you find yourself calling `repo.index()` in a write, stop — use `fresh_index()`.

### I2 — Merges preserve the other side's deletions · HOLDS (00200)
Merge tree-construction is a true three-way merge (libgit2 `merge_commits`), so a path only the other side deleted is simply absent from the result — never resurrected. (Staging `Delta::Deleted` onto a two-way diff was the naive fix 00200 rejected: it deletes doogats ours created since the merge base.)

**Why**: skipping deleted deltas silently resurrects a doogat the other device deleted, and turns a rename (delete+add) into a duplicate. The resurrect-with-marker policy in `sync_manager` only sees git-reported conflicts, so it cannot catch this.

**Current state**: closed by **00200** (make-merge-tree-three-way). `commit_merge` now rebuilds the conflicted-merge tree as a true three-way merge — it re-runs `merge_commits(our_commit, their_commit)` in memory and overlays only the CRDT-resolved blobs onto the conflicting paths; every non-conflicting path (theirs' plain deletions, ours' non-conflicting edits, renames) is already correct at stage 0, so libgit2 preserves it instead of the old two-way ours→theirs diff silently resurrecting or reverting it. See `git-ops.md` § "Merge" for the mechanism and `crdt-resolver.md` / `sync.md` for the per-path outcome table.

### I3 — IDs are minted through one repo-aware path · HOLDS (00164; throughput-ceiling decision tracked as 00170)
No mint path uses `exists = |_| false` or allocates future-dated timestamps; all minting checks actual repo/index existence.

**Why**: three minting paths disagree today (`generate_unique_id`'s per-process static, the raw-FFI/`install_bundled_type` paths with no repo check, and `unique_ids` which claims future seconds). A create landing in a second a batch already claimed overwrites it. The 14-digit second-resolution format caps throughput at ~1 create/sec/process — a known limitation, tracked as an explicit decision in **00170** (id-throughput-ceiling-decision), not something to patch ad hoc.

**Current state**: the invariant as stated above HOLDS. The four paths that used to break it — `parser::generate_unique_id`, the `service/create.rs` raw path, `service/discovery.rs`, and `sql_engine::unique_ids` — were unified onto one repo-aware existence oracle (HEAD tree + `doogats` index) by **00164** (unify-id-minting); none of them now uses `exists = |_| false` or allocates future-dated timestamps. What remains open is not a correctness gap but a format decision: the 14-digit second-resolution ID caps throughput at ~1 create/sec/process, an accepted, tracked limitation whose resolution is **00170** (id-throughput-ceiling-decision). Do not patch that ceiling ad hoc.

**A second, deliberately separate mint path**: PRD 00167 (atomic sync collision-loser reassignment) adds `id_minting::derive_content_id`, a deterministic sibling of `existence_oracle` for reassigning a collision loser's ID. It is content-addressed (seeded from `(old_id, losing_blob_oid)`) rather than wall-clock-minted, and checks existence against the in-memory merge-tree index being built inside `commit_merge` — the freshest possible state, with no separately-queried oracle and no TOCTOU window. This path is additive rather than a fix to the four paths above: it is repo-aware by construction (never `exists = |_| false`, never a future-dated timestamp), so it satisfies I3 on its own terms without being what closed it — 00164 did that. Its existence check covers every ID already present anywhere under `ddb/` in the merge tree being built, including other type folders and `ddb/_typedef/`.

### I4 — Conflict resolution is a pure function of its inputs · HOLDS (00165, 00166)
Set stable Automerge actor IDs (derived from ours/theirs blob OIDs or node UUIDs). If a decision claims HLC ordering, the commits that feed it must carry an HLC trailer.

**Why**: Automerge breaks scalar/text ties by actor ID. With random actor IDs, two nodes resolving the same conflict independently pick different winners and never converge in one round. And an HLC that no write commit carries cannot order anything — the LWW decisions it governs fall through to role-dependent defaults that disagree by path ("theirs wins" for add/add, "ours wins" for text LWW), so two devices diverge.

**Current state**: closed by **00165** (deterministic-crdt-resolution) — `crdt_resolver.rs` sets a content-derived actor ID at every doc-build site — and **00166** (make-hlc-load-bearing): every `git_ops` write commit now stamps an `HLC:` trailer from a machine-local clock (the private `create_commit` chokepoint + untracked `.git/ddb-hlc`), merges and fast-forwards absorb the peer's HLC via `HlcClock::recv`, and one content-deterministic `lww_pick` fallback reconciles the two previously-disagreeing missing-HLC defaults. See `sync.md` § "Hybrid Logical Clocks" and `crdt-resolver.md` § "Preset: Last-Writer-Wins".

### I5 — One bad file never fails the batch · HOLDS (00169; quarantine listing 00181)
Read/list/index paths degrade per-item into structured warnings; only an explicit `--strict` mode hard-fails.

**Why**: doogats are plain markdown users edit with other tools, and sync brings untrusted content. The full rebuild already collects per-file warnings, but the incremental path (`batch_index_changes`) aborts the whole batch on one unparsable file — and `ensure_fresh` gates ~40 entry points on it, so one stray file turns every read *and* write into an error.

**Current state**: closed by **00169** (poison-file-reindex-resilience). `batch_index_changes` now collects each file's read/parse failure as a `ConsistencyWarning` (`UnreadableFile` for the read stage, `MalformedYaml` for the parse stage) and skips that path instead of aborting the batch — the same collect-and-skip shape the full rebuild's `parallel_parse` already used — so one stray file no longer turns every `ensure_fresh`-gated read *and* write into an error. The old fatal behavior survives as an opt-in: `incremental_reindex` threads a `strict` flag into `batch_index_changes` (always `false` from the implicit/background path), while the user-facing `DoogatService::reindex_strict()` and `ddb reindex --strict` route through the equivalent strict full-rebuild path. Skips are never silent — every skipped file is logged with its path. Only the `create`, `update`, and `apply_schema` verb families attach one summarized `REINDEX_SKIPPED_FILES` `AppWarning` to their result envelope; the summary counts only genuinely skipped files (`UnreadableFile` and `MalformedYaml`), deduplicates them by path, and renders the first file plus ``(+N more; run `ddb reindex` for the full list)``. Advisory warnings for files that were indexed (`CrossZoneDuplicate` and `MissingRequired`) do not produce that envelope warning. The other `ensure_fresh`-gated verb families — read, search, SQL/query, discovery, batch, delete, and utility — currently discard the returned warnings, so their skips reach the user only through the per-file `tracing::warn!` line; **00176** (route-search-through-contract) owns closing that remaining envelope gap. FFI's explicit `reindex()` separately exposes the raw per-file warnings as `RebuildWarningRecord` values. The parser also caps frontmatter size at the parse boundary, so an oversized synced file degrades into the same lenient skip rather than a fatal error or an unbounded parse. One follow-up remains: the per-file quarantine *listing* surface is **00181** (doctor); until it ships, the full list lives only in the logs.

### I10 — The destructive index rebuild is serialized across processes · HOLDS (00169; two documented gaps)
Every implicit path to the full `rebuild` holds one cross-process advisory lock on the index directory (`<index-dir>/ddb-rebuild.lock`). The git write lock and the index rebuild lock are never held simultaneously by the same call stack; if a future change needs both, fully release one before acquiring the other — never acquire the second while the first is still held.

**Why**: `rebuild` drops every table before repopulating it. On a cold index (no stored HEAD in `_ddb_meta`) *every* process's `ensure_fresh` takes that path, so unserialized cold-start writers stomp on each other's `_ddb_fts`/`_ddb_boost` bootstrap and the loser dies with `no such table: _ddb_fts` — a plain `ddb create` failing because another one started at the same moment. The repo write lock (I1) does not cover this: it serializes git commits, not SQLite index bootstrap. Two locks also mean a nesting hazard, which is why the ordering rule above is fixed once at design time rather than per call site.

**Current state**: shipped by **00169**. `Index::locked_rebuild` takes `<index-dir>/ddb-rebuild.lock` through the same `git_ops::write_lock::acquire` primitive the git lock uses, then re-checks integrity and staleness before doing anything destructive, so a process that queued behind another's rebuild skips its own instead of repeating it. All three implicit routes to `rebuild` go through it: `rebuild_if_stale`'s corruption branch, its no-stored-HEAD branch, and `incremental_reindex`'s `diff_paths`-failure fallback. `locked_explicit_rebuild` holds the same lock for user-invoked `ddb reindex` (and `--strict`), unconditionally — the caller asked for a rebuild, so it does not re-check. `Index::configure_connection`'s schema-version drop-and-recreate takes that lock too, and holds it across the recreate *and* the version stamp, not just the drop loop: a second process acquiring it mid-window would still read the old version and drop what the first had just recreated. The ordering rule is recorded in `git_ops/write_lock.rs`'s module doc; today nothing nests them at all (git commits live in `git_ops/`, rebuilds in `indexer/`, and neither call chain reaches the other). Two gaps remain, both accepted at design time rather than silently claimed as covered: (a) `ddb-core/src/bundle/mod.rs:229` calls `index.rebuild(repo)` directly, outside the lock — bundle import serializes its own git work under the write lock (I1) and is not part of this invariant's concurrency matrix; (b) `ddb-server`'s `ReadPool` readers set `skip_stale_check(true)` (`read_pool.rs:253,270`), so `ensure_fresh` never reaches `locked_rebuild` for them — a reader issuing a raw query while another connection is mid-`drop_all_tables()` can still hit `no such table: _ddb_fts` by that route.

**Rule for new code**: any new path that can reach the destructive `rebuild` goes through `locked_rebuild`/`locked_explicit_rebuild` — and never while the git write lock is held.

## Coherence invariants

### I6 — One error policy · GAP (00173; FFI/PgWire legs 00177/00180)
Exactly one table maps a code → (category, HTTP status, redaction, FFI variant). Transports derive from it and never re-decide redaction or status.

**Why**: today four mappers disagree (`classify` for GraphQL, `http_error` for REST/NoSQL, the FFI allowlist, and PgWire's raw pass-through), and `From<DoogatError> for AppError` has no `SqlEngine` arm — so a SQL error is `SQL_ERROR`+message on one path and redacted `INTERNAL_ERROR` on another. Every new code otherwise has to be taught up to four places.

**Current gap**: `ddb-server/src/error.rs`, `http_error.rs`, `ddb-core/src/ffi/records.rs:111-126`, `pgwire.rs`. Unified by **00173** (unify-server-error-mapping-paths); FFI/PgWire wired in **00177**/**00180**.

### I7 — App-contract-first, FFI included · GAP (00172, 00175-00178)
Every verb flows through `AppCommand → DoogatService → AppOutput`. FFI is a transport and uses the contract, not raw service methods. Do not add per-verb actor command/reply enum plumbing.

**Why**: the contract is only wired for create/update/applySchema; read/delete/search command DTOs are dead code, FFI CRUD takes raw markdown and can't surface warnings, and ~1000 LOC of twin-enum actor plumbing grows ~30 lines per verb. Finishing the contract is what makes transports thin and a generated client possible.

**Current gap**: `ddb-core/src/app_contract/` (unused DTOs), `ddb-core/src/ffi/driver.rs` (raw path), `ddb-server/src/actor/` (twin enums). Closed by **00172** (collapse-actor-plumbing) → **00175/00176/00177** (route read/delete, search, FFI CRUD) → **00178** (structured-fields input). Whether the remaining verbs (sql passthrough, sync/maintenance, discovery, batch) get AppCommand DTOs or are declared specialized surfaces is decided when this invariant flips to HOLDS.

### I8 — A result type has one core definition · GAP (00176)
Transports serialize the core type; they must not redefine field subsets. A new public result field lands in the core type first.

**Why**: `SearchResult` has 10 fields in core but FFI exposes 6 and REST 4 — three drifted copies. Any consumer that switches transports silently loses fields.

**Current gap**: `ddb-core/src/ffi/records.rs`, `ddb-server/src/rest.rs`. Closed by **00176** (route-search-through-contract).

### I9 — SQL identifiers are escaped; values are bound · HOLDS in where-filter/schema-apply, GAP in materializer (00179)
Never `format!` a user-influenced identifier (type name, column, id) into SQL without `escape_sql_ident`; validate type/field-key charset at the write boundary. Values are always `?N` bound parameters.

**Why**: two quoting helpers already exist — `escape_sql_ident` (`ddb-core/src/indexer/filter.rs:7`, used by the where-filter/search path) and `quote_ident` (`ddb-core/src/schema_diff/plan.rs:14`, used by schema-apply DDL, backed by `validate_identifier`). But the materializer and delete paths interpolate table/column/`dtype` bare-quoted with NEITHER. Names derive from attacker-influenceable frontmatter, so a `"`-bearing type name can corrupt the DDL (bounded to single-statement DoS/corruption, not exfiltration). The two-helper duplication is itself worth consolidating.

**Current gap**: `ddb-core/src/indexer/materialize.rs`, `ddb-core/src/service/delete.rs`, plus bare-quoted (schema-resolved) columns in `ddb-server/src/filter.rs`. Closed by **00179** (sql-identifier-escaping: one canonical helper in all three paths + charset-validate at the write boundary).

## Using this page

- Adding a write path, a transport, an error code, or a SQL-emitting path? Find the matching invariant above and satisfy it before you finish.
- A GAP invariant means the fix is already scoped — reference the PRD, don't re-solve it ad hoc, and don't regress the parts that already hold.
- When an invariant flips from GAP to HOLDS (its PRD lands), update the status here and remove the "current gap" note.
