# PRD: Transport Adapter Thinning

## Status (updated 2026-06-09)

Not started, but the landscape moved under it. All hard dependencies are
complete (00141, 00142, 00146, 00147, 00148, 00150, 00151). **PRD 00143
closed 2026-06-09** (user decision: fold and close) with Phases 0-1 complete
and its leftovers folded into this PRD — see "Folded scope from PRD 00143"
under Implementation Phases. 00143's Phase 1 centralized the three
highest-drift glue items this PRD's Problem statement names:

- HTTP error mapping: `ddb-server/src/http_error.rs`, shared by REST and
  NoSQL HTTP.
- Schema-reload classification: `ddb-core/src/sql_engine/classify.rs`,
  shared by GraphQL `executeSql`/`executeBatch` and PgWire.
- GraphQL input decoding: `ddb-server/src/schema/input.rs`, used by all four
  dynamic mutations.

Additionally, PRD 00147 already routed the createDoogat workflow through the
app-command facade (`DoogatService::create` taking `CreateCommand` →
`AppOutput`) from GraphQL, CLI, **and** REST (the REST handler calls the
same actor command), with warnings surfaced on all three — which is exactly
this PRD's Phase 1 slice. Phase 1 below is therefore already satisfied;
the remaining substance of this PRD is Phases 0 (inventory refresh +
checklist reconstruction), 2 (error/warning centralization beyond the
create slice; legacy REST vocabulary removal D-01/D-03/D-08), and 3
(specialized adapters: FFI thinning, REST typed create, remaining
deprecated-shape retirements).

**Sequencing (added 2026-06-09): do not start this PRD before PRDs 00155
and 00156.** 00155 (fix-create-unregistered-type-regression) changes the
`CreateCommand`/facade contract this PRD thins toward, and nightly Tier 2
must be green before behavior-preserving refactors can be validated against
it. 00156 (split-oversized-core-modules) splits `service/crud.rs` (1845
lines), `schema/mutations.rs`, and `ffi.rs` — the exact files this PRD
edits; without the split, planning stalls on the 40K replan floor exactly
as PRD 00142 did (memory `project_large_files_block_replan`). An autopilot
PRD-selection turn that picks this PRD (lowest number) while 00155/00156
are still open should bounce it and pick those instead.

**Source-of-truth change:** the PRD 00150 artifacts this PRD's Compatibility
constraints gated on (`dev/local/notes/interface-deprecations.md`,
`interface-compatibility-inventory.md`) were deleted in a dev/local cleanup
and are unrecoverable (gitignored). The Compatibility constraints section
below has been rewritten against the surviving sources; the §3
release-readiness checklists and the §4 correctness-overrides list survive
nowhere and must be reconstructed in Phase 0.

## Problem

Transport adapters currently contain overlapping business decisions: input
decoding, validation, schema reload classification, response shaping, and
error mapping. This duplicates behavior across CLI, GraphQL, REST, PgWire,
FFI, and NoSQL HTTP and makes bug fixes slow.

