> **ARCHIVED EVIDENCE for PRD 00200** (2026-07-16). This is the pre-split design doc of PRD
> 00163, review loop HALTED after dispatch 1 — kept because its `## Review log` carries the
> empirical (real-git) proof of W1/W2/W3 that 00200 is scoped around. Do NOT plan tasks from
> this doc; 00163 (reduced scope) gets a fresh design, and 00200's design phase re-takes the
> A-vs-B decision recorded here.

# Design: Git tree-construction correctness (PRD 00163)

PRD: `dev/local/prds/wip/00163-git-tree-construction-correctness-v1.md`
Design date: 2026-07-16 · Line refs verified at HEAD `8e4b4aa`.

## Architecture fit

All production changes land in the **git storage layer** (`ddb-core/src/git_ops/`), the
lowest layer of the Clean-Architecture stack: it owns git as the source of truth and is
consumed by `service/`, `sync_manager/`, and `bundle.rs`. One additive change lands in the
**CLI adapter** (`ddb-cli/src/commands/sync.rs`).

The layer already owns both primitives this PRD needs — they are simply not applied to the
merge and delete paths:

- `GitRepo::fresh_index()` (`git_ops/mod.rs:263-270`) — the "stage from fresh state" primitive.
- `GitRepo::with_write_lock()` (`git_ops/mod.rs:294-305`) — the re-entrant, repo-scoped,
  cross-process advisory write lock shipped by PRD 00162.

So this PRD adds **no new abstraction**. It closes three sites that bypass existing ones.
That is the whole design: the fix is application of existing primitives, not new machinery.

Dependency direction is unchanged: `merge.rs` already lives in the same `impl GitRepo` as the
lock, so it calls `self.with_write_lock` / `self.fresh_index` directly with no new trait
surface and no signature change anywhere. Callers (`sync_manager`, `bundle.rs`, `service/`)
are untouched.

**Invariant this restores** (`AGENTS.md` → Data-safety invariants, `docs/src/technical/invariants.md` I1):
"Every git write holds the repo write lock — merges, sync, and bundle import included" and
"Merges preserve the other side's deletions."

## Module placement

All **edits to existing files**. No new production files.

| File | Change | Maps to |
|------|--------|---------|
| `ddb-core/src/git_ops/merge.rs` | `collect_theirs_only_changes` returns adds **and** removes; `build_merge_tree_and_commit` stages from `fresh_index()` and applies removes; `commit_merge` body wrapped in `with_write_lock`; `merge_remote` body wrapped in `with_write_lock` | H1, H3 |
| `ddb-core/src/git_ops/mod.rs` | `delete_file:408` and `delete_files:438`: `self.repo.index()` → `self.fresh_index()` | H2 |
| `ddb-cli/src/commands/sync.rs` | one `outln!` for `report.resurrected` | CLI visibility |

New **test** files:

| File | Change |
|------|--------|
| `ddb-core/src/git_ops/tests.rs` | merge-deletion-preservation, theirs-rename, delete-freshness, merge-vs-concurrent-write (all unit; see Test strategy for why not e2e) |
| `ddb-core/src/sync_manager/tests.rs` | resurrect-policy-still-fires regression |
| `tests/integration.sh` + `tests/integration.ps1` | theirs-deletes-then-conflict sync scenario |

Docs: `docs/src/technical/git-ops.md`, `crdt-resolver.md`, `sync.md`, `invariants.md` (I1 wording), `CHANGELOG.md`.

## Interfaces & contracts

### 1. `TheirsOnlyChanges` (new private struct, `merge.rs`)

`collect_theirs_only_changes` currently returns `Result<Vec<String>>` (paths to stage). It must
now carry removals too. Introduce a private struct in `ddb-core/src/git_ops/merge.rs`:

```rust
/// Theirs-side changes outside our resolved set: paths to stage, and paths whose
/// deletion on theirs must be carried into the merge tree.
struct TheirsOnlyChanges {
    /// Paths added or modified on theirs; written to disk, staged with `add_path`.
    to_stage: Vec<String>,
    /// Paths deleted on theirs; removed from disk and from the index with `remove_path`.
    to_remove: Vec<String>,
}
```

### 2. `collect_theirs_only_changes` (changed signature, `merge.rs:105`)

```rust
/// Collect theirs-side changes not covered by our resolved set.
///
/// `Added`/`Modified` blobs are written to disk and returned in `to_stage`.
/// `Deleted` paths are removed from disk and returned in `to_remove`, so a
/// conflicted merge carries the other side's deletions instead of reverting
/// them. Paths in `resolved_set` are skipped in BOTH arms: a path git reported
/// as conflicted is owned by the CRDT/resurrect resolution, which is what keeps
/// the deliberate delete-vs-edit resurrect policy intact.
fn collect_theirs_only_changes(
    &self,
    ours_tree: &git2::Tree,
    theirs_tree: &git2::Tree,
    resolved_set: &std::collections::HashSet<&str>,
) -> Result<TheirsOnlyChanges>
```

