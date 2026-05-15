# Doogat DB

Hybrid Git-CRDT decentralized database. Git is source of truth; SQLite index is derived/rebuildable.

## Stack

- Rust 2021 edition, workspace with three crates
- Git storage via `git2`, CRDT via `automerge`
- SQLite index via `rusqlite` (FTS5), SQL parsing via `sqlparser`
- CLI via `clap` (binary: `ddb`)
- GraphQL server via `axum` + `async-graphql` (dynamic schema)
- FFI via `uniffi` (proc-macro approach, generates Swift/Kotlin bindings)

## Structure

```text
ddb-core/src/       Library crate
  parser/            Three-zone Markdown parsing (frontmatter/body/references)
  search_query.rs   Search query parsing and normalization to canonical form
  git_ops/          Git repository CRUD, merge, remote sync
    merge.rs          Merge/conflict resolution
    remote.rs         Push/pull/fetch operations
    read.rs           File reads, diffs, revision queries
    rename.rs         Rename with backlink rewrite
  crdt_resolver.rs  Automerge CRDT conflict resolution
  indexer/          SQLite FTS5 index, type inference, materialization
    search.rs         FTS5 search, tag queries
    filter.rs         Search filter/negation SQL building
    rebuild.rs        Rebuild, reindex, staleness checks
  service/          Unified orchestration layer (DoogatService) for CLI/FFI/server
  sql_engine/       SQL DDL/DML translation (tables as doogat types)
  bundled_types.rs  Built-in type templates (project, contact)
  sync_manager/     Multi-device sync orchestration
  compaction/       CRDT temp cleanup and git gc
  consistency/      Detect/apply/migrate auto-fixes
    migrations.rs     Versioned data migrations
    zone_migrate.rs   Cross-zone field migration
  hlc.rs            Hybrid Logical Clock for causal ordering
  traits.rs         Core trait abstractions (DoogatSource, DoogatStore, GitBackend
                      supertrait + GitRemote, GitMerge, GitHistory, GitBinary,
                      GitRename, GitDesktopHooks, DoogatIndex, SqlBackend,
                      ConflictResolver)
  ffi.rs            UniFFI DoogatDriver facade for Swift/Kotlin bindings
  types/            Shared data structures (directory module)
    value.rs          Value enum, path utilities
    doogat.rs         Domain model types (DoogatId, ParsedDoogat, etc.)
    schema.rs         Schema/consistency types (TableSchema, ColumnDef, Fix)
  error.rs          Error types and Result alias
  ddb.udl           UniFFI interface definition (documentation reference)
ddb-core/benches/   Criterion benchmarks
  crud.rs           CRUD operations at 1K doogats
  search.rs         FTS5 search, SQL SELECT, reindex at 1K doogats
ddb-uniffi-bindgen/ UniFFI bindgen binary (isolated from ddb-core)
ddb-cli/src/        Binary crate
  main.rs           CLI struct definitions, dispatch, utilities
  commands/         Subcommand handlers (crud, query, sync, maintenance, discover)
ddb-server/src/     GraphQL server crate
  lib.rs            Server entrypoint (axum router, actor spawn)
  actor/            Thread-safe core bridge (mpsc + oneshot)
    handlers.rs       Command dispatch logic
  schema/           Dynamic GraphQL schema from _typedef doogats
    type_defs.rs      GraphQL type/input/enum definitions
    queries.rs        Query field resolvers
    mutations.rs      Mutation field resolvers
    subscriptions.rs  Subscription field resolvers
    discovery_queries.rs  Discovery query resolvers (orphans, sequences, etc.)
  auth.rs           Bearer token generation + middleware
  config.rs         Server config (~/.config/ddb/)
  error.rs          DoogatError → GraphQL error mapping
tests/e2e/          E2E tests (assert_cmd, exercises ddb binary)
tests/smoke.sh      CLI smoke test (init, CRUD, search, SQL, types, compact)
tests/integration.sh  Full integration tests (server, sync, CRDT, bundles)
tests/smoke.ps1     PowerShell port of tests/smoke.sh
tests/integration.ps1 PowerShell port of tests/integration.sh
tests/fixtures/     Test fixtures
dev/bin/             Developer scripts
  release              Version bump, tag, push
  build-xcframework    iOS/macOS XCFramework from UniFFI bindings
  build-android        Android .aar from UniFFI bindings
  safe-showboat-verify Sandboxed wrapper for `showboat verify` (PRD 00135)
  showboat-verify-no-contamination-test.sh
                       Regression test for verify-side contamination
docs/src/           mdbook documentation (architecture, technical, guide)
```

