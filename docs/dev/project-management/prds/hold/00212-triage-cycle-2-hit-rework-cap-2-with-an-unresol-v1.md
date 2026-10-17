---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 09ff992fa72f
source_prd: 00168-bundle-import-conflict-recovery-v1.md
severity: critical
---

# Triage: Cycle 2 hit rework_cap 2 with an unresolved CRITICAL: import_bundle merges th...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `stall`, site `cap_critical`, op_id `c389f89aed18`, raised against `00168-bundle-import-conflict-recovery-v1.md`.

Detail: Cycle 2 hit rework_cap 2 with an unresolved CRITICAL: import_bundle merges the hardcoded refs/remotes/bundle/master rather than the refs this bundle fetched, so with a stale bundle ref present (the state the cycle-1 CRITICAL fix now creates) importing a different bundle reports success while importing the OLD bundle's data and leaving the requested commits unreachable. Reproduced by Blake and independently confirmed by the orchestrator (/tmp/ddb-ffprobe-a, git 2.55.0). Same root cause as 2 of the 3 HIGHs; adding + to the fetch refspec is NOT the fix (it reintroduces the cycle-1 CRITICAL). See dev/local/reviews/00168-bundle-import-conflict-recovery-v1-review-02.md.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `09ff992fa72f`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `09ff992fa72f` from `00168-bundle-import-conflict-recovery-v1.md` until a human triages it
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
