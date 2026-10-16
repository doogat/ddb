---
default_model: opus
model_tier_rationale: concurrency/timing - port read-back, signal-driven teardown, and a stdout contract
---

# ddb serve --ephemeral test mode

**Priority:** P3 · **Roadmap:** Phase 3 (downstream enablement) · **Depends on:** none

## Overview

### Problem Statement
Every downstream that integration-tests against ddb rewrites the same harness: check the binary, init a fresh repo, pick free ports, spawn `ddb serve`, poll health, and tear it down. This scaffolding is generic yet re-implemented per app.

Evidence (jink):

- `jink: tests/common/mod.rs` is 430 LOC. The generic server scaffolding is all there: `free_port()` (line 6), `check_ddb_binary()` (lines 11-12, `Command::new("ddb").arg("--version")`), `ddb init` (line 39), spawning `ddb serve` on free HTTP + pg ports (lines 49-69), `poll_health` (lines 411-430, hitting `/health`), and SIGTERM teardown (`graceful_kill`, lines 127-132). Only the `create_test_*` fixture helpers (lines 179-410) are jink-domain-specific.

The spawn/init/health/teardown portion (~150-200 LOC) is pure boilerplate a downstream should get from ddb, not re-author.

### Target Users
Downstream apps (jink) writing integration tests that need a real, disposable ddb server and currently hand-roll the lifecycle.

### Success Metrics
- `ddb serve --ephemeral` boots against a fresh tempdir (auto `ddb init`), binds OS-assigned free HTTP + pg ports, and prints a single machine-readable connection block on stdout: `{ url, pg_port, token, data_dir }`.
- The server answers a full CRUD round-trip on the printed url + token.
- On SIGINT/SIGTERM the tempdir is removed (no leak); the connection block is the only content on stdout (logs stay on stderr).
- Replaces the ~150-200 LOC of `jink: tests/common/mod.rs` scaffolding (`free_port`/`check_ddb_binary`/spawn/init/health/teardown) so jink can delete it and keep only its domain fixture helpers (jink-side work, not verified in this repo).
- The ephemeral repo, token, and ports are fully isolated from the user's `~/.config/ddb` and any running server, so the harness never clobbers real state.
- A downstream test suite spawns one process per ephemeral server and reads its connection block, with no ddb-specific lifecycle code of its own.
- The connection-block schema (`url`, `pg_port`, `token`, `data_dir`) is documented and stable, so harness parsers do not break across ddb releases.

## Functional Decomposition

### Capability: Ephemeral server bootstrap
#### Feature: Tempdir auto-init
- **Description**: create and initialize a throwaway repo.
- **Inputs**: the `--ephemeral` flag on `Command::Serve`.
- **Outputs**: a `tempfile::TempDir` holding an initialized ddb repo (with its own token file). `tempfile` is a dev-dependency in both `ddb-cli` and `ddb-server` today (`Cargo.toml:32` / `:35`) — promote it to a runtime dependency of `ddb-cli`.
- **Behavior**: create the tempdir under an OS temp path with a `ddb-ephemeral-*` prefix, run the core init, and place the token file inside the tempdir (isolated from `~/.config/ddb`).

#### Feature: Free-port binding with read-back
- **Description**: bind OS-assigned HTTP + pg ports and learn the actual numbers.
- **Inputs**: port `0` for both surfaces.
- **Outputs**: the resolved bound ports.
- **Behavior**: `run()` (ddb-server/src/lib.rs:37) binds `TcpListener::bind` at `cfg.port`; extend it so port `0` binds an ephemeral port and the resolved `local_addr()` is read back for both the HTTP listener and the pgwire listener.

#### Feature: Machine-readable connection block
- **Description**: emit the connection info as parseable JSON.
- **Inputs**: the resolved ports + token + data dir.
- **Outputs**: a single-line JSON `{ url, pg_port, token, data_dir }` on stdout.
- **Behavior**: print exactly one JSON object to stdout; keep all tracing/`eprintln!` logs (currently "listening on …", "auth token: …") on stderr.

