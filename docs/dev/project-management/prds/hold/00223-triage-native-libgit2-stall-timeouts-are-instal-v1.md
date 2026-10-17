---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: c469b16a572e
source_prd: 00174-decouple-maintenance-from-write-actor-v1.md
severity: high
---

# Triage: Native libgit2 stall timeouts are installed only at server startup, so CLI/FF...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `design-scope`, consensus `1/4`, raised against `00174-decouple-maintenance-from-write-actor-v1.md`.

Issue: Native libgit2 stall timeouts are installed only at server startup, so CLI/FFI callers of the changed core fetch/push run unbounded (ddb-core/src/git_ops/remote.rs:72)

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `c469b16a572e`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `c469b16a572e` from `00174-decouple-maintenance-from-write-actor-v1.md` until a human triages it
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
