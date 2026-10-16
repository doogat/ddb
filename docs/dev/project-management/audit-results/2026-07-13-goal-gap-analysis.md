# Goal-gap analysis — what is still missing to reach ddb's goal

Date: 2026-07-13. Method: three parallel read-only sweeps (goal docs, all 29 backlog PRDs, code/CI/test surface) cross-checked against the 2026-07-12 purpose-fit critique and the 2026-07 roadmap. No code was changed.

> **Numbering note (added 2026-07-16):** this document predates the 2026-07-16 renumber (harness PRD inserted at 00171; former 00171-00198 shifted +1). PRD numbers below are pre-shift — current slugs: G1 → **00193**-pgwire-ffi-conformance-legs, G3 → **00194**-ffi-bindings-ci (sequenced after **00191**-ffi-git-sync), G5 → blockquote on **00180**-server-security (corrected 2026-07-16: /ws auth already enforced, regression test only).

> **Status (2026-07-13, maintainer chose "proof PRDs + scope note"):** G1 → PRD 00192 (pgwire-ffi-conformance-legs); G3 → PRD 00193 (ffi-bindings-ci; sequenced after 00190, not before — numeric-order contract has no free slot and 00190 hand-builds during its own execution); G5 → scope-addition blockquote on 00179; G4 → "V1.0 Finish Line" entry in design-decisions.md (commit 3545ff3), which also makes G2's jink-glue measurement a v1.0 acceptance item. Remaining items closed same day (commit 842ceb7): G6 → deferred post-v1 as a Known Limitations entry in design-decisions.md, with the git-native inspection path documented in the sync guide ("Inspecting What a Merge Did"); G7 → "Backup and Restore" section in `docs/src/guide/sync.md` (restore = clone + register-node + reindex; `ddb node retire` for dead devices); G8 → Non-Goals row in design-decisions.md (foreign importers are downstream-owned; batch create verbs are the platform's part); hygiene → AGENTS.md jink-feedback pointer reworded to machine-local/conditional (C2 memory miscount was fixed earlier). ALL GAPS HANDLED.

## The goal, restated

Standalone platform for MANY downstreams (jink first, not the product), on three pillars: (1) markdown-as-a-database, (2) many client surfaces — six transports, all Guaranteed full-CRUD, 120% parity, no specialization exceptions, (3) distributed thanks to git, mobile included (full replica model). Roadmap end state: **Safe / Coherent / Consumable** (`dev/local/audit-results/roadmap-2026-07.md:13-16`).

## Verdict

**The backlog builds the goal; almost nothing proves the goal.** All nine data-safety/coherence invariants (I1-I9) map to backlog PRDs, and the four purpose-fit gaps were materialized into 00189-00191 plus scope notes. What remains missing is mostly the *verification layer*: after draining 00163-00191, ddb still could not demonstrate that parity holds on all six transports, that "consumable" delivered its promised jink-glue deletion, or that the mobile distribution artifacts even build. Plus one untracked security hole and a handful of decisions/docs.

---

## A. Untracked gaps (not covered by any backlog PRD)

### G1 — Parity has no proof for PgWire and FFI (highest leverage)

- The conformance harness (`tests/e2e/conformance/`, from PRD 00148) compares CLI↔GraphQL today (`cross_driver_crud.rs`, `cross_driver_validation.rs`; only `driver_cli` + `driver_graphql` exist). The backlog extends it: REST driver (00174), search leg (00175), typed fields (00177), error parity (00172), quarantine (00180), NoSQL leg + "parity failures block" gate (00191). End of backlog: **4 of 6 transports compared**.
- FFI is pinned only by interface-local tests; PgWire only by scattered hand-written checks (`tests/integration.sh` §54.D singleton parity, e2e `pgwire` modules). Harness docs mark PgWire "Specialized/internal" — that label contradicts the 2026-07-13 decision "NO specialization exceptions anywhere" (design-decisions.md).
- AGENTS.md's own rule: "Do not call interfaces equivalent unless the same downstream workflow passes conformance tests on each." By that rule, "120% parity" stays unfalsifiable for 2/6 transports even after the backlog drains.
- Cheapest close: one PRD after 00191 adding a PgWire driver (simple-protocol client) and an FFI driver (link ddb-core into ddb-e2e) to the existing harness.

### G2 — "Consumable" is never validated; the goal's only measurable signal has no owner