#### Feature: Signal-driven teardown
- **Description**: clean up on interrupt.
- **Inputs**: SIGINT/SIGTERM (cross-platform ctrl-c).
- **Outputs**: the tempdir removed.
- **Behavior**: install a `tokio::signal` handler (enable tokio's `signal` feature — absent from `ddb-server/Cargo.toml:11` and `ddb-cli/Cargo.toml:18` today; no signal handling exists anywhere in the workspace); on signal, drop the `TempDir` and exit. SIGKILL cannot be caught and is documented as a leak (OS temp reaps it).

### Capability: Harness ergonomics
#### Feature: Readiness on the connection block
- **Description**: the block prints only once the server is ready to serve.
- **Inputs**: the server's post-bind ready state.
- **Outputs**: a connection block a downstream can use immediately (no extra health poll).
- **Behavior**: emit the block after the listeners are bound and the index is reachable, so downstreams skip re-implementing `poll_health` (`jink: tests/common/mod.rs:411`).

#### Feature: Isolated token + data dir
- **Description**: the ephemeral repo never touches the user's `~/.config/ddb`.
- **Inputs**: the tempdir path.
- **Outputs**: a token file inside the tempdir.
- **Behavior**: place the token in the tempdir so concurrent ephemeral servers and the user's real server never share auth state.

## Structural Decomposition

### Repository Structure

```
ddb-cli/src/
├── main.rs                     # Maps to: Command::Serve { …, ephemeral }
└── commands/maintenance.rs     # Maps to: serve() ephemeral branch (tempdir, init, block, teardown)
ddb-server/src/
├── lib.rs                      # Maps to: run() binds port 0 → OS-assigned, reads back both ports
└── config.rs                   # Maps to: ephemeral token-file placement (inside tempdir)
```

### Module: maintenance::serve (ddb-cli/src/commands/maintenance.rs)
- **Maps to capability**: Ephemeral server bootstrap
- **Responsibility**: the `--ephemeral` branch — tempdir, init, invoke `run()` with port 0, print the connection block, own teardown.
- **Exports**: `pub(crate) fn serve(repo, port, pg_port, bind, playground, ephemeral)` (today `pub(crate) fn serve(repo: &Path, port: u16, pg_port: u16, bind: &str, playground: bool)`, `maintenance.rs:273-288`, dispatched from `commands/mod.rs:69-74`)

### Module: server run (ddb-server/src/lib.rs)
- **Maps to capability**: Ephemeral server bootstrap
- **Responsibility**: bind ephemeral ports and expose the resolved HTTP + pg port numbers.
- **Exports**: `run(...)` reporting the resolved bound ports

### Module: CLI mode and configuration (ddb-cli/src/main.rs; ddb-server/src/config.rs)
- **Maps to capability**: Ephemeral server bootstrap; Harness ergonomics
- **Responsibility**: declare ephemeral mode and keep its token/configuration within the temporary repository.
- **Exports**: ephemeral CLI option and temporary server configuration.

### Module: capability publication (ddb-server/src/capabilities.rs)
- **Maps to capability**: Ephemeral server bootstrap
- **Responsibility**: publish ephemeral_serve from the existing 00186 registry and verify its declared interface/build scope.
- **Exports**: the registry entry and matching handshake fixtures.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **server run() port read-back**: expose OS-assigned ports; prerequisite for a printable connection block.

### Core Layer (Phase 1)
- **CLI ephemeral branch**: Depends on [run() read-back] - tempdir, init, connection block, teardown.

### Integration Layer (Phase 2)
- **tests + docs**: Depends on [CLI ephemeral branch].

## Implementation Phases

### Phase 0: Foundation
**Goal**: OS-assigned ports, read back.

**Tasks**:
- [ ] Extend `run()` so HTTP + pg port `0` binds an ephemeral port and the resolved `local_addr()` numbers are reported back to the caller (no deps) - Acceptance: `run()` with port `0` yields non-zero resolved HTTP + pg ports; an integration test asserts both are bound and distinct.

**Exit Criteria**: the server can bind free ports and surface them.

### Phase 1: Core
**Goal**: the `--ephemeral` mode end to end.

**Tasks**:
- [ ] Add `ephemeral` to `Command::Serve` → `maintenance::serve`; on `--ephemeral` create a tempdir, init it, run with port `0`, print the JSON connection block on stdout, and drop the tempdir on SIGINT/SIGTERM (depends on: Phase 0) - Acceptance: `ddb serve --ephemeral` prints a single parseable `{ url, pg_port, token, data_dir }` block; SIGTERM removes the tempdir.

**Exit Criteria**: ephemeral boot + connection block + teardown work.

### Phase 2: Integration
**Goal**: prove the harness use case, document it.

**Tasks**:
- [ ] `tests/e2e/` test: spawn `--ephemeral`, parse the block, run a CRUD round-trip on the printed url/token, assert the tempdir is gone after teardown (depends on: Phase 1) - Acceptance: spawn → parse → CRUD → teardown all pass.
- [ ] Add e2e scenarios in the Rust harness (`tests/e2e/`; the shell harness was removed by 00171) beyond the Phase-2 CRUD round-trip (concurrent double-spawn distinct ports; teardown) + a walkthrough (new CLI mode) + docs (`docs/src/technical/server.md` ephemeral mode, `docs/src/guide/building-apps.md` testing section) + CHANGELOG (depends on: Phase 1) - Acceptance: e2e scenarios green; docs describe the connection-block contract. Include the `ephemeral_serve` registry addition and update exact-set plus health/GraphQL agreement fixtures; Acceptance also requires the flag to match compiled availability and the implemented workflow.

**Exit Criteria**: e2e + scenarios + docs landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: spawn `ddb serve --ephemeral`, parse the block, `createDoogat` + read back on the printed url/token → success.
- **Edge case**: two concurrent `--ephemeral` instances get distinct ports + distinct tempdirs (no collision).
- **Error case**: SIGTERM mid-run removes the tempdir (the `data_dir` path is gone afterward); a repo with no typedefs still boots (base schema).

## Risks

- **Port read-back plumbing**: `run()` today takes `Option<u16>` and binds `cfg.port`; port `0` requires reading `local_addr()` and threading both the HTTP and pgwire resolved ports back to the CLI. Mitigation: expose the resolved ports from the bind step; test both.
- **Stdout hygiene**: any stray stdout line breaks block parsing. Mitigation: verified at `d77ad58` that stdout is ALREADY clean — "auth token:" (`lib.rs:49`), "listening on" (`lib.rs:87`), and "pgwire listening on" (`pgwire.rs:358`) are all `eprintln!`, and tracing is stderr-bound — so the work is only ADDING the one JSON stdout write, not relocating existing output; the test parses stdout strictly.
- **Teardown on hard kill**: SIGKILL cannot run cleanup and leaks the tempdir. Mitigation: use an OS temp path (reaped by the OS) with a `ddb-ephemeral-*` prefix; document that SIGKILL leaks; rely on SIGINT/SIGTERM/ctrl-c.
- **Windows signals**: SIGTERM semantics differ on Windows. Mitigation: use `tokio::signal::ctrl_c` (cross-platform) plus the Unix SIGTERM handler; the Windows stop path is exercised by the e2e teardown scenario on CI's windows leg (the shell harness is gone by this PRD's slot — 00171).
- **pgwire port read-back**: the pgwire listener starts separately from the axum listener (lib.rs:90), so both resolved ports must be surfaced, not just HTTP. Mitigation: read `local_addr()` from each listener and include both in the connection block; test that the printed `pg_port` is reachable.
- **Init cost per spawn**: auto-`ddb init` on every ephemeral boot adds startup latency to a downstream suite. Mitigation: keep init minimal (empty repo, no bundled types unless requested) and document the per-spawn cost.
- **The e2e `ServerGuard` cannot exercise SIGTERM teardown as-is**: it probes ports from a counter (19100+), scrapes stderr banners for readiness, and tears down with `child.kill()` (`tests/e2e/common.rs:34-45,:81-102,:330-333` at `8fe51c9`). Mitigation: the Phase 2 e2e spawns `--ephemeral` directly and sends SIGTERM itself; swapping `ServerGuard` onto `--ephemeral` is optional follow-up work, not this PRD's scope.

