---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 3862b938e892
source_prd: 00168-bundle-import-conflict-recovery-v1.md
severity: high
---

# Triage: single-write-lease-not-held

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `settled-deferral`, topic `single-write-lease-not-held`, cycle `1`, consensus `2/4`, raised against `00168-bundle-import-conflict-recovery-v1.md`.

Detail: Blake and Bob both flag that import_bundle takes the repo write lock three separate times (merge_remote, commit_merge, delete_remote_ref) with unlocked windows between, plus unbundle_git_objects writing objects and refs/remotes/bundle/* fully outside the lock - against the PRD's 'merge -> resolve -> ref-delete runs inside the 00162 write lease'. Deferred: the 00168 design doc records this as the exact already-accepted gap the shipped network sync() path has lived with since 00163, and Bob himself filed the unbundle/fetch leg as KNOWN. Collapsing the three re-entrant acquisitions into one outer lease is a sync-wide change, so it needs its own PRD.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `3862b938e892`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `3862b938e892` from `00168-bundle-import-conflict-recovery-v1.md` until a human triages it
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