Body contract, replacing the `if !matches!(...) { continue; }` guard at `merge.rs:117`:

```rust
for delta in diff.deltas() {
    match delta.status() {
        git2::Delta::Added | git2::Delta::Modified => {
            // path from delta.new_file(); skip if in resolved_set;
            // find_blob(delta.new_file().id()); create parent dirs; fs::write; push to to_stage
        }
        git2::Delta::Deleted => {
            // path from delta.old_file();  <-- old_file, NOT new_file
            // skip if in resolved_set;
            // if full.exists() { fs::remove_file(&full)?; }
            // push to to_remove
        }
        _ => continue, // Typechange/Renamed/Copied: see Risks
    }
}
```

- **`Delta::Deleted` MUST read `delta.old_file().path()`**, not `new_file()`. For a deletion
  libgit2 populates `new_file` with a zero oid; `old_file` is the canonical source of the path.
- Existing `Added|Modified` behavior is unchanged byte-for-byte apart from moving into the match arm.
- The `resolved_set.contains(path)` skip is retained in both arms and is load-bearing (see Data flow).

### 3. `build_merge_tree_and_commit` (changed param + fresh staging, `merge.rs:23`)

```rust
fn build_merge_tree_and_commit(
    &self,
    files: &[(&str, &str)],
    binary_paths: &[&str],
    theirs_only: &TheirsOnlyChanges,   // was: &[String]
    message: &str,
    our_commit: &git2::Commit,
    their_commit: &git2::Commit,
) -> Result<CommitHash>
```

Body contract — the **only** two changes to the existing body:

```rust
let mut index = self.fresh_index()?;          // was: self.repo.index()?  (merge.rs:32)
for (rel_path, _) in files {
    index.add_path(Path::new(rel_path))?;
}
for path in binary_paths {
    index.add_path(Path::new(path))?;
}
for path in &theirs_only.to_stage {
    index.add_path(Path::new(path))?;
}
for path in &theirs_only.to_remove {          // NEW
    index.remove_path(Path::new(path))?;
}
index.write()?;
// ... unchanged from here: write_tree / find_tree / signature / commit / write_commit_graph
```

`fresh_index()` does `index.read_tree(head_tree)`, so the index starts as **exactly ours' tree
(HEAD)**. Adding the resolved files + theirs-only stages and removing the theirs-only deletes
yields precisely the intended merge tree, with no dependence on whatever stale state the cached
`self.repo.index()` happened to hold.

### 4. `commit_merge` (locked body, `merge.rs:61`)

Public signature **unchanged**:

```rust
pub fn commit_merge(
    &self,
    files: &[(&str, &str)],
    binary_paths: &[&str],
    message: &str,
    theirs: &CommitHash,
) -> Result<CommitHash>
```

The **entire existing body** (path validation → `head_commit` → `find_commit` →
`write_resolved_files` → `collect_theirs_only_changes` → `build_merge_tree_and_commit`) moves
inside `self.with_write_lock(|| { ... })`. Wrapping the whole body — not just the commit — is
required: `our_commit = self.head_commit()` is the merge parent, so it must be resolved under
the same lock that guards the commit, or the parent can go stale between read and commit.

### 5. `merge_remote` (locked body, `merge.rs:185`)

Public signature **unchanged**:

```rust
pub fn merge_remote(&self, remote: &str, branch: &str) -> Result<MergeResult>
```

The **entire existing body** (reference lookup → `merge_analysis` → up-to-date / fast-forward /
`perform_normal_merge` dispatch) moves inside `self.with_write_lock(|| { ... })`.

This single wrap covers **two** of the PRD's three merge write sections:

- the fast-forward branch (`set_target` + `set_head` + forced `checkout_head`, `merge.rs:199-211`), and
- `perform_normal_merge` (commit + forced `checkout_head`, `merge.rs:169-178`), which is private
  and reached only from here, so it inherits the lock. `with_write_lock` is re-entrant on the
  owning thread (`write_lock_depth`, `mod.rs:294-305`), so no separate wrap is added there —
  a second wrap would be a no-op. **Do not add one.**

Wrapping the whole body (rather than only the write sections) is deliberate and safe:

- **No network under the lock.** `merge_remote` does **not** fetch. `sync_manager::sync` fetches
  first (`sync_manager/mod.rs:237`) and only then calls `merge_remote` (`:246`). `merge_analysis`
  is a local, in-memory computation.
