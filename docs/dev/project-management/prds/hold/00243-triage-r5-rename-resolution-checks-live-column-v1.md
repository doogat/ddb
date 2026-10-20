---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 7cf23a1d0516
source_prd: 00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md
severity: high
---

# Triage: [R5] Rename resolution checks live column names before replaying rename histo...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, cycle `2`, consensus `1/3`, op_id `9e10ac6d43d5-dd5`, raised against `00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md`.

Issue: [R5] Rename resolution checks live column names before replaying rename history, so a reused name probes the wrong data (b->c, then a->b, then ADD UNIQUE(b) checks original b instead of original a); renames must be replayed backwards chronologically, with a regression test (ddb-core/src/sql_engine/ddl/unique_ops.rs:53)

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `7cf23a1d0516`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `7cf23a1d0516` from `00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md` until a human triages it
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