- The roadmap's single measurable success signal: jink retires ~2,400 backend + ~1,250 frontend LOC after 00184-00188 (`roadmap-2026-07.md:93-107`). No PRD anywhere migrates jink onto ddb-client/codegen/structured inputs and measures the retirement. Phase 3 success will be claimed by ledger, never demonstrated.
- Also: the end state says "a **published** client crate", but 00184 builds a workspace-internal crate (exit criteria: "jink can depend on it") — no crates.io, no registry. "Published" is at best ambiguous, at worst unmet by scope.
- Cheapest close: a jink-adoption acceptance step (tracked here even though the work lands in jink's repo) + an explicit decision on what "published" means for v1.

### G3 — Distribution pillar: mobile artifacts are never built or tested by CI

- No workflow references uniffi/xcframework/android/swift/kotlin; bindings are never generated, tested, or shipped in CI. Releases publish only 4 desktop CLI binaries. The Swift/Kotlin surface can silently rot, and 00190 (mobile replica — the moat) will hand-build with local scripts and local toolchains only.
- No published consumption path for mobile downstreams (SwiftPM/CocoaPods/Maven all absent and untracked); acceptable for jink, not for "many downstreams".
- Cheapest close: a CI job that generates bindings and builds the XCFramework/.aar (publish can wait; building catches rot). Natural rider on or before 00190.

### G4 — No finish line

- No v1.0 criteria, no metric- or version-gated definition of "goal reached" anywhere. Success is implicitly "drain 00163-00191 + flip I1-I9 to HOLDS".
- Cheapest close: a half-page doc: v1.0 = I1-I9 HOLDS + conformance green on 6/6 transports + jink migrated (LOC ledger settled) + mobile replica demo on a real device. Makes "done" checkable instead of vibes.

### G5 — `/ws` (GraphQL subscriptions) bypasses bearer auth, and 00179 doesn't cover it

- `ddb-server/src/lib.rs:163-165` merges `ws_routes` and `health_routes` *after* the auth middleware; subscriptions stream data with no token check at the HTTP layer. Verified 00179 (the security-hardening PRD) contains zero mentions of ws/subscription/health (control: "auth" appears 18×).
- `/health` unauthenticated is conventional; `/ws` is not — it matters in exactly the non-loopback scenario 00179 hardens.
- Cheapest close: scope note on 00179 (auth on upgrade or `connection_init` token check).

### G6 — Conflict resolution will be deterministic but still invisible

- After 00165/00166, CRDT resolution is reproducible — and still silent. Surfaces expose only counts (00168 conflict count, 00190 sync report). No user can review *what* was auto-resolved; the named failure mode is "quiet corruption", and determinism removes the randomness, not the quietness. Git merge commits keep both parents, so the data to reconstruct a conflict journal already exists.
- Cheapest close: a decision first (is a reviewable conflict log wanted?), then a small PRD (`ddb log --conflicts <id>` reconstructed from merge commits).

### G7 — Backup/restore story is undocumented

- "Git is the backup" is true and written nowhere; no restore runbook (clone → reindex → re-register node?). Docs-only fix (section in building-apps.md or sync.md).

### G8 — Foreign bulk import: no tooling, no decision

- Nothing imports from other systems (bundle import is ddb↔ddb). Post-00170 the mint ceiling stops blocking it; GraphQL `create_many` exists. Either declare "importers are downstream's job" as a non-goal in design-decisions.md, or add a small `ddb import` PRD later. Currently it's neither scoped nor rejected.

## B. Conscious deferrals that still gate the goal (re-surface at the right time, don't re-litigate)

- **PgWire extended/prepared protocol is a placeholder** (`pgwire.rs:311-319`). Most real Postgres drivers (psycopg, JDBC, npgsql) default to the extended protocol, so today only simple-protocol clients (psql) can talk to ddb. Deferred in the roadmap ("parameterized SQL over transports"), but it caps the "many client surfaces" pillar for the entire PgWire audience. (Inferred from code; not tested against a live driver.)
- **Incremental indexing beyond ~5K doogats** deferred — PKM-scale downstreams cross 5K quickly; the platform claim eventually collides with this ceiling.
- **In-process TLS** is a non-goal (proxy recommended) — fine, but interacts with G5 and with any future hosted-thin-client mode.

## C. Hygiene (minutes, not PRDs)

- AGENTS.md points to `dev/local/specs/jink-feedback/` (incl. `run-all.sh`) — **the directory does not exist** (nor does `dev/local/specs/`). The "independent verification channel" for SQL regressions is gone; restore it or drop the reference.
- Project memory said "28 PRDs remain: 00163-00191" — the range holds **29** files; memory also pointed at `dev/local/prds/roadmap-2026-07.md` while the roadmap actually lives in `dev/local/audit-results/`. (Both fixed in memory on 2026-07-13.)

## What is NOT missing (verified — don't re-audit)

- All nine invariants tracked to specific backlog PRDs; purpose-fit critique fully materialized (00189-00191 + scope notes + design-decisions commit 4368e9d).
- Six transports genuinely exist in code; REST is real and full-CRUD (`ddb-server/src/rest.rs`), not an alias of the NoSQL routes.
- Auth exists where claimed: bearer on GraphQL/REST/NoSQL/attachments, MD5-token on PgWire; default bind is loopback.
- Downstream docs are substantial (76 KB `docs/src/guide/building-apps.md` + mobile-constraints + technical/conformance pages).
- CI is strong for the core: clippy `-D warnings`, cargo-deny, 70% coverage gate, nightly full validation, macOS/Windows jobs, tag-gated release pipeline, maintained CHANGELOG.

## Recommended order

1. Fold G5 (`/ws` auth) into 00179 now — scope note, no new PRD.
2. Write the finish line (G4) — half a page, makes everything else rankable.
3. One new PRD: PgWire + FFI conformance legs (G1), sequenced right after 00191.
4. One new PRD or CI task: bindings build in CI (G3), before/with 00190.
5. Track jink migration + LOC measurement (G2) as the v1 acceptance step.
6. Decisions when convenient: conflict journal (G6), import posture (G8); docs: backup runbook (G7); hygiene fixes (C) anytime.