- **No CRDT compute under the lock.** The conflict path returns `MergeResult::Conflicts` early,
  which drops the guard. CRDT resolution then runs unlocked in `sync_manager`, and `commit_merge`
  re-acquires for its own write. This is exactly the PRD's stated mitigation ("acquire around the
  write section, not the fetch or the CRDT resolution compute").
- **It closes a TOCTOU the narrower wrap would leave open**: `merge_analysis` decides
  fast-forward-vs-merge, and that decision must not be invalidated by a concurrent HEAD move
  before the branch acts on it.
- **No lock-ordering violation.** The `with_write_lock` docstring pins "this lock is always the
  innermost resource; no path takes it and then opens a SQLite immediate transaction."
  `merge_remote` and `commit_merge` touch git only — every SQLite call on the sync path
  (`validate_clean_merge_or_fallback`, `rebuild_if_stale`) happens after they return.

### 6. Delete freshness (`git_ops/mod.rs`)

Two one-line swaps; **no other change** in either function:

- `delete_file` — `mod.rs:408`: `let mut index = self.repo.index()?;` → `let mut index = self.fresh_index()?;`
- `delete_files` — `mod.rs:438`: `let mut index = self.repo.index()?;` → `let mut index = self.fresh_index()?;`

This makes both consistent with every other commit path (`commit_files:361`, `commit_binary_file:327`,
`commit_batch`), and with `fresh_index`'s own docstring warning (`mod.rs:259-263`).

### 7. CLI `resurrected` line (`ddb-cli/src/commands/sync.rs`)

Insert directly after the `collisions_reassigned` block (`sync.rs:18-20`), matching its
print-only-when-non-zero shape exactly:

```rust
if report.resurrected > 0 {
    outln!("  resurrected: {}", report.resurrected)?;
}
```

`SyncReport.resurrected` is already populated (`sync_manager/mod.rs:312`,
`report.resurrected = delete_edit.len()`). No core change; CLI-only.

## Data flow

### Conflicted merge (H1 — the deletion-preservation path)

```
sync_manager::sync
  └─ repo.fetch(remote, branch)                        [unlocked, network]
  └─ repo.merge_remote(remote, branch)                 [LOCKED (new)]
       └─ merge_analysis → conflicts → merge_commits detects conflicts
       └─ returns MergeResult::Conflicts(conflicts, theirs_oid)   → lock released
  └─ resolve_merge_conflicts                           [unlocked, CRDT compute]
       ├─ partition_conflicts → (binary_ref, delete_edit, add_add, normal)
       ├─ resolve_delete_edit_conflicts(delete_edit)   → resurrect marker, report.resurrected
       └─ create_conflict_commit(resolved, binary_ref, theirs_oid)
            └─ repo.commit_merge(files, binary_paths, msg, theirs)   [LOCKED (new)]
                 ├─ our_commit = head_commit()          ← resolved INSIDE the lock
                 ├─ write_resolved_files(files)         → worktree
                 ├─ collect_theirs_only_changes(ours_tree, theirs_tree, resolved_set)
                 │    ├─ Added|Modified & !resolved  → write blob to disk, to_stage
                 │    └─ Deleted      & !resolved  → remove from disk, to_remove   ← NEW
                 └─ build_merge_tree_and_commit
                      ├─ index = fresh_index()          ← = ours' HEAD tree (NEW; was cached)
                      ├─ add_path(files ∪ binary_paths ∪ to_stage)
                      ├─ remove_path(to_remove)         ← NEW
                      └─ write_tree → commit(HEAD, [our_commit, their_commit])
```

**Why the resurrect policy survives, precisely.** The two cases are disjoint by construction, and
`resolved_set` is what separates them:

| case | git sees | in `resolved_set`? | Deleted arm | outcome |
|------|----------|--------------------|-------------|---------|
| theirs deleted Y, **ours untouched** Y | not a conflict — a clean theirs-side delete | **no** | fires → Y removed from tree | **deletion preserved** (the fix) |
| theirs deleted Y, **ours edited** Y | delete-vs-edit **conflict** | **yes** (it is in `files`, resolved with the marker) | skipped | **resurrect-with-marker** (policy intact) |

So the new `Deleted` arm can only ever fire on paths git did *not* report as conflicted, which is
exactly the set the resurrect policy at `sync_manager/mod.rs:427-451` never sees. No interaction.

**Theirs-side rename.** `diff_tree_to_tree(..., None)` performs no rename detection (no
`find_similar` call), so a rename on theirs arrives as an independent `Deleted(old)` +
`Added(new)` pair. The Deleted arm removes `old`; the Added arm stages `new`. One doogat at the
new path, none at the old — the PRD's edge case falls out of the two arms with no rename-specific
code.

