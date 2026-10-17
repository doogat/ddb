---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: d7183dec8c76
source_prd: 00174-decouple-maintenance-from-write-actor-v1.md
severity: high
---

# Triage: R2/R15 not resolved. `probe_overlap_with_burst_gets` computes an overlap flag...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, cycle `2`, consensus `2/4`, raised against `00174-decouple-maintenance-from-write-actor-v1.md`.

Issue: R2/R15 not resolved. `probe_overlap_with_burst_gets` computes an overlap flag, and the test discards it as `_observed_overlap`. The test therefore still passes against the pre-change single-actor code whenever compact finishes in milliseconds, which the doc comment itself admits. Burst GETs do not make this deterministic. A deterministic alternative needs no hook in `ddb-core`. Reuse `stalled_remote()` from `integration_sync_decoupling.rs`: start a `sync` that blocks the maintenance worker for the libgit2 timeout, assert `!sync.is_finished()`, issue the NoSQL GET, and assert it returns well before the sync ends. At minimum, assert the overlap flag, or rename the test so it does not claim to prove decoupling.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `d7183dec8c76`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `d7183dec8c76` from `00174-decouple-maintenance-from-write-actor-v1.md` until a human triages it
- **Exports**: none

### Dependencies
- triage: No dependencies (foundation)

## Tasks

### Phase 0: Foundation
- [ ] triage: promote to backlog or close - Acceptance: this file is no longer under docs/dev/project-management/prds/hold/

### Phase 1: Core
No implementation tasks until attended triage.

## Success Criteria

- This file is no longer under `docs/dev/project-management/prds/hold/`.
