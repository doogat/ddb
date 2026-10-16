---
catchup: force
default_model: opus
model_tier_rationale: invented contract plus concurrency - existed flag across four transports and a concurrent-create race
---

# Idempotent create: return-existing / upsert on unique conflict

**Priority:** P3 · **Roadmap:** Phase 3 (downstream enablement) · **Depends on:** 00177 (the FFI leg rides `create_structured`/`CreateResultRecord`, replacing the removed raw create_doogat) (soft: 00178 structured-fields-input is helpful but not required — it trims the returned create payload)

## Overview

### Problem Statement
Downstreams pay two avoidable round-trips on create. They catch `UNIQUE_VIOLATION` and then re-fetch the existing row, and they create-then-re-read to obtain a complete payload after follow-up writes.

Evidence (jink):

- `jink: src/graphql/mutations/category.rs:82-101` — creates a `category-membership`, and on `graphql_code() == Some("UNIQUE_VIOLATION")` falls back to `fetch_membership_via_ddb` (an idempotent "add" that then re-fetches). Two requests for one logical upsert.
- `jink: src/graphql/mutations/link.rs:253-265` — creates a link, attaches tags via a follow-up `updateDoogat`, then (because "the createDoogat payload's `tags` is stale") re-reads via `fetch_link_via_ddb`. A create-then-read dance for a complete row.

The direction is already sanctioned in ddb: `ddb-server/src/schema/mod.rs:38-43` (PRD 00129) added nested typed accessors specifically to "eliminate the round-trip jink does today through `links(where:{id:eq:...})` after every mutation." Return-existing closes the remaining `UNIQUE_VIOLATION`-catch case the accessor does not cover.

**Current state (this is an EXTENSION, not a new field):** `CreateCommand` already carries `on_conflict: ConflictAction` (`ddb-core/src/app_contract/commands.rs:33`), and `ConflictAction` (`ddb-core/src/types/doogat.rs:530-538` at `8fe51c9`) already has two variants — `Error` (default) and `Ignore` (documented "skip the insert and return the existing doogat when a unique constraint matches"). `service/create.rs` already reads `cmd.on_conflict` (`:75`). **GraphQL is already wired end-to-end (2026-08-14 grounding, re-verified 2026-09-05):** `createDoogat` and `createMany` both expose an `onConflict` arg today — declared `schema/mutations/operations.rs:48-51` (`createMany` at `:139,:174`; there is NO per-type create mutation), decoded via `schema/input.rs:81-90` (defaults `Error`, accepts `IGNORE`), enum registered `type_defs.rs:450` — so the GraphQL leg of this PRD is only the new enum variant + the `existed` field, not new arg plumbing. REST hardcodes `ConflictAction::Error` (`rest.rs:403`, no `CreateBody.on_conflict`), and CLI/FFI have no conflict arg at all — those three get the full wiring. What is missing everywhere is (1) any way for a caller to learn whether the row was created or already existed — there is no `existed`/`created` flag on the create result today, so `Ignore` returns the existing row silently and callers still re-read to disambiguate — and (2) a guarantee the returned row carries full typed fields. So the work is to EXTEND `ConflictAction` (add `ReturnExisting`, reconciled with the existing `Ignore`) and add an `existed` flag to the create result — NOT to add `on_conflict`, which already exists.

### Target Users
Downstream apps (jink) performing idempotent inserts (membership joins, tag attach, singletons) that want a duplicate create to return the existing row instead of erroring — with no second query.

### Success Metrics
- Creating a row that violates a unique/singleton constraint with `onConflict: RETURN_EXISTING` returns the EXISTING doogat (id + typed fields), with no error and no second query.
- Default create behavior is unchanged (still errors `UNIQUE_VIOLATION`/`SINGLETON_VIOLATION`) — the policy is additive and opt-in.
- The create response signals whether the row was created or already existed (an `existed` indicator — the name is pinned, see Risks), so callers never re-read.
- Covers jink's `UNIQUE_VIOLATION`-catch-then-refetch (`category.rs:82-101`) and create-then-read (`link.rs:253-265`) dances so jink can delete them (jink-side work, not verified in this repo).
- The upsert (`Update`) variant, if shipped, is an explicit maintainer decision recorded in the PRD; the MVP `RETURN_EXISTING` does not block on it.
- Default (no `onConflict`) create behavior and latency are unchanged; the opt-in policy adds no cost when unused.
- The `existed` signal appears identically on GraphQL, REST, FFI, and CLI, so callers branch on one field rather than per-transport shapes.