### Delete (H2)

```
service::delete → repo.delete_file(path, msg)          [already LOCKED by 00162]
  └─ index = fresh_index()   ← read_tree(HEAD): sees externally-committed F  (was: cached, F invisible)
  └─ remove_path(G) → write_tree → commit([HEAD])       → removes G only; F survives
```

### Fast-forward / normal merge (H3)

```
repo.merge_remote                                       [LOCKED (new)]
  ├─ fast-forward: set_target → set_head → checkout_head(force)   ← forced checkout now serialized
  └─ perform_normal_merge: merge_commits → write_tree_to → commit → checkout_head(force)
                                                        ← re-entrant; inherits the lock
```

A concurrent CRUD writer holds the same advisory lock across its own
stage → `write_tree` → resolve-parent → commit section, so the merge's forced checkout can no
longer land between that writer's worktree write and its `index.add_path`, and the merge's commit
parent can no longer be a stale HEAD.

## Reuse inventory

Every primitive this PRD needs already exists. Nothing new was written.

| Need | Existing helper | Path | How to use |
|------|-----------------|------|------------|
| Stage from fresh state | `GitRepo::fresh_index()` | `git_ops/mod.rs:263-270` | Call instead of `self.repo.index()`. Does `read_tree(head_tree)` so the index == HEAD's tree. Already the pattern in `commit_files:361`, `commit_binary_file:327`. |
| Cross-process write lock | `GitRepo::with_write_lock()` | `git_ops/mod.rs:294-305` | `self.with_write_lock(\|\| { ...body... })`. Re-entrant on the owning thread via `write_lock_depth` Cell — nested calls run the body directly. |
| Lock primitive | `write_lock::acquire()` | `git_ops/write_lock.rs:51` | Not called directly; reached via `with_write_lock`. 10 s timeout (`WRITE_LOCK_TIMEOUT`, `mod.rs:39`) → loud `DoogatError::Conflict`. |
| Remove a path from a git index | `git2::Index::remove_path` | (git2) | Already used at `mod.rs:409`, `:440`, `:510`. Same call shape. |
| Classify a `git2::Delta` incl. `Deleted` | `classify_delta` | `git_ops/read.rs:207-216` | **Precedent, not reused**: proves the codebase already matches on `Delta::Deleted`; `merge.rs:117` is the outlier that only matches `Added\|Modified`. Not called (it maps to the `DiffKind` reporting type, a different purpose). |
| Print an optional sync counter | `outln!` + the `collisions_reassigned` block | `ddb-cli/src/commands/sync.rs:18-20` | Copy the `if report.X > 0 { outln!(...) }` shape verbatim. |
| Populate `resurrected` | `report.resurrected = delete_edit.len()` | `sync_manager/mod.rs:312` | Already populated. CLI only needs to print it. |
| Two-repo merge test fixture | `setup_two_repos()` | `git_ops/tests.rs:~230-252` | Returns `(dir_a, repo_a, dir_b, repo_b, bare_dir)` wired with an `origin` remote. Existing merge tests (`push_and_fetch_cycle:255`, `merge_detects_conflicts:280`) use it. |

**Greps run** (Bash `rg`, per `rationalizations.md` Synonyms-to-grep):

- removals: `remove_path|remove_dir|index.remove` → found `mod.rs:409,440,510` (git index) + `value.rs:515` / `remove_doogat` (unrelated domains). No merge-side removal helper exists.
- delta handling: `Delta::Deleted|Delta::Added|Delta::Renamed|Delta::Modified|Delta::Typechange|find_similar` → only `merge.rs:117` and `read.rs:210-213`. **`find_similar` returns nothing** — confirming no rename detection is configured anywhere, which is what makes the Deleted+Added pair the actual runtime shape.
- lock/index primitives: `fn with_write_lock|fn fresh_index|fn delete_file|fn delete_files|with_write_lock\(|lock_depth` → the 11 lock sites are all in `mod.rs`; **zero in `merge.rs`**, confirming H3.
- merge/checkout call sites: `merge_remote|commit_merge|import_bundle|checkout_head|set_target` → production writers are `merge.rs:178,205,208` only; the rest are tests + trait delegations (`traits.rs:381-390`, `mod.rs:704-715`).

Verification that the greps themselves work: the `remove_path` and `with_write_lock` searches
both returned known-present hits, so the empty `find_similar` result is a real absence, not a
pattern error.

## Alternatives considered