State 2026-06-09: input decoding, schema-reload classification, and HTTP
error mapping are now centralized (PRD 00143 Phase 1). The duplication that
remains, and that this PRD owns: per-transport command routing (only the
create workflow goes through the PRD 00147 app facade), error semantics
outside the shared HTTP helper (CLI text/exit codes, FFI untyped string
errors D-02/D-05/D-06/D-07, PgWire D-PGWIRE-1), warning channels (FFI D-13
and PgWire D-12 have none; CLI's is unstructured text D-10), legacy REST
short-string error vocabulary removal (D-01/D-03/D-08), and the REST typed
create gap (REST `CreateBody` has no `fields` member — rest.rs:18-25 — so
typed create is not expressible over REST despite D-04/D-09 migration notes
claiming otherwise).

After the app contract and conformance harness exist, transports should become
thin adapters over shared application behavior.

## Origin

- App contract PRD: `00147-application-contract-model-v1.md` (complete; PRD
  file deleted with `dev/local/prds/done/` — summary survives in
  `dev/local/project-capsule.md` § Active Work)
- Conformance PRD: `00148-conformance-harness-v1.md` (complete; same note)
- Transport contract PRD: `00143-architecture-phase-4-transport-parity-v1.md`
  (closed 2026-06-09, in `done/`; Phases 0-1 complete, remaining Phase 2/3
  items folded into this PRD — see "Folded scope from PRD 00143")

## Submitted Context

- Do not refactor transports before the downstream promise matrix is known.
  (Satisfied: the matrix is published in `docs/src/guide/building-apps.md`.)
- GraphQL remains the near-term flagship network API.
- CRUD remains guaranteed for every public application interface.
  Protocol-specialized mechanics may remain only where they do not weaken
  public product capability parity.

## Goals

- Move transport-owned business policy into app/service code.
- Share input conversion, error/warning mapping, and schema-classification
  logic where behavior is promised equivalent.
- Keep public API behavior stable unless a compatibility PRD says otherwise.
- Reduce duplicated code paths that make bug fixes slow.
- Use conformance tests to prove behavior remains consistent.

## Non-goals

- Do not redesign all public APIs.
- Do not force REST, PgWire, FFI, or NoSQL HTTP to use GraphQL-shaped protocol
  mechanics, but do preserve public product capability parity.
- Do not use specialization to bypass baseline CRUD on public application
  interfaces.
- Do not replace server framework, CLI framework, UniFFI, or PgWire approach.
- Do not change storage or sync semantics.

## Do Not Build

- Do not move transport-specific auth/session/serialization concerns into the
  app layer.
- Do not keep business policy in a transport because it is convenient.
- Do not create hidden compatibility shims without documenting them.
- Do not thin a transport without conformance coverage or a documented
  specialized/absent promise label.

## Functional Decomposition

### Capability: Adapter policy inventory

The project knows which transport decisions belong in the app contract.

#### Feature: Duplicated policy map

- **Inputs**: GraphQL mutations, CLI commands, REST routes, PgWire handlers,
  FFI methods, NoSQL HTTP routes.
- **Outputs**: list of duplicated validation, input decoding, error mapping,
  warning behavior, schema reload, and response-shape decisions.
- **Behavior**: each item is marked app-owned, transport-owned, or
  intentionally specialized.

### Capability: Shared conversion helpers

Adapters translate into app commands and out of app results.

#### Feature: Input conversion

- **Inputs**: transport payloads and app command DTOs.
- **Outputs**: shared conversion helpers where multiple interfaces promise the
  same behavior.
- **Behavior**: validation belongs in app/service code unless the validation is
  purely transport syntax.

#### Feature: Error/warning formatting

- **Inputs**: app error envelope and app warnings.
- **Outputs**: transport-specific payload/status/exit-code formats.
- **Behavior**: adapters preserve app semantics and stable codes.

### Capability: Thin transport paths

Promised workflows delegate behavior to the app contract.

#### Feature: GraphQL flagship path

- **Inputs**: GraphQL create/update/search/SQL operations.
- **Outputs**: resolver code that converts requests to app commands and
  formats app output.
- **Behavior**: no resolver-specific business policy for migrated workflows.

#### Feature: CLI automation path

- **Inputs**: CLI commands for golden workflows.
- **Outputs**: command handlers that call app commands.
- **Behavior**: exit codes/stdout/stderr remain compatible.

#### Feature: FFI embedded path

- **Inputs**: FFI driver methods.
- **Outputs**: app command delegation with typed errors.
- **Behavior**: no panic paths or discarded warnings.

#### Feature: Specialized REST/PgWire/NoSQL paths

- **Inputs**: specialized workflow promises.
- **Outputs**: adapters that clearly delegate shared behavior and keep only
  specialized protocol concerns.
- **Behavior**: no false equivalence claims.

## Structural Decomposition

### Modules

- `ddb-server/src/schema/**`
  - Thin GraphQL resolvers over app commands.
- `ddb-cli/src/commands/**`
  - Thin command handlers over app commands.
- `ddb-core/src/ffi.rs`
  - FFI facade delegates to app commands and preserves typed errors/warnings.
- `ddb-server/src/rest.rs`
  - Specialized REST adapter over app commands where promised.
- `ddb-server/src/pgwire.rs`
  - SQL/reporting adapter over app SQL command where promised.
- `ddb-server/src/nosql_api.rs`
  - Specialized document adapter over app command or documented direct adapter.

## Compatibility constraints

Adapter thinning must respect the compatibility decisions recorded by PRD
00150. **Rewritten 2026-06-09:** the original source-of-truth artifact,
`dev/local/notes/interface-deprecations.md`, was deleted in a dev/local
cleanup and is unrecoverable (gitignored, never committed). What survives,
and what must be reconstructed:

- **Compatibility shims** (was §1, empty at deletion time — per the
  building-apps.md note "no shim entries exist as of this writing"). If a
  thinning task introduces a shim (old shape preserved alongside new during
  transition), record it with owner, removal condition, test reference, and
  doc snippet. With the registry gone, record shims in
  `docs/src/guide/building-apps.md` § Migration Notes (plus a CHANGELOG
  entry) before the thinning lands.
- **Deprecation entries D-01 through D-13** (was §2) — **survive** as
  per-entry Migration Notes in `docs/src/guide/building-apps.md`
  ("Compatibility and Deprecation"), with per-interface lists in
  `docs/src/technical/server.md` and `ffi.md`. These name the legacy
  response/error/warning shapes thinning may remove and the gating
  replacement paths. Any task that retires a deprecated shape must reference
  the matching D-NN entry there. Known defect: the D-04/D-09 notes overclaim
  that REST typed create shipped (see Problem); correct them when Phase 3
  touches REST.
- **Release-readiness and rollback criteria** (was §3) — **lost.** Phase 0
  must reconstruct the per-category checklists (response shape, error shape,
  warning shape) as a committed doc this time (suggested:
  `docs/src/technical/compatibility-checklist.md`), and re-derive the
  high-risk rows from the published promise labels since the inventory is
  also gone. One fragment survives: the warning-shape category required a
  conformance fixture asserting the GraphQL `WarningEntry` shape — that
  fixture exists (`tests/e2e/conformance/workflows.rs`,
  `warnings_shape_contract`, 6 structural assertions) and any warning-shape
  change must update it. Until the reconstruction lands, treat this minimum
  as the checklist for every high-risk shape change: (a) a conformance or
  integration fixture pins the new shape, (b) the matching Migration Note in
  building-apps.md is updated, (c) CHANGELOG entry in the same commit,
  (d) the change is revertable as a single commit and the revert path is
  named in the PRD task. Flag: this minimum is a reconstruction from
  surviving references, not the original §3 text.
- **Correctness fixes that override compatibility** (was §4) — the list is
  **lost**; the principle stands and is inlined here: silent-data-loss and
  invalid-write behaviors must be fixed even when a downstream consumer
  depends on them; thinning tasks must not preserve such behavior under a
  compatibility justification. Phase 0's inventory should re-flag any such
  cases it finds.

## Dependency Graph

All dependencies satisfied as of 2026-06-09:

- Depends on PRD 00146 downstream recovery readiness. **Complete.**
- Depends on PRD 00147 application contract model. **Complete** (AppOutput/
  AppError/CreateCommand facade; create workflow routed on GraphQL/CLI/REST).
- Depends on PRD 00148 conformance harness. **Complete** (CLI + GraphQL
  drivers; FFI/PgWire drivers explicitly deferred — a constraint on which
  slices can claim harness-backed conformance here).
- Depends on PRD 00150 compatibility/deprecation decisions. **Complete**,
  but its working artifacts were deleted — see Compatibility constraints for
  what survives and what Phase 0 must reconstruct.
- Depends on PRD 00141/00142 boundary work where adapter thinning needs the
  new service/foundation shape. **Complete** (QueryValue foundation;
  IndexPort/NoSqlMirrorPort injection).
- Coordinates with PRD 00143 transport consolidation. **00143 closed
  2026-06-09** with Phases 0-1 done (shared glue exists — do not re-plan it
  here); its leftover conformance-scenario expansion and CLI NoSQL-wording
  fix are folded into this PRD ("Folded scope from PRD 00143" below).
- Soft-depends on PRD 00155 (create-policy contract this PRD thins toward;
  nightly green) and PRD 00156 (file splits for crud.rs/mutations.rs/
  ffi.rs). Both should land first — see the Sequencing note in Status.

## Implementation Phases

### Folded scope from PRD 00143 (closed 2026-06-09)

PRD 00143 closed with Phases 0-1 complete; this PRD now owns its open items.
(00143's third leftover, the missing CHANGELOG entry for the NoSQL HTTP
error-status change, was resolved at close — commit 1e9cc50.)

- [ ] **CLI NoSQL help-wording fix** (execute with Phase 3). `ddb get` help
  says "Get doogat by ID via NoSQL index (O(1) lookup)"
  (`ddb-cli/src/main.rs:434`) but the handler calls `svc.read_doogat`
  (`commands/query.rs:63`); `ddb backlinks` help says "List backlinks via
  NoSQL index" (`main.rs:448`) but the handler calls `svc.backlink_ids` →
  SQLite `index.backlinks` (`service/discovery.rs:75`). `ddb scan` is
  accurate. Reword the help text (low-risk fix), or rewire to the `nosql_*`
  service methods — but note those error with "nosql not available" when the
  `nosql` feature is off while `get`/`backlinks` work in every build, so a
  rewire changes behavior for non-nosql builds and needs a new
  `nosql_backlinks` service wrapper (redb support exists,
  `ddb-core/src/nosql.rs:136`).
- [ ] **Conformance-scenario expansion** (execute with Phase 0 slice planning
  and per-slice Phases 2/3). From 00143 Phase 2: scenario classes for typed
  create, typed update, SQL DDL response shape, unique/not-null/FK errors,
  attachments, and search normalization are not in the harness (only
  `validation_error`, `crud_baseline`, `warnings_shape_contract` exist).
  Map these classes onto this PRD's slice plan; every workflow this PRD
  migrates gets harness coverage on its promised interfaces.
- [ ] **Baseline CRUD conformance for FFI and PgWire, or explicit downgrade**
  (tied to the FFI/PgWire-driver open question). From 00143 Phase 2/SC2:
  CLI/GraphQL are harness-covered and REST has `tests/e2e/rest.rs`; FFI and
  PgWire have only interface-local tests. Either land shared-shape coverage
  or downgrade/annotate the matrix labels before this PRD claims success.

### Phase 0: Inventory and slice selection

Tasks:

- [ ] Inventory transport-owned business policy. Acceptance: policy map covers
  all public interfaces.
  Note 2026-06-09: start from `docs/src/technical/server.md` § "Shared
  transport glue" + § "Thin adapters" (documents what is already centralized
  and the remaining transport-scoped inline logic) rather than from scratch;
  the original PRD 00150 inventory artifact is gone. CLI and FFI are the
  surfaces the landed consolidation did not touch.
