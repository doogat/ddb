---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 6aa97bb3d336
source_prd: 00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md
severity: high
---

# Triage: cap 2 reached with 5 unresolved HIGH findings, all classified as regressions...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `stall`, site `cap_regression`, op_id `9e10ac6d43d5`, raised against `00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md`.

Detail: cap 2 reached with 5 unresolved HIGH findings, all classified as regressions introduced by this PRD's own rework: R1 c40ef4f, R2 784fce4, R3 784fce4, R4 784fce4, R5 34c5488

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `6aa97bb3d336`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `6aa97bb3d336` from `00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md` until a human triages it
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