### A. Minimal patch: `Delta::Deleted` arm + `fresh_index()` + two `with_write_lock` wraps — **CHOSEN**

The smallest-diff version of the idea. ~40 production lines across 3 files, no new
abstraction, no signature change on any public or trait method.

- **Pro**: smallest blast radius on a P0 data-loss path; each of the three bugs is fixed at its
  own site by a primitive the codebase already trusts everywhere else; `resolved_set` gives the
  resurrect-policy preservation for free; renames fall out of the Deleted+Added pair with no
  rename-specific code.
- **Con**: `build_merge_tree_and_commit` still reconstructs the merge tree by hand from ours'
  tree + a diff, rather than letting libgit2 produce it. The hand-rolled reconstruction is the
  thing that had the bug in the first place, and it stays.

### B. Build the merge tree from `merge_commits`' in-memory index

The PRD offers this explicitly ("Alternatively build the merge tree from `merge_commits`' in-memory
index rather than ours' on-disk index — choose one and document it").

- **Pro**: libgit2 computes the merge tree, including theirs-side deletions and renames, so the
  entire class of "we forgot a delta kind" bugs disappears rather than being patched.
- **Con**: **rejected** — it does not fit `commit_merge`'s position in the flow.
  `commit_merge` runs *after* CRDT resolution, on a conflict set libgit2 already flagged.
  Re-running `merge_commits` there would re-produce an index **with conflict entries**, which we
  would then have to walk and overwrite with the resolved blobs (and `write_tree_to` refuses an
  index with conflicts). That is a substantially larger rewrite of the riskiest function in the
  PRD, on the P0 path, for a benefit the `resolved_set` + Deleted arm already delivers. It also
  discards the resurrect policy's clean separation, since the conflict entries would need
  re-mapping onto resolved files by hand.
- Revisit only if a future delta kind (Typechange) proves to matter in practice.

### C. Wrap the whole `sync_manager::sync` in one lock

- **Pro**: one wrap; no intermediate windows at all, including the resolve→commit gap noted in Risks.
- **Con**: **rejected** — it holds an exclusive cross-process lock across `fetch` and `push`
  (network I/O) and the whole CRDT resolution. The 10 s `WRITE_LOCK_TIMEOUT` would turn any
  ordinary slow sync into a spurious `Conflict` error for every concurrent CLI write, converting
  a correctness fix into an availability regression. Directly contradicts the PRD's mitigation.

**Why the chosen design is not the absolute smallest**: A is the smallest. The only place it
spends more than the strict minimum is wrapping the *whole* `merge_remote` body instead of only
its two write sections — that buys the `merge_analysis` TOCTOU closure for zero extra lines and
zero extra lock hold (no network, no CRDT compute inside).

## Risks & edge cases

### Risks in this change

- **`fresh_index()` in the merge path resets the index to HEAD.** `read_tree` discards any staged
  state. On the merge path that is exactly right (the merge tree is defined as ours' tree plus the
  computed deltas), but it means `build_merge_tree_and_commit` now ignores pre-staged entries that
  the cached index might have carried. Nothing on the sync path stages without committing, so this
  is a correctness improvement — but it is the one behavioral change with a blast radius beyond
  the three named bugs. **Mitigation**: the existing merge tests in `git_ops/tests.rs` and
  `sync_manager/tests.rs` (which cover clean, conflicted, and fast-forward merges end-to-end) must
  stay green unmodified; if one needs editing, that is a signal to stop and re-examine, not to
  adjust the test.
- **`Delta::Typechange` and `Delta::Renamed` stay skipped** by the `_ => continue` arm. Renamed is
  unreachable (no `find_similar`, verified by grep). Typechange (file ↔ symlink) is skipped today
  and stays skipped — a deliberate no-change. Doogats are regular files, so this is not reachable
  through the product's own write paths. Flagged rather than fixed, per surgical-changes.
- **Lock hold now spans `merge_commits`** inside `perform_normal_merge`. That is an in-memory
  three-way merge over the doogat tree, not network or CRDT work; at ddb's scale (benchmarks run
  1K doogats) it is milliseconds against a 10 s timeout. Acceptable, but it is a real widening of
  the hold versus wrapping only the commit+checkout.
- **Test must warm the index cache to fail on old code (H2).** `self.repo.index()` only returns a
  *stale* index if libgit2 has already cached one for that `Repository`. A delete-freshness test
  that opens `GitRepo`, externally commits F, then deletes G will read the index fresh from disk,
  see F, and **pass against the unfixed code** — a false green. The test MUST perform a write
  through `repo1` first (e.g. `repo1.commit_file(...)`) to populate the cache, *then* land F via a
  second `GitRepo` instance, *then* call `repo1.delete_file(G)`. Watch it fail once against the old
  code before trusting it (`rules/testing.md`).