- [ ] Reconstruct the release-readiness checklist as a committed doc and
  re-derive high-risk rows from the published promise labels (see
  Compatibility constraints — the §3/§4 content of the deleted
  `interface-deprecations.md` survives nowhere).
- [ ] Choose first slice from golden workflows. Acceptance: slice has
  conformance coverage or a concrete test plan.
  Note 2026-06-09: the create slice is already migrated (PRD 00147) and
  harness-covered; this task now picks the *next* slice (update and search
  are the natural candidates — update has harness coverage via
  `crud_baseline`).
- [ ] Prefer the CRUD baseline as the first thinning slice unless PRD 00146
  identifies a higher-risk downstream workflow.
  Note 2026-06-09: effectively satisfied — create (CRUD baseline) was the
  first slice, via PRD 00147.
- [ ] Identify compatibility risks. Acceptance: behavior changes are deferred
  to PRD 00150 unless urgent.
  Note 2026-06-09: PRD 00150 is complete; new behavior changes can no longer
  be "deferred to 00150" — they must follow this PRD's Compatibility
  constraints (deprecation note + fixture + CHANGELOG) or be dropped.

Exit criteria: thinning starts with a tested workflow.

### Phase 1: GraphQL and CLI slice

**Already satisfied by PRD 00147 + 00148 (annotated 2026-06-09)** — kept for
the record; do not re-implement. The next workflow slice falls under
Phase 2/3 with the slice chosen in Phase 0.

