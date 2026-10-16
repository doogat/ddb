# Design: Bundle-import conflict recovery (00168)

## Architecture fit

`ddb-core/src/bundle.rs` (air-gapped import/export) and `ddb-core/src/sync_manager/mod.rs`
(conflict resolution orchestration) are both leaves the network-sync path already
depends on for the identical merge→resolve→commit shape. `git_ops/merge.rs`
(`GitRepo::merge_remote`, `GitRepo::commit_merge`) is the libgit2, write-lock-covered
merge engine that `SyncManager::sync` already drives for network sync. This PRD wires
the bundle-import leaf onto the SAME engine instead of a second, ad hoc, CLI-shelling
implementation — closing the last unlocked write path named in
`docs/src/technical/invariants.md` (I1/I2) and memory `project_data_loss_write_path`.

## Module placement

- **Edit** `ddb-core/src/bundle.rs` — replace `merge_bundle_and_resolve`'s CLI `git
  merge` + `stderr` string-match body with calls into `GitRepo::merge_remote` +
  `SyncManager::apply_merge_result` (widened to `pub(crate)`, see below). Change its
  return type from `Result<usize>` to `Result<SyncReport>`. Update `import_bundle` to
  thread the fuller report through and call the new `delete_remote_ref` instead of the
  raw CLI `update-ref -d`.
- **Edit** `ddb-core/src/sync_manager/mod.rs` — widen `fn apply_merge_result` to
  `pub(crate) fn apply_merge_result` (no body change). This is the only visibility
  change; `resolve_merge_conflicts`, `validate_clean_merge_or_fallback`, and everything
  else it calls stay private and untouched.
- **Edit** `ddb-core/src/traits.rs` — add one method to the `GitRemote` trait:
  `fn delete_remote_ref(&self, remote: &str, branch: &str) -> Result<()>;`.
- **Edit** `ddb-core/src/git_ops/remote.rs` — add the inherent
  `GitRepo::delete_remote_ref` implementation (git2, write-lock-covered), next to
  `add_remote`/`fetch`/`push`.
- **Edit** `ddb-core/src/git_ops/mod.rs` — add the one-line delegating arm to `impl
  crate::traits::GitRemote for GitRepo` (matches the existing `add_remote`/`fetch`/
  `push` delegation pattern at :687-699).
- **No new files.** No changes to `unbundle_git_objects`, `export_bundle`,
  `export_full_bundle`, `verify_bundle`, or any of the tar/checksum machinery — out of
  this PRD's scope (the bug is in merge/resolve/ref-delete only).

## Interfaces & contracts

### `ddb-core/src/traits.rs` — `GitRemote` trait (add one method)

```rust
pub trait GitRemote {
    fn add_remote(&self, name: &str, url: &str) -> Result<()>;
    fn fetch(&self, remote: &str, branch: &str) -> Result<()>;
    fn push(&self, remote: &str, branch: &str) -> Result<()>;

