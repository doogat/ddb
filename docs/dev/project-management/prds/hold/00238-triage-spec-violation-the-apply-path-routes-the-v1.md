---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: feef48e98855
source_prd: 00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md
severity: 🟠
---

# Triage: Spec violation: the apply path routes the new ops through new public raw-SQL...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `deferred_decision`, cycle `1`, op_id `9e10ac6d43d5-dd0`, raised against `00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md`.

Issue: Spec violation: the apply path routes the new ops through new public raw-SQL grammar (`ALTER TABLE x ADD UNIQUE (...)` and `DROP UNIQUE (...)`). The PRD says PgWire has no SQL surface for this and that the constraint execution is internal. These statements are now intercepted by `try_custom_ddl`, so they are reachable from `ddb query`, GraphQL `executeSql` and PgWire. A raw `DROP UNIQUE` skips the `allow_destructive` gate that only `apply_schema` enforces. `docs/src/technical/sql-engine.md` and the CHANGELOG describe it only as apply output, not as user-callable SQL.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `feef48e98855`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `feef48e98855` from `00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md` until a human triages it
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
