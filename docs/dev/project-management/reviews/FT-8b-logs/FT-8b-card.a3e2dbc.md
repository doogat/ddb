# FT-8b render kept-verbatim collision losers on CLI and GraphQL

## Goal

After FT-8a is reviewed and committed, make the verbatim fold visible on every
interface that renders `SyncReport`, prove it end to end, and document it.

1. `ddb sync` prints `  collision losers kept verbatim: N` only when the list
   is non-empty, then one `    {old_path} -> {new_path}` line per record.
   `ddb bundle import` prints the same lines through the same helper.
2. The GraphQL `sync` mutation result gains `collisionLosersKeptVerbatim:
   [CollisionLoserKeptVerbatim!]!` with `{ oldId, oldPath, newPath }`,
   mirroring the core vector field for field.
3. An e2e scenario proves one-round convergence through the CLI and the same
   record through GraphQL.
4. sync.md, invariants.md I1 and the CHANGELOG describe the two-branch rule;
   walkthrough 00203 (whose failing A used a frontmatter-less loser) is
   re-recorded, and a 00204 walkthrough shows the fold.

Scope source: PRD 00204 Phase 2. Prepared 2026-09-30 at a3e2dbc; depends on
reviewed, committed FT-8a (rebase the base SHA at build time).

## Tests

`tests/e2e/collision.rs` (module already registered):

- add_add_collision_with_frontmatterless_loser_converges_in_one_sync: two
  nodes commit the same `ddb/{id}.md`, the losing side without frontmatter.
  `ddb sync` on the resolving node exits 0 and prints the verbatim lines;
  both files exist; the frontmatter-less bytes on disk are unchanged; a
  second `ddb sync` reports nothing new.
- graphql_sync_reports_the_same_kept_verbatim_record_as_the_cli: build the
  identical collision (same bytes, so the same derived path) on a second node
  pair, resolve it through the GraphQL `sync` mutation on a server started
  with the e2e helpers, and assert exactly one entry whose `oldPath`/`newPath`
  equal what the CLI scenario printed.

`ddb-server` colocated test (next to the sync field or in its tests module):

- sync_result_exposes_collision_losers_kept_verbatim: the schema exposes the
  field with the three non-null string members.

Fail-first replay (driver): run both e2e scenarios against the FT-8a commit
without this item's renderers; they must fail on missing output/field, not on
a compile or setup error.

## Allowlist

- ddb-cli/src/commands/sync.rs (one render helper used by `sync` and bundle
  import)
- ddb-server/src/schema/mutations/operations.rs (sync field object)
- ddb-server/src/schema/mutations/types.rs (the new object type)
- ddb-server/src/schema/mutations/mod.rs (only: register the type)
- ddb-server tests beside those files (only: the named schema test)
- tests/e2e/collision.rs
- docs/src/technical/sync.md (replace the atomic-abort paragraph at :121 with
  the two-branch rule and the operator recovery: what is printed, where the
  file lands, that it stays unindexable until frontmatter is added)
- docs/src/technical/invariants.md (one sentence under I1)
- docs/src/technical/server.md (only: the sync mutation result fields)
- CHANGELOG.md (only: one `### Added` line; FT-8a's Fixed/Changed lines stay)
- dev/local/walkthroughs/00203-bundle-import-ref-namespace.md (re-record
  through showboat only; the failing A uses a YAML-invalid loser)
- dev/local/walkthroughs/00204-sync-collision-loser-kept-verbatim.md (new,
  showboat only)

## Read-only context

- AGENTS.md
- CLAUDE.md
- agent_docs/showboat-walkthroughs.md
- ddb-core/src/types/doogat.rs
- ddb-core/src/sync_manager/mod.rs
- ddb-core/src/git_ops/merge.rs
- ddb-server/src/actor/mod.rs
- tests/e2e/common.rs
- tests/e2e/server_mutations.rs
- dev/local/prds/backlog/00204-sync-collision-loser-without-frontmatter-v1.md
- docs/src/architecture/design-decisions.md

## Constraints

Transports adapt the core `SyncReport`; they hold no policy and re-derive
nothing. The GraphQL field serializes the same vector; no field subset, no
second core type. Purely additive: no existing CLI line or GraphQL field
changes shape. No runtime change in ddb-core; any core defect goes back to a
bounded FT-8a correction and a fresh review.

Tests first; the implementor runs no Cargo. Walkthroughs only through the
showboat CLI in temp dirs; verify only with `dev/bin/safe-showboat-verify`.
Never run `ddb` in the checkout. No FFI record or `sync` method (FFI has none
today; do not invent one). No actor/.kiro edits, no FT-9 work, no S3
leftovers, no PRD 00206, no release.

## Gate

Driver, sequential, Rust 1.89.0, `--locked`, `--manifest-path
<worktree>/Cargo.toml`, `--target-dir /Users/bob/git/src/github.com/doogat/ddb/target`:

- build
- clippy --workspace --all-targets -- -D warnings
- test-ci expansion (run_in_background, no other Cargo meanwhile)
- test -p ddb-server sync_result_exposes_collision_losers_kept_verbatim
- test -p ddb-core --test sync_test
- build -p ddb-cli
- `env CARGO_TARGET_DIR=...` test -p ddb-e2e --test e2e collision
- `env CARGO_TARGET_DIR=...` test -p ddb-e2e --test e2e bundle
- `env CARGO_TARGET_DIR=...` test -p ddb-e2e (batch end)
- dev/bin/safe-showboat-verify on both walkthroughs
- mdbook build, touched-file rustfmt, git diff --check

Nonzero selected count per filter. Logs under dev/local/reviews/FT-8b-logs/.

## Docs and CHANGELOG

sync.md, invariants.md I1, server.md sync fields as allowed above; the phrase
"kept verbatim" appears in sync.md. CHANGELOG `### Added`: **sync**: `ddb
sync`, `ddb bundle import` and the GraphQL `sync` mutation list collision
losers kept verbatim (PRD 00204). After review and CI, move PRD 00204 to
done/.

FT-8a review leftovers this item must cover (see PRD 00204 "Deferred work"):
the sync.md recovery text says a verbatim-kept file is unindexable (and
`reindex --strict` fails) until frontmatter is added, and that a body with
two `---` lines still takes the rewrite branch (known residual, own PRD).
Since d7867d2 the verbatim branch lands the loser's ORIGINAL blob (raw bytes),
and later losers' link rewrites skip verbatim-kept paths.

## Transport impact

- CLI: `ddb sync` and `ddb bundle import` print the kept-verbatim lines.
- GraphQL: `sync` gains `collisionLosersKeptVerbatim`; additive.
- FFI: no sync method; `import_bundle` returns `()`. No change, no parity
  claim (pre-existing gap in the 00168 deferred ledger).
- REST: no sync endpoint; no change.
- PgWire: sync is not a SQL verb; no change.
- NoSQL HTTP: read-only; no change.

Conformance: CLI and GraphQL read the same `SyncReport` vector; the e2e pair
asserts the same record on both.

## Model

sonnet

Fresh implementor receives Tests, Allowlist, Constraints and Read-only
context. Build session ends after gates; a fresh session reviews.
