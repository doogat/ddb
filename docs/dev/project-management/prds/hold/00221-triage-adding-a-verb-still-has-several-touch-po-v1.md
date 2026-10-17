---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 9c987601ea22
source_prd: 00172-collapse-actor-plumbing-v1.md
severity: high
---

# Triage: Adding a verb still has several touch points: a new return type needs entries...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `settled-deferral`, cycle `1`, consensus `1/4`, raised against `00172-collapse-actor-plumbing-v1.md`.

Issue: Adding a verb still has several touch points: a new return type needs entries in both no_emit_intent! and no_reload_intent!, plus a wrapper method, so up to three edits instead of one

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `9c987601ea22`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `9c987601ea22` from `00172-collapse-actor-plumbing-v1.md` until a human triages it
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
