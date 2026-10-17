---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 62043125cfc6
source_prd: 00167-sync-collision-reassignment-atomicity-v1.md
severity: high
---

# Triage: independent-resolution-test-can-false-green

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, topic `independent-resolution-test-can-false-green`, cycle `2`, consensus `1/5`, raised against `00167-sync-collision-reassignment-atomicity-v1.md`.

Issue: The independent-resolution test can false-green against the old wall-clock minter because both universes may resolve within the same second; the 5ms sleep only orders HLC commits. Assert each result's loser path equals the exact derive_content_id result from the known losing blob OID.

Detail: Raised by Bob (codex, doubt lens). Orchestrator-verified against sync_manager/tests.rs:1888-1914: add_add_collision_resolution_is_independent_of_push_direction asserts only (1) universe_1 == universe_2, (2) the winner keeps the colliding id, (3) some filename differs from the colliding id. It never pins the loser filename to a specific derive_content_id output, so the OLD wall-clock minter - which yields the same YYYYMMDDHHmmss in both universes whenever they complete inside the same second - passes all three assertions. The test therefore fails against the old code only when the two universes straddle a second boundary, i.e. non-deterministically. This is the very task (17) created to close the cycle-1 'no genuine two-node agreement test' finding, and it is weaker than its record claims. Mitigating: determinism IS pinned deterministically elsewhere - id_minting::derive_content_id's own unit tests (task 2) assert same-input-same-output and deterministic advance, and cycle 1 verified two live nodes deriving the identical loser id. So the PRD's acceptance criterion is met; this specific regression pin is just soft. Fix is cheap and surgical: assert the loser path equals derive_content_id(old_id, losing_blob_oid) explicitly in both universes.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `62043125cfc6`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `62043125cfc6` from `00167-sync-collision-reassignment-atomicity-v1.md` until a human triages it
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