    /// Delete a remote-tracking ref (`refs/remotes/{remote}/{branch}`), e.g. after a
    /// successful bundle import. A no-op (`Ok(())`) if the ref does not exist —
    /// deletion is idempotent. Runs inside the write lock (see `GitRepo::delete_remote_ref`).
    fn delete_remote_ref(&self, remote: &str, branch: &str) -> Result<()>;
}
```

`GitRepo` is the sole implementor of `GitBackend`/`GitRemote` in this codebase (only
one `impl ... GitRemote for` site exists), so this is a contained addition.

### `ddb-core/src/git_ops/remote.rs` — `GitRepo::delete_remote_ref` (new inherent method)

```rust
impl GitRepo {
    /// Delete `refs/remotes/{remote}/{branch}`. No-op if the ref is already absent.
    pub fn delete_remote_ref(&self, remote: &str, branch: &str) -> Result<()> {
        self.with_write_lock(|| {
            let ref_name = format!("refs/remotes/{remote}/{branch}");
            match self.repo.find_reference(&ref_name) {
                Ok(mut reference) => {
                    reference.delete()?;
                    Ok(())
                }
                Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        })
    }
}
```

`with_write_lock` is re-entrant (`GitRepo::write_lock_depth`, `git_ops/mod.rs:94-99`),
so this composes safely as its own locked step after `commit_merge`'s locked step —
same pattern the existing `sync()` path already uses across `merge_remote` +
`commit_merge` (two separate lock acquisitions, not one enclosing one; see Alternatives).

### `ddb-core/src/git_ops/mod.rs` — trait delegation (add one arm)

```rust
impl crate::traits::GitRemote for GitRepo {
    // ...existing add_remote/fetch/push arms unchanged...
    fn delete_remote_ref(&self, remote: &str, branch: &str) -> Result<()> {
        self.delete_remote_ref(remote, branch)
    }
}
```

### `ddb-core/src/sync_manager/mod.rs` — visibility only

```rust
// was: fn apply_merge_result(&mut self, merge_result: MergeResult, index: &Index) -> Result<SyncReport>
pub(crate) fn apply_merge_result(
    &mut self,
    merge_result: MergeResult,
    index: &Index,
) -> Result<SyncReport>
```

No change to the body, to `resolve_merge_conflicts`, `validate_clean_merge_or_fallback`,
or any other private helper — they stay exactly as the network-sync path already
exercises and tests them.

### `ddb-core/src/bundle.rs` — `merge_bundle_and_resolve` (rewritten body, new return type)

```rust
/// Merge bundle/master into local master, resolving conflicts via the same
/// libgit2 + CRDT pipeline the network-sync path uses (`SyncManager::apply_merge_result`).
/// No CLI `git merge`, no stderr parsing, no MERGE_HEAD: `merge_commits` computes the
/// merge entirely in memory, so a resolution failure here leaves the repo exactly as
/// it was before this call (see Risks — no `git merge --abort` step is needed).
fn merge_bundle_and_resolve(
    repo: &impl GitBackend,
    sync_mgr: &mut SyncManager<impl GitBackend>,
    index: &crate::indexer::Index,
) -> Result<SyncReport> {
    let merge_result = repo.merge_remote("bundle", "master")?;
    sync_mgr
        .apply_merge_result(merge_result, index)
        .map_err(|e| DoogatError::Sync(format!("bundle merge failed: {e}")))
}
```

### `ddb-core/src/bundle.rs` — `import_bundle` (updated call sites only)

```rust
pub fn import_bundle(
    repo: &impl GitBackend,
    sync_mgr: &mut SyncManager<impl GitBackend>,
    index: &crate::indexer::Index,
    bundle_path: &Path,
) -> Result<SyncReport> {
    // ...unpack, checksum, manifest, unbundle_git_objects — all unchanged...

    let mut report = merge_bundle_and_resolve(repo, sync_mgr, index)?;
    import_node_registrations(repo, &work_dir)?;

    repo.delete_remote_ref("bundle", "master")?;

    index.rebuild(repo)?;

    report.direction = "bundle-import".to_string();
    Ok(report)
}
```

The `?` after `merge_bundle_and_resolve` is unchanged from today's code — it already
short-circuits `import_node_registrations` and the ref-delete on error; the bug was
never in this control flow, only in `merge_bundle_and_resolve` returning a false `Ok(0)`
instead of a real error or real count. Fixing conflict detection is what makes this
existing `?` finally do its job. The ref-delete moves from an ignored CLI subprocess
(`let _ = Command::new("git")...`) to `repo.delete_remote_ref(...)?` — errors now
propagate instead of being swallowed, and the call is write-lock-covered.

`commits_transferred`, `resurrected`, and `collisions_reassigned` are no longer
hardcoded to `0` — they come from the real `SyncReport` `apply_merge_result` builds, so
a bundle import that resurrects a delete/edit conflict or reassigns a collision loser
now reports it honestly (previously always `0`/`0` regardless of what happened).

## Data flow

1. `import_bundle` unpacks the tar, verifies checksum, unbundles git objects into
   `refs/remotes/bundle/master` (unchanged — CLI `git bundle unbundle` + `git fetch`,
   out of scope: this only populates a ref, it does not merge or touch HEAD).
2. `merge_bundle_and_resolve` calls `repo.merge_remote("bundle", "master")` — same
   entry point `SyncManager::sync` uses for `origin/master`. Internally: write-locked,
   `merge_analysis` → fast-forward / already-up-to-date / `perform_normal_merge`.
   `perform_normal_merge` computes `merge_commits(our_commit, their_commit)` **entirely
   in memory** (no worktree write, no `MERGE_HEAD`); on conflict it extracts
   `Vec<ConflictFile>` from the in-memory index, calls `cleanup_state()` defensively,
   and returns `MergeResult::Conflicts(conflicts, theirs_oid)` — no error, no partial
   state change.
3. `apply_merge_result` dispatches on the `MergeResult`:
   - `AlreadyUpToDate` / `FastForward` / `Clean` → same handling as network sync
     (`Clean` re-validates via `validate_clean_merge_or_fallback`, unchanged).
   - `Conflicts` → `resolve_merge_conflicts`: partitions conflicts
     (delete/edit, add/add collision, binary-ref, normal), resolves each via the
     existing CRDT cascade / collision-reassignment / LWW paths (all unchanged,
     already tested by the network-sync suite and PRD 00167's suite), then
     `create_conflict_commit` → `GitRepo::commit_merge` (write-locked, builds the
     real 2-parent merge commit, folds collision losers into the SAME commit).
4. Back in `import_bundle`: node registrations import (unchanged), then
   `repo.delete_remote_ref("bundle", "master")` — only reached if step 2-3 returned
   `Ok`, i.e. only after a real merge landed (or genuinely nothing needed to land).
5. `index.rebuild(repo)` (unchanged), `report.direction` overridden to
   `"bundle-import"`, returned.

On any `Err` from step 2/3 (e.g. `commit_merge`'s divergence guard, a collision loser
with no blob OID, or any other resolver failure), `merge_bundle_and_resolve` maps it to
`DoogatError::Sync`, `import_bundle`'s `?` returns immediately — `refs/remotes/bundle/master`
is never deleted (data stays reachable, next import retries against the same ref) and
the repo's on-disk state is untouched from before the call (no CLI merge ever ran, so
there is nothing to abort).

## Reuse inventory

- `GitRepo::merge_remote` / `perform_normal_merge` (`ddb-core/src/git_ops/merge.rs:373-452`)
  — the write-locked, libgit2, index-state (not stderr) conflict-detecting merge engine
  the network-sync path already uses. Reused as-is via the `GitBackend`/`GitMerge` trait,
  called with `("bundle", "master")` instead of `("origin", "master")`.
- `SyncManager::apply_merge_result` / `resolve_merge_conflicts` / `create_conflict_commit`
  / `validate_clean_merge_or_fallback` (`ddb-core/src/sync_manager/mod.rs:271-370,555-628`)
  — the full conflict-partition → CRDT-cascade → collision-reassignment →
  `commit_merge` pipeline, unchanged; only `apply_merge_result`'s visibility widens.
- `GitRepo::commit_merge` (`ddb-core/src/git_ops/merge.rs:13-94`) — the write-locked,
  atomic, PRD-00167-hardened merge-commit builder (divergence guard, atomic loser
  folding). Reused as-is, invoked transitively via `resolve_merge_conflicts`.
- `GitRepo::with_write_lock` (`ddb-core/src/git_ops/mod.rs:94-99,310+`) — re-entrant
  advisory write lock. Reused for the new `delete_remote_ref`, matching every other
  git-mutating `GitRepo` method.
- `GitRemote::fetch`/`add_remote`/`push` (`ddb-core/src/git_ops/remote.rs`) — the
  sibling file and delegation pattern `delete_remote_ref` is added alongside.
- **Nothing found for**: a pre-existing "delete a remote-tracking ref" helper (greps
  tried: `rg -e "delete_remote" -e "update-ref" -e "prune_ref" -n ddb-core/src`; only
  hit is the bundle.rs CLI call this PRD replaces) — confirms `delete_remote_ref` is
  new, not a duplicate.
- **Nothing found for**: an existing index/merge-state (non-stderr) conflict detector
  bolted onto a CLI `git merge` (greps tried: `rg -e "MERGE_HEAD" -e "merge --abort" -n
  ddb-core/src`; zero hits anywhere in the crate) — confirms the codebase has no
  precedent for "detect conflicts after CLI merge"; the libgit2 path is the only
  existing conflict-detection mechanism, which is why this design routes through it
  instead of retrofitting stderr-detection with index reads.

## Alternatives considered

1. **(chosen) Route bundle import through `merge_remote` + `apply_merge_result`
   (reuse the network-sync engine).** Smallest diff that satisfies every PRD success
   metric: index/merge-state detection, honest count, ref-after-success ordering (falls
   out of the existing `?` once the count is honest), and "no dangling MERGE_HEAD, no
   partial index" (never created in the first place — libgit2's `merge_commits` never
   touches the worktree or `MERGE_HEAD` on conflict). Cost: two small, contained
   interface widenings (`apply_merge_result` → `pub(crate)`, new `delete_remote_ref`
   trait method).
2. **Keep the CLI `git merge`, add index-state detection on top (the PRD's own
   documented Risk mitigation option 1).** After a failed CLI merge, read
   `repo.index()` for unmerged (stage 1/2/3) entries instead of parsing `stderr`, run
   the *existing* resolver against them, then commit via `commit_merge`, then `git
   merge --abort` only if unresolvable. Rejected: this still leaves conflict markers
   written into the working tree by the CLI merge (a second toolchain touching the
   same worktree files `commit_merge`'s in-memory approach never touches), requires a
   real `git merge --abort` / index-reset step this design doesn't need, and mixes two
   git toolchains (CLI index vs. libgit2 index) reading the same `.git/index` — exactly
   the risk the PRD itself flags as unresolved by this option. Larger diff, more new
   failure surface, for no metric this design doesn't already satisfy.
3. **Smallest-diff variant: fix only conflict detection in place, leave `merge_bundle_and_resolve`
   returning `Result<usize>`, no `delete_remote_ref`.** Keep today's raw CLI
   `update-ref -d` (still swallowed-error `let _ = ...`) and don't touch `GitRemote`.
   Rejected: the AGENTS.md P0 invariant this PRD exists to close explicitly names the
   *whole* bundle-import write path as needing the write lock, not just the merge step;
   leaving ref-delete on an unlocked, error-swallowing CLI call would ship 00168 without
   actually closing the invariant gap it is tracked against. The `SyncReport` widening
   (option 1's byproduct) is also strictly smaller code than rebuilding a mostly-hardcoded
   report struct by hand, so "smaller" doesn't actually favor this variant either.

## Risks & edge cases

- **PRD-text reconciliation needed: this design does not literally run `git merge
  --abort`, and that is deliberate — `/plan-tasks` and any PRD-only (blind) reviewer
  must read this note before treating the PRD's literal wording as an unmet
  acceptance criterion.** The PRD's Phase 1 task and Test Strategy say "on any error
  run `git merge --abort` ... and return a Sync error" / "on an unresolvable merge,
  `git merge --abort` + surfaced error, repo clean." Under this design there is no CLI
  `git merge` in the bundle-import path at all (see Data flow step 2), so there is
  nothing to abort — `merge_commits` computes the merge entirely in memory and never
  creates `MERGE_HEAD` or touches the worktree on conflict (verified against
  `perform_normal_merge`). The PRD's own **Success Metrics** state the actual
  observable contract this design satisfies exactly: "the repo is left clean (no
  dangling `MERGE_HEAD`, no partial index)" and "a real `Sync` error surfaces" — both
  true here, achieved by never entering a conflicted git-CLI state in the first place
  rather than by entering one and then aborting it. `/plan-tasks` should NOT generate a
  task to invoke `git merge --abort`; the Phase 2 "clean-repo-on-error" test (Test
  strategy outline) pins the actual guarantee (no `MERGE_HEAD`, no unmerged index
  entries, `Sync` error returned) instead. A PRD-only blind reviewer checking for the
  literal words "git merge --abort" in the implementation should be pointed at this
  note rather than raising it as a missing-behavior gap.
- **Unrelated-histories merge via `merge_commits` instead of CLI `--allow-unrelated-histories`.**
  `merge_remote`'s `merge_analysis`/`merge_commits` have no explicit
  "refuse unrelated histories" guard in this codebase (that refusal is a CLI-only safety
  check `git merge` adds since git 2.9; libgit2's core merge algorithm has no such gate).
  The existing test `full_bundle_import_on_new_repo` (`bundle.rs` tests) already
  exercises exactly this shape today (a brand-new repo, disjoint root commit, importing
  a full bundle) — it stays in this PRD's Phase 2 regression set, so if this assumption
  is wrong the existing test fails immediately and visibly, not silently.
- **`apply_merge_result` visibility (`pub(crate)`) is a genuine, if small, API surface
  widening inside the crate.** Scoped to `pub(crate)`, not `pub` — no change to
  `ddb-core`'s public API, no FFI/CLI/server-visible surface change. Grep confirmed
  `GitRepo` is `GitBackend`'s only implementor, so no downstream trait-impl breakage.
- **The two-lock-acquisition gap (`merge_remote` releases the lock before
  `apply_merge_result`'s pure CRDT computation, `commit_merge` re-acquires it) is not
  new** — it is the exact gap the already-shipped network `sync()` path has lived with
  since 00163 (memory `project_data_loss_write_path`: "F1 remains 00163 T2... merge lock
  + fresh staging base, arm semantics untouched"). Bundle import inherits the identical,
  already-accepted risk profile rather than a new one.
- **`delete_remote_ref` on a ref that was concurrently deleted or never created**
  (e.g. a retried import after a prior successful one already deleted it) returns
  `Ok(())` (`ErrorCode::NotFound` treated as already-satisfied), matching today's
  effectively-idempotent `let _ = ...` behavior but now for the right reason (checked
  no-op) rather than swallowing a real error too.
- **Next likely follow-ups this design should not box in:** (1) a delta bundle import
  (`export_bundle`'s targeted variant) hits the identical `merge_bundle_and_resolve` /
  `import_bundle` code path — already covered by this fix, no separate work; (2) if a
  future PRD wants bundle import to expose `commits_transferred` accurately (today
  approximated as `0`/`1` same as network sync, a pre-existing limitation this PRD does
  not touch), `SyncReport` is already the return type, so that's a `merge_remote`-level
  fix, not a bundle.rs one; (3) FFI's `import_bundle` binding (`ffi/records.rs` or
  similar) returns `SyncReport` today and after this change — no signature change
  downstream, only field values become more honest.

## Test strategy outline

- **Unit** (`ddb-core/src/bundle.rs` `#[cfg(test)]`): a new conflicted-bundle test —
  two repos diverge on the same doogat path from a shared ancestor, node1 exports a
  bundle, node2 imports it → assert `report.conflicts_resolved > 0`, a real 2-parent
  merge commit exists at HEAD (`repo.commit_parent_count(&head) == 2` via the existing
  `GitHistory` trait method), `refs/remotes/bundle/master` is gone, and the resolved
  content is present. A locale/wording-independence unit test is redundant with this
  design (there is no stderr string match left to test), but keep the shape: assert
  detection works for both an auto-mergeable-with-conflict case and an add-add
  collision case (exercises `partition_conflicts`' existing branches through the new
  entry point).
- **Reachability test**: import a bundle whose merge is forced to fail (e.g. inject a
  collision loser with no frontmatter, per PRD 00167's `rewrite_id_field` failure mode)
  → assert `refs/remotes/bundle/master` still exists and resolves to the unbundled
  commits (`repo.find_reference` / `revparse` succeeds), and a `DoogatError::Sync` is
  returned.
- **Clean-repo-on-error test**: same failure injection → assert no `MERGE_HEAD` file
  exists under `.git/` and `repo.index()` (or `git status --porcelain` if easier from a
  test) has no unmerged entries. Given the design, this should pass structurally; the
  test exists to pin the property (the PRD's own explicit success metric), not because
  a `git merge --abort` call needs testing (there is none).
- **Regression**: existing `full_bundle_export_and_verify`, `checksum_verification_catches_tampering`,
  `full_bundle_import_on_new_repo`, `delta_export_targets_node_and_uses_known_heads`,
  `delta_export_fails_for_unknown_node` (all in `bundle.rs`) stay green unmodified.
- **Integration** (`tests/integration.sh` + `.ps1`, `=== bundle sync ===` section,
  after the existing "delta bundle export + import" scenario ~line 3191): a new
  numbered scenario — two `ddb` CLI instances diverge, export/import a bundle through a
  real conflict, assert on `ddb bundle import`'s reported conflict count and that a
  second import (no-op) doesn't error.
- **e2e** (`tests/e2e/bundle_import_conflict_recovery.rs`, new file, following the flat
  naming convention of `tests/e2e/collision.rs` / `tests/e2e/sync_crdt.rs`): drives the
  `ddb` binary end-to-end through export → conflicting local write → import, asserting
  on CLI output and post-import file state — the durable proof-of-work artifact per
  AGENTS.md Definition of Done.
- All new/changed tests must fail against the current (pre-fix) code and pass after,
  per the PRD's stated Critical Scenarios and this repo's `rules/testing.md`.

## Review log

### Dispatch 1 (Claude)

Fixed the one blocker: added an explicit PRD-text-reconciliation note to Risks &
edge cases explaining that "git merge --abort" is never literally invoked by design
(nothing is ever in a conflicted CLI-merge state to abort), and that the PRD's own
Success Metrics wording (clean repo, no MERGE_HEAD, Sync error surfaces) is what this
design actually satisfies.

Logged, not fixed (non-blocking / question):
- `SyncManager::resolve_post_merge_conflicts` becomes an orphaned `pub fn` once its
  only caller (`bundle.rs`'s old `merge_bundle_and_resolve` body) is replaced — decide
  at implementation time whether to delete it or repurpose it; not decided here.
- Risks section didn't call out that `unbundle_git_objects`'s CLI `git bundle
  unbundle`/`git fetch` remain unlocked (deliberately out of this PRD's scope — a
  side-ref-only leg, not master) — worth a future reader's awareness, not a blocker.
- Question: collapsing every resolver error (including `DoogatError::Conflict`) into
  `DoogatError::Sync` in `merge_bundle_and_resolve` matches the PRD's literal wording
  but loses the original variant; whether FFI/CLI callers need to distinguish "real
  conflict, retry" from other failures is left to implementation-time judgment.
- Question: the equivalence between libgit2 `merge_commits` and CLI
  `--allow-unrelated-histories` is asserted (no explicit refusal gate in libgit2's
  core merge) and backed by the existing `full_bundle_import_on_new_repo` test, but
  that test hasn't yet been run against the new `merge_remote`-based path specifically
  (it will be, as part of this PRD's Phase 1/2 regression pass).

dispatch 1 (claude): cardinal-sin 0, blocker 1, non-blocker 2, question 2
