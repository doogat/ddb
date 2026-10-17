---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 59b817d6d7aa
source_prd: 00174-decouple-maintenance-from-write-actor-v1.md
severity: high
---

# Triage: R4/R14 only partly resolved. Config parsing and the `.max(1)` floor now have...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, cycle `2`, consensus `3/4`, raised against `00174-decouple-maintenance-from-write-actor-v1.md`.

Issue: R4/R14 only partly resolved. Config parsing and the `.max(1)` floor now have tests, and the libgit2 option install is behind a `Once`. `remote_callbacks` (the fetch `transfer_progress`/`sideband_progress` wall-clock deadline) still has no test, and the module doc now just states that as a known limitation. A deterministic test needs no wire-format emulation. Fetch from a local `file://` repo that has objects, using `timeout = Duration::ZERO`, so the deadline is already past when the first transfer callback fires. Assert `REMOTE_TIMEOUT`, and assert that a normal timeout succeeds. This fails if the callback layer is removed.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `59b817d6d7aa`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `59b817d6d7aa` from `00174-decouple-maintenance-from-write-actor-v1.md` until a human triages it
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
