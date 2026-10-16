# Design: Migrate smoke/integration shell scenarios to the Rust e2e crate

PRD: `dev/local/prds/wip/00171-migrate-shell-scenarios-to-e2e-v1.md`

## Architecture fit

This is test-harness infrastructure, not product code — it touches only the
`tests/` crate (`ddb-e2e`, workspace member `tests/`, single test binary
`tests/e2e/main.rs`) and CI workflow YAML. No `ddb-core`/`ddb-cli`/`ddb-server`
production code changes. The e2e crate already exercises all six transports
(CLI via `assert_cmd`, GraphQL, REST, PgWire via `tokio_postgres`, WebSocket via
`tungstenite`, FFI is out of scope here) through the `ServerGuard`/`DdbTestRepo`
harness in `tests/e2e/common.rs` — this PRD extends that harness's *coverage*,
not its *shape*.

## Module placement

New files only; no existing production or e2e file is restructured.

- `tests/e2e/smoke_*.rs` — one file per `tests/smoke.sh` numbered section (or a
  small cluster of adjacent, related sections), test functions named
  `smoke_NN_slug` (e.g. `smoke_04_reserved_name_rejection`).
- `tests/e2e/integration_*.rs` — same pattern for `tests/integration.sh`,
  functions named `integration_NN_slug`; server-lifecycle sections (§54-57)
  land in `tests/e2e/integration_server_*.rs`.
- `tests/e2e/main.rs` — gains one `mod smoke_*;` / `mod integration_*;` line per
  new file, inserted alphabetically into the existing list (line 1-56 today).
- `.github/workflows/test.yml`, `.github/workflows/full-validation.yml` — edited
  in place (exact diffs below).
- `dev/local/designs/00171-migrate-shell-scenarios-to-e2e-v1-mapping.md` — NEW,
  the coverage-proof mapping table (Phase 0 deliverable; durable PRD satellite,
  not deleted until the PRD itself moves to `done/`).
- `AGENTS.md` — "Per-PRD deliverables" bullet rewritten (final task). That
  bullet's trailing sentence about `dev/local/specs/jink-feedback/ddb-repros/`
  is unrelated to the 4-file mandate and must carry into the rewritten text
  unchanged.
- **Every other doc referencing the four shell files** — the PRD's own Phase 2
  acceptance check (`rg -l "smoke.sh"` finds only historical docs) fails
  unless these are also updated in the same task: `CONTRIBUTING.md:42-43`
  (duplicates the exact 4-file mandate AGENTS.md is dropping — rewrite
  identically), `README.md:101-102` (quickstart commands that 404 post-
  deletion — replace with the `cargo test -p ddb-e2e` equivalent),
  `docs/src/technical/walkthrough.md:38,430` (describes the four files as
  "additional test surfaces" — rewrite to describe the e2e crate instead),
  `docs/src/technical/sql-engine.md:622,634` (cites specific shell sections
  for the SQL-engine bug-cluster table — repoint at the mapping table's
  target e2e test names for those rows).
- `tests/smoke.sh`, `tests/smoke.ps1`, `tests/integration.sh`,
  `tests/integration.ps1` — deleted (final task, after CI proves green on the
  replacements).

`tests/e2e/common.rs` is **not modified** unless a ported scenario needs a
capability it genuinely lacks (see Reuse inventory — it already covers spawn,
port allocation, tempdir, multi-node, and every transport client this PRD
needs).

## Interfaces & contracts

**Test naming.** `smoke_NN_slug` / `integration_NN_slug`, `NN` zero-padded to
match the source section number (`04`, not `4`). Two shell files each carry a
duplicate `# 50.` section header (`integration.sh:2159` PRD 00132 rename-across-
protocols, and `integration.sh:3697` PRD 00136 FK-freshness — same duplication
mirrored in `integration.ps1`); the mapping table keys these by `file:line`,
and the Rust side disambiguates by giving each a distinct slug
(`integration_50_alter_table_rename`, `integration_50_cross_process_fk_freshness`)
— never two functions named `integration_50`.

**Must-fail assertions.** The shell `ddb-fails` helper (PowerShell-only,
`integration.ps1:67`; bash negates with bare `!`) maps directly to the existing
`assert_cmd` idiom already used throughout the crate:

```rust
repo.ddb()
    .args(["update", "99999999999999", "--title", "Whatever"])
    .assert()
    .failure()
    .stderr(predicate::str::contains("error:"));
```

