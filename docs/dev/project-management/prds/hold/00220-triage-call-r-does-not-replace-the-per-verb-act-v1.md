---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 232789639ed6
source_prd: 00172-collapse-actor-plumbing-v1.md
severity: high
---

# Triage: call<R> does not replace the per-verb ActorHandle methods; all 36 remain as t...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `settled-deferral`, cycle `1`, consensus `1/4`, raised against `00172-collapse-actor-plumbing-v1.md`.

Issue: call<R> does not replace the per-verb ActorHandle methods; all 36 remain as thin wrappers and every adapter still calls them (blind lens reads this as PRD metric non-compliance)

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `232789639ed6`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `232789639ed6` from `00172-collapse-actor-plumbing-v1.md` until a human triages it
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