**Capability registry (depends on 00186):** add `ephemeral_serve` to the existing sorted registry only with this implemented surface; document the applicable interface/workflow and compile-time availability. Internal refactors and tests are not separate flags.

## Transport Impact

Tooling PRD; framed as which ddb interfaces the mode brings up for downstream tests.

- **CLI**: Guaranteed — `ddb serve --ephemeral` (new mode) is the deliverable.
- **GraphQL / REST / PgWire / NoSQL HTTP**: the ephemeral mode brings up ALL server transports via the existing `run()` path; they answer on the printed connection block. These are not new per-interface capabilities — the mode is a test-harness convenience over the existing transports.
- **FFI**: Intentionally absent — FFI is embedded/in-process; downstream FFI tests construct `DoogatDriver` directly, with no server to spawn.
- **Cross-interface conformance plan**: an e2e asserts the connection block is parseable and the server answers a CRUD round-trip on it (GraphQL plus one additional transport), then verifies teardown removed the tempdir. No new business capability is introduced; conformance is the spawn → parse → CRUD → teardown loop.

## Deferred observation from 00171 (2026-09-07)

00171 closed with maintainer acceptance of the unexplained macOS `ConnectionReset` in `integration_58_a_graphql_update_warnings_empty` at `tests/e2e/common.rs:186`, observed in Nightly 34020494801 at `c8b72bd`. Full Validation 34092612666 at `876a153` subsequently passed the complete e2e suite on all three platforms without retries or skips of this scenario. The cause remains unconfirmed; no retry strategy or readiness change is prescribed. Preserve this as a harness follow-up when 00188 is eventually undertaken. Evidence and approval: [00171 acceptance](../../reviews/00171-acceptance-2026-09-07.md), finding 00171-C2-01. This note does not start 00188.
