# Sync & Compaction

## Sync Manager

**Source**: `ddb-core/src/sync_manager/mod.rs`

Orchestrates multi-device synchronization.

### SyncManager

```rust
pub struct SyncManager<'a> {
    pub repo: &'a GitRepo,
    pub node: NodeConfig,
}
```

### Node Registration

`register_node(repo, name) -> Result<NodeConfig>`

1. Generate UUIDv4
2. Create `NodeConfig` with name, uuid, empty `known_heads`
3. Write `.nodes/{uuid}.toml` (Git-tracked)
4. Write UUID to `.git/ddb-node` (local-only, not tracked)
5. Commit the `.nodes/` file

The local `.git/ddb-node` file identifies which node this device is.

### Auto-Registration

When `DoogatService::sync()` is called and no node is registered (`.git/ddb-node` missing), the node is automatically registered using the system hostname. This removes the need to run `ddb register-node` before the first sync. Users can still call `ddb register-node <name>` explicitly to choose a custom name.

### Opening

`SyncManager::open(repo) -> Result<Self>`

Reads the UUID from `.git/ddb-node`, then loads the corresponding `.nodes/{uuid}.toml` from the Git tree.

### Full Sync Cycle

`sync(remote, branch, index) -> Result<SyncReport>`

1. **Fetch**: `git fetch {remote} {branch}`
2. **Merge**: `merge_remote(remote, branch)` → get `MergeResult`
3. **Handle result**:
   - `AlreadyUpToDate` → report "up-to-date", skip push
   - `FastForward` → report 1 commit transferred
   - `Clean` → report 1 commit transferred (Git auto-committed)
   - `Conflicts` → three-bucket partition and resolution (see below)
4. **Update state**: set `known_heads = [current HEAD]`, `last_sync = now`, commit `.nodes/{uuid}.toml`
5. **Push**: single `git push {remote} {branch}` carries both merge result and node state
6. **Commit-graph**: write once (deferred from per-commit writes during sync)
7. **Reindex**: `index.rebuild_if_stale(repo)` — incremental reindex via `diff_paths` processes only changed files

> **Write-lock coverage**: the merge step's write section (the merge or fast-forward commit, the ref move, and the forced checkout) holds the repo-scoped cross-process write lock, and the conflicted-merge path additionally rebuilds its tree from a fresh in-memory `merge_commits` three-way merge re-run under the lock (as of PRD 00200), so a `ddb sync` racing a concurrent locked CRUD write can neither revert that write's working-tree file nor orphan its commit. See `git-ops.md` § "Write serialization (cross-process lock)". Bundle import's `git merge` subprocess is the one merge path still outside the lock (tracked by 00168).

### Four-Bucket Conflict Partition

When `merge_remote()` returns `Conflicts`, the conflict list is partitioned into four buckets before resolution:

| Bucket | Condition | Handling |
|--------|-----------|----------|
| **binary-ref** | `path.starts_with("reference/")` | LWW via HLC on raw blob bytes |
| **delete-vs-edit** | `ours` or `theirs` is empty | Edit wins; `resurrected: true` marker added |
| **add-add** | `ancestor.is_none()` and both sides non-empty | Winner keeps ID; loser reassigned (see below) |
| **normal** | Everything else | Three-step merge cascade |

Each bucket is resolved independently. The resolved files from all four are collected into a single merge commit.

### Three-Way Merge Tree Construction

The four buckets above only cover paths Git itself reports as conflicting. Everything else — a path only one side touched, including a plain deletion — is not a conflict and never enters bucket partitioning at all. As of PRD 00200, the merge commit (`commit_merge`) rebuilds its tree as a true three-way merge: it re-runs `merge_commits(our_commit, their_commit)` in memory and overlays only the four buckets' resolved blobs onto the conflicting paths. Every non-conflicting path is already correct at stage 0 of that re-run, so it is committed as-is.

This distinguishes two deletion scenarios that are easy to conflate:

- **Plain deletion, untouched by the other side**: not a conflict, never enters the delete-vs-edit bucket. The path is simply absent from the three-way merge result, so the deletion survives in the final tree.
- **Delete-vs-edit** (the **delete-vs-edit** bucket above): a real conflict — one side deletes, the other edits the *same* doogat. This keeps the deliberate resurrect-with-marker policy: the edit wins and `resurrected: true` is added to the frontmatter. Unchanged by PRD 00200.

