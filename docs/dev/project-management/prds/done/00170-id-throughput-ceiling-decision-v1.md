---
design: skip
---

# Decide the ID throughput ceiling and format

**Priority:** P1 · **Roadmap:** Phase 1 (decision spike) · **Depends on:** 00164 (build on the unified ID mint path). (`design: skip`: the Phase 0 blast-radius research IS the design work; a separate design doc would duplicate it.)

## Overview

This is a **decision / spike PRD**. Its deliverable is a recorded, justified decision plus a `design-decisions.md` entry, and — only if the decision is to change the ID format — a scoped follow-up implementation PRD stub. It is not a large code change.

**Maintainer pre-decision (2026-07-13, purpose-fit critique discussion) — SUPERSEDES the delegation default below.** The outcome is a **format change**: minting moves to UTC with a node discriminator so two devices can never mint the same ID. The framing is identity correctness, not throughput — cross-device collision (and the local-time DST fold) is the defect; the lifted ceiling is a side effect. Option (a) "keep" is off the table. The spike's remaining job: pick the exact shape within that direction — (b)-style suffix (node/counter, lexical-sort-preserving, 14-digit human-readable prefix retained) vs (c) HLC-derived — design wikilink backward compatibility and migration, record it in `design-decisions.md`, and emit the complete, create-prd-compliant follow-up implementation PRD into `dev/local/prds/hold/` for maintainer review (Phase 2 is no longer conditional; `hold/` keeps the format change out of the unattended batch — decided at the 2026-07-13 backlog review). The session still runs Phase 0 blast-radius research and commits the decision autonomously.

~~The maintainer has delegated this decision to the autopilot session.~~ (Original default retained for history: absent evidence, keep (a) second-resolution and document the ~1 create/sec/process ceiling. Overruled above.)

### Problem Statement
Create throughput is hard-capped at roughly one doogat per second per process, and the ID that enforces that cap is also the public contract, so any change ripples across every interface and every stored wikilink.

- **The mint spin-waits on the wall clock.** `generate_unique_id` (`ddb-core/src/parser/mod.rs:933`-`:947`) holds a global `static LAST: Mutex<String>` (`:935`), formats a candidate from `chrono::Local::now()` at second resolution (`:940`), and, when the candidate equals the last-minted ID or already exists, `std::thread::sleep(std::time::Duration::from_millis(100))` (`:945`) and loops until the wall second advances. Net effect: about one create per second per process. Post-00164 (verified at `d77ad58`): the clock is unchanged, but the entry points are now the unified repo-aware oracle — `id_minting::existence_oracle` (`ddb-core/src/id_minting.rs:22`) consumed by `DoogatService::unique_id` (`service/write_helpers.rs:244`), `SqlEngine::unique_ids` (`sql_engine/mod.rs:126-150`, which sidesteps the sleep for batches by +1s stepping from one seeded mint), and `SyncManager::reassign_single_loser` (`sync_manager/mod.rs:694`) — so the ceiling now bites per call-site rather than per-row in SQL batch INSERT.
- **The ID is second-resolution and public.** The 14-digit `YYYYMMDDHHmmss` value is embedded in filenames (`ddb/{id}.md`) and in wikilinks (`[[20260226120000]]` — illustrative ID, not a citation, link-ok: example); `docs/src/architecture/design-decisions.md:36` ("## ID-Only Filenames") documents the tradeoff, noting at `:40` that filenames never change so wikilinks stay stable.
- **An HLC exists but is not used for identity.** `ddb-core/src/hlc.rs` provides the `Hlc` value type and, post-00166, a machine-local persisted `HlcClock` lives at `ddb-core/src/git_ops/hlc_clock.rs` (state in `.git/ddb-hlc`) stamping trailers on every write — but identity is still minted purely from the wall clock, not the HLC.
- **Three candidate resolutions, each with a different cost.** (a) keep second-resolution and document the ~1/sec/process cap as a permanent product ceiling; (b) add a sub-second suffix or counter to the ID, preserving lexical sort, wikilink stability, and backward compatibility; (c) derive identity from the HLC. The ID's role as the public contract means the wrong choice is expensive to reverse, so the choice must be made deliberately, not drifted into.

### Target Users
Downstream apps (jink) or any bulk importer that needs to create more than one doogat per second. Today they cannot, and they have no documented statement of whether that ceiling is permanent. This PRD gives them either a lifted ceiling or an explicit, justified "no".

### Success Metrics
- A recorded, justified decision selecting exactly one of the options below, with the blast radius of each documented.
- The decision names the chosen option, the rejected alternatives, and the single deciding factor, so a future reader sees not just what was chosen but why the others lost.
- The throughput characteristic is stated as a number (creates per second per process) under the chosen option, not left qualitative.
- `docs/src/architecture/design-decisions.md` updated to reflect the decision, recording the identity-correctness framing (UTC mint + node discriminator), the chosen shape, and why the rejected "keep" option lost.
- A complete, create-prd-compliant follow-up implementation PRD exists in `dev/local/prds/hold/` (NOT `backlog/` — it must pass `/review-prd-backlog` before any autopilot batch picks it up), carrying the transport-conformance and migration work; this spike ships no format change itself.

