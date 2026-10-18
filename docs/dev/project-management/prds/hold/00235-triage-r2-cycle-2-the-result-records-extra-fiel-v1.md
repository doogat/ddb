---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 9fb752b66a6a
source_prd: 00177-ffi-crud-through-contract-v1.md
severity: high
---

# Triage: R2 (cycle 2): the result records extra field is not lossless and nothing docu...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, cycle `2`, consensus `2/3`, raised against `00177-ffi-crud-through-contract-v1.md`.

Issue: R2 (cycle 2): the result records extra field is not lossless and nothing documents the limit - a frontmatter map becomes FieldValue::List([]) and nested list/map items are dropped with only a tracing::warn!, so update_structured reports a wrong value rather than an omitted one.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `9fb752b66a6a`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `9fb752b66a6a` from `00177-ffi-crud-through-contract-v1.md` until a human triages it
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
