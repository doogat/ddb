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
5. `git merge` bundle refs into local master
6. Resolve conflicts via CRDT cascade (if any)
7. Import node registrations
8. Rebuild index

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