## Design Principles

Follow SOLID and Clean Architecture principles as adapted for Rust. These are mandatory for all code changes:

- `technical/solid.md` - SOLID principles translated to Rust idioms (traits over inheritance, small focused traits, dependency inversion via generics)
- `technical/clean-architecture.md` - Layer boundaries, dependency direction, I/O at the edges, no panics in library code

## Architecture Guardrails

- Keep domain/shared types adapter-neutral: no `rusqlite`, `git2`, `redb`, `axum`, or `async_graphql` in `ddb-core/src/types/**`; convert at adapter boundaries.
- Do not add concrete adapter construction inside `DoogatService` service modules; inject dependencies or document a temporary exception.
- User-facing read/write/list paths must not silently drop rows, schemas, or parse failures; return errors or structured warnings.
- No runtime panics in library/FFI code for mutex, database, filesystem, repository, or user-input state.
- 120% parity is the product posture: public product capabilities ship across every public application interface. If a capability cannot be applied coherently everywhere, prefer deferring/dropping it or making the interface non-public/specialized for that workflow over shipping drift.
- CRUD is the minimum `Guaranteed` baseline for every public application interface; exceptions require an explicit maintainer decision that the interface is specialized or not public for that workflow.
- Every feature PRD needs Transport Impact coverage for CLI, GraphQL, REST, PgWire, FFI, and NoSQL HTTP plus a cross-interface implementation/conformance plan.
- Do not call interfaces equivalent unless the same downstream workflow passes conformance tests on each. Undocumented client scaffolding means the interface contract is incomplete.
- For cross-interface architecture work, define the downstream workflow contract before refactoring internals that serve those interfaces.
- New cross-interface behavior belongs in the app contract first; transports should adapt commands/results, not own business policy.
- Public response/error/warning shape changes need compatibility/deprecation review and updated developer guidance.
- A `Guaranteed` capability needs a golden workflow and conformance plan before implementation is considered complete.
- Contract changes must cover support diagnostics, auth/setup expectations, timeout/performance expectations, and release/migration impact when relevant.

## Setup

```text
git config core.hooksPath dev/hooks
```

## Conventions

- All modules return `error::Result<T>`
- DoogatId: 14-digit timestamp string (YYYYMMDDHHmmss)
- Doogats stored at `ddb/{id}.md`, typedefs at `ddb/_typedef/{id}.md`
- Data dir: `.ddb/`, node file: `.git/ddb-node`, git signature: `ddb`
- Plan documents go in `dev/local/plans/` (gitignored), NOT in `docs/`
- Product Requirement Documents (PRDs) go in `dev/local/prds/` (gitignored), NOT in `docs/`
- Git worktrees go in `dev/local/worktrees/` (gitignored), nowhere else
- Releases: use `dev/bin/release` only. Never `gh release create` manually - CI creates the GitHub release with binary artifacts on tag push

## Definition of Done

A task is NOT complete unless ALL of these pass:

1. **Tests** — unit tests in the module AND integration/e2e tests in `tests/` (not just unit tests; use `cargo test --workspace` for the full cargo suite)
2. **Smoke/integration test** — if the change adds a CLI command or user-facing behavior, add a scenario to `tests/smoke.sh` and `tests/smoke.ps1`. If it adds a server endpoint, sync behavior, or CRDT logic, add it to `tests/integration.sh` and `tests/integration.ps1`. All four files follow the numbered-section + `pass` helper pattern. Upstream jink-feedback repros for the SQL regression suite live at `dev/local/specs/jink-feedback/ddb-repros/` (gitignored); run `bash dev/local/specs/jink-feedback/ddb-repros/run-all.sh` for an independent verification channel
3. **Docs** — update relevant files in `docs/src/` to reflect any behavioral or API changes
4. **Build** — `cargo clippy --workspace`, fast-tier `cargo test`, and full-suite `cargo test --workspace` all pass
5. **Walkthrough** — if the task adds a CLI command, server endpoint, or user-facing behavior, create an executable showboat walkthrough in `dev/local/walkthroughs/` (see Showboat Walkthroughs below)
6. **Architecture doc** — if the task changes module boundaries, data flow, or key types, update `docs/src/technical/walkthrough.md`