## Functional Decomposition

### Capability: ID-ceiling decision
#### Feature: Blast-radius research of each option
- **Description**: characterize what each candidate does to filename stability, wikilink rewriting, sort assumptions, sync collision behavior, and downstream ID parsing (including jink).
- **Inputs**: the three options — (a) keep second-resolution; (b) sub-second suffix/counter; (c) HLC-derived identity — plus the current mint, filename, and wikilink code.
- **Outputs**: a written comparison of each option against each blast-radius axis.
- **Behavior**: trace each axis through the code and docs, recording per option:
  - *Filename stability* — does the ID length/shape stay compatible with `ddb/{id}.md`?
  - *Wikilink rewrite* — do stored `[[<id>]]` links stay resolvable, or is a migration forced?
  - *Lexical-sort assumptions* — does 14-char lexical order still equal chronological order?
  - *Sync collision behavior* — how do two devices minting concurrently avoid collisions?
  - *Downstream ID parsing* — do consumers (incl. jink) that parse the 14-digit shape still work?

#### Feature: Recorded decision + design-decisions entry
- **Description**: choose one option with rationale and record it.
- **Inputs**: the blast-radius comparison.
- **Outputs**: a decision statement + an updated `design-decisions.md`.
- **Behavior**: pick the option, write the rationale, and update the "ID-Only Filenames" section (or add an adjacent entry) to reflect the decision and, if "keep", the documented throughput ceiling.

#### Feature: Follow-up implementation PRD (to hold/)
- **Description**: scope the pre-decided format change as a separate, complete PRD held for maintainer review.
- **Inputs**: the chosen format shape.
- **Outputs**: a full, create-prd-compliant PRD in `dev/local/prds/hold/` with the transport + migration plan.
- **Behavior**: the PRD must name the new ID shape, the wikilink backward-compatibility rule (old `[[<id>]]` links stay resolvable), the reindex/migration path, and the per-transport conformance obligations. It lands in `hold/` so the unattended loop never drains it unreviewed; the maintainer promotes it to `backlog/` via `/review-prd-backlog`.

## Structural Decomposition

This spike edits documentation and (conditionally) adds a PRD stub. It changes no runtime module. The files it reads to assess blast radius:

```
ddb-core/src/parser/mod.rs          # Read: generate_unique_id (the ~1/sec mint)
ddb-core/src/hlc.rs                 # Read: HLC — candidate identity source for option (c)
docs/src/architecture/design-decisions.md  # Write: record the decision ("ID-Only Filenames")
dev/local/prds/hold/                # Write: follow-up implementation PRD (maintainer reviews + promotes via /review-prd-backlog)
```

### Module: docs/architecture (docs/src/architecture/design-decisions.md)
- **Maps to capability**: ID-ceiling decision
- **Responsibility**: hold the recorded decision and, if "keep", the documented throughput ceiling as a known limitation.
- **Exports**: the updated "ID-Only Filenames" decision entry (the only guaranteed write of this PRD).

### Module: parser + hlc (ddb-core/src, read-only in this spike)
- **Maps to capability**: ID-ceiling decision (evidence)
- **Responsibility**: the current wall-clock mint (`generate_unique_id`) and the candidate HLC identity source, both read to assess blast radius.
- **Exports**: none changed by this PRD. Any change to these is deferred to the conditional follow-up implementation PRD.

