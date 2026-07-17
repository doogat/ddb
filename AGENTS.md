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
ddb-core/src/        Library crate: parser/ (three-zone Markdown), git_ops/ (repo CRUD, merge,
                     remote, rename), indexer/ (SQLite FTS5, ports), service/ (DoogatService,
                     per-verb submodules), sql_engine/, sync_manager/, compaction/, consistency/,
                     crdt_resolver.rs, hlc.rs, traits.rs (core trait abstractions),
                     ffi/ (UniFFI DoogatDriver facade), types/ (adapter-neutral shared types)
ddb-core/benches/    Criterion benchmarks (CRUD + search at 1K doogats)
ddb-uniffi-bindgen/  UniFFI bindgen binary (isolated from ddb-core)
ddb-cli/src/         Binary crate: main.rs dispatch + commands/ subcommand handlers
ddb-server/src/      GraphQL server: lib.rs entrypoint, actor/ core bridge, schema/ dynamic
                     GraphQL (queries, mutations/, subscriptions), auth.rs, config.rs, error.rs
tests/               e2e/ (assert_cmd), smoke.sh + integration.sh (+ .ps1 ports), fixtures/
dev/bin/             Developer scripts (release, build-xcframework, build-android,
                     safe-showboat-verify + its regression test)
docs/src/            mdbook documentation (architecture, technical, guide)
```

Per-file responsibilities: `docs/src/architecture/modules.md` (Module Summary table).

## Design Principles

Follow SOLID and Clean Architecture principles as adapted for Rust. These are mandatory for all code changes:

- `technical/solid.md` - SOLID principles translated to Rust idioms (traits over inheritance, small focused traits, dependency inversion via generics)
- `technical/clean-architecture.md` - Layer boundaries, dependency direction, I/O at the edges, no panics in library code

## Architecture Guardrails

- Keep domain/shared types adapter-neutral: no `rusqlite`, `git2`, `redb`, `axum`, or `async_graphql` in `ddb-core/src/types/**`; convert at adapter boundaries.
- Do not add concrete adapter construction inside `DoogatService` service modules; inject dependencies or document a temporary exception.
- User-facing read/write/list paths must not silently drop rows, schemas, or parse failures; return errors or structured warnings.
- No runtime panics in library/FFI code for mutex, database, filesystem, repository, or user-input state.
- **Product positioning (do not re-litigate): ddb is a standalone platform for many future downstream apps; jink is the first, not the product. The six transports and 120% parity are deliberate bets to keep ddb flexible and complete so downstreams don't reinvent. See `docs/src/architecture/design-decisions.md` → "Product Positioning". Treating "one consumer, one transport" as over-engineering is a misread — assess within this positioning, not against it.**
- 120% parity is the product posture: public product capabilities ship across every public application interface. If a capability cannot be applied coherently everywhere, prefer deferring/dropping it or making the interface non-public/specialized for that workflow over shipping drift.
- CRUD is the minimum `Guaranteed` baseline for every public application interface; exceptions require an explicit maintainer decision that the interface is specialized or not public for that workflow.
- Every feature PRD needs Transport Impact coverage for CLI, GraphQL, REST, PgWire, FFI, and NoSQL HTTP plus a cross-interface implementation/conformance plan.
- Do not call interfaces equivalent unless the same downstream workflow passes conformance tests on each. Undocumented client scaffolding means the interface contract is incomplete.
- For cross-interface architecture work, define the downstream workflow contract before refactoring internals that serve those interfaces.
- New cross-interface behavior belongs in the app contract first; transports should adapt commands/results, not own business policy.
- Public response/error/warning shape changes need compatibility/deprecation review and updated developer guidance.
- A `Guaranteed` capability needs a golden workflow and conformance plan before implementation is considered complete.
- Contract changes must cover support diagnostics, auth/setup expectations, timeout/performance expectations, and release/migration impact when relevant.

### Data-safety invariants (P0 — violating these risks SILENT data loss)

Full rationale, evidence, and current-gap tracking in `docs/src/technical/invariants.md`. These are non-negotiable; a change that breaks one is wrong even if tests pass.

- **Every git write holds the repo write lock — merges, sync, and bundle import included.** All commit/delete/rename/merge/checkout-moving paths must run inside the repo-scoped advisory write lock and build trees from fresh state. Never use the cached `repo.index()` for a write; never add a new write path that skips the lock. (Lock shipped for the 8 CRUD paths: 00162, done. Delete-path `fresh_index()` shipped 2026-07-16 as 00163 T1; the unlocked merge/sync path (`git_ops/merge.rs`) still tracked: 00163.)
- **Merges preserve BOTH sides' changes — deletions included.** The conflicted-merge tree must be a true three-way merge classified against the merge base. A two-way ours→theirs diff cannot express this: today it resurrects theirs' deletions AND silently reverts ours' non-conflicting edits, and the naive fix (staging `Delta::Deleted` gated by the conflict set) deletes ours-created files — proven empirically 2026-07-16 (`dev/local/designs/00200-make-merge-tree-three-way-v1-evidence.md`). Never patch individual delta arms; classify against the merge base. (Tracked: 00200 make-merge-tree-three-way, runs immediately after 00163.)
- **IDs are minted through one repo-aware path.** No mint path may use `exists = |_| false` or allocate future-dated timestamps. All minting checks actual repo/index existence. The ~1/sec/process ceiling is a known, tracked limitation (00170) — do not "fix" it ad hoc. (Tracked: 00164 unify-id-minting, 00170 id-throughput-ceiling-decision.)
- **Conflict resolution is a pure function of its inputs.** Set stable Automerge actor IDs (derived from blob OIDs / node UUIDs); never rely on default random actor IDs. If a decision claims HLC ordering, the feeding commits must carry an HLC trailer. (Tracked: 00165 deterministic-crdt-resolution, 00166 make-hlc-load-bearing.)
- **One bad file never fails the batch.** Read/list/index paths degrade per-item into structured warnings (as the full rebuild already does); only an explicit `--strict` mode hard-fails. (Tracked: 00169 poison-file-reindex-resilience.)

### Coherence invariants

- **One error policy.** Exactly one table maps a code → (category, HTTP status, redaction, FFI variant). Adding an error code means editing that ONE table; transports derive from it and never re-decide redaction or status. No per-transport error switch. (Tracked: 00173 unify-server-error-mapping-paths; FFI/PgWire legs 00177/00180.)
- **App-contract-first, FFI included.** Every verb flows through `AppCommand → DoogatService → AppOutput`. FFI is a transport, not exempt — it uses the contract, not raw service methods. Do not add per-verb actor command/reply enum plumbing; it is being replaced by a closure message (00172). (Tracked: 00172 collapse-actor-plumbing; route verbs + structured fields 00175-00178.)
- **A result type has one core definition.** Transports serialize the core type; they must not redefine field subsets. Any new public result field lands in the core type first. (Tracked: 00176 route-search-through-contract.)
- **Every interpolated SQL identifier is escaped; every value is a bound parameter.** Never `format!` a user-influenced string (type name, column, id) into SQL text without an identifier-quoting helper (`escape_sql_ident` in `indexer/filter.rs` / `quote_ident` in `schema_diff/plan.rs`). Validate type/field-key charset at the write boundary. (Tracked: 00179 sql-identifier-escaping.)

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
- Never close GitHub issues (`gh issue close`) — the maintainer closes them; report open issues as observations instead
- Push `master` at the end of each PRD (after the PRD's commits land). This overrides the global autopilot "don't push" default and applies to this repository only — nightly CI (`full-validation.yml`) needs the new commits on `origin/master` to exercise the heavy Tier 2 battery against the latest work.

## Definition of Done

Validation is two-tiered: a fast per-task gate runs locally; the heavy battery is delegated to CI. Test/scenario authoring stays a per-PRD deliverable — only local *execution* of the heavy battery is dropped.

### Tier 1 — per-task local gate (runs after every task)

A task is NOT complete unless ALL of these pass locally:

1. **Build** — `cargo build` succeeds.
2. **Lint** — `cargo clippy --workspace --all-targets` is clean. (`--all-targets` is required: CI lints test targets too; without it, test-code lints slip through Tier 1 and break Nightly — happened 2026-07.)
3. **Fast tests** — `cargo test-ci` passes.
4. **TDD unit tests** — unit tests for the change live alongside the module being touched and pass under `cargo test-ci`.

Do NOT run `cargo test --workspace`, the e2e suite, `tests/integration.sh`, the `.ps1` ports, property tests, cross-platform jobs, coverage, or `showboat verify` per task. Those belong to Tier 2.

One narrow exception: `CLAUDE.md` instructs Claude-routed sessions to run `cargo test -p ddb-e2e` after any task that deletes or replaces existing code paths (skipped for purely additive tasks). That conditional, scope-limited e2e run is intentional — it is a deletion safety net, not a relaxation of Tier 1. See `CLAUDE.md` for the exact wording.

### Tier 2 — delegated to CI (runs automatically, not per task)

The heavy battery is owned by the GitHub Actions workflows:

- `test.yml` runs on every push: full workspace tests, cross-platform jobs, and the matrix that catches regressions the fast tier can miss.
- `full-validation.yml` runs nightly: full `cargo test --workspace`, the e2e suite, `tests/integration.sh`, property tests, cross-platform validation, and coverage.

Do not duplicate these locally. If a CI failure is reproducible only locally, run the specific failing command in isolation rather than the full battery.

### Per-PRD deliverables (created once per PRD, not per task)

These are authoring obligations, not local execution gates:

- **Smoke/integration scenario authoring** — if the PRD adds a CLI command or user-facing behavior, add a scenario to `tests/smoke.sh` and `tests/smoke.ps1`. If it adds a server endpoint, sync behavior, or CRDT logic, add it to `tests/integration.sh` and `tests/integration.ps1`. All four files follow the numbered-section + `pass` helper pattern. Execution of these scripts is delegated to CI (Tier 2). Upstream jink-feedback repros for the SQL regression suite live at `dev/local/specs/jink-feedback/ddb-repros/` (gitignored and machine-local — absent on fresh checkouts; skip this channel when the directory is missing); if present, run `bash dev/local/specs/jink-feedback/ddb-repros/run-all.sh` for an independent verification channel when investigating.
- **E2E test authoring** — new user-facing behavior still requires its e2e test written under `tests/e2e/`. The test is authored as part of the PRD; execution happens in CI.
- **Docs** — update relevant files in `docs/src/` to reflect any behavioral or API changes.
- **Walkthrough** — if the PRD adds a CLI command, server endpoint, or user-facing behavior, create an executable showboat walkthrough in `dev/local/walkthroughs/` (see Showboat Walkthroughs below).
- **Architecture doc** — if the PRD changes module boundaries, data flow, or key types, update `docs/src/technical/walkthrough.md`.
- **Push `master`** — after the PRD's commits land, push `master` so nightly Tier 2 CI exercises the new work. This overrides the global autopilot "don't push" default for this repo only (see Conventions above for the full rule).

## Showboat Walkthroughs

Executable feature demos (proof of work) at `dev/local/walkthroughs/{5-digit}-{slug}.md`, built with [showboat](https://github.com/simonw/showboat). Full guide (CLI reference, execution model, CLI/server patterns, regeneration, wrapper regression test): `agent_docs/showboat-walkthroughs.md`. Two safety rules always apply:

- Never edit walkthrough files directly (no `Edit`, `Write`, `sed`) — all content flows through the showboat CLI (`init`/`note`/`exec`/`pop`/`extract`).
- Never run `showboat verify` from the project root — it re-executes blocks in the caller's cwd and writes real commits to master. Use `dev/bin/safe-showboat-verify <walkthrough>`.

## Gotchas

- Never run `ddb` commands (init, create, SQL, etc.) in the project root. Use `/tmp` or a temp directory. Running in-repo creates `ddb/`, `.ddb/`, `.crdt/`, `.nodes/` artifacts that pollute the source tree and git history
- E2E tests require the `ddb` binary — run `cargo build -p ddb-cli` before `cargo test -p ddb-e2e`
- `head_oid()` returns `CommitHash` (a String newtype, access inner via `.0`), not `git2::Oid`
- Run cargo commands in the foreground only — backgrounded cargo builds/tests contend on the build lock, queue up, and can deadlock the whole session
- `merge_frontmatter` is called from both `resolve_conflicts` and `resolve_append_log`

## Commands

```text
cargo build                           Build default workspace members (fast local loop)
cargo build --workspace               Build all crates
cargo test                            Run cargo's default workspace test selection
cargo test-ci                         Run the fast local test tier (bounded CI matrix; unit/bin targets only)
cargo test-full                       Run full cargo test suite (Tier 2; delegate to CI, not per-task)
cargo test -p ddb-e2e                 Run e2e tests only
cargo bench                           Run criterion benchmarks (CRUD + search)
cargo bench --no-run                  Compile benchmarks without running
cargo build -p ddb-core --features profiling   Build with tracing instrumentation
./tests/smoke.sh                      CLI smoke test
./tests/integration.sh                Full integration tests (runs smoke first)
cargo test -p ddb-core --test property_tests   Property-based integration tests
cargo test -p ddb-core --test sync_test        Core sync integration tests
cargo clippy --workspace --all-targets   Lint (incl. test targets; matches CI)
cargo doc --no-deps --document-private-items   Generate rustdoc
cd docs && mdbook build               Build documentation
cd docs && mdbook serve               Serve documentation locally
cargo llvm-cov --workspace            Coverage report (CI gate: 70% lines; needs llvm-tools-preview + cargo-llvm-cov)
```

Performance-threshold tests (nfr01-03): `docs/src/technical/performance.md`. UniFFI binding generation: `docs/src/technical/ffi.md`.

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