(`predicates` is already a `tests/Cargo.toml` dependency; see
`tests/e2e/cli_update_app_error.rs` for the existing pattern this PRD reuses
verbatim, not reinvents.)

**Server-spawn helper.** `ServerGuard::start(&DdbTestRepo)` (already implemented,
`tests/e2e/common.rs:55`) is the contract every server-backed port uses. Its
public API (`start`, `.url()`, `.rest_*`, `.graphql*`, `.ws_*`, `.kill`, `Drop`)
is unchanged by this PRD — no new methods are required for any currently-known
shell scenario (see Reuse inventory). If a specific integration-section port
surfaces a genuine gap during Phase 1, add the method to `ServerGuard` directly
(same file, same struct) rather than a parallel helper.

**Concurrency / "serial" sections.** No `serial_test` crate, no
`--test-threads=1`. Every test gets its own port
(`next_available_port()`'s process-wide `AtomicU16` counter, `common.rs:34`)
and its own `TempDir`-backed repo, so cargo's default parallel test runner is
already safe — this is the existing pattern, not a new one (see
`tests/e2e/singleton_cross_process_upsert.rs` for a live two-server race test
that runs under ordinary `cargo test` parallelism today). Timing-sensitive
*within-test* ordering (two CLI/GraphQL calls that must interleave a specific
way) is expressed with `std::thread::scope` + explicit joins, not a serial
marker — same file, same pattern as the existing race test cited above.

**PgWire sections — psql dependency dropped, not preserved.** The shell scripts'
§54.D/55.D "psql-absent" skip (`integration.sh:2355-2358`,
`integration.sh:2513`) exists only because the shell scripts shell out to the
external `psql` binary and some CI runners (macOS) don't have it. The shell
scripts' own pass messages already point at the replacement:
`"54.D: PgWire parity covered by cargo test -p ddb-e2e --test e2e -- pgwire_singleton (no psql on host)"`.
The Rust e2e crate already ports PgWire scenarios through `tokio_postgres`
(pure-Rust client, already a `tests/Cargo.toml` dependency — see
`tests/e2e/pgwire_singleton.rs`, `tests/e2e/pgwire.rs`), never shelling out to
`psql`. **The port therefore drops the skip entirely**: §54.D/55.D's PgWire
assertions run unconditionally on every platform via `tokio_postgres`, which
exceeds the shell semantic (always-run beats conditional-skip) rather than
merely preserving it. Do not add a `command -v psql` equivalent check in Rust —
there is nothing to check for.

**CI rewiring — exact diffs.**

`test.yml` (fast per-push gate): replace the shell smoke step —

```diff
-      - name: Smoke test
-        env:
-          DDB_BIN: ${{ github.workspace }}/target/debug/ddb
-        run: ./tests/smoke.sh
+      - name: Smoke test (e2e)
+        env:
+          DDB_BIN: ${{ github.workspace }}/target/debug/ddb
+        run: cargo test -p ddb-e2e --test e2e smoke_
```

(cargo test's substring filter on the harness matches every `smoke_*` function;
job ordering is unchanged — this step already runs after "Build CLI binary".)
The five hand-picked `E2E - *` steps and `Sync integration tests` step in
`test.yml` are untouched — they are an independent fast-gate sampling, not this
PRD's concern.

`full-validation.yml`: the `linux-smoke` job's only content is
`./tests/integration.sh`; once ported, that content is a strict subset of what
`linux-e2e`'s unfiltered `cargo test -p ddb-e2e` already runs on the same
platform. **Delete the `linux-smoke` job** (not just its shell step) and remove
it from `mark-validated`'s `needs:` list. `macos-full` and `windows-full` each
run `cargo test-full` (= `cargo test --workspace`, which includes the `tests`
workspace member unfiltered — see `Cargo.toml`/`​.cargo/config.toml`) followed
by a separate `./tests/integration.sh` / `integration.ps1` step; once ported,
that separate step is redundant with the `cargo test-full` step directly above
it on the same job. **Delete the `Integration test` step from `macos-full` and
`windows-full`** (keep every other step, including `Full cargo test suite`,
which now carries the ported coverage). This is a net simplification (3 fewer
CI steps, no coverage lost — `cargo test-full`/`linux-e2e` already run
unfiltered) and satisfies the PRD's Phase 2 acceptance ("all platforms green on
the e2e replacements with the shell steps removed") without inventing new job
structure.

**Mapping table.** `dev/local/designs/00171-migrate-shell-scenarios-to-e2e-v1-mapping.md`,
one row per shell section: `file:line | section # | target e2e test (or
"ALREADY COVERED by <existing test>" or "justified survivor: <reason>") |
needs server? | must-fail? | status`. Built in Phase 0 before any port task
starts; updated to DONE per section as Phase 1 tasks land (PRD's own Exit
Criteria requires it show 100% ported before Phase 2).

## Data flow

Unchanged from the existing e2e crate: a test calls `DdbTestRepo::init()`
(spawns `ddb init` in a fresh `TempDir`, disables git commit signing so CI
doesn't need a signing key), optionally `ServerGuard::start(&repo)` (spawns
`ddb serve`, blocks on stderr readiness lines for HTTP+PgWire listeners,
returns a guard exposing base URL/PG port/auth token), then drives the CLI via
`assert_cmd::Command` and/or the server via GraphQL/REST/WS/PgWire clients, and
asserts. `Drop` kills the child process; the `TempDir` cleans up on scope exit.
No new data-flow shape — this PRD is pure coverage porting through the existing
pipes.

## Reuse inventory

- **`ServerGuard`** (`tests/e2e/common.rs:48-334`) — full server lifecycle:
  spawn, free-port allocation (`next_available_port`, an `AtomicU16` counter
  that skips already-bound ports), stderr-based readiness detection for both
  HTTP and PgWire listeners, auth-token capture, GraphQL/REST/WebSocket client
  methods, `kill()`, `Drop`. Covers 100% of the PRD's "Server-spawn helper"
  ask already — used in 30+ existing e2e files (`rg -n "ServerGuard::start"
  tests/e2e/` — 100+ call sites). No extension needed for any scenario
  identified so far.
- **`DdbTestRepo`** (`common.rs:361-401`) — tempdir-backed repo init +
  `ddb()`/`ddb_at()` command builders, git-signing disabled for CI. Direct
  replacement for the shell scripts' per-scenario `mktemp -d` + `ddb init`.
- **`MultiNodeSetup` / `TwoNodeSetup`** (`common.rs:404-594`) — bare-remote +
  N-node clone/register/push/sync helpers, already used for multi-device sync
  scenarios. Covers the PRD's cross-process sections that need >1 clone, not
  just >1 server.
- **`assert_cmd::Command` + `predicates`** — the CLI-invocation and must-fail
  idiom (`.assert().success()/.failure()`, `predicate::str::contains`) used
  everywhere in the crate; direct replacement for shell's `ddb ... || fail` /
  `ddb-fails` patterns. Greps tried: `assert_cmd`, `predicates`, `ddb-fails`,
  `\.failure\(\)` — all confirm the pattern is established, nothing new to
  write.
- **`tokio_postgres`** (`tests/Cargo.toml:22`, used in `pgwire.rs`,
  `pgwire_singleton.rs`) — replaces every shell `psql` invocation; see
  Interfaces & contracts above.
- **Existing PgWire e2e coverage may already satisfy several shell sections
  outright.** `integration.sh` sections 54.D and 55.D literally name
  `pgwire_singleton` as their own replacement in their pass messages
  (`integration.sh:2358`, `:2513`) — the mapping table (Phase 0) must check
  each server-lifecycle section against `tests/e2e/pgwire*.rs`,
  `serve.rs`, `server_mutations.rs`, `subscriptions.rs`, and
  `singleton_cross_process_*.rs` for an existing equivalent before writing a
  new test, to avoid duplicate coverage of the same assertion.
- Nothing found for: a distinct "spawn helper" beyond `ServerGuard`
  (greps: `ServerGuard`, `serve.*spawn`, `health.*poll` — all resolve to the
  existing struct); a `serial_test`-style crate (greps: `serial_test`,
  `#\[serial\]`, `test-threads` — zero hits in `tests/`, `Cargo.toml`,
  workflows — confirms no such dependency exists or is needed, per the
  Concurrency contract above).

## Alternatives considered

1. **Smallest-diff: port only the sections currently reachable from CI** (the
   ~10 sections `test.yml` names explicitly, plus whatever `linux-smoke`/
   `macos-full`/`windows-full` exercise) and leave the rest of the shell files
   in place as a "just in case" archive. Rejected: fails the PRD's own Success
   Metric ("every scenario... exists as a Rust e2e test — no silent drops")
   and leaves the dual-dialect authoring tax exactly as expensive for every
   future PRD, which is the whole point being fixed.
2. **Rewrite the shell scripts as a single shared dialect** (e.g. drop
   PowerShell, keep only bash, run bash-on-Windows via WSL/git-bash in CI).
   Rejected: still a shell-based harness with the same fragility class
   (21 commits of pure PowerShell-dialect fixes were about *shell* fragility,
   not multi-dialect-ness specifically — bash-only still needs its own
   Windows-compatibility workarounds) and does not gain the compile-time
   type-checking, IDE support, and single-authoring-surface `cargo test`
   already gives the existing 100+ e2e tests.
3. **Chosen: full 1:1 port to the existing Rust e2e crate**, reusing
   `ServerGuard`/`DdbTestRepo` as-is (no new abstraction layer), one Rust test
   per shell section, mapping table as the coverage proof, CI rewired to run
   the replacements, shell files deleted last. The extra size over option 1
   buys exactly what the PRD asks for: zero authoring tax for every
   remaining backlog PRD (00172+), which the growth-roadmap memory frames as
   the reason this PRD was inserted ahead of the contract block.

## Risks & edge cases

- **Duplicate `# 50.` section numbers** in both `integration.sh` and
  `integration.ps1` (verified: `integration.sh:2159` and `:3697`, mirrored in
  `.ps1` at `:2026`/`:3654`) — mapping table keys by `file:line`; Rust test
  names disambiguate with distinct slugs (see Interfaces & contracts).
- **`tests/smoke.ps1` has zero CI coverage today** (absent from both
  `test.yml` and `full-validation.yml` — confirmed by grep). Its sections still
  need porting per the PRD's 100%-coverage metric, but the port costs zero
  *additional* CI risk since nothing currently depends on it running.
- **A ported section might duplicate an already-existing e2e test** (see
  Reuse inventory's PgWire finding) — the mapping table must record
  "ALREADY COVERED" rather than write a redundant test; a redundant test
  is not wrong, just wasted authoring effort the PRD is trying to eliminate.
- **Windows-specific path/escaping behavior** currently exercised only via
  PowerShell must still be asserted somewhere in Rust once the `.ps1` files
  are gone. Rust's `std::path::Path` already handles platform separators
  correctly, so most of these sections likely need no special porting beyond
  running on `windows-full`'s existing runner — but any assertion that
  specifically tests *string* path formatting (not just behavior) needs a
  `cfg!(windows)` branch in the ported test, called out explicitly in the
  mapping table per PRD's own Risk note.
- **Next likely follow-ups after this PRD** (do not build now, but this design
  should not box them in): (a) PRD 00188 `ephemeral-serve-test-mode-v1` may
  want to swap `ServerGuard`'s child-process spawn for an in-process embedded
  server — the public API this PRD locks in (`start`/`url`/`graphql`/etc.) is
  exactly what 00188 needs to stay stable across that swap, per the PRD's own
  note; (b) a future PRD may want to run a curated e2e subset per-package
  rather than the whole crate for faster fast-gate feedback — the
  `smoke_`/`integration_` naming convention this design pins is what makes
  substring-filtered `cargo test` invocations like the one added to `test.yml`
  possible without a third mechanism (tags, feature flags, etc.).

## Test strategy outline

This PRD *is* the test suite — there is no separate "tests for the tests"
layer beyond what the PRD's own Success Metrics already specify: the mapping
table is the coverage-completeness proof (100% of shell sections mapped, zero
unmapped rows), each ported test is verified green locally before being marked
DONE in the table, and a green manual `full-validation` dispatch on all three
platforms is the CI-parity proof gate before the shell files are deleted. No
new test-of-tests infrastructure is introduced.

## Review log

**Dispatch 1 (Claude) non-blockers/questions (not fixed):**
- Non-blocking: `windows-full`'s deleted "Integration test" step carried a `timeout-minutes: 15` hang guard (relevant given the PRD's own history of PowerShell `Pop-Location` hangs); the consolidated "Full cargo test suite" step has no equivalent per-step timeout. Consider adding one, or note why the risk is now negligible (no PowerShell process involved).
- Non-blocking: Reuse inventory's "used in 30+ existing e2e files" is imprecise — verified actual count is 21 files (151 call sites, matching the separately-stated "100+ call sites" claim). Doesn't affect the design's conclusion.
- Question: this design supersedes the PRD's own named mitigation for §55 races ("serial markers", PRD line 148) with port/tempdir isolation instead. Well-justified by existing precedent, but the mapping table / Phase 1 task text should say explicitly that this supersedes the PRD's suggestion, so a task planner doesn't add an unneeded `serial_test` dependency.

dispatch 1 (claude): cardinal-sin 0, blocker 1, non-blocker 2, question 1
