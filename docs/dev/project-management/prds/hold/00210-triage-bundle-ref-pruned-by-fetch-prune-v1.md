---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 202ecc321adf
source_prd: 00168-bundle-import-conflict-recovery-v1.md
severity: critical
---

# Triage: bundle-ref-pruned-by-fetch-prune

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `critical-finding`, topic `bundle-ref-pruned-by-fetch-prune`, cycle `1`, consensus `1/4`, raised against `00168-bundle-import-conflict-recovery-v1.md`.

Detail: Blake (blind lens) reproduced live: unbundle_git_objects fetches refs/heads/*:refs/remotes/bundle/* with no --no-prune, so on a machine with fetch.prune=true (set globally here) the NEXT bundle import deletes the refs/remotes/bundle/master that a FAILED import deliberately kept. Observed `- [deleted] (none) -> bundle/master`; with -c fetch.prune=false the ref survived. This defeats the PRD's headline reachability guarantee. Logged here because classification routes every CRITICAL to batch end, but it is ALSO queued for cycle-1 rework as task [D1-1] and blocks convergence until fixed - a CRITICAL is never a settled deferral. Not escalated to a batch halt: the bundle tarball on disk remains the recoverable source, and the post-00168 behavior is strictly better than the pre-00168 unconditional delete, so this is a weaker-than-claimed guarantee rather than unrecoverable loss.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `202ecc321adf`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `202ecc321adf` from `00168-bundle-import-conflict-recovery-v1.md` until a human triages it
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