### Known gaps left open (deliberate, out of scope)

- **Bundle import's `git merge` subprocess is still unlocked and untouched.** The PRD says callers
  "inherit the protection with no signature change" — that holds for `sync_manager` (both its call
  sites go through the two wrapped functions), but **not** for bundle import.
  `bundle.rs:118-126` (`merge_bundle_and_resolve`) shells out to `git merge` via
  `std::process::Command`, which moves HEAD and runs a checkout entirely outside git2 and outside
  our lock. Wrapping it is not a merge.rs change: `bundle.rs` holds `repo: &impl GitBackend`, and
  `with_write_lock` is a private inherent method on `GitRepo`, absent from the `GitBackend` trait —
  so covering it needs a new trait method with a closure parameter (and a decision about
  object-safety). That is a different change of a different shape. **Recommend it lands in backlog
  PRD 00168 (bundle-import-conflict-recovery), which already owns this file.** Flagged, not fixed.
  Its `SyncReport` also hardcodes `resurrected: 0` (`bundle.rs:192`) — same PRD.
- **Resolve→commit window.** `merge_remote` releases the lock when it returns `Conflicts`, and
  `commit_merge` re-acquires later. HEAD can move in between, so the CRDT resolution may be
  computed against a slightly stale ours. No data loss results: `commit_merge` re-reads
  `head_commit()` inside the lock, so the commit parent is always current and nothing is orphaned.
  Closing this window fully requires alternative C's trade-off. Out of scope; PRD's success
  metrics do not cover it.
- **`maintenance::maybe_auto_run` runs unlocked** (`sync_manager/mod.rs:423`), after the merge.
  `git maintenance --auto` pruning is the PRD's stated orphan hazard, but by then HEAD is stable
  and every writer commits under the lock, so there is no orphan left to prune. Pre-existing;
  unchanged.

### Likely next changes after this PRD, and whether this design boxes them in