Tasks:

- [x] Route one GraphQL workflow through app command.
  Done by PRD 00147: `createDoogat` resolver → actor → `DoogatService::create`
  (`CreateCommand` → `AppOutput<ParsedDoogat>`), errors via
  `to_graphql_error_from_app`.
- [x] Route corresponding CLI workflow through app command where promised.
  Done by PRD 00147: CLI create routes through the same facade
  (`format_app_error`, warnings on stderr).
- [x] Run conformance check for the migrated workflow.
  Done: `crud_baseline`, `validation_error`, and `warnings_shape_contract`
  harness workflows run CLI + GraphQL in CI (`cargo test -p ddb-e2e`).

Exit criteria: first migrated workflow behaves consistently. **Met** (create
slice).

### Phase 2: Error/warning centralization

State 2026-06-09: partially pre-built. The AppError envelope with 14 stable
codes + `INTERNAL_ERROR` exists (PRDs 00147/00151) and serves the create
slice on GraphQL/CLI; REST and NoSQL HTTP share `http_error_response`
(unified code vocabulary, but mapped transport-side from `DoogatError`, not
from AppError). Warnings: GraphQL `extensions.warnings` (PRD 00154), CLI
stderr text (D-10), REST `warnings` array — all create-slice; FFI (D-13) and
PgWire (D-12) have no warning channel. Remaining here: extend app-level
error/warning semantics beyond the create slice, remove the legacy REST
short-string vocabulary (D-01/D-03/D-08), and decide the FFI/PgWire/CLI-JSON
channels (see Open Questions — each has a named candidate follow-up PRD).

Tasks:

- [ ] Move structured error semantics to app-level mapping.
  - Compatibility: Must satisfy release-readiness checklist for error shape (Compatibility constraints — reconstructed checklist).
