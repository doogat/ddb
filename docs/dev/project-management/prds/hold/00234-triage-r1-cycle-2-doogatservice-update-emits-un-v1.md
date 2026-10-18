---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 5dbab45b28a4
source_prd: 00177-ffi-crud-through-contract-v1.md
severity: high
---

# Triage: R1 (cycle 2): DoogatService::update emits UNREGISTERED_TYPE_BASE_ONLY on ever...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, cycle `2`, consensus `3/3`, raised against `00177-ffi-crud-through-contract-v1.md`.

Issue: R1 (cycle 2): DoogatService::update emits UNREGISTERED_TYPE_BASE_ONLY on every transport (CLI stderr, GraphQL/REST warnings), ungated by any policy, while the PRD Transport Impact says CLI/GraphQL/REST are unchanged. No CHANGELOG entry, docs still call the warning create-only, no service-level or non-FFI test pins it.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `5dbab45b28a4`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `5dbab45b28a4` from `00177-ffi-crud-through-contract-v1.md` until a human triages it
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