## Functional Decomposition

### Capability: Idempotent create
#### Feature: Extend ConflictAction with ReturnExisting + an existed flag
- **Description**: `on_conflict` already exists on the create AppCommand (`CreateCommand.on_conflict: ConflictAction`); extend its `ConflictAction` enum (today `Error`/`Ignore`) with `ReturnExisting`, and add an `existed` flag to the create result.
- **Inputs**: `on_conflict: Error (default) | Ignore | ReturnExisting` on the existing `CreateCommand.on_conflict`.
- **Outputs**: a create result carrying the doogat + a NEW `existed` flag (absent today).
- **Behavior**: `service/create.rs` already reads `cmd.on_conflict` (`:69`); add the `ReturnExisting` arm — on a unique/singleton conflict, resolve to the existing doogat with `existed=true` and the full typed payload, resolving identity inside the atomic core create path from the actual violated constraint. Ordinary UNIQUE_VIOLATION provides table/columns/values but no existing_id; singleton context can carry the id. Preserve the matched identity internally or resolve it from that actual constraint without assuming a missing public field. Reconcile with the existing `Ignore` variant: `Ignore` keeps its SQL-`DO NOTHING` semantics (return existing, minimal), while `ReturnExisting` guarantees the full typed row PLUS the `existed=true` signal for the idempotent-upsert use case. Document the distinction so the two "return existing" variants are not conflated; the `existed` flag is populated for both.

#### Feature: Return existing row with typed fields
- **Description**: the returned doogat carries its id + typed fields, ready to use.
- **Inputs**: the actual violated constraint metadata and a core-resolved existing identity, resolved atomically with create/conflict handling.
- **Outputs**: the full existing doogat.
- **Behavior**: fetch the existing row through the normal typed read path so the payload matches a fresh create's shape.

### Capability: Cross-interface exposure
#### Feature: GraphQL / REST / FFI / CLI conflict arg
- **Description**: each write-capable interface exposes the policy.
- **Inputs**: `onConflict` (GraphQL arg), `on_conflict` (REST `CreateBody` member / FFI `create_structured` param), `--on-conflict` (CLI flag).
- **Outputs**: the existing (or newly created) doogat + `existed` indicator.
- **Behavior**: thread the policy through each transport into the single create AppCommand — transports adapt the command/result, they do not own the policy.

### Capability: Conflict-policy variants
**Excluded:** ConflictAction::Update and upsert-by-unique-key updates are outside this PRD. Ship ReturnExisting without an implementation-time policy question.

