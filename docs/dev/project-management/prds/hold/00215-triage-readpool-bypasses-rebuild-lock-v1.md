---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 342dfda56587
source_prd: 00169-poison-file-reindex-resilience-v1.md
severity: high
---

# Triage: readpool-bypasses-rebuild-lock

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `settled-deferral`, topic `readpool-bypasses-rebuild-lock`, cycle `1`, consensus `1/5`, raised against `00169-poison-file-reindex-resilience-v1.md`.

Issue: ReadPool readers set skip_stale_check, so they never enter the rebuild-lock protocol and can still observe tables while another connection is mid-drop_all_tables - the PRD's own named symptom, reached via the read-pool path rather than the N-concurrent-ddb-create path.

Detail: Raised by Bob. Already an explicitly accepted residual gap: named in the design doc's Risks section before implementation, listed in docs/src/technical/invariants.md I10, and independently placed in KNOWN by Eve. Not a regression - ReadPool has always trusted WAL visibility. Recorded here so the gap stays visible at batch end rather than living only in an invariants footnote.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `342dfda56587`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `342dfda56587` from `00169-poison-file-reindex-resilience-v1.md` until a human triages it
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
