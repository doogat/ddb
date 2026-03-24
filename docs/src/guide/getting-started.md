# Getting Started

## Prerequisites

- Rust toolchain (rustup, cargo)
- Git (for sync features)

## Installation

Clone the repository and build:

```bash
git clone https://github.com/doogat/zetteldb.git
cd zetteldb
cargo build --release
```

The binary is at `target/release/zdb`. Add it to your `PATH` or symlink it.

## Initialize a Zettelkasten

```bash
zdb init ~/my-zettelkasten
```

This creates:

```text
my-zettelkasten/
├── .git/                   # Git repository
├── zettelkasten/           # Your zettels go here
├── reference/              # Binary/asset files
├── .nodes/                 # Device registry
├── .crdt/temp/             # Temporary merge files
├── .gitignore              # Excludes .zdb/
└── (initial commit)
```

## Create Your First Zettel

```bash
cd ~/my-zettelkasten
zdb create --title "My first note" --tags "personal,learning"
```

Output: a 14-digit timestamp ID (e.g., `20260226153042`).

The zettel is saved as `zettelkasten/20260226153042.md` and committed to Git.

## Read It Back

```bash
zdb read 20260226153042
```

Output:

```markdown
---
id: 20260226153042
title: My first note
date: 2026-02-26
tags:
  - personal
  - learning
---
```

## Check Status

```bash
zdb status
```

Output:

```text
head: abc123def456...
node: not registered
index stale: true
registered nodes: 0
```

## Build the Search Index

```bash
zdb reindex
```

This parses all zettels and populates the SQLite FTS5 index at `.zdb/index.db`.

## Type Definitions

Install a bundled type definition:

```bash
zdb type install project
```

Or infer a typedef from existing data:

```bash
zdb type suggest mytype
```

See [Type Definitions](./types.md) for details.

## In-Depth Guides

`zdb` includes built-in guides for common workflows:

```bash
zdb help              # list available guides
zdb help create-app   # data modeling, zones, title resolution, API access
```

The `create-app` guide covers `CREATE TABLE` usage, zone inference, ENUM/SET constraints, title templates, junction tables, and API access patterns. See [Building Apps](./building-apps.md) for the full documentation.

## Set Up for Multi-Device Sync

See [Multi-Device Sync](./sync.md) for configuring remotes and registering nodes.

## Updating

`zdb` auto-updates in the background. Every hour (at most), a detached process checks for new releases and, if one exists, downloads, verifies (SHA-256), and replaces the binary. Before replacing, the current binary is backed up to `~/.config/zetteldb/zdb.previous`. On your next command you'll see:

```text
zdb updated v0.1.1 -> v0.2.0. restart your shell to use the new version.
```

To update immediately:

```bash
zdb update-bin
```

### Rollback

If an update causes problems, restore the previous binary:

```bash
zdb update-bin --rollback
```

Only the most recent pre-update binary is kept.

### Disabling auto-update

To prevent background auto-updates, add to `~/.config/zetteldb/config.toml`:

```toml
[update]
auto = false
```

When disabled, `zdb` still checks for new versions but won't apply them. Manual updates via `zdb update-bin` always work regardless of this setting.

## Global Options

| Flag | Default | Description |
|------|---------|-------------|
| `--repo PATH` | `.` (current directory) | Path to the zettelkasten repository |