### Module: prds/hold (dev/local/prds/hold/)
- **Maps to capability**: ID-ceiling decision (handoff)
- **Responsibility**: hold the complete follow-up implementation PRD for maintainer review (autopilot never reads `hold/`).
- **Exports**: the follow-up PRD (the second of this spike's two guaranteed writes, alongside the design-decisions entry).

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **Blast-radius research**: reads the mint, filename, wikilink, and sync code; independent.

### Core Layer (Phase 1)
- **Decision + design-decisions entry**: Depends on [Blast-radius research].

### Integration Layer (Phase 2)
- **Follow-up PRD (to hold/)**: Depends on [Decision].

## Implementation Phases

### Phase 0: Foundation
**Goal**: understand the blast radius of each option.

**Tasks**:
- [ ] Research each option against filename stability, wikilink rewrite cost, lexical-sort assumptions, sync collision behavior, and downstream ID parsing (incl. jink), building on the 00164 unified mint path (no deps) - Acceptance: a written comparison covering all three options across every axis, citing the mint, filename, and wikilink code.
- [ ] Capture the comparison as a working note under `dev/local/` (the decision's evidence trail), one verdict per option per axis (no deps) - Acceptance: the note exists and every option has an explicit verdict on every axis, with the throughput number per option.

**Exit Criteria**: the tradeoffs of (a) keep, (b) sub-second suffix, (c) HLC identity are documented with per-axis verdicts.

### Phase 1: Core
**Goal**: decide and record.

**Tasks**:
- [ ] The session picks the exact format autonomously WITHIN the pre-decided direction (UTC + node discriminator; option (a) keep is overruled — see the 2026-07-13 note above), records the rationale, and updates `docs/src/architecture/design-decisions.md` (depends on: Phase 0) - Acceptance: the decision and rationale are recorded; the doc reflects it; a `CHANGELOG.md` entry notes the documented decision if user-facing.

**Exit Criteria**: a single decision is recorded and justified in `design-decisions.md`.

### Phase 2: Integration
**Goal**: hand off the implementation.

**Tasks**:
- [ ] Emit a complete, create-prd-compliant follow-up implementation PRD into `dev/local/prds/hold/` carrying the per-transport ID-format conformance + wikilink migration + backward-compatibility plan (and the AGENTS.md per-PRD deliverables: smoke/integration + e2e + docs + walkthrough). No longer conditional: the 2026-07-13 pre-decision makes a format change certain. `hold/` is deliberate — the unattended loop must not drain the format change before `/review-prd-backlog` clears it (depends on: Phase 1) - Acceptance: the PRD exists in `hold/`, follows the create-prd template top-to-bottom, and carries the transport/migration plan.

**Exit Criteria**: the decision is recorded and handed to the complete follow-up PRD in `hold/`.

## Test Strategy

### Critical Scenarios
This spike ships no runtime code, so it authors no runtime tests; its "tests" are the completeness of the recorded decision. Any test/scenario obligations belong to the follow-up implementation PRD (if any).

The decision-completeness check below stands in for a test suite: if any scenario's expected outcome is unmet, the PRD is not done.

- **Happy path**: the decision is recorded with rationale and `design-decisions.md` reflects it → Expected: a reader can tell which shape was chosen and why "keep" lost.
- **Edge case**: the chosen shape keeps the 14-digit human-readable prefix → Expected: the lexical-sort and wikilink axes explicitly verify compatibility; the follow-up PRD pins the rule.
- **Error case**: the decision is recorded but no follow-up PRD lands in `hold/` → Expected: the PRD is incomplete; Phase 2 exit is not met.

## Risks

- **Deciding without measuring reach**: choosing a format change without tracing wikilink/sort/sync impact would leak breakage into a follow-up. Mitigation: Phase 0 is mandatory and covers every blast-radius axis before the decision.
- **Backward compatibility of stored wikilinks**: any longer/reshaped ID must keep `[[<existing-id>]]` resolvable. Mitigation: make it an explicit decision axis; the follow-up PRD owns migration if the format changes.
- **Lexical-sort dependence**: code may assume 14-char lexical order equals chronological order. Mitigation: a sub-second suffix must preserve lexical monotonicity; the research task calls this out.
- **HLC identity vs. human-readable IDs**: an HLC-derived ID (option c) may no longer read as a plain timestamp, weakening the "ID is a legible date" property that filenames and wikilinks rely on. Mitigation: the research task weighs legibility as an explicit cost of option (c).
- **Multi-device collision under sub-second minting**: options (b) and (c) raise create rate, which stresses cross-device uniqueness. Mitigation: the research task documents each option's collision-avoidance story against the sync model before the decision.
- **Scope creep into implementation**: a spike must not start rewriting the mint. Mitigation: this PRD's only writes are docs + a conditional PRD stub.

## Transport Impact

The ID is a contract-level artifact: every interface that returns or accepts a doogat ID depends on its format, so the decision is cross-cutting. This spike, however, ships no per-transport code change; if the decision changes the format, the follow-up implementation PRD owns per-transport conformance.

- **CLI**: Intentionally absent (this spike) — surfaces IDs in create output and wikilinks; a format change would be owned by the follow-up PRD.
- **GraphQL**: Intentionally absent (this spike) — `id` fields return the contract value; decision-gated, no code change here.
- **REST**: Intentionally absent (this spike) — `id` in JSON payloads; decision-gated.
- **PgWire**: Intentionally absent (this spike) — the `id` column type/shape; decision-gated.
- **FFI**: Intentionally absent (this spike) — `DoogatId` in FFI records; decision-gated.
- **NoSQL HTTP**: Intentionally absent (this spike) — the document `id`; decision-gated.
- **Cross-interface conformance plan**: not applicable to this spike (it produces a decision, not code). If the decision changes the ID format, the follow-up implementation PRD must define and own a cross-interface ID-format conformance suite across CLI/GraphQL/REST/PgWire/FFI/NoSQL plus a wikilink-migration plan. jink impact: this decision determines whether jink (or any downstream) can ever bulk-create faster than ~1 doogat/sec/process.
