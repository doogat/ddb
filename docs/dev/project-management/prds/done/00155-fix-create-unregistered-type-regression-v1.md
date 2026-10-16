# Fix `ddb create --type <unregistered>` regression (nightly red since 2026-05-29)

## Overview

### Problem Statement

Nightly `full-validation` has been red since 2026-05-29 with one signature:
`error: type "project" is not a registered typedef`, failing
`tests/integration.sh`'s embedded smoke at `tests/smoke.sh:33`
(`ddb create --title "Project Alpha" --type project ...` on a fresh repo
with no `ddb type install project`).

**Reproduced locally 2026-06-09** with the current binary:
`ddb --repo /tmp/x init && ddb --repo /tmp/x create --title "Project Alpha"
--type project --tags active --body "A project doogat"` → exit 1, same
message. This is a CLI behavior regression, not a CI environment issue.
Tier 1 never sees it because smoke.sh execution is Tier 2 (CI-only), so it
went unnoticed locally for ~12 days.

Root cause: PRD 00147 rerouted CLI create through the app facade —
`ddb-cli/src/commands/crud.rs:51-68` builds a `CreateCommand` and calls
`DoogatService::create` (`ddb-core/src/service/crud.rs:170`), which
delegates to `batch_create_with_message` whose contract deliberately
includes `TYPE_NOT_REGISTERED` rejection (crud.rs:166-169) — GraphQL
`createDoogat` parity. The CLI's documented contract was the opposite:
the strict typed pipeline fires only for *registered* typedefs;
unregistered types get a silent base-only create (PRD 00129 §T3, preserved
by PRD 00133). That released (≤ v0.2.5) behavior was replaced silently:
no CHANGELOG entry, no deprecation note — exactly what PRD 00150's
compatibility rules forbid. The REST create route goes through the actor's
shared create command and has likely inherited the same strictness
(unverified; Phase 0 task). The actor-level strictness itself is pinned as
intended GraphQL behavior by
`actor_create_with_unregistered_type_returns_type_not_registered`
(`ddb-server/tests/create_via_actor_t9.rs`).

### Target Users

- The maintainer: nightly Tier 2 is the heavy validation battery; while it
  is red, regressions in everything else it covers are invisible too.
- Downstream CLI/REST scripts written against the released v0.2.5 contract
  (quick capture with ad-hoc types, no typedef ceremony).

### Success Metrics

- Nightly `full-validation` green on the first run after merge.
- The chosen contract is pinned by tests on every affected interface and
  recorded in CHANGELOG + docs; no silent behavior delta remains.
- Local repro command above behaves per the chosen contract.

## Functional Decomposition

### Capability: Contract decision

One deliberate, recorded decision on what unregistered-type create means
per interface.

#### Feature: Unregistered-type create policy

- **Description**: Decide and record the per-interface policy.
- **Inputs**: Pre-00147 behavior per interface (CLI lenient, GraphQL
  strict; REST/actor to be verified via git history), promise matrix
  (`docs/src/guide/building-apps.md`), PRD 00150 compatibility rules.
- **Outputs**: Decision recorded in this PRD + docs; default is
  **Option A** unless the maintainer overrides before implementation.
  - **Option A (default, recommended)**: restore the released contract —
    CLI (and REST if it was lenient) create base-only doogats for
    unregistered types; GraphQL keeps strict rejection. Improvement over
    the old silence: the lenient path now emits an
    `UNREGISTERED_TYPE_BASE_ONLY` AppWarning (free delivery via CLI
    stderr, REST `warnings` array).
  - **Option B**: ratify strict-everywhere. Breaking change for released
    CLI users: requires a D-NN migration note, CHANGELOG `Changed`,
    smoke.sh:33 + smoke.ps1 mirror updates (`type install project`
    first), and an escape hatch consideration (e.g. `--allow-unregistered`).
- **Behavior**: Option A preserves v0.2.5 compatibility and the
  documented invariant; the CLI/GraphQL difference is already a
  maintainer-decided specialization (D-CLI-1 precedent). Option B is the
  120%-parity-pure path but must go through the compatibility process,
  not ship as a side effect.

### Capability: Policy-aware create path

The app facade supports both strict and lenient callers explicitly.

#### Feature: CreateCommand policy field

- **Description**: Make strictness an explicit input instead of an
  accident of shared routing.
- **Inputs**: `CreateCommand` (ddb-core/src/app_contract/), caller policy.
- **Outputs**: `CreateCommand` gains an unregistered-type policy (enum,
  e.g. `Strict | BaseOnly`), default `Strict` so GraphQL and the pinned
  actor test are unchanged.
- **Behavior**: `DoogatService::create` routes `BaseOnly` commands with an
  unregistered type through the base-create path (pre-00147 semantics) and
  attaches the `UNREGISTERED_TYPE_BASE_ONLY` warning; `Strict` keeps the
  current `TYPE_NOT_REGISTERED` rejection. CLI passes `BaseOnly`; GraphQL
  resolver/actor default stays `Strict`; REST passes whatever Phase 0
  verification says its released behavior was.

#### Feature: Regression pinning

- **Description**: Tests that fail if either contract drifts again.
- **Inputs**: chosen policy matrix.
- **Outputs**: e2e test for CLI unregistered-type create; service unit
  test for both policy values; smoke/integration scenario.