1. **00164 unify-id-minting** — untouched territory; no coupling.
2. **00169 poison-file-reindex-resilience** — adds a *second* cross-process lock for the SQLite
   index rebuild, and must declare its ordering against this one. This design **helps**: it keeps
   the git write lock strictly innermost (the docstring's stated ordering invariant) and adds no
   SQLite call inside a locked section, so 00169 can safely take its rebuild lock outer.
   **Constraint recorded for 00169**: never take the git write lock and then the rebuild lock.
3. **00168 bundle-import-conflict-recovery** — inherits the deliberate gap above. It will likely
   need `with_write_lock` on the `GitBackend` trait; this design does not block that, but also
   does not pre-build it (YAGNI — one consumer, and the trait shape is 00168's call).

## Test strategy outline

Every test is written to **fail against the current code first** (`rules/testing.md`: mandatory
for bug fixes). Tier 1 gate per `AGENTS.md`: `cargo build`, `cargo clippy --workspace --all-targets`,
`cargo test-ci`.

### Unit — `ddb-core/src/git_ops/tests.rs` (uses `setup_two_repos()`)

| Test | Rule it pins | Fails on old code because |
|------|--------------|---------------------------|
| `conflicted_merge_preserves_theirs_side_deletion` | theirs deletes Y (ours never touched Y), ours edits X, theirs edits X → merge tree has **no** Y | `Delta::Deleted` is `continue`d → Y stays in the tree |
| `conflicted_merge_theirs_rename_does_not_duplicate` | theirs renames Z → new path present, old path absent | Deleted(old) skipped → both paths alive |
| `delete_file_builds_tree_from_fresh_index` | warm repo1's index cache → external `GitRepo` commits F → `repo1.delete_file(G)` → G gone, **F survives** | cached index predates F → commit reverts F |
| `delete_files_builds_tree_from_fresh_index` | same, multi-path via `delete_files` | same |
| `merge_does_not_revert_concurrent_locked_write` | a thread holding the write lock mid-write races `merge_remote` → its file survives the forced checkout; merge commit's parent is current HEAD | merge runs unlocked → checkout reverts / stale parent |

**Why unit, not e2e** (the PRD sketches `tests/e2e/delete_concurrency.rs` + `merge_lock.rs`): both
scenarios reproduce *in-process* — H2 needs two `GitRepo` instances (one warm cache, one external
committer), and the lock is cross-process *and* cross-thread (`write_lock.rs` tests already prove
mutual exclusion with two threads at `write_lock.rs:96-130`). In-process tests run in Tier 1
(`cargo test-ci`), where a real regression gets caught per-task; e2e is Tier-2/CI-only and needs a
built `ddb` binary. Cheaper, stricter, faster feedback. **This is a deliberate deviation from the
PRD's file sketch** — same coverage, better tier. If a reviewer disagrees, the e2e versions are
additive, not a rewrite.

### Unit — `ddb-core/src/sync_manager/tests.rs`

| Test | Rule it pins |
|------|--------------|
| `theirs_deleted_ours_untouched_stays_deleted` | the new arm fires: file absent, `report.resurrected == 0` |
| `theirs_deleted_ours_edited_resurrects_with_marker` | the policy still fires: file present, marker applied, `report.resurrected == 1` — **the no-regression test the PRD requires** |

These two are the pair that proves the `resolved_set` separation from Data flow holds at runtime,
not just on paper. Existing resurrect tests must stay green **unmodified**.

### CLI

| Test | Rule it pins |
|------|--------------|
| `sync_output_reports_resurrected_count` | a sync that resurrects prints the count; zero stays silent (parity with `collisions_reassigned`) |

### Integration scenario (shell harness — still current at this queue position; 00171 migrates it later)

`tests/integration.sh` + `tests/integration.ps1`: numbered section following the existing
`pass`-helper pattern — theirs-deletes-then-conflict sync → assert the deletion survives the
conflicted merge. Execution is delegated to CI (Tier 2).

### Docs (per-PRD deliverables)

`git-ops.md` (delete freshness + merge locking), `crdt-resolver.md` + `sync.md` (deletion
preservation vs. the resurrect policy — the table from Data flow), `invariants.md` I1 (flip the
merge-gap wording; the interim 2026-07-16 correction stands until this ships), `CHANGELOG.md`
`Fixed` entry covering all three regressions.

## Review log

dispatch 1 (claude): cardinal-sin 0, blocker 2, non-blocker 3, question 2

**STATUS: review loop HALTED after dispatch 1 — escalated to the maintainer (data-loss scope
decision). Dispatch 2 (codex) NOT run. The two blockers are NOT yet fixed, because fixing them
requires a scope decision that exceeds this PRD's stated scope. Do not plan tasks from this doc
until that decision lands and the design is reworked.**

### BLOCKER 1 (open) — the designed `Delta::Deleted` arm destroys doogats *ours* created

`collect_theirs_only_changes` diffs `diff_tree_to_tree(Some(ours_tree), Some(theirs_tree), None)`
(`merge.rs:111-113`), so `old = ours`, `new = theirs`. `Delta::Deleted` means "in old, not in new"
— which is the **union** of two disjoint cases:

1. theirs deleted it (ours untouched) ← the intended target
2. **ours added it since the merge base** (theirs never had it) ← collateral

Case 2 is not a conflict, so it is never in `resolved_set`, so the designed arm would
`fs::remove_file` + `index.remove_path` it and commit a merge tree with ours' brand-new doogat
gone. Fires on the most ordinary sync there is: device A creates a doogat and edits X; device B
edits X; B syncs.

**Verified empirically at HEAD `8e4b4aa`** with real git plumbing (not inspection):

```
base: X.md
ours: created N.md, created Z.md, edited X.md
theirs: created T.md, edited X.md

$ git diff --name-status OURS THEIRS      # == diff_tree_to_tree(old=ours, new=theirs)
D	N.md      <-- ours' NEW doogat reports as Deleted
A	T.md
M	X.md
D	Z.md      <-- ours' NEW doogat reports as Deleted
```

The §Data flow disjointness table is correct as far as it goes (the `resolved_set` → resurrect-policy
separation genuinely holds; the reviewer traced `partition_conflicts` `sync_manager/mod.rs:115-125`
→ `resolve_delete_edit_conflicts` `:428-456` → `files` `:349-352` → `resolved_set` `merge.rs:85-89`).
The defect is that it enumerates 2 of ~6 cases that produce a `Deleted` delta, and the missing row is
catastrophic. **`resolved_set` is necessary but NOT sufficient to gate the `Deleted` arm.**

### BLOCKER 2 (open) — Alternative B was rejected on a false premise; the A-vs-B decision must be re-taken

B was rejected partly because it delivers "a benefit the `resolved_set` + Deleted arm already
delivers." Blocker 1 shows it does not. Worse, the same two-way-diff defect means the **existing**
`Added|Modified` arms are already wrong, which this design asserted was safe ("Existing
`Added|Modified` behavior is unchanged byte-for-byte") without checking whether that behavior is
correct.

**Verified empirically** — a pre-existing P0 data-loss bug, NOT introduced by this PRD:

```
base: X.md="base-X", Z.md="base-Z"
ours: edited X.md, edited Z.md="OURS-IMPORTANT-EDIT"
theirs: edited X.md only — never touched Z.md

$ git diff --name-status OURS THEIRS
M	X.md      <-- the real conflict; in resolved_set; skipped (correct)
M	Z.md      <-- edited ONLY by ours; NOT a conflict; NOT in resolved_set
$ git show theirs:Z.md
base-Z            <-- what the Modified arm writes over ours' edit
$ git show master:Z.md
OURS-IMPORTANT-EDIT   <-- silently reverted to base in the merge tree
```

By symmetry the `Added` arm resurrects ours' clean deletes (ours deleted D.md → absent in ours,
present-at-base in theirs → reports `Added` → arm rewrites it).

**Conclusion: `collect_theirs_only_changes` is not "missing an arm." A two-way diff structurally
cannot express the three-way merge it is simulating.** Every conflicted merge silently reverts
ours' non-conflicting local edits since the merge base. This is a more probable data-loss path
than the PRD's own H1, and the PRD/audit does not know about it.

The merge base needed to fix it is already computed and discarded at `merge.rs:153`
(`let _ancestor = self.repo.merge_base(our_commit.id(), their_commit.id())?;`).

### Non-blockers (recorded, not fixed)

- **NB-1 — pre-existing `Added`/`Modified` loss needs an explicit scope decision.** Per
  `rules/surgical-changes.md` it is out of this diff, but it is the same invariant (I1) and the same
  function, and once the ancestor tree is threaded in for Blocker 1 the incremental cost is small.
  Record the decision in §Risks; do not let it ride silently under "unchanged byte-for-byte."
- **NB-2 — invariant I1 must NOT be marked closed by this PRD.** I1 reads "merges, sync, and *bundle
  import* included"; bundle import stays unlocked after 00163 (deferred to 00168). Flipping I1 to
  closed would ship a false claim. State that I1 stays **open**, narrowed to the bundle-import leg,
  and reconcile the PRD's incorrect "callers inherit the protection" wording in the same pass.
- **NB-3 — the lock-hold risk bullet omits a subprocess.** The wrapped `merge_remote` body calls
  `self.write_commit_graph()` (`merge.rs:179`, `:209`), which shells out to
  `git commit-graph write --reachable` (`mod.rs:626-633`) — a subprocess walking every reachable
  commit, now inside a lock with a 10 s `WRITE_LOCK_TIMEOUT`. Suppressed on the sync path
  (`set_skip_commit_graph(true)`, `sync_manager/mod.rs:232`) but NOT for non-sync callers of
  `merge_remote`, who could push a concurrent writer to a spurious `DoogatError::Conflict` on a
  large repo. Add to the §Risks lock-hold bullet.

### Questions (recorded, not fixed)

- **Q-1 — the `old_file()` rationale is imprecise (the choice is still right).** The doc claims
  libgit2 populates `new_file` with a zero oid *and* that `old_file` is the canonical path source.
  The oid half is correct; the path half is not — libgit2 populates `new_file.path` for deletions
  too, so `new_file().path()` would also work. Keep `old_file()`, fix the stated reason.
- **Q-2 — the CLI-only `resurrected` line should say why it satisfies 120% parity.** GraphQL already
  exposes `resurrected` (`ddb-server/src/schema/mutations/operations.rs:357-358`, `types.rs:55`, e2e
  `tests/e2e/server_mutations.rs:139`), so the CLI was the only public interface missing it — this
  *closes* a parity gap rather than creating drift. Add one line to §7 saying so.

### Verified sound by dispatch 1 (no action)

- The `resolved_set` → resurrect-policy separation holds; traced end-to-end.
- `fresh_index()` on the merge path is correct: `read_tree(head_tree)` yields exactly ours' tree, and
  under the new lock `our_commit` and `fresh_index`'s internal `head_commit()` agree.
- The H2 warm-the-cache test requirement is real: a cold `repo1` reads `.git/index` fresh, sees F, and
  would **falsely pass against unfixed code**. The prescribed warm → external-commit → delete
  sequence does reproduce.
- The `merge_remote` whole-body wrap is safe: no fetch inside, `merge_analysis` is in-memory, no
  SQLite inside, git stays innermost per the `mod.rs:290-293` ordering invariant, and
  `perform_normal_merge`'s re-entrancy makes a second wrap a genuine no-op.
- Bundle-import deferral to 00168 is a scoped, justified, owned deferral — **not** a cardinal sin.
- All line refs and grep claims check out at HEAD `8e4b4aa`; `find_similar` is genuinely absent, so
  theirs-side renames do decompose into `Deleted`+`Added`. **Corollary: an *ours*-side rename
  decomposes the same way and hits Blocker 1 from both directions.**
