---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 0633d8b63c68
source_prd: 00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md
severity: high
---

# Triage: [R4] A column absent from SQLite is not data-free: existing documents can alr...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, cycle `2`, consensus `1/3`, op_id `9e10ac6d43d5-dd4`, raised against `00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md`.

Issue: [R4] A column absent from SQLite is not data-free: existing documents can already carry its frontmatter key, so adding that column plus uniqueness skips validation entirely and lets duplicates reach rematerialization after the git commit; effective document values must be validated pre-commit (ddb-core/src/sql_engine/ddl/unique_ops.rs:105)

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `0633d8b63c68`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `0633d8b63c68` from `00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md` until a human triages it
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
