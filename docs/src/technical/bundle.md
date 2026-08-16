# Bundle Protocol

**Source**: `ddb-core/src/bundle.rs`

Air-gapped sync via tar bundles for environments without network connectivity.

## Bundle Format

```
bundle.tar
├── manifest.toml    # source_node, target_node, timestamp, format_version
├── objects.bundle   # git bundle (delta or --all)
├── nodes/           # .toml files for node registrations
│   └── {uuid}.toml
└── checksum.sha256  # SHA-256 of all other files
```

## Manifest

```toml
source_node = "abc-123"
target_node = "def-456"   # or "*" for full export
timestamp = "2026-03-01T12:00:00Z"
format_version = 1
```

## Export Modes

### Delta bundle

Exports only commits the target hasn't seen, based on `known_heads`:

```bash
ddb bundle export --target <uuid> --output path.tar
```

### Full bundle

Exports all refs for bootstrapping a new node:

```bash
ddb bundle export --full --output path.tar
```

## Import

```bash
ddb bundle import path.tar
```

Steps:
1. Extract tar to temp directory
2. Verify SHA-256 checksum
3. Parse manifest
4. `git bundle unbundle` + `git fetch` from bundle
5. Merge bundle refs into local master via `GitRepo::merge_remote` — the same write-locked libgit2 merge engine `ddb sync` uses
6. Resolve conflicts via the CRDT cascade (`SyncManager::apply_merge_result`), producing a real merge commit and the true resolved-conflict count
7. Import node registrations
8. Delete `refs/remotes/bundle/master` (only reached after a successful merge)
9. Rebuild index

## Conflict Recovery

A conflicted import is resolved, not silently dropped. `merge_bundle_and_resolve` detects conflicts from git's merge state (unmerged index entries), never from `stderr` text, and drives them through the same conflict-resolution cascade `ddb sync` uses (three-way git merge → CRDT per-zone merge → LWW fallback — see `sync.md` § "Conflict Resolution Cascade"). The resolution lands as a real two-parent merge commit reporting the true conflict count.

`refs/remotes/bundle/master` is deleted only after that merge commit succeeds, so the unbundled data stays reachable — and therefore un-prunable by `git gc` — until the import actually lands. On an unresolvable merge, `import_bundle` returns a `Sync` error and the ref is left in place so the next import attempt can retry against the same data; no `git merge --abort` step is needed because the merge is computed entirely in memory (libgit2's `merge_commits`) and never touches the worktree or creates `MERGE_HEAD` on conflict.

Re-importing the same bundle after a successful import is a no-op: its commits are already ancestors of local HEAD, so the merge classifies as already-up-to-date and reports zero (new) conflicts.

## Pre-compaction Backup

Compaction automatically exports a full bundle before mutating data, providing a recovery path if compaction corrupts the repository. Backups are stored at `.ddb/backups/pre-compact-{ISO8601}.bundle.tar` by default.

```bash
ddb compact                          # backup + compact
ddb compact --no-backup              # skip backup
ddb compact --backup-path /tmp/b.tar # custom path
```

The GraphQL `compact` mutation accepts `noBackup: Boolean` and returns `backupPath: String` (null when skipped). To recover from a backup: `ddb bundle import <backup.bundle.tar>` on a fresh `ddb init`.

## Verification

```rust
let manifest = bundle::verify_bundle(&path)?;
// Returns BundleManifest without importing
```

## FFI Access

Both export modes and import are available through `DoogatDriver` (UniFFI bindings):

- `exportFullBundle(outputPath)` — full export
- `exportDeltaBundle(targetNodeUuid, outputPath)` — delta export targeting a specific node
- `importBundle(bundlePath)` — import with merge and reindex

## Security

Bundles include a SHA-256 checksum covering all files except the checksum itself. Import verifies this checksum before processing any git objects.
