# Doogat DB

A hybrid Git-CRDT decentralized Doogat database written in Rust.

Doogat DB stores Markdown notes in a Git repository, syncs across personal devices without cloud infrastructure, and automatically resolves conflicting edits using Automerge CRDT.

## Key Properties

- **Git-native** — full version history, human-readable Markdown files, durable storage
- **Decentralized** — no server required; sync via any Git remote (bare repos, SSH, local paths)
- **Offline-first** — all functionality works without network; sync is explicit
- **Automatic conflict resolution** — Git handles non-overlapping edits (>99% of cases); Automerge CRDT resolves the rest at character/field/line level

## Deployment Modes

Doogat DB is cross-platform in the sense that matters: same storage model, same typed data model, same sync semantics, same application contract. The process topology varies by platform.

| Mode | Target | Transport | Runtime |
|------|--------|-----------|---------|
| **Server** | Web apps, remote desktops, admin tools | GraphQL, REST, pgwire | `ddb serve` |
| **Embedded native** | Native apps that own the repo locally | UniFFI (Swift/Kotlin) | Rust core in-process |
| **Mobile host-shell** | Multiple mini-app experiences on one device | In-process calls to embedded core | One host app with feature modules |

On mobile, the recommended shape is one installed app containing the embedded Doogat DB core, one shared repository, and multiple feature modules that feel like mini-apps. Separately installed mobile apps sharing one phone-local backend server are not supported — mobile OS sandboxing and background execution limits make that topology non-portable.

See the [Building Apps guide](guide/building-apps.md) for details on each mode.

## Current Status

MVP implementation. The core library (`ddb-core`) and CLI (`ddb`) are functional with 10 modules, 12 CLI commands (+ 2 subcommands), and full two-node sync with conflict resolution. Includes a SQL engine for typed doogat tables, implicit type inference, and bundled type definitions.

## Documentation Structure

This book is split into three parts:

1. **System Architecture** — high-level design, module relationships, data flow, and key design decisions
2. **Technical Design** — detailed documentation of each module's internals, data structures, and algorithms
3. **User Guide** — how to install, configure, and use Doogat DB day-to-day