- [ ] Preserve warnings through GraphQL/CLI/FFI for promised workflows.
  - Compatibility: Must satisfy release-readiness checklist for warning shape (Compatibility constraints — reconstructed checklist).
- [ ] Add tests for warning/error preservation.
  Note: build on the existing fixtures — `warnings_shape_contract`
  (conformance), integration.sh §56/§57 (+ .ps1 mirrors),
  `graphql_response_warnings_t154.rs`, `rest_response_warnings_t19.rs`,
  `http_error_t143.rs`.

Exit criteria: downstream error/warning scaffolding is reduced for migrated
workflows.

### Phase 3: Specialized adapters

State 2026-06-09: REST and NoSQL HTTP already delegate through the shared
helpers and the actor/read-pool (PRD 00143 Phase 3); PgWire uses the shared
classifier. The substance left here is FFI (still calls service methods
directly, drops warnings, untyped string errors), REST typed create
(`CreateBody` lacks `fields`; D-04/D-09 closure — also fix those two
overclaiming migration notes in building-apps.md when touching this), and
retiring the legacy shapes that D-01..D-13 gate on this PRD.

Tasks:

- [ ] Thin FFI, REST, PgWire, and NoSQL paths for workflows they promise.
  - Compatibility: Must satisfy release-readiness checklist for response shape (Compatibility constraints — reconstructed checklist).
  - Compatibility: Must satisfy release-readiness checklist for error shape (Compatibility constraints — reconstructed checklist).
- [ ] Document specialized behavior where direct app-command routing is not
  appropriate.
- [ ] Run conformance or specialized workflow checks.
  Note: FFI/PgWire harness drivers do not exist (PRD 00148 deferral) —
  either author them as part of this phase or use interface-local e2e with
  shared expected-shape assertions; do not claim harness-backed conformance
  without a driver.

Exit criteria: transports contain protocol concerns, not duplicated business
policy.

## Test Strategy

- Run conformance harness for every migrated guaranteed workflow.
- Keep existing smoke/integration/e2e tests green.
- Add focused unit tests for conversion helpers.
- Full Definition of Done commands for production changes.

## Success Criteria

1. Transport policy inventory exists.
2. CRUD baseline has a thinning plan across public application interfaces.
3. At least one golden workflow is thin across GraphQL and CLI.
   (Pre-met by the PRD 00147 create slice — for this PRD to add value, at
   least one *additional* workflow must be migrated; see Phase 0 slice
   selection.)
4. Structured errors and warnings come from app semantics for migrated
   workflows.
5. Protocol-specialized interfaces document protocol differences without
   weakening public product capability parity.
6. Conformance tests pass for migrated guaranteed workflows.
7. The folded PRD 00143 leftovers are closed: the CLI NoSQL help wording
   matches the implementation, and every public application interface either
   has baseline-CRUD conformance coverage or an explicit downgrade/
   specialization recorded in the capability matrix.

## Risks

- **Accidental behavior change**: thinning can change public behavior.
  Mitigation: conformance tests and PRD 00150 compatibility review.
- **Over-centralization**: protocol concerns can leak into app code.
  Mitigation: app code owns behavior; adapters own protocol framing.
- **Partial migration mess**: two patterns may coexist too long. Mitigation:
  each slice includes removal tasks for old paths.

## Open Questions

Both original questions are settled (annotated 2026-06-09):

- ~~Should GraphQL and CLI be the first thinning pair, or GraphQL and FFI?~~
  **Answered by events**: GraphQL + CLI (+ REST) via the PRD 00147 create
  slice. FFI is now the least-migrated surface and the natural Phase 3
  focus.
- ~~Which duplicated schema-reload path should move first?~~ **Moot**: PRD
  00143 Phase 1 moved GraphQL and PgWire onto the shared classifier in one
  step.

Current open questions:

- ~~Should PRD 00143's leftovers fold into this PRD so 00143 can close?~~
  **Answered 2026-06-09** (user: fold and close): yes — see "Folded scope
  from PRD 00143"; the CHANGELOG leftover was resolved at close (1e9cc50).
- The deprecation notes name three candidate follow-up PRDs as replacement
  gates: `ffi-typed-errors-v1` (D-02/05/06/07/13), `pgwire-structured-errors-v1`
  (D-12, extends PRD 00139 §T22), `cli-json-errors-v1` (D-10). Does this
  PRD absorb them as Phase 2/3 slices, or stay structural and leave them as
  separate PRDs? Absorbing widens scope considerably; leaving them means
  this PRD's "preserve warnings through FFI" task can only land the
  plumbing, not the public FFI surface.
- FFI/PgWire conformance drivers (deferred by 00148): author here or as a
  harness follow-up PRD before this one runs?