Before PRD 00200, the merge tree was built by replaying a two-way ours→theirs diff over the resolved paths only, which silently reverted ours' non-conflicting edits back to the base and resurrected paths the other side had plainly deleted (see `invariants.md` § I2).

#### Per-path outcome

| Path shape | Final tree | Before PRD 00200 |
|---|---|---|
| both edit same region | resolved by the bucket above (CRDT/LWW/binary) | resolved (ok) |
| ours edits only | ours' edit kept | reverted to base |
| ours clean-deletes | absent | resurrected |
| theirs deletes, ours untouched | absent | resurrected |
| theirs-only add/edit | theirs' change | theirs' change (ok) |
| ours creates | present | present (ok) |
| both edit, auto-mergeable (different regions) | both edits (git line-merges) | theirs' whole blob |
| theirs renames | renamed path only | old path resurrected → duplicate |
| delete-vs-edit (theirs deletes, ours edits) | resurrect + `resurrected: true` marker | resurrect + marker (ok, unchanged) |

### Binary Asset LWW Resolution

Binary files under `reference/` (attachments, images, etc.) are resolved via LWW using HLC timestamps from the conflicting commits. The branch with the higher HLC wins. On tie or missing HLC, the side with the higher blob OID wins — a role-independent, content-deterministic fallback that converges regardless of which device calls a side "ours" or "theirs" (PRD 00166). Every LWW site shares one `lww_pick` helper (see [crdt-resolver.md](crdt-resolver.md#preset-last-writer-wins)).

Resolution uses raw blob bytes via `ConflictFile.ours_blob_oid` / `theirs_blob_oid` to avoid corruption from UTF-8 lossy conversion. As of PRD 00200 the winning blob is overlaid by its OID onto the conflicting path in the in-memory merge tree (no pre-commit worktree write) and materialized to disk by the post-commit `checkout_head`, alongside text conflict resolutions; an aborted merge therefore strands no stale winner bytes. The losing version remains accessible in Git history as a parent of the merge commit.

### Add-Add Collision Detection

When two devices independently create a doogat with the same ID (same-second creation), Git sees a content conflict at that path with no common ancestor. Previously, the CRDT/LWW cascade would pick one version and silently lose the other. Now both doogats survive.

**Winner selection**: Compare HLC timestamps from the conflicting commits. Later HLC wins. On tie or missing HLC, the side with the higher content key wins — the same role-independent `lww_pick` fallback used by every LWW site (PRD 00166), so both devices pick the same winner regardless of merge direction. (Before PRD 00166 this defaulted to "theirs wins", which disagreed with the text-LWW "ours wins" default; the unified content-key rule reconciles them.)

**Merge commit**: The winner's content goes into the resolved vec alongside other conflict resolutions, keeping the original path.

**Post-merge loser reassignment** (`reassign_collision_losers()`):

1. Generate a new unique ID via `generate_unique_id()`, checking both filesystem and the winner's ID for collisions.
2. Update the loser's frontmatter `id` field to the new ID.
3. Compute the new path via `doogat_path(new_id, type_name, folder)`, respecting folder-typed storage (e.g., `ddb/contact/{new_id}.md`).
4. Walk the HEAD tree scanning all `.md` files for references to the old ID. For each file containing the old ID, call `rewrite_links()` twice: once for the bare ID form, once for the path form (minus `.md`).
5. Commit the loser at its new path plus all rewritten files atomically: `fix: reassign collided doogat ID {old} -> {new}`.
6. Emit `tracing::warn!` with old/new IDs and paths.

**Reporting**: `SyncReport.collisions_reassigned` counts reassigned doogats. The CLI displays this when > 0.

### Three-Step Merge Cascade

Normal conflicts (those with a common ancestor and both sides present) go through a three-step cascade:

1. **Step 1: Git merge** — already performed by `merge_remote()`. If clean, validate affected files with `parser::parse()`. Invalid → extract pre-merge versions, fall through.
2. **Step 2: CRDT resolve** — call `resolve_conflicts()` with the typedef's `crdt_strategy` (or repo `default_strategy`). Validate result. Invalid or error → fall through.
3. **Step 3: LWW by HLC** — whole-file last-writer-wins using HLC comparison. Always produces a valid file.

This replaces the previous "ours-wins" fallback with a proper HLC-based resolution.

### State Update

`update_sync_state() -> Result<()>`

Sets the node's `known_heads` to the current HEAD OID and `last_sync` to the current UTC timestamp (RFC3339). Commits the updated `.nodes/{uuid}.toml`.

This propagates to other nodes on their next fetch, allowing compaction to compute the shared head.

### Listing Nodes

`list_nodes() -> Result<Vec<NodeConfig>>`

Walks `.nodes/*.toml` files in the HEAD tree, deserializes each into `NodeConfig`.

## Hybrid Logical Clocks

**Source**: `ddb-core/src/hlc.rs`

HLC combines wall clock time, a logical counter, and node ID for causally-ordered timestamps across devices.

```rust
pub struct Hlc {
    pub wall_ms: u64,   // wall clock milliseconds since UNIX epoch
    pub counter: u32,   // logical counter for same-millisecond events
    pub node: String,   // first 8 chars of node UUID (deterministic tie-break)
}
```

### Operations

- **`Hlc::now(node_id, &last)`** — tick for local event: `max(wall_clock, last.wall_ms)`, increment counter if equal
- **`Hlc::recv(node_id, &local_last, &remote)`** — merge on receive: `max(wall, local, remote)`, bump counter on ties
- **`Hlc::parse(s)` / `Display`** — sortable format: `{wall_ms}-{counter:04}-{node}`
- **`Ord`** — compare wall_ms → counter → node (deterministic total order)

### Integration

As of PRD 00166 the HLC is **load-bearing** for LWW ordering: every write commit carries a trailer, remote clocks are absorbed on merge, and one content-deterministic fallback covers the missing/tie case.

- **The write clock (`HlcClock`, `.git/ddb-hlc`)**: a machine-local, monotonic HLC counter that stamps a trailer on **every** write commit — create, update, delete, rename, and batch, not just merge commits. `GitRepo` loads it in `open`/`init` and ticks it at one private `create_commit` chokepoint that all write paths route through, so no commit path can ship a trailer-less commit. Its state is the last-issued HLC, persisted atomically (temp + rename) in the untracked `.git/ddb-hlc` file — the same "local, per-device, not git-tracked" pattern as `.git/ddb-node`. The per-commit cost is one small filesystem write under the already-held repo write lock, not an extra git object. This clock is the **only** source of commit-trailer HLCs.
- **Commit trailers**: `\n\nHLC: {hlc}` in `Hlc::Display` form (`{wall_ms}-{counter:04}-{node}`), parsed via `extract_hlc()`. Because ordinary content commits now carry a trailer, `find_hlc_for_path()` returns `Some(hlc)` at the first commit touching a path. It still caps the ancestry walk at `MAX_REVWALK_DEPTH` (1000) commits, logging a `warn` and returning `None` if a path's nearest touch lies beyond that — a miss is then observable rather than a silent degrade.
- **Absorbing a remote clock**: all three HEAD-advancing sync outcomes advance the local clock past a peer's, under the write lock — the two merge-commit sites (`commit_merge`, `perform_normal_merge`) and the fast-forward branch (which produces no commit) each `extract_hlc()` theirs' commit and call `HlcClock::recv`. A device whose wall clock ran ahead therefore stops winning indefinitely once a peer absorbs its clock. The live merge `recv` is `HlcClock::recv` on the shared write clock, not the older, still-uncalled `SyncManager::recv_hlc`.
- **`SyncManager::tick_hlc` / `NodeConfig.hlc`**: a **separate**, in-memory per-process marker, deliberately NOT the write clock. It still stamps the singleton sweep's `singleton_conflict_resolved_at` frontmatter marker; the sweep's own commit is auto-stamped by the shared write clock. The two clocks may drift harmlessly — they never feed the same decision.
- **ConflictFile**: HLC fields populated from commit trailers for LWW resolution. `extract_conflicts()` calls `find_hlc_for_path()` to walk commit ancestry and extract HLC from the most recent commit touching each conflicting path. `validate_clean_merge_or_fallback()` does the same for post-merge validation conflicts.
- **Corruption recovery**: `.git/ddb-hlc` is a derived cache; the committed trailer on `HEAD` is the durable source of truth. On load the clock seeds from `max(.git/ddb-hlc, extract_hlc(HEAD))` and repairs the file, so a wiped or torn cache can never regress the clock below the HLC stamped on a trailer-carrying `HEAD`. Recovery seeds from `HEAD`'s own trailer, not deeper ancestry, so a trailer-less tip (legacy history, or a bundle-import merge — 00168) with a wiped cache falls back to wall-clock time rather than a committed HLC. An in-memory floor blocks regression within a process even if the file becomes unreadable. The node discriminator (last in `Hlc::Ord`) may be an ephemeral id before `register_node` writes `.git/ddb-node`; this only affects the node tie-break field, never `wall_ms`/`counter` ordering.

## Compaction

**Source**: `ddb-core/src/compaction/mod.rs`

Cleans up temporary CRDT files, merges per-doogat CRDT docs, and runs Git garbage collection. Reports before/after storage measurements.

### Shared Head Calculation

`shared_head(repo, nodes) -> Result<Option<Oid>>`

Finds the greatest common ancestor (GCA) commit across all **active** nodes' `known_heads`. Stale and retired nodes are excluded — this allows compaction to proceed even when some devices are offline.

1. Collect the first `known_head` from each active node
2. If only one node, return its head directly
3. Iteratively compute `merge_base()` across all heads

The shared head represents the latest commit that all active devices have synced. Anything before it is safe to compact.

### CRDT Temp Cleanup

`cleanup_crdt_temp(repo, shared_head) -> Result<usize>`

Removes files in `.crdt/temp/` whose commit OID is an ancestor of the shared head (i.e., all devices have already applied those changes). Preserves `.gitkeep` and files newer than the shared head.

### CRDT Doc Compaction

`compact_crdt_docs(repo) -> Result<usize>`

Groups remaining CRDT temp files by `(doogat_id, is_frontmatter)` and merges multiple Automerge documents per group into a single compacted doc. Body and frontmatter are compacted independently.

### Git GC

`run_gc(repo_path) -> Result<bool>`

Runs `git gc` (not `--aggressive`) for pack consolidation and object deduplication.

### Full Pipeline

`compact(repo, sync_mgr, force) -> Result<CompactionReport>`

1. **Threshold check**: skip if `.crdt/temp/` < `threshold_mb` (unless `force`)
2. Measure `.crdt/temp/` size and file count (before)
3. Measure `.git/` directory size (before)
4. Compute shared head from active nodes
5. Clean up CRDT temp files older than shared head
6. Compact CRDT docs per doogat
7. Measure `.crdt/temp/` size and file count (after)
8. Run `git gc`
9. Measure `.git/` directory size (after)

### CompactionReport

```rust
pub struct CompactionReport {
    pub files_removed: usize,        // temp files deleted in step 5
    pub crdt_docs_compacted: usize,  // doogats merged in step 6
    pub gc_success: bool,            // git gc exit status
    pub crdt_temp_bytes_before: u64, // .crdt/temp/ bytes before cleanup
    pub crdt_temp_bytes_after: u64,  // .crdt/temp/ bytes after compaction
    pub crdt_temp_files_before: usize,
    pub crdt_temp_files_after: usize,
    pub repo_bytes_before: u64,      // .git/ bytes before gc
    pub repo_bytes_after: u64,       // .git/ bytes after gc
}
```

## Conflict Resolution Cascade

When a sync detects conflicting changes, resolution follows a three-step cascade (see `cascade_resolve` in `sync_manager/mod.rs`):

```
Step 1: Git three-way merge (libgit2)
  ↓ if conflicts remain
Step 2: CRDT per-zone merge (Automerge)
  ↓ if result fails validation (parser::parse)
Step 3: LWW by HLC (whole-file, later timestamp wins)
  ↓ if LWW also fails (shouldn't happen)
Step 4: Ours-wins (last resort)
```

### Step 2: CRDT merge (default strategy)

Each conflicting file is split into three zones and resolved independently:

| Zone | Strategy | What wins on conflict |
|------|----------|----------------------|
| Frontmatter scalars | Automerge Map CRDT | Deterministic by actor/op ordering |
| Frontmatter lists | Three-way set merge | Union of additions, removals honored |
| Body | Automerge Text CRDT | Non-overlapping edits merge; overlapping resolved by CRDT |
| References | Automerge List CRDT | Union, deduplicated, sorted alphabetically |

### Step 3: LWW fallback

Triggered when Step 2 produces invalid output or errors (e.g., corrupted CRDT state). Compares HLC timestamps on the conflicting commits; the later writer's **entire file** replaces the earlier one. If an HLC is missing on either side or the two tie, the side with the higher content key wins — a role-independent fallback that converges under an ours/theirs swap (PRD 00166), replacing the earlier "ours wins" default.

### After compaction

When CRDT temp files have been compacted away, Step 2 still runs — it reconstructs the three-way merge from `ancestor`/`ours`/`theirs` content in Git. The difference is that prior CRDT operation history is lost, so the merge creates fresh Automerge docs rather than extending existing ones. In practice this produces identical results for most edits.

If the reconstructed merge produces invalid markdown (rare edge case), the cascade falls through to Step 3 (LWW).

### Preset strategies

| Strategy | When used | Behavior |
|----------|-----------|----------|
| `preset:default` | No typedef or typedef doesn't specify | Per-zone CRDT merge (Step 2) |
| `preset:last-writer-wins` | Typedef specifies LWW | Skip Step 2; go straight to HLC comparison |
| `preset:append-log` | Typedef specifies append-log | Frontmatter + references use CRDT; body log sections use union + chronological sort |

### User-visible outcomes

| Scenario | Result |
|----------|--------|
| Non-overlapping edits | Both edits preserved |
| Same field edited on both sides | CRDT picks one deterministically (not random) |
| One side deletes, other edits | Edit wins; `resurrected: true` added to frontmatter |
| Both devices create same ID | Both doogats survive; loser gets new ID, links rewritten |
| Stale node returns after compaction | Step 2 runs from Git content; usually succeeds |
| CRDT error + no/tied HLC | Falls back to the higher-content-key side (role-independent, converges on both devices) |

**E2E tests proving these paths:**
- `stale_node_resync_after_compaction` — LWW fallback after CRDT state removed
- `stale_node_edits_deleted_doogat_after_compaction` — edit-vs-delete after compaction
- `multiple_stale_nodes_return_sequentially` — cascade through multiple compaction cycles
- `bundle_recovery_after_compaction` — bootstrap from post-compaction bundle

## Test Coverage

### Sync Manager (12 unit tests)
- Register and open node
- List nodes
- Open without registration fails
- Sync state update
- Node status defaults to active
- Retire and list nodes
- Backward compat: old TOML without status field
- Resurrected marker added (delete-vs-edit)
- Resurrected marker not duplicated
- Clean merge validation falls back to CRDT
- LWW fallback when CRDT produces invalid output
- Sync error resets skip-commit-graph flag

### Compaction (4 tests)
- GC runs successfully
- Cleanup empty temp directory
- Cleanup removes temp files (preserves `.gitkeep`)
- Full compact pipeline

### Integration (2 tests in `tests/sync_test.rs`)
- Two-node sync without conflicts
- Two-node sync with conflict resolution (both nodes reach identical state)

### Multi-device simulation (6 tests in `tests/e2e/multi_device.rs`)
- 3-node convergence (all edit, sync round-robin, verify identical state)
- Concurrent creates (all nodes create simultaneously, sync cascade)
- Stale node return (one node offline, others sync, stale returns)
- Network partition and reconnect (split groups, both edit, reconnect and merge)
- Bundle full bootstrap (export full from node, import on fresh node)
- Air-gapped delta transfer (export bundle, import on disconnected node)
- Stale node resync after compaction (conflict with compacted CRDT state, LWW fallback)

## Performance

### NFR-03: Two-node sync under 2s at 5K doogats

Measured on macOS (Apple Silicon), release build, localhost bare remote.

Scenario: 5000 doogats seeded, fast-forward sync of 10 new doogats.

| Phase | Before (ms) | After (ms) |
|-------|------------|-----------|
| fetch | 13 | 12 |
| merge | 65 | 44 |
| push | 18 (2 pushes) | 13 (1 push) |
| update_sync_state | 26 | 8 |
| reindex | 10101 | 24 |
| **total** | **10226** | **118** |

### Optimizations applied

1. **Incremental reindex** — replaced `index.rebuild()` (full 5K re-parse) with `rebuild_if_stale()` which uses `incremental_reindex` to diff `old_head..new_head` and process only changed files. 421x improvement for the reindex phase.
2. **Single push** — merged two pushes (content + node state) into one by reordering `update_sync_state()` before push.
3. **Deferred commit-graph** — `write_commit_graph()` skipped during mid-sync commits, written once at the end via `set_skip_commit_graph` flag on `GitRepo`.
