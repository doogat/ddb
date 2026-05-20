# Architecture Overview

Doogat DB is a modular monolith with 12 core library modules, a GraphQL server crate, a CLI binary, and UniFFI bindings for Swift/Kotlin. Each module has a single responsibility and clear dependency boundaries.

## System Layers

```text
┌─────────────────────────────────────────────────────────┐
│                     CLI (ddb-cli)                        │
│                 clap-based command interface             │
├─────────────────────────────────────────────────────────┤
│               GraphQL Server (ddb-server)                │
│     axum + async-graphql · actor bridge · Bearer auth    │
├─────────────────────────────────────────────────────────┤
│            FFI Bindings (DoogatDriver facade)            │
│         uniffi proc-macro · Swift · Kotlin              │
├─────────────────────────────────────────────────────────┤
│                Core Library (ddb-core)                   │
│  ┌────────────────────────────────────────────────────┐ │
│  │  Orchestration: sync_manager, compaction           │ │
│  ├────────────────────────────────────────────────────┤ │
│  │  SQL: sql_engine, indexer (type inference + mat.)  │ │
│  ├────────────────────────────────────────────────────┤ │
│  │  Merge: crdt_resolver, git_ops                     │ │
│  ├────────────────────────────────────────────────────┤ │
│  │  Index: indexer (SQLite + FTS5)                    │ │
│  ├────────────────────────────────────────────────────┤ │
│  │  Storage: git_ops (libgit2)                        │ │
│  ├────────────────────────────────────────────────────┤ │
│  │  Parser: parser (three-zone Markdown)              │ │
│  ├────────────────────────────────────────────────────┤ │
│  │  Foundation: types, error, traits, hlc             │ │
│  └────────────────────────────────────────────────────┘ │
├─────────────────────────────────────────────────────────┤
│               External Dependencies                     │
│  git2 · automerge · rusqlite · serde_yaml · similar ·   │
│  uniffi                                                 │
└─────────────────────────────────────────────────────────┘
```

## Stability Tiers

Features are classified as **stable** or **experimental**:

| Tier | Scope |
|------|-------|
| Stable | CLI CRUD, search, query, sync, type management; Git storage format; FTS5; SQL DDL/DML; `ddb-core` public API; GraphQL server (incl. subscriptions over graphql-ws), REST, PgWire, ReadPool, background maintenance |
| Experimental | NoSQL API, UniFFI bindings, bundles, attachments, auto-update |

Stable APIs follow semver. Experimental APIs may change in any release.

There is no standalone "WebSocket interface". The WebSocket transport implemented in `ws.rs` is exclusively the graphql-ws upgrade path for GraphQL subscriptions — it is not a separate public interface with its own contract.

