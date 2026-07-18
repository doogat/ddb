# Git Operations

**Source**: `ddb-core/src/git_ops/mod.rs`

Wraps libgit2 (`git2` crate) for all Git repository interactions. `GitRepo` is the concrete implementation behind the `GitBackend` trait.

## GitBackend Trait

All callers access git operations through the `GitBackend` trait (defined in `traits.rs`), not through `GitRepo` directly. This enables swapping libgit2 for gitoxide per-feature in Phase 3 without changing callers.

`GitBackend` extends `DoogatSource + DoogatStore` with:

- **Remote ops**: `add_remote`, `fetch`, `push`
- **Merge ops**: `merge_remote`, `commit_merge`
- **Binary file ops**: `commit_binary_file`, `commit_binary_and_text`, `read_blob`
- **Commit introspection**: `merge_base`, `commit_parent_count`, `commit_parent_oid`, `read_file_at`, `walk_tree_files`
- **History queries**: `find_hlc_for_path`, `revision_date`
- **Config**: `load_config`, `repo_path`
- **Desktop-only hooks** (default no-ops): `set_skip_commit_graph`, `write_commit_graph`, `increment_session_commits`, `reset_session_commits`

No `git2` types appear in the trait signature. All OIDs are passed as `&str` hex strings.

## GitRepo

The sole concrete `GitBackend` implementation. Constructors (`init`, `open`) and format migration remain inherent methods on `GitRepo`.

```rust
pub struct GitRepo {
    pub repo: Repository,  // git2::Repository
    pub path: PathBuf,
}
```

## Initialization

`GitRepo::init(path) -> Result<Self>`

Creates a new Git repository with the standard directory structure:

| Directory | Purpose |
|-----------|---------|
| `ddb/` | Doogat Markdown files |
| `reference/` | Binary/asset files |
| `.nodes/` | Node registry (TOML configs) |
| `.crdt/temp/` | Temporary CRDT files |

Each directory gets a `.gitkeep` file. A `.gitignore` is created/updated to exclude `.ddb/` (the local SQLite index directory). A `.ddb-version` file is written with the current format version (currently `1`). An initial commit is made with all scaffolding.

## Format Versioning

The `.ddb-version` file at the repository root tracks the on-disk format version (currently `1`).

- **On init**: written with `CURRENT_FORMAT_VERSION`
- **On open**: read and checked:
  - Repo version > driver version → `VersionMismatch` error (upgrade ddb)
  - Repo version < driver version → auto-migrate (e.g. v0→v1 writes the version file)
  - Missing file → treated as v0, auto-upgraded

Future format changes increment `CURRENT_FORMAT_VERSION` and add a migration step in `migrate_format()`.

## File Operations

| Method | Purpose |
|--------|---------|
| `commit_file(rel_path, content, msg)` | Write, stage, and commit a single file |
| `commit_files(files, msg)` | Write, stage, and commit multiple files atomically |
| `commit_merge(files, binary, msg, theirs_oid)` | Rebuild the conflicted-merge tree as a true three-way merge, overlaying only the CRDT-resolved blobs onto conflicting paths, and commit it with two parents |
| `read_file(rel_path)` | Read file content from HEAD tree (not working directory) |
| `list_doogats()` | Walk HEAD tree, return all `ddb/*.md` paths |
| `head_oid()` | Get current HEAD commit OID |

Note: `read_file` reads from the Git tree, not the filesystem. This ensures consistency with the committed state and avoids platform-specific working-tree transforms such as CRLF checkout conversion on Windows.

All commit methods (`commit_files`, `commit_merge`, `delete_file`) and successful merge paths (`merge_remote`) write the commit-graph file via `git commit-graph write --reachable`. This accelerates `merge_base()` and log traversal. Best-effort: silently ignored if `git` CLI unavailable.

## Write serialization (cross-process lock)

**Source**: `ddb-core/src/git_ops/write_lock.rs`

Every git write path holds a repo-scoped, cross-process advisory lock for its whole critical section, so concurrent writers (a downstream app running `ddb serve` while the user runs the CLI, a script firing two `ddb create`s, or several threads) cannot lose commits.

