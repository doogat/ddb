# Migrate smoke/integration shell scenarios to the Rust e2e crate

**Accepted with documented exceptions — 2026-09-07.** The six checked tasks reflect maintainer acceptance at `876a153`, subject to [the acceptance record](../../reviews/00171-acceptance-2026-09-07.md). Four assertion gaps, cleanup/documentation findings, and the unexplained macOS flake remain deferred. The remaining tail sweep was explicitly waived. Original requirements below are retained for traceability; these checkmarks do not claim perfect assertion parity.

**Priority:** P2 (leverage; ends the 4-file authoring tax before the contract block) · **Roadmap:** inserted 2026-07-16 by maintainer decision (assessment F5) — runs after P0/P1, before the contract block · **Depends on:** none (00162 done; builds its own server-spawn helper — do NOT depend on 00188 ephemeral-serve, a higher number; that PRD may later simplify the helper's internals).

Source: `dev/local/audit-results/evolution-assessment-2026-07-16.md` F5 (lens 7, commit-history mining). Maintainer decisions 2026-07-16: migrate soon, inserted early; later PRDs' scenario tasks were rewritten to target the e2e harness directly (backlog review 2026-07-16, Decision 1).

## Overview

### Problem Statement

Every smoke/integration scenario is authored twice, in two shell dialects: `tests/smoke.sh`/`smoke.ps1` and `tests/integration.sh`/`integration.ps1`. History quantifies the tax: the .sh↔.ps1 pairs co-change in 75/69 commits, 96 fix commits target the four harness files, 21 commits were pure PowerShell-dialect fixes (7 on 2026-03-18 alone: escaping, byte-array decode, `Pop-Location` hangs, `ErrorActionPreference`, ANSI stripping), and one behavior fix routinely touches all four files (`5a55296`). Every remaining backlog PRD that adds user-facing behavior pays this tax again. The cross-platform Rust e2e crate (`tests/e2e/`, assert_cmd) already runs on all three CI platforms and needs each scenario authored once.

### Target Users

Maintainer + autopilot: every subsequent PRD's scenario authoring drops from 4 files in 2 dialects to 1 Rust test. CI keeps identical coverage.

### Success Metrics

- Every scenario currently in the four shell files exists as a Rust e2e test — a section-by-section mapping table proves 100% coverage, no silent drops.
- CI (`test.yml`/`full-validation.yml`) runs the e2e suite where it ran the shell scripts, on all three platforms; a manual full-validation dispatch is green before any deletion.
- The four shell files are deleted (or each survivor is justified in the mapping table).
- AGENTS.md "Per-PRD deliverables" is rewritten: scenario authoring targets `tests/e2e/`; the 4-file mandate is removed.
- No lost semantics: must-fail assertions (`ddb-fails` pattern), server lifecycle scenarios (§54-57 SINGLETON/PgWire/warnings), and multi-process races keep their exact assertions.

## Functional Decomposition

### Capability: e2e server/process harness

#### Feature: Server-spawn helper
- **Description**: a shared helper in `tests/e2e/common.rs` that spawns `ddb serve` on a free port in a tempdir, polls `/health`, and tears down — replacing the shell scripts' server management.
- **Inputs**: the built `ddb` binary (e2e precondition), a tempdir, a free port.
- **Outputs**: a guard struct exposing base URL + token + repo path; teardown on drop.
- **Behavior**: extend the existing `ServerGuard` pattern in `tests/e2e/common.rs`; keep the helper's public API stable so 00188 (`--ephemeral`) can later replace its internals without touching call sites.

### Capability: Scenario ports

#### Feature: Smoke-section ports
- **Description**: every numbered section of `tests/smoke.sh`/`smoke.ps1` becomes a Rust e2e test.
- **Inputs**: the mapping table (Phase 0); the current smoke sections.
- **Outputs**: `tests/e2e/smoke_*.rs` tests named for their source section (`smoke_04_reserved_name_rejection` style).
- **Behavior**: port assertions verbatim in intent; must-fail cases use `assert_cmd`'s failure assertions (the `ddb-fails` equivalent); section identity preserved in test names so the mapping table stays auditable.

#### Feature: Integration-section ports
- **Description**: every numbered section of `tests/integration.sh`/`integration.ps1` becomes a Rust e2e test, server sections included.
- **Inputs**: the mapping table; the spawn helper.
- **Outputs**: `tests/e2e/integration_*.rs` tests.
- **Behavior**: server sections (§54-57 PgWire parity, SINGLETON races, warnings envelopes) port last; the psql-absent graceful degradation of §54.D keeps its semantic (skip-with-note when no client is available); timing-sensitive multi-process sections are marked serial.

### Capability: CI rewiring and retirement

#### Feature: Workflow rewiring
- **Description**: CI runs the e2e replacements where it ran the scripts.
- **Inputs**: `.github/workflows/test.yml`, `full-validation.yml`.
- **Outputs**: workflows invoking the e2e suite on all three platforms; shell-script steps removed.
- **Behavior**: keep job ordering so `cargo build -p ddb-cli` precedes e2e; run one manual `full-validation` dispatch green before deletion.

#### Feature: Shell-file deletion + docs rewrite
- **Description**: delete the four shell files and rewrite the authoring mandate.
- **Inputs**: the green CI run; the mapping table.
- **Outputs**: files deleted (or justified survivors); AGENTS.md "Per-PRD deliverables" + `docs/src/technical/` testing references updated.
- **Behavior**: deletion is the FINAL task, only after CI is green on the replacements. This is a deletion PRD: run `cargo test -p ddb-e2e` per CLAUDE.md.

## Structural Decomposition

### Repository Structure

```text
tests/
├── e2e/common.rs            # Maps to: Server-spawn helper (extend existing ServerGuard)
├── e2e/smoke_*.rs           # Maps to: Smoke-section ports
├── e2e/integration_*.rs     # Maps to: Integration-section ports
├── smoke.sh / smoke.ps1     # Maps to: deletion (Phase 2)
└── integration.sh / integration.ps1  # Maps to: deletion (Phase 2)
.github/workflows/
├── test.yml                 # Maps to: Workflow rewiring
└── full-validation.yml      # Maps to: Workflow rewiring
AGENTS.md                    # Maps to: Shell-file deletion + docs rewrite (per-PRD deliverables)
```

### Module: e2e common (tests/e2e/common.rs)
- **Maps to capability**: e2e server/process harness
- **Responsibility**: server spawn/health/teardown for all ported server scenarios; stable API for the 00188 swap.
- **Exports**: the spawn-helper guard (extends `ServerGuard`).

### Module: smoke/integration ports (tests/e2e/smoke_*.rs, integration_*.rs)
- **Maps to capability**: Scenario ports
- **Responsibility**: one Rust test per shell section, names carrying section identity.
- **Exports**: the test functions (no library surface).

### Module: workflows (.github/workflows/)
- **Maps to capability**: CI rewiring and retirement
- **Responsibility**: run the e2e suite where the scripts ran; keep platform matrix coverage.
- **Exports**: n/a (CI configuration).

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.
- **mapping table**: section-by-section inventory; the coverage proof artifact.

### Core Layer (Phase 1)
- **spawn helper**: depends on [mapping table] (identifies which sections need a server).
- **smoke ports**: depends on [mapping table].
- **integration ports**: depends on [spawn helper].

### Integration Layer (Phase 2)
- **CI rewiring**: depends on [smoke ports, integration ports].
- **deletion + docs**: depends on [CI rewiring] (green run first).

## Implementation Phases

### Phase 0: Foundation
**Goal**: the coverage proof exists before any port.

**Tasks**:
- [x] Build the mapping table: every numbered section in the four files → target e2e test name (or justified survivor), including which sections need a server and which are must-fail assertions. Known quirks to encode (verified at `d77ad58`): section numbering is sparse AND duplicated (both integration files carry two `# 50.` sections — key rows by file:line, not number alone); the `ddb-fails` must-fail helper exists only in PowerShell (`integration.ps1:67` — bash uses bare `!` negation); `tests/smoke.ps1` is invoked by NO workflow, so its port costs zero CI coverage (no deps) - Acceptance: the table is complete with zero unmapped sections (duplicate-numbered sections disambiguated by file:line); it lives in the PRD workdir notes and is referenced by later acceptance checks.

**Exit Criteria**: 100% of shell sections have a mapped target.

### Phase 1: Core
**Goal**: every scenario exists in Rust.

**Tasks**:
- [x] Extend `tests/e2e/common.rs` with the server-spawn helper (free port, tempdir, health poll, teardown; stable API) (depends on: Phase 0) - Acceptance: one round-trip test spawns, queries `/health`, and tears down cleanly.
- [x] Port all smoke sections to `tests/e2e/smoke_*.rs` per the mapping table (depends on: Phase 0) - Acceptance: each ported test green locally; must-fail assertions preserved via assert_cmd failure checks.
- [x] Port all integration sections to `tests/e2e/integration_*.rs`, server sections (§54-57) and multi-process races last, marked serial; keep §54.D's psql-absent graceful skip semantic (depends on: spawn helper) - Acceptance: each ported test green locally; the mapping table marks every section DONE.

**Exit Criteria**: mapping table shows 100% ported; `cargo test -p ddb-e2e` green locally.

### Phase 2: Integration
**Goal**: CI proves parity; the shell files retire.

**Tasks**:
- [x] Rewire `test.yml` + `full-validation.yml` to run the e2e replacements on all three platforms; dispatch a manual full-validation run (depends on: Phase 1) - Acceptance: all platforms green on the e2e replacements with the shell steps removed.
- [x] Delete the four shell files (per the mapping table; each survivor justified); rewrite AGENTS.md "Per-PRD deliverables" (scenario authoring targets `tests/e2e/`) and the `docs/src/technical/` testing references; run `cargo test -p ddb-e2e` (deletion PRD per CLAUDE.md) (depends on: CI rewiring) - Acceptance: `rg -l "smoke.sh"` finds only historical docs; CI green after deletion; AGENTS.md carries the new mandate.

**Exit Criteria**: shell files gone, CI green, AGENTS.md updated.

## Test Strategy

### Critical Scenarios
- **Happy path**: a ported smoke section (create/query round trip) → Expected: identical assertion outcome to the shell original.
- **Edge case**: a must-fail section (reserved-name rejection) → Expected: assert_cmd failure assertion fires exactly where `ddb-fails` did.
- **Error case**: a timing-sensitive multi-process race section under the cargo test runner → Expected: serial-marked test stays deterministic; no parallelism-induced flake.

## Risks

- **Timing-sensitive multi-process scenarios** (§55 races) may behave differently under cargo's parallel test runner. Mitigation: serial markers / the e2e crate's existing single-thread patterns; port these last.
- **Windows-specific behaviors currently only exercised via PowerShell** (path handling) must be asserted in Rust, not lost. Mitigation: the mapping table calls each out explicitly; CI's windows leg is the oracle.
- **e2e requires the built binary** (`cargo build -p ddb-cli` first). Mitigation: already the documented e2e precondition; CI job ordering respects it.

## Transport Impact

No product surface changes on any of the six transports (CLI, GraphQL, REST, PgWire, FFI, NoSQL HTTP). CI coverage parity is the contract: the mapping table proves CLI/GraphQL/REST/PgWire/NoSQL scenarios survive the port; FFI is untouched (its CI guard is 00194). Cross-interface conformance plan: the mapping table + the green manual full-validation dispatch before deletion are the conformance evidence; no new behavior surface exists to conform.

## Acceptance evidence

All 391 mapping rows are DONE; all four shell files are retired. The acceptance record links the pre-deletion, deletion, and final-SHA validation evidence and lists every exception. Settled substitutions are existing ServerGuard readiness plus its health round-trip test, per-test TempDir/port isolation rather than serial markers, and compiled-in Rust PgWire rather than an external-psql skip. Review cycle 2 completed every required lens and all five doubt rules, including corrected Eve input isolation. The maintainer accepted the remaining findings and waived the tail sweep; a fresh done gate performs lifecycle finalization.
