# Git Scalability Audit

Audit of libgit2/gitoxide feature coverage for scalability features relevant to Doogat DB.

## Feature Matrix

| Feature | libgit2 (git2 0.19) | gitoxide | git CLI fallback | Notes |
|---------|---------------------|----------|------------------|-------|
| Sparse index | Not supported | Partial (read only) | `git sparse-checkout` | Not needed: Doogat DB reads all doogat paths |
| fsmonitor | Not supported | Not supported | `git config core.fsmonitor` | Requires watchman/fsmonitor daemon |
| Background maintenance | Not supported | Not supported | `git maintenance start` | Runs gc, commit-graph, prefetch on schedule |
| Pack optimization | `git2::Odb` (low-level) | `gix pack` (full) | `git repack` | Already covered by `git gc` |
| Shallow clone | `FetchOptions::depth()` | Partial | `git clone --depth` | Useful for initial mobile sync |
| Commit-graph | Read: yes. Write: no | Read: yes. Write: partial | `git commit-graph write` | Doogat DB shells out for write (Phase 1) |
| Bundle protocol | Not supported | Not supported | `git bundle` | Doogat DB shells out (Phase 2, PRD 5) |
| Multi-pack index | Not supported | Read: yes | `git multi-pack-index` | Transparent via git gc |
| Partial clone | Not supported | Not supported | `git clone --filter` | Not applicable (all content needed) |

| Incremental reindex | `diff_tree_to_tree`: full support | Full support | N/A | **Implemented in Phase 2** |

## Current Doogat DB Usage

- **git2 0.19**: all CRUD, merge, conflict detection, merge-base, tree traversal, `diff_tree_to_tree` for incremental reindex
- **Shell out to git CLI**: commit-graph write, gc, bundle create/unbundle
- **Not used**: gitoxide (gix)

## Incremental Reindex (Phase 2)

The primary scalability lever. Uses `git2::Diff` between stored HEAD and current HEAD to reindex only changed doogat files — O(changed) instead of O(total).

**How it works:**
1. `Index::stored_head_oid()` retrieves the last indexed HEAD from `_ddb_meta`
2. `GitRepo::diff_paths(old_oid, new_oid)` calls `diff_tree_to_tree` to get changed paths
3. For `Added`/`Modified`: read, parse, and upsert into SQLite
4. For `Deleted`: remove from SQLite index
5. If any `_typedef` path changed, trigger full `materialize_all_types` (schema change)
6. Falls back to full rebuild if the old HEAD is unreachable (e.g. after aggressive gc)

**Impact at scale:**
- 1K doogats, 1 change: ~1ms incremental vs ~200ms full rebuild
- 50K doogats, 10 changes: expected <10ms incremental vs seconds for full rebuild

## Recommendations

### Phase 3+ candidates

1. **Background maintenance** — shell out to `git maintenance start` on desktop/server. Skip on mobile (battery, no git CLI). Low effort, high value for repos >10K doogats.

2. **fsmonitor** — valuable for repos >5K doogats where `is_stale()` check becomes expensive. Requires watchman. Desktop-only. Medium effort.

3. **Shallow clone** — useful for mobile initial sync. `git2::FetchOptions::depth()` works. Requires careful handling of merge-base (may not exist in shallow history). Medium effort, medium value.

### Skip

- **Sparse index**: Doogat DB indexes all doogats, sparse checkout adds no value
- **Partial clone**: all doogat content is needed for indexing
- **gitoxide migration**: not mature enough for merge/conflict workflows; stick with git2 + CLI fallback
- **Multi-pack index**: transparent via git gc, no explicit handling needed

## Desktop vs Mobile

| Capability | Desktop/Server | Mobile (FFI) |
|-----------|---------------|--------------|
| git CLI | Available | Not available |
| Background maintenance | `git maintenance start` | App-triggered compaction |
| fsmonitor/watchman | Available | Not available |
| Commit-graph write | Shell out | Skip (read still works) |
| Bundle create/unbundle | Shell out | Embed tar logic in Rust |