## Showboat Walkthroughs

Executable feature demos built with [showboat](https://github.com/simonw/showboat), a CLI tool by Simon Willison. Each walkthrough is a Markdown file containing commentary and code blocks with real captured output. `showboat verify` re-runs all code blocks and confirms outputs still match.

### Location and naming

`dev/local/walkthroughs/{5-digit}-{slug}.md` — e.g. `00001-crud-basics.md`

### Installation

Run via uvx (no install needed):

```text
uvx showboat --help
```

Or install persistently: `uv tool install showboat` / `pip install showboat`

### Critical rule: showboat CLI only

Agents **must not** edit walkthrough files directly (no `Edit`, `Write`, `sed`, etc.). All content flows through showboat CLI:

| Command | Purpose |
|---------|---------|
| `showboat init <file> <title>` | Create new walkthrough |
| `showboat note <file> <text>` | Add commentary (also accepts stdin) |
| `showboat exec <file> <lang> <code>` | Run code, capture real output |
| `showboat pop <file>` | Remove last entry (undo failed exec) |
| `showboat image <file> <path>` | Embed an image |
| `showboat verify <file>` | Re-run all blocks, diff against recorded output (never run directly from project root — creates real commits on master; see "Verifying walkthroughs safely" below) |
| `showboat extract <file>` | Emit commands to recreate file (for rebuilding) |

Output blocks contain real captured output. Direct file editing defeats the purpose — walkthroughs are proof of work.

### Execution model

Each `showboat exec` runs in its own shell. Variables, background jobs, and working directory do **not** persist between calls. Use `--workdir <dir>` to set the working directory for a command.

`exec` prints captured output to stdout and exits with the same code as the executed command, so agents can react to errors. Use `pop` to remove a failed entry before retrying.

### Patterns

**CLI walkthrough** — use `--workdir` with a fixed temp path (not `mktemp`, since the path must be reused across exec calls):

```text
WD=/tmp/ddb-demo-feature
showboat init dev/local/walkthroughs/00001-feature.md "Feature Name"
showboat note dev/local/walkthroughs/00001-feature.md "Initialize a repo."
showboat exec --workdir $WD dev/local/walkthroughs/00001-feature.md bash "mkdir -p $WD && ddb init"
showboat exec --workdir $WD dev/local/walkthroughs/00001-feature.md bash "ddb create --title 'Test'"
showboat exec dev/local/walkthroughs/00001-feature.md bash "rm -rf $WD"
```

**Server walkthrough** — use PID file pattern since background jobs don't persist across exec calls:

```text
showboat exec --workdir $WD ... bash "ddb serve --port 19201 --pg-port 19202 & echo \$! > /tmp/ddb-serve.pid"
showboat exec ... bash "sleep 1 && curl -s http://127.0.0.1:19201/graphql -H 'Content-Type: application/json' -d '{...}'"
showboat exec ... bash "kill \$(cat /tmp/ddb-serve.pid) && rm /tmp/ddb-serve.pid"
```

### Maintenance

Walkthroughs are local working documents (`dev/local/` is gitignored). They can be regenerated anytime. When CLI output changes cause `showboat verify` to fail, regenerate using `showboat extract` to get the original commands, then re-execute.

### Verifying walkthroughs safely

Never run `showboat verify <walkthrough>` directly from the project root. Use the wrapper:

```text
dev/bin/safe-showboat-verify <walkthrough> [<walkthrough>...]
```

`showboat verify` re-executes the walkthrough's bash blocks in **the caller's cwd**. The original `--workdir` from `showboat exec` is not recorded in the rendered Markdown, so verify has no way to honor it. If the cwd is itself a git repo, blocks that auto-commit (e.g. `ddb create`, `ddb query "INSERT INTO ..."`) write real commits to that repo. PRD 00135 documents two `git reset --hard` cleanups caused by this.

The wrapper runs verify inside a throwaway `git worktree` under `dev/local/worktrees/showboat-verify-<id>/` and removes it on exit, so any contamination lands in the worktree, never on the active checkout.

To confirm the wrapper still works after upgrading showboat or editing the wrapper, run the regression test:

```text
dev/bin/showboat-verify-no-contamination-test.sh
```

It runs the wrapper against a known walkthrough (a 00050+ fixture exhibiting the contamination pattern) and asserts HEAD, working-tree status, project-root data dirs (`ddb/`, `.ddb/`, `.crdt/`, `.nodes/`), and the worktree list are unchanged afterward. If no contaminating fixture is available, the test fails fast rather than passing vacuously on a self-isolating walkthrough.

## Gotchas

- Never run `ddb` commands (init, create, SQL, etc.) in the project root. Use `/tmp` or a temp directory. Running in-repo creates `ddb/`, `.ddb/`, `.crdt/`, `.nodes/` artifacts that pollute the source tree and git history
- E2E tests require the `ddb` binary — run `cargo build -p ddb-cli` before `cargo test -p ddb-e2e`
- `head_oid()` returns `CommitHash` (a String newtype, access inner via `.0`), not `git2::Oid`
- `merge_frontmatter` is called from both `resolve_conflicts` and `resolve_append_log`

## Commands

```text
cargo build                           Build default workspace members (fast local loop)
cargo build --workspace               Build all crates
cargo test                            Run fast local test tier
cargo test-ci                         Run bounded CI matrix tier (unit/bin targets only)
cargo test-full                       Run full cargo test suite (workspace + e2e)
cargo test -p ddb-e2e                 Run e2e tests only
cargo bench                           Run criterion benchmarks (CRUD + search)
cargo bench --no-run                  Compile benchmarks without running
cargo build -p ddb-core --features profiling   Build with tracing instrumentation
./tests/smoke.sh                      CLI smoke test
./tests/integration.sh                Full integration tests (runs smoke first)
cargo test -p ddb-core --test property_tests   Property-based integration tests
cargo test -p ddb-core --test sync_test        Core sync integration tests
cargo clippy --workspace              Lint
cargo doc --no-deps --document-private-items   Generate rustdoc
cd docs && mdbook build               Build documentation
cd docs && mdbook serve               Serve documentation locally

# Coverage (requires: rustup component add llvm-tools-preview, cargo install cargo-llvm-cov)
cargo llvm-cov --workspace              Run coverage report (baseline: 70%, target: 80%)
cargo llvm-cov --workspace --html       Generate HTML coverage report in target/llvm-cov/html/
cargo llvm-cov --workspace --exclude ddb-e2e --fail-under-lines 70   Fail if below CI threshold

# Performance thresholds (require --release, local only — not run in CI)
cargo test --release -p ddb-core --test query_thresholds nfr01_
cargo test --release -p ddb-core --test growth_thresholds nfr02_
cargo test --release -p ddb-core --test sync_thresholds nfr03_

# Property tests (thorough run, ~20 min at 5000 cases)
PROPTEST_CASES=5000 cargo test -p ddb-core --test property_tests

# UniFFI binding generation
cargo run -p ddb-uniffi-bindgen --bin uniffi-bindgen -- generate \
  --library target/debug/libddb_core.dylib \
  --language swift --out-dir out/swift
cargo run -p ddb-uniffi-bindgen --bin uniffi-bindgen -- generate \
  --library target/debug/libddb_core.dylib \
  --language kotlin --out-dir out/kotlin
```

## Documentation

Read from `docs/src/` before working on related modules:

- `architecture/overview.md` - System design and data flow
- `architecture/modules.md` - Module responsibilities and boundaries
- `architecture/design-decisions.md` - Key architectural choices and non-goals. **Check before proposing new features or PRDs** - if a proposal conflicts with a decision or non-goal, the decision must be revisited first
- `technical/data-model.md` - Doogat format, frontmatter schema
- `technical/parser.md` - Three-zone Markdown parsing details
- `technical/git-ops.md` - Git storage layer
- `technical/crdt-resolver.md` - Conflict resolution strategy
- `technical/indexer.md` - SQLite index, FTS5, type inference
- `technical/sql-engine.md` - SQL translation layer
- `technical/sync.md` - Multi-device sync protocol
- `technical/server.md` - GraphQL server architecture and API
- `technical/ffi.md` - UniFFI bindings (DoogatDriver facade)
- `technical/errors.md` - Error handling patterns
- `technical/solid.md` - SOLID principles in Rust
- `technical/clean-architecture.md` - Clean Architecture in Rust