- **Behavior**: written first (TDD) against the failing case.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/
├── app_contract/commands.rs   # Maps to: CreateCommand policy field
├── service/crud.rs            # Maps to: policy-aware DoogatService::create
ddb-cli/src/commands/crud.rs   # Maps to: CLI passes BaseOnly
ddb-server/src/
├── actor/handlers.rs          # Maps to: actor command policy plumb-through
├── rest.rs                    # Maps to: REST policy per Phase 0 verification
tests/
├── e2e/                       # Maps to: regression pinning (new test file)
├── smoke.sh + smoke.ps1       # Maps to: scenario per chosen option
CHANGELOG.md                   # Maps to: contract decision record
docs/src/guide/building-apps.md  # Maps to: contract decision record
```

### Module: app_contract

- **Maps to capability**: Policy-aware create path
- **Responsibility**: carry the policy as adapter-neutral command data
- **Exports**: `CreateCommand` with policy field (name TBD at
  implementation, e.g. `UnregisteredTypePolicy`)

### Module: service/crud

- **Maps to capability**: Policy-aware create path
- **Responsibility**: branch on policy in `DoogatService::create`; emit
  the warning on the lenient branch
- **Exports**: unchanged public signature apart from `CreateCommand`

### Module: transports (cli, actor, rest)

- **Maps to capability**: Contract decision
- **Responsibility**: each caller states its policy explicitly
- **Exports**: none (call-site changes only)

## Dependency Graph

### Foundation Layer (Phase 0)

No dependencies - built first.

- **Decision + verification**: confirm REST/actor pre-00147 behavior (git
  history of actor create handler), record the decision.

### Core Layer (Phase 1)

- **app_contract + service/crud**: Depends on [Phase 0 decision].

### Integration Layer (Phase 2)

- **transports + tests + docs**: Depends on [Core Layer].

## Implementation Phases

### Phase 0: Foundation

**Goal**: A recorded decision, with REST's released behavior verified.

**Tasks**:

- [ ] Verify pre-00147 actor/REST create behavior for unregistered types
  via git history (no deps) - Acceptance: this PRD's decision section
  updated with the finding and the REST policy chosen.
- [ ] Confirm Option A or B (no deps; default Option A if no maintainer
  override) - Acceptance: decision + rationale recorded here and reflected
  in the task list below.

**Exit Criteria**: Implementation can proceed without revisiting policy.

### Phase 1: Core

**Goal**: Policy-aware facade, failing tests first.

**Tasks**:

- [ ] Write failing service unit tests for both policy values (depends on:
  Phase 0) - Acceptance: `BaseOnly`+unregistered creates base doogat with
  `UNREGISTERED_TYPE_BASE_ONLY` warning; `Strict`+unregistered returns
  `TYPE_NOT_REGISTERED`; both red before the fix.
- [ ] Add policy to `CreateCommand`, branch in `DoogatService::create`
  (depends on: failing tests) - Acceptance: unit tests green;
  `actor_create_with_unregistered_type_returns_type_not_registered` still
  green (Strict default).

**Exit Criteria**: Tier 1 gate green with the new tests.

### Phase 2: Integration

**Goal**: Transports pinned, contract recorded, nightly green.

**Tasks**:

- [ ] CLI passes `BaseOnly`; e2e test for the smoke.sh:33 shape (depends
  on: Phase 1) - Acceptance: local repro command succeeds with warning on
  stderr (Option A).
- [ ] REST policy per Phase 0 finding + e2e/conformance pin (depends on:
  Phase 1) - Acceptance: REST POST with unregistered type behaves per
  decision and the `warnings` array carries the new code when lenient.
- [ ] smoke.sh + smoke.ps1 scenario for unregistered-type create; mirrors
  edited in the same commit (depends on: CLI task) - Acceptance: scenario
  asserts the chosen behavior; under Option B this is where smoke.sh:33
  gains `type install project`.
- [ ] CHANGELOG entry + `building-apps.md` note documenting the contract
  (depends on: all above) - Acceptance: behavior delta from v0.2.5 is
  either "none (restored)" or a documented migration note.

**Exit Criteria**: Next nightly run is green; no interface differs from
the recorded contract.

## Test Strategy

### Critical Scenarios

- **Happy path**: CLI create with unregistered type (Option A) → Expected:
  base doogat created, `UNREGISTERED_TYPE_BASE_ONLY` warning on stderr,
  exit 0.
- **Edge case**: GraphQL `createDoogat` with unregistered type → Expected:
  `TYPE_NOT_REGISTERED` in `extensions.code` (unchanged; pinned by the
  existing actor test).
- **Edge case**: CLI create with a *registered* type → Expected: strict
  typed pipeline unchanged (validation, defaults, zone routing).
- **Error case**: `Strict` policy + unregistered type → Expected:
  structured `TYPE_NOT_REGISTERED`, no partial writes.

## Risks

- **Policy field becomes a parity loophole**: future features could abuse
  `BaseOnly` to skip validation. Mitigation: the enum is specific to
  unregistered-type handling, documented as the D-CLI-1-style maintainer
  decision it implements; reviewers treat new `BaseOnly` callers as a
  compatibility decision.
- **Option B chosen but escape hatch skipped**: scripted quick-capture
  workflows break at v0.2.6. Mitigation: the Option B task list forces the
  migration note and smoke updates; release notes call it out.
- **REST archaeology inconclusive**: if pre-00147 REST behavior cannot be
  established, default REST to `Strict` (its typed-create is `Specialized`
  / deferred per D-REST-1 anyway) and document it.

## Transport Impact

- **CLI**: behavior restored (A) or deliberately changed with migration
  note (B); e2e + smoke pin it.
- **GraphQL**: no change; strict rejection re-pinned by existing actor
  test.
- **REST**: policy set per Phase 0 verification; pinned by e2e; lenient
  branch surfaces the warning via the existing `warnings` array.
- **PgWire**: unaffected — SQL `INSERT` targets a table; unregistered
  tables fail as missing tables today (no contract change).
- **FFI**: unaffected — `DoogatDriver.create` is the raw-content path;
  typed FFI create already follows the registered-typedef chokepoint.
- **NoSQL HTTP**: read-only surface; not applicable.