- **What it guards**: all eight write functions — `commit_file`, `commit_binary_file`, `commit_files`, `delete_file`, `delete_files`, `rename_file`, `commit_batch`, `commit_binary_and_text` — run their bodies inside `GitRepo::with_write_lock`. The lock is held across the entire write → stage → `write_tree` → resolve-parent → `commit` section. Every guarded path (deletes included) stages from `fresh_index()`, never the process-cached `repo.index()`.
- **Merge/sync writes** (`ddb-core/src/git_ops/merge.rs`): the merge write path also runs inside `with_write_lock` — `commit_merge`, and `merge_remote`'s normal 3-way merge commit + forced checkout (`perform_normal_merge`) and fast-forward `set_target` + forced checkout. The lock covers all three write sections; the fresh-index staging is specific to the conflicted-merge tree construction — only `commit_merge`'s `build_merge_tree_and_commit` builds its tree from `fresh_index()`. The normal 3-way merge commits from libgit2's merge-result index, and the fast-forward path stages no tree (it moves the ref and force-checks-out). The `git commit-graph write` is relocated outside the locked section so its subprocess cannot stall a contended writer under the acquire timeout. The one write path still outside the lock is bundle import (`bundle::merge_bundle_and_resolve` shells out to `git merge` directly), tracked by 00168.
- **Why**: without it, two processes can each build a tree from the same `HEAD`, then commit in turn; the second resolves a stale parent and force-updates the ref with no compare-and-swap, so the first commit is silently dropped (and later pruned by `git maintenance --auto`). Under the lock, each writer re-resolves `HEAD` and re-reads the index against the latest committed tree.
- **Mechanism**: an exclusive advisory lock on `<repo_root>/.git/ddb-write.lock`, taken via `fs2` (`flock` on Unix, `LockFileEx` on Windows). `.git/` is never tracked, so the lock file is a pure runtime artifact — never committed, and it survives `git gc`.
- **Reentrancy**: reentrant on the same `GitRepo` instance (the owning thread), tracked by a per-instance `write_lock_depth`. A write path that delegates to another (`commit_file` → `commit_files`, `rename_file` → `commit_batch`) runs the inner body directly instead of re-acquiring, which a second exclusive acquisition would otherwise deadlock. Reentrancy does not span instances: a second `GitRepo` handle on the same path (even in the same process) goes through the OS flock and blocks until the first releases.
- **Fail-loud**: acquisition blocks up to a bounded timeout, then returns a retryable `DoogatError::Conflict` naming the lock file rather than hanging forever.
- **Ordering**: the write lock is the innermost resource. SINGLETON create/upsert paths take a SQLite `BEGIN IMMEDIATE` transaction first, then commit (SQLite-outer, git-inner); no path takes the git lock and then opens a SQLite immediate transaction, so the two cannot deadlock.

## Remote Operations

| Method | Purpose |
|--------|---------|
| `add_remote(name, url)` | Register a named remote |
| `fetch(remote, branch)` | Fetch from remote |
| `push(remote, branch)` | Push to remote |

Remote URLs can be local filesystem paths, SSH URLs, or any Git-compatible transport.

## Merge

`merge_remote(remote, branch) -> Result<MergeResult>`

### Algorithm

1. Find the remote branch ref (`refs/remotes/{remote}/{branch}`)
2. Run `merge_analysis()` to determine the merge type:
   - **Up-to-date**: nothing to do
   - **Fast-forward**: update ref and checkout
   - **Normal merge**: perform 3-way merge
3. For normal merges, call `merge_commits(ours, theirs)` to get a merge index
4. If the index has conflicts:
   - Extract each conflict's ancestor/ours/theirs blob content
   - Return `MergeResult::Conflicts(vec, theirs_oid)`
   - Clean up merge state
5. If clean: write the merge tree, create a merge commit with two parents, checkout

### Conflict Extraction

`extract_conflicts(index) -> Result<Vec<ConflictFile>>`

For each conflict entry in the merge index, reads the blob content for ancestor (if present), ours, and theirs. Returns `ConflictFile` structs ready for CRDT resolution.

### Conflicted-Merge Commit (Three-Way)

`commit_merge(files, binary, message, theirs) -> Result<CommitHash>`

Called by `sync_manager` after CRDT/LWW resolution to commit a conflicted merge. As of PRD 00200, the tree is rebuilt as a true three-way merge — the same construction the clean-merge path (`merge_remote`'s normal-merge branch above) already uses:

1. Re-run `merge_commits(our_commit, their_commit)` in memory. Every non-conflicting path is already correct at stage 0 of this fresh three-way result (ours-only edits kept, theirs-only changes kept, both-edited-in-different-regions files line-merged, theirs' plain deletions absent, ours' creates present) — libgit2 preserves it without any special-casing.
2. **Divergence guard**: snapshot the conflicting paths from this fresh merge index and compare them to the resolved set (`files` + `binary`, the paths `sync_manager` actually resolved). If `HEAD` moved between conflict resolution and this commit, the sets diverge and `commit_merge` fails loud with a retryable `Conflict` rather than committing a stale resolution.
3. Overlay the resolved blobs onto only the conflicting paths at stage 0 — CRDT/LWW text and delete-vs-edit resurrect markers from `files`, binary LWW winners overlaid by OID (no working-tree write) from `binary`.
4. Write the resulting tree and commit it with two parents (`[ours, theirs]`).
5. `checkout_head` force-syncs the worktree to the newly committed merge tree.

This replaces the previous two-way ours→theirs diff, which silently reverted non-conflicting local edits back to the base, resurrected paths the other side had plainly deleted, and (for a path both sides edited in different regions) discarded one side's edit by keeping only theirs' whole blob.

## Signature

Uses the repository's configured `user.name` and `user.email`. Falls back to `"ddb"` / `"ddb@local"` if not configured.

## Test Coverage

8+ tests:
- Init creates directory structure and `.gitignore`
- Open existing repo
- Commit and read file round-trip
- Multi-file commits
- List doogats (filters to `ddb/*.md`)
- Push/fetch cycle between two repos
- Merge already-up-to-date
- Merge conflict detection with blob extraction