#### Feature: Singleton conflict coverage
- **Description**: singleton creates resolve under the same policy.
- **Inputs**: a `SINGLETON_VIOLATION` on create with `RETURN_EXISTING`.
- **Outputs**: the existing singleton.
- **Behavior**: map singleton conflicts to return-existing so create-or-return needs no bespoke catch (generalizing jink 00023's config-singleton upsert).

#### Feature: Existed signal per interface
- **Description**: every interface's create result distinguishes created from returned-existing.
- **Inputs**: the resolution outcome.
- **Outputs**: an `existed` boolean on the response.
- **Behavior**: surface the flag on GraphQL/REST/FFI/CLI so no caller re-reads to disambiguate.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/
├── app_contract/                    # Maps to: extend ConflictAction (add ReturnExisting) + result.existed
├── service/create.rs                # Maps to: conflict detection → return existing
└── ffi/driver.rs                    # Maps to: create_structured on_conflict + existed on CreateResultRecord (raw create_doogat export removed by 00177)
ddb-server/src/
├── schema/mutations/operations.rs   # Maps to: createDoogat onConflict arg
└── rest.rs                          # Maps to: CreateBody.on_conflict member (a single file, not a directory)
ddb-cli/src/commands/crud.rs         # Maps to: ddb create --on-conflict flag
```

### Module: service/create (ddb-core/src/service/create.rs)
- **Maps to capability**: Idempotent create
- **Responsibility**: resolve a unique/singleton conflict under `ReturnExisting` to the existing doogat.
- **Exports**: the create path honoring `on_conflict`, returning `{ doogat, existed }`

### Module: app_contract (ddb-core/src/app_contract/)
- **Maps to capability**: Idempotent create
- **Responsibility**: extend the existing `on_conflict`/`ConflictAction` (add `ReturnExisting`) and add `existed` to the result — the single policy home.
- **Exports**: `ConflictAction` (now with `ReturnExisting`), create result with `existed`

### Module: create adapters and conflict types (ddb-core/src/types/doogat.rs, ffi/driver.rs, ffi/records.rs; ddb-server/src/schema/mutations/operations.rs, schema/input.rs, schema/type_defs.rs, rest.rs; ddb-cli/src/commands/crud.rs)
- **Maps to capability**: Idempotent create; Cross-interface exposure; Conflict-policy variants
- **Responsibility**: extend ConflictAction, expose return-existing consistently, and preserve every core result field and warning.
- **Exports**: ReturnExisting, existed, and the adapted create inputs/results.

### Module: capability publication (ddb-server/src/capabilities.rs)
- **Maps to capability**: Idempotent create
- **Responsibility**: publish idempotent_create from the existing 00186 registry and verify its declared interface/build scope.
- **Exports**: the registry entry and matching handshake fixtures.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **app_contract + service/create**: the conflict policy and its resolution live in the app contract, not the transports.

### Core Layer (Phase 1)
- **GraphQL / REST / FFI / CLI wiring**: Depends on [app_contract + service/create] - each transport adapts the command/result.

### Integration Layer (Phase 2)
- **conformance + docs**: Depends on [transport wiring].

## Implementation Phases

### Phase 0: Foundation
**Goal**: the policy + resolution in the app contract.

**Tasks**:
- [ ] `on_conflict` already exists on `CreateCommand` (`ConflictAction`: `Error` default / `Ignore`, `types/doogat.rs:530-538`); EXTEND `ConflictAction` with `ReturnExisting` and add an `existed` flag to the create result; implement the `ReturnExisting` arm in `service/create.rs` (which already reads `cmd.on_conflict` at `:75`), reconciled with the existing `Ignore` semantics — NO new `on_conflict` field is added. NOTE (2026-07-16 grounding, refs refreshed 2026-09-05): `create.rs:97-99` documents that `on_conflict` is NOT honored on the `create_doogat_with_extra` path and `:139-143` flags the `Ignore` skip as a known caveat — the `ReturnExisting` arm must cover that path too, with no exclusion for that public create path (no deps) - Acceptance: core unit tests — create-existing with `ReturnExisting` returns the existing id + typed fields and `existed=true`; `Ignore` still returns the existing row and now also sets `existed=true`; default `Error` policy still errors `UNIQUE_VIOLATION`; the create_doogat_with_extra path is covered under the same return-existing contract.
- [ ] Resolve conflict identity in the core create operation from the actual singleton/composite unique constraint, preserving the matched id or querying its values while the operation is serialized (depends on: conflict result types) - Acceptance: single and composite keys, several constraints, an ambiguous match, and a conflict appearing after pre-check are covered; ambiguity returns a structured error, returned rows match the actual constraint, and no public existing_id error field is assumed.

**Exit Criteria**: idempotent create resolves in core, unit-tested.

### Phase 1: Core
**Goal**: expose the policy on every write-capable interface.

**Tasks**:
- [ ] Wire the transports into the Phase 0 command: GraphQL needs ONLY the `ReturnExisting` enum value + the `existed` response field (the `onConflict` arg already exists — `operations.rs:48-51`, `input.rs:81-90`, `type_defs.rs:450`); REST gains `CreateBody.on_conflict` (today hardcoded `Error` at `rest.rs:403`); FFI gains `create_structured(on_conflict)` + `existed` on `CreateResultRecord` (the raw export was removed by 00177); CLI gains `ddb create --on-conflict return-existing` (depends on: Phase 0, 00177) - Acceptance: each interface returns the existing row on conflict with an `existed` indicator; per-interface e2e green.

**Exit Criteria**: all write interfaces honor the policy.

### Phase 2: Integration
**Goal**: conformance, scenarios, docs.

**Tasks**:
- [ ] Conformance: create-existing across GraphQL + REST returns the existing row (no error, one round trip); CLI + FFI interface-local e2e; default-errors regression pinned (depends on: Phase 1) - Acceptance: golden workflow passes on GraphQL + REST.
- [ ] Add e2e coverage in the Rust harness (`tests/e2e/`; the shell harness was removed by 00171): CLI `--on-conflict` and GraphQL/REST conflict scenarios; + a walkthrough + docs (`docs/src/guide/building-apps.md` CRUD baseline, `docs/src/technical/server.md`, `docs/src/technical/ffi.md`) + CHANGELOG (depends on: Phase 1) - Acceptance: e2e scenarios green; docs describe the opt-in policy + response shape. Include the `idempotent_create` registry addition and update exact-set plus health/GraphQL agreement fixtures; Acceptance also requires the flag to match compiled availability and the implemented workflow.

**Exit Criteria**: cross-interface conformance + scenarios + docs landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: `createDoogat(onConflict: RETURN_EXISTING)` on a duplicate unique key → the existing doogat (id + typed fields), no error, no second query.
- **Edge case**: `RETURN_EXISTING` with no conflict → a normal create with `existed=false`.
- **Error case**: default create (no `onConflict`) on a duplicate → still `UNIQUE_VIOLATION` (regression pin); `RETURN_EXISTING` on a type/field with no unique constraint → a documented `BAD_REQUEST` (decision pinned).

## Risks

- **Prior-art naming**: the singleton upsert path already returns `UpsertOutcome { id, created: bool }` (`ddb-core/src/service/create.rs:21-24`, set at `:341`/`:350`) and GraphQL `UpsertResult.created` (`schema/mutations/types.rs:136-141`). DECIDED (2026-09-05 backlog review): the new flag is `existed` on every transport, as specified throughout this PRD; the singleton-upsert `created` stays as is and the docs note the two names (`existed == !created`).
- **Which unique key to match**: a type may carry several unique constraints or a singleton. Mitigation: resolve against the constraint that actually fired (ordinary unique violations carry table/columns/values, so the core must resolve identity inside the same atomic operation); if genuinely ambiguous, error. Test the multi-constraint case.
- **Singleton overlap**: singletons are a conflict class too (`SINGLETON_VIOLATION`); jink 00023 already upserts its config singleton. Mitigation: map singleton conflicts to `ReturnExisting` as well; cover a singleton scenario.
- **Response-shape change**: adding `existed`/`created` to create responses is a public shape change. Mitigation: additive optional field; run the compatibility/deprecation review AGENTS.md requires for public response-shape changes.
- **NoSQL is read-only**: create-with-conflict cannot land on the NoSQL HTTP surface. Mitigation: NoSQL stays `Intentionally absent` and writes route through GraphQL/REST — this corrects the initial brief (which listed NoSQL as a target) to the house invariant that NoSQL HTTP has no write path.
- **Concurrent create race**: two `RETURN_EXISTING` creates of the same key can both miss a pre-read and one still hits `UNIQUE_VIOLATION`. Mitigation: resolve on the constraint error itself (catch-and-return-existing), not just a pre-check read; test the concurrent path.
- **Partial typed-create rollback**: a conflicting typed create must not leave a half-written base doogat before it resolves to the existing row. Mitigation: detect the conflict inside the transactional write and return the existing row atomically; test that no orphan row is left.

**Capability registry (depends on 00186):** add `idempotent_create` to the existing sorted registry only with this implemented surface; document the applicable interface/workflow and compile-time availability. Internal refactors and tests are not separate flags.

## Transport Impact

This is a cross-interface CRUD capability (not tooling), so it uses the standard promise labels.

- **GraphQL**: Guaranteed — `createDoogat(onConflict: RETURN_EXISTING)` returns the existing typed row.
- **REST**: Guaranteed — `POST /rest/doogats` with a `CreateBody.on_conflict` member returns the existing row in the `{ data }` envelope.
- **FFI**: Guaranteed — `create_structured(..., on_conflict)` returns the existing doogat with `existed` on `CreateResultRecord` (all public FFI creates now use the structured path).
- **CLI**: Guaranteed — `ddb create --on-conflict return-existing` prints the existing id + an existed indicator.
- **PgWire**: Specialized — SQL `INSERT … ON CONFLICT` already expresses upsert; the typed `onConflict` arg is not a target here.
- **NoSQL HTTP**: Intentionally absent — read-only by design; route creates through GraphQL/REST (deliberate correction of the brief to the codebase invariant).
- **Cross-interface conformance plan**: the golden workflow is create-existing across GraphQL + REST returning the existing row (no error, one round trip; the harness's REST driver lands in 00175); CLI + FFI are pinned by interface-local e2e; the default-errors path is a pinned regression so the policy stays strictly opt-in.