Stability tier (semver) is separate from interface capability promises. GraphQL, REST, PgWire, and NoSQL HTTP are not equivalent: they guarantee different operations and workflows. Stable interfaces can still have narrower promises than GraphQL, and experimental interfaces can document intentional boundaries. See [Choosing an interface](../guide/building-apps.md#choosing-an-interface) for the promise matrix.

## Hybrid Git-CRDT Strategy

Git handles >99% of merges (non-overlapping edits). When Git detects a conflict, Doogat DB falls back to Automerge CRDT with per-zone merge strategies:

| Zone | Merge Strategy |
|------|---------------|
| Frontmatter (YAML) | Field-level Automerge Map CRDT |
| Body (Markdown) | Character-level Automerge Text CRDT |
| Reference section | Automerge List CRDT (sorted on export) |

## Storage Model

- **Source of truth**: Git repository (Markdown files)
- **Read cache**: SQLite database with FTS5 (derived, always rebuildable from Git)
- **Node registry**: TOML files in `.nodes/` (tracked by Git)
- **Local state**: `.git/ddb-node` (node UUID, not tracked)

## Deployment Modes

Doogat DB supports three deployment modes. All three run the same backend (storage, types, sync, queries). The public application interfaces — GraphQL, REST, PgWire, NoSQL HTTP, and FFI — expose different capability subsets and are not interchangeable. See [Choosing an interface](../guide/building-apps.md#choosing-an-interface) for the promise matrix.

### Mode 1: Server

```text
Web / Desktop app
      │
      ▼
ddb serve (HTTP :2891)
      │
      ├── GitRepo (storage)
      ├── Index (SQLite FTS5)
      └── SqlEngine (DDL/DML)
```

Target: web apps, remote desktop apps, shared local desktops, admin tools. Transports: GraphQL (primary, CRUD `Guaranteed`), REST (specialized CRUD), PgWire (SQL/reporting), NoSQL HTTP (read-only document access). Not equivalent — see [interface selection](../guide/building-apps.md#choosing-an-interface).

### Mode 2: Embedded native

```text
Native app (Swift / Kotlin)
      │
      ▼
DoogatDriver (UniFFI, in-process)
      │
      ├── GitRepo (storage)
      ├── Index (SQLite FTS5)
      └── SqlEngine (DDL/DML)
```

Target: native apps that own the repo locally. Transport: UniFFI function calls.

### Mode 3: Mobile host-shell

```text
Host App
├── DoogatDriver (one instance)
│   ├── GitRepo (shared repo)
│   ├── Index (shared index)
│   └── SqlEngine
├── Module: Bookmarks
│   └── schema + queries + UI
├── Module: Contacts
│   └── schema + queries + UI
└── Widget / Extension (read-only access)
```

Target: multiple mini-app experiences on one mobile device. All modules share one embedded DoogatDriver, one repository, and one index.

Doogat DB does not support multiple separately installed mobile apps sharing one phone-local backend server. Mobile OS sandboxing, background execution limits, and IPC restrictions make this topology non-portable.

## Project Structure

```text
ddb/
├── Cargo.toml                  # Workspace root
├── ddb-core/                   # Core library
│   ├── src/
│   │   ├── lib.rs              # Public re-exports + UniFFI scaffolding
│   │   ├── error.rs            # Error types
│   │   ├── types/              # Shared data structures
│   │   │   ├── mod.rs          # Config types, re-exports
│   │   │   ├── value.rs        # Value enum, path utilities
│   │   │   ├── doogat.rs       # Domain model types
│   │   │   └── schema.rs       # Schema/consistency types
│   │   ├── traits.rs           # Core trait abstractions
│   │   ├── hlc.rs              # Hybrid Logical Clock
│   │   ├── parser/             # Markdown parsing/serialization
│   │   ├── git_ops/            # Git repository operations
│   │   ├── crdt_resolver.rs    # Automerge conflict resolution
│   │   ├── indexer/            # SQLite FTS5 index + type inference
│   │   ├── sql_engine/         # SQL DDL/DML translation
│   │   ├── search_query.rs     # Search query parsing/normalization
│   │   ├── bundled_types.rs    # Built-in type definitions
│   │   ├── service/            # Unified orchestration (DoogatService)
│   │   ├── consistency/        # Schema auto-fixes + versioned migrations
│   │   ├── sync_manager/       # Multi-device sync
│   │   ├── compaction/         # CRDT cleanup + git gc
│   │   ├── attachments.rs      # File attachment operations
│   │   ├── bundle.rs           # Air-gapped bundle export/import
│   │   ├── maintenance.rs      # Maintenance task orchestration
│   │   ├── nosql.rs            # NoSQL index (O(1) lookups, scans)
│   │   ├── ffi.rs              # UniFFI DoogatDriver facade
│   │   └── ddb.udl             # UniFFI interface definition (docs)
│   └── benches/
│       ├── crud.rs             # CRUD benchmarks (1K doogats)
│       ├── search.rs           # Search/reindex benchmarks
│       ├── growth.rs           # Growth simulation benchmarks
│       ├── sync.rs             # Sync benchmarks
│       ├── large_scale.rs      # Large-scale benchmarks (50K)
│       └── helpers.rs          # Shared benchmark utilities
├── ddb-cli/                    # CLI binary
│   └── src/
│       ├── main.rs             # CLI struct definitions, dispatch
│       ├── updater.rs          # Auto-update mechanism
│       └── commands/           # Subcommand handlers
│           ├── mod.rs
│           ├── crud.rs         # create, read, update, delete, list
│           ├── query.rs        # search, query, sql, type
│           ├── sync.rs         # sync, compact, register
│           ├── maintenance.rs  # reindex, fix, status, help
│           └── discover.rs     # orphans, stale, recent, links
├── ddb-server/                 # GraphQL server library
│   └── src/
│       ├── lib.rs              # Server entrypoint (axum)
│       ├── actor/              # Thread-safe core bridge
│       │   ├── mod.rs          # RepoActor, event bus
│       │   └── handlers.rs     # Command dispatch logic
│       ├── schema/             # Dynamic GraphQL schema
│       │   ├── mod.rs          # Schema builder
│       │   ├── base_types.rs   # Value converters, type builders
│       │   ├── queries.rs      # Query field resolvers
│       │   ├── mutations.rs    # Mutation field resolvers
│       │   ├── subscriptions.rs # Subscription field resolvers
│       │   ├── type_defs.rs    # Type/input/enum definitions
│       │   └── discovery_queries.rs # Discovery query resolvers
│       ├── auth.rs             # Bearer token auth
│       ├── config.rs           # Server config
│       ├── error.rs            # Error mapping
│       ├── read_pool.rs        # Concurrent read pool (spawn_blocking)
│       ├── rest.rs             # REST API endpoints
│       ├── pgwire.rs           # PostgreSQL wire protocol
│       ├── ws.rs               # WebSocket transport
│       ├── events.rs           # Event bus for subscriptions
│       ├── filter.rs           # GraphQL filter implementation
│       ├── reload.rs           # Hot schema reload
│       ├── maintenance.rs      # Server maintenance routes
│       └── nosql_api.rs        # NoSQL API endpoints
└── docs/                       # This documentation
```

## Runtime Directory Layout

Created by `ddb init`:

```text
my-ddb/
├── .git/
│   └── ddb-node                # Local node UUID (gitignored)
├── ddb/               # Doogat Markdown files
│   ├── 20260226120000.md
│   ├── _typedef/               # Type definition doogats
│   │   └── 20260226143000.md
│   └── ...
├── reference/                  # Binary/asset files
├── .nodes/                     # Node registry (git-tracked)
│   └── {uuid}.toml
├── .crdt/temp/                 # Temporary CRDT files
├── .ddb/                       # Local-only (gitignored)
│   └── index.db                # SQLite search index
└── .gitignore                  # Ignores .ddb/
```
