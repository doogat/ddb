# Multi-Device Sync

Doogat DB syncs across devices using Git remotes. No server or cloud required — any Git remote works (bare repo, SSH, local path).

## Setup

### 1. Create a Bare Remote

On a shared location (NAS, USB drive, SSH server):

```bash
git init --bare /path/to/ddb-remote.git
```

### 2. Add the Remote

On each device:

```bash
cd ~/my-ddb
git remote add origin /path/to/ddb-remote.git
# or: git remote add origin ssh://user@host/path/to/repo.git
```

### 3. Register Each Device

On each device:

```bash
ddb register-node "Laptop"
```

This:
- Generates a UUID for this device
- Creates `.nodes/{uuid}.toml` in the repository
- Stores the UUID locally in `.git/ddb-node`

Each device needs a unique name. The node registry is Git-tracked, so all devices learn about each other through sync.

### 4. Initial Push

From the first device:

```bash
git push -u origin master
```

## Syncing

```bash
ddb sync [remote] [branch]
```

Defaults: `origin` remote, `master` branch.

The sync cycle:
1. Fetches from remote
2. Merges (fast-forward, clean merge, or conflict resolution)
3. Pushes resolved state
4. Updates node sync state
5. Rebuilds search index

### Output

```text
sync: bidirectional | commits: 1 | conflicts resolved: 0
```

## Conflict Resolution

When both devices edit the same doogat between syncs, Doogat DB resolves conflicts automatically:

- **Frontmatter**: Each field is merged independently. Different fields → both kept. Same field changed on both sides → CRDT picks one deterministically.
- **Body**: Character-level merge. Non-overlapping edits → both applied. Overlapping edits → CRDT merges at character granularity.
- **Reference section**: Set union. Both sides add fields → both kept. Same field changed on both sides → local version wins.

No manual conflict resolution is ever needed.

### Example Scenario

Device A changes the title; Device B adds a reference field:

```text
# Ancestor
---
title: Original
---
Body text.
---
- source:: Wikipedia

# Device A (changed title)
---
title: Renamed Note
---
Body text.
---
- source:: Wikipedia

# Device B (added field)
---
title: Original
---
Body text.
---
- source:: Wikipedia
- author:: Bob

# After sync (both changes merged)
---
title: Renamed Note
---
Body text.
---
- author:: Bob
- source:: Wikipedia
```

## Compaction

Over time, the Git repository accumulates objects. Run compaction periodically:

```bash
ddb compact
```

This:
1. Computes the shared head (the latest commit all devices have synced)
2. Removes temporary CRDT files
3. Runs `git gc` for pack consolidation

Use `--force` to bypass the size threshold, or `--dry-run` to preview without changes.

### Reading the compaction report

```text
files removed: 12 | crdt compacted: 3 | gc: ok
crdt temp: 1.2 MB → 0.3 MB (47 files → 12)
repo (.git): 8.4 MB → 6.1 MB
```

| Field | Meaning |
|-------|---------|
| files removed | CRDT temp files older than the shared head, safely deleted |
| crdt compacted | Doogats whose multiple CRDT docs were merged into one |
| gc | Whether `git gc` succeeded |
| crdt temp | Total size and file count of `.crdt/temp/` before and after cleanup |
| repo (.git) | Git directory size before and after `git gc` |

If **crdt temp** shows no reduction, all devices are already caught up — nothing to clean.
If **repo (.git)** shows little change, Git's pack files are already efficient.

For measured growth data, see [Storage Budget](../technical/storage-budget.md).

## Checking Status

```bash
ddb status
```

Shows the current HEAD, node registration, index staleness, and number of registered nodes.
