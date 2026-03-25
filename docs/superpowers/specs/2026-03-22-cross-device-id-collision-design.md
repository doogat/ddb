# Cross-Device ID Collision Detection & Resolution

## Problem

DoogatIds are 14-digit timestamps (`YYYYMMDDHHmmss`). When two devices independently create a doogat in the same second, both produce the same ID and the same file path. During sync, Git sees a content conflict on a single path. The current CRDT/LWW cascade treats this as a normal edit conflict and picks one version — the other doogat's content is silently lost.

This is a data loss scenario. Automated creation workflows (scripts, FFI-driven mobile apps) make same-second collisions plausible.

## Scope

Phase 1 pre-release fix. Detect and resolve cross-device ID collisions at sync time. No changes to the DoogatId format.

## Spec Reference

From `initial-system-specification.md` section 2 (Filename Convention):

> On ID collision (same-second create on the same device), the driver waits 1 second and regenerates the ID. Cross-device same-second collisions are detected at sync time and resolved by assigning a new ID to one doogat and updating its wikilinks.

## Design

### Detection

**Where:** `sync_manager.rs`, after `merge_remote()` returns `MergeResult::Conflicts`.

**How:** Extend the existing 2-bucket conflict partition to 3 buckets:

| Bucket | Condition | Existing? |
|--------|-----------|-----------|
| Delete-vs-edit | `ours.is_empty() \|\| theirs.is_empty()` | Yes |
| **Add-add collision** | **`ancestor.is_none()` and both sides non-empty** | **New** |
| Normal conflict | Everything else | Yes |

`ancestor` is `None` when the file does not exist in the merge base. Both sides non-empty means both devices independently created a file at the same path — which can only happen when both generated the same DoogatId.

### Resolution

For each add-add collision:

1. **Pick winner by HLC.** The doogat with the later HLC timestamp keeps its ID. On HLC tie or missing HLC, "theirs" (remote) wins. This differs from the LWW fallback (which prefers "ours" on tie). Here "theirs" wins because the remote doogat may already be linked from other synced devices; reassigning the local doogat minimizes cross-device link breakage.

2. **Add winner to merge commit.** The winner's content goes into the `resolved` vec alongside other conflict resolutions. It is included in the normal merge commit that `sync()` creates — same pattern as delete-vs-edit resurrection. No explicit delete is needed for the loser at the original path because the winner's content overwrites it.

3. **Post-merge: generate new ID for loser.** After the merge commit, call `generate_unique_id()` with a collision checker covering both the filesystem and the winner's ID.

4. **Update loser's frontmatter.** Rewrite its `id` field to the new ID.

5. **Compute new path.** Parse `type` from loser's frontmatter. Use `doogat_path(new_id, type_name, folder)` to respect folder-typed storage (e.g., `ddb/contact/{new_id}.md`).

6. **Rewrite wikilinks and write loser.** Full tree scan of all `.md` files from Git HEAD. For each file containing the old ID, call `parser::rewrite_links()` twice: once with the bare old ID (`20260101120000`), once with the path form (`ddb/contact/20260101120000`). Collect all rewrites plus the loser doogat at its new path into a single `commit_batch()`: `fix: reassign collided doogat ID {old} -> {new}`.

7. **Report.** Add `collisions_reassigned: usize` field to `SyncReport`. Emit `tracing::warn!` with both IDs.

### Why full tree scan instead of index lookup

The SQLite index is stale during sync (pre-reindex). The link rewrite must scan `.md` files from Git HEAD directly. This is a different constraint than `rename_doogat`, which can rely on the index because it runs outside the sync path. The post-sync `reindex()` call rebuilds the index with the corrected state.

### Reused machinery

| Component | Purpose |
|-----------|---------|
| `parser::generate_unique_id(exists_fn)` | Generate collision-free new ID |
| `parser::rewrite_links(content, old, new)` | Rewrite all wikilink formats (wikilinks, markdown links, embeds) |
| `repo.commit_batch(writes, deletes, msg)` | Atomic multi-file commit |
| `git_ops::doogat_path(id, type, folder)` | Compute storage path respecting folder-typed layout |

## Edge Cases

| Case | Handling |
|------|----------|
| Multiple collisions in one sync | Each resolved independently. `generate_unique_id()` checks filesystem, so sequential resolutions don't collide with each other. N collisions take O(N * 100ms) for ID generation due to spin-wait. |
| Reassigned ID also collides | `generate_unique_id()` spin-waits until unused ID found. Covered by existing logic. |
| Bundle import collision | `import_bundle()` merges via the same conflict-handling code path. No separate handling needed. |
| Folder-typed doogat | Parse `type` from loser's frontmatter, compute path via `doogat_path()` with folder flag. |
| Wikilink formats | `rewrite_links()` handles path-qualified, ID-only, markdown links, and embeds. Called twice per file: once with bare ID, once with path form (minus `.md` suffix). |
| Index rebuild | Normal post-sync `reindex()` rebuilds index with corrected state. No special handling. |
| Link rewrite failure | If the post-merge fixup commit fails partway, the repo has the merge commit (both doogats' data preserved under the winner's ID) but stale links. Data is safe; links are repairable by re-syncing or manual `reindex`. Acceptable for this edge-of-edge case. |

## Data Flow

```
merge_remote() returns Conflicts
  |
  v
Partition conflicts into 3 buckets
  |
  +-- delete-vs-edit --> existing resurrection logic --> add to resolved vec
  |
  +-- add-add (ancestor=None, both non-empty) --> pick winner by HLC
  |     |                                         add winner to resolved vec
  |     |                                         stash loser content for post-merge
  |
  +-- normal conflicts --> existing cascade_resolve() --> add to resolved vec
  |
  v
commit_merge() with all resolved files (winner occupies original path)
  |
  v
Post-merge collision fixup (for each stashed loser):
  +-- Generate new ID
  +-- Update loser frontmatter + compute new path
  +-- Full tree scan from HEAD: rewrite_links(old_id -> new_id) x2 (bare + path form)
  +-- Single commit_batch: loser at new path + all rewritten files
  +-- Update SyncReport.collisions_reassigned
  |
  v
reindex() rebuilds SQLite from corrected Git state
```

## SyncReport Change

```rust
pub struct SyncReport {
    pub direction: String,
    pub commits_transferred: usize,
    pub conflicts_resolved: usize,
    pub resurrected: usize,
    pub collisions_reassigned: usize,  // NEW
}
```

## Testing

### Unit tests (sync_manager)

- Two devices create doogat with same ID, sync detects add-add collision
- Winner determined by HLC (later wins)
- HLC tie: theirs wins
- Loser gets new ID, frontmatter `id` field updated, file at new path
- Wikilinks in other doogats rewritten to new ID
- `SyncReport.collisions_reassigned == 1`

### Unit tests (parser)

- `rewrite_links` coverage for bare ID, path-qualified, and folder-typed paths (extend existing tests if gaps found)

### E2E test (multi_device.rs)

- Force two devices to create same ID (direct `commit_file` with identical ID), sync, verify both doogats exist with distinct IDs and correct content
- Third doogat with wikilink to collided ID: verify link rewritten to new ID after sync

### Property test

- Random concurrent creation bursts on 2-3 nodes, sync all, verify no duplicate IDs across nodes and all content preserved
