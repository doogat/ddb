# Getting Started

## Prerequisites

- Rust toolchain (rustup, cargo)
- Git (for sync features)

## Installation

Clone the repository and build:

```bash
git clone https://github.com/doogat/ddb.git
cd ddb
cargo build --release
```

The binary is at `target/release/ddb`. Add it to your `PATH` or symlink it.

## Initialize a Repository

```bash
ddb init ~/my-ddb
```

This creates:

```text
my-ddb/
├── .git/                   # Git repository
├── ddb/           # Your doogats go here
├── reference/              # Binary/asset files
├── .nodes/                 # Device registry
├── .crdt/temp/             # Temporary merge files
├── .gitignore              # Excludes .ddb/
└── (initial commit)
```

## Create Your First Doogat

```bash
cd ~/my-ddb
ddb create --title "My first note" --tags "personal,learning"
```

Output: a 14-digit timestamp ID (e.g., `20260226153042`).

The doogat is saved as `ddb/20260226153042.md` and committed to Git.

## Read It Back

```bash
ddb read 20260226153042
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
ddb status
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
ddb reindex
```

This parses all doogats and populates the SQLite FTS5 index at `.ddb/index.db`.

## Type Definitions

Install a bundled type definition:

```bash
ddb type install project
```

Or infer a typedef from existing data:

```bash
ddb type suggest mytype
```

See [Type Definitions](./types.md) for details.

## In-Depth Guides

`ddb` includes built-in guides for common workflows:

```bash
ddb help              # list available guides
ddb help create-app   # data modeling, zones, title resolution, API access
```

The `create-app` guide covers `CREATE TABLE` usage, zone inference, ENUM/SET constraints, title templates, junction tables, and API access patterns. See [Building Apps](./building-apps.md) for the full documentation.

## Set Up for Multi-Device Sync

See [Multi-Device Sync](./sync.md) for configuring remotes and registering nodes.

## Updating

`ddb` auto-updates in the background. Every hour (at most), a detached process checks for new releases and, if one exists, downloads, verifies (SHA-256), and replaces the binary. Before replacing, the current binary is backed up to `~/.config/ddb/ddb.previous`. On your next command you'll see:

```text
ddb updated v0.1.1 -> v0.2.0. restart your shell to use the new version.
```

To update immediately:

```bash
ddb update-bin
```

### Rollback

If an update causes problems, restore the previous binary:

```bash
ddb update-bin --rollback
```

Only the most recent pre-update binary is kept.

### Disabling auto-update

To prevent background auto-updates, add to `~/.config/ddb/config.toml`:

```toml
[update]
auto = false
```

When disabled, `ddb` still checks for new versions but won't apply them. Manual updates via `ddb update-bin` always work regardless of this setting.

## Global Options

| Flag | Default | Description |
|------|---------|-------------|
| `--repo PATH` | `.` (current directory) | Path to the ddb repository |
