# Bundle Protocol

**Source**: `ddb-core/src/bundle/mod.rs`

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
4. Require `objects.bundle` (a missing Git payload fails before any repo change) and compute the payload id: the full 64-hex SHA-256 of the `objects.bundle` bytes
5. Take the bundle-import lease (see below)
6. Read the bundle's advertised `refs/heads/*` and their OIDs (`git bundle list-heads`, parsed strictly; `HEAD` and tags never select a branch), and check any refs already under `refs/remotes/bundle/<payload-id>/` against them
7. `git bundle unbundle` + `git fetch --no-prune --no-tags` of `refs/heads/*` into `refs/remotes/bundle/<payload-id>/*`, then verify the fetched OIDs. No `+` or `--force` is passed, but git still accepts non-fast-forward updates under `refs/remotes/`, which is why step 6 checks every preexisting namespace ref against the advertised heads before the fetch. Tags are never fetched
8. Require an advertised `master`, then merge `refs/remotes/bundle/<payload-id>/master` via `GitRepo::merge_remote_allowing_unrelated` — the same write-locked libgit2 merge engine `ddb sync` uses
9. Resolve conflicts via the CRDT cascade (`SyncManager::apply_merge_result`), producing a real merge commit and the true resolved-conflict count
10. Import node registrations
11. Rebuild index
12. Delete this payload's namespace refs through `delete_remote_ref` (only reached after steps 8-11 succeed)

### Payload identity and namespaced refs

The payload id depends only on the Git payload. The same `objects.bundle` bytes always map to the same namespace, so a retry of a failed import reuses its refs; a change to the manifest or node files alone keeps the id, and node registration still runs on every import. The v1 archive checksum is unchanged and is not used as an identity.

Before fetching, every ref already in the namespace must name an advertised branch at its advertised OID. Anything else (an extra branch, a wrong OID) is refused with a `Sync` error naming the namespace; nothing is fetched, forced, merged or deleted. Only an advertised `master` is merged. A bundle without one fails as `Sync` with `bundle merge failed:`, the payload id and the delivered branch names, and its fetched refs stay in place. There is no fallback to another namespace's `master`, to `main`, or to the first branch.

Cleanup removes only the advertised branches of this namespace, after checking they still match. Refs of other payloads, older flat `refs/remotes/bundle/<branch>` refs from earlier versions, and unrelated remotes are never touched; nothing migrates or garbage-collects them. Cleanup is not transactional: a cleanup error is loud and may leave the namespace partly deleted.

### Import serialization

Cooperating imports on one repo are serialized by a bundle-import lease: an exclusive advisory lock on `ddb-bundle-import.lock` in Git's common metadata directory (`git rev-parse --git-common-dir`), taken through the same `git_ops::write_lock::acquire` primitive as the Git write lock but on a separate, persistent file that is never unlinked. It is held from namespace inspection through final cleanup or error. Waiting for it is bounded at 30 seconds; a timeout fails with a `Conflict` error naming the bundle-import lease. The timeout bounds only that wait, not the import.

The lease is not the Git write lock and is taken before, never inside, the Git, SQLite or rebuild locks. It excludes only other importers running this version. Sync, CRUD writes and older binaries do not take it, so mixed-version concurrent imports are outside its guarantee. It does not make the unbundle/fetch leg or the final `index.rebuild` hold the Git write lock or the rebuild lock (see `invariants.md`).

## Conflict Recovery

A conflicted import is resolved, not silently dropped. `merge_bundle_and_resolve` detects conflicts from the structured result of an in-memory libgit2 merge (`merge_commits`, surfaced as `MergeResult::Conflicts`), never from `stderr` text and never from unmerged entries in the repo's on-disk index, and drives them through the same conflict-resolution cascade `ddb sync` uses (three-way git merge → CRDT per-zone merge → LWW fallback — see `sync.md` § "Conflict Resolution Cascade"). The resolution lands as a real two-parent merge commit reporting the true conflict count.

The payload's `refs/remotes/bundle/<payload-id>/` refs are deleted only after the merge, node registration and the final index rebuild all succeed, so the unbundled data stays reachable — and therefore un-prunable by `git gc` — until the import actually lands. On an unresolvable merge, `import_bundle` returns a `Sync` error naming that namespace and the refs are left in place. They stay until a retry of the same bundle succeeds or an operator removes that one namespace by hand after recovering what they need; there is no abort command. On the conflicted path, no `git merge --abort` step is needed because the merge is computed entirely in memory (libgit2's `merge_commits`) and never touches the worktree or creates `MERGE_HEAD` on conflict. The clean (non-conflicted) merge path differs: the merge commit lands and the worktree is force-checked-out before post-merge validation runs. If that validation, node registration or the final rebuild then fails, no data is lost (the bundle refs survive), but `HEAD` has already moved rather than the repo staying untouched. A node-registration or final-rebuild failure also logs a warning naming the kept namespace; the returned error itself is unchanged.

**Design note**: the PRD's success metric ("a locale/wording change in git output does not flip conflict detection") is satisfied structurally, not by a dedicated test. Conflict detection reads the in-memory merge result, not CLI git output, so there is no `stderr` string for a locale change to alter. CLI git stdout is parsed only during import preparation, and only after a successful exit: the lease directory from `git rev-parse --git-common-dir`, and `<oid> <ref>` records from `git bundle list-heads` and `git for-each-ref`, which are checked strictly so a malformed record fails loud. The only guard against reintroducing `stderr` parsing is an incidental assertion that `.git/MERGE_HEAD` is absent.

Re-importing the same bundle after a successful import is a no-op: its commits are already ancestors of local HEAD, so the merge classifies as already-up-to-date and reports zero (new) conflicts.

### Recovery workflow and interface conformance

Kept refs make a failed import safe to retry. Fix the cause (for example, repair the colliding file locally), then import the same bundle again: the same Git payload maps to the same namespace, the retry merges it, and only that namespace is deleted. Other imports' kept refs are never merged or deleted by it. A later bundle that delivers no `master` fails rather than merging a kept `master` from an earlier failed import. An independent successful import leaves every other namespace, legacy flat ref and unrelated remote ref untouched.

The same golden workflow (failed A, local repair, masterless B refused, independent C succeeds, repaired retry of A, no-op reimport; under `fetch.prune` true and false) runs through both public interfaces that expose bundle import:

| Interface | Success | Failure | Conformance tests |
|-----------|---------|---------|-------------------|
| CLI `ddb bundle import <path>` | exit 0, `imported: conflicts resolved: <n>` | nonzero exit, no success line; a merge failure prints `bundle merge failed:` naming the payload namespace; a bundle without `master` also lists its delivered branches | `tests/e2e/integration_bundle_import_ref_namespace.rs` |
| FFI `DoogatDriver::import_bundle` | `Ok(())` (no `SyncReport` crosses FFI) | `DdbError::Git { msg }` carrying the CLI's `bundle merge failed:` text: the payload namespace, plus the delivered branches for a bundle without `master` | `ddb-core/src/ffi/tests.rs` (`bundle_import_ffi_*`) |

The FFI tests exercise the public Rust driver, not generated Swift/Kotlin bindings. GraphQL, REST, PgWire and NoSQL HTTP have no bundle-import endpoint and make no bundle-import promise.

Limits: there is no abort or discard command. A kept namespace stays until a retry of the same bundle succeeds or an operator deletes that one namespace by hand; a recovery API is deferred to its own design. Kept refs are not garbage-collected automatically. The bundle-import lease serializes cooperating importers only and does not close the separate Git write-lease gaps listed in `invariants.md`.

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
