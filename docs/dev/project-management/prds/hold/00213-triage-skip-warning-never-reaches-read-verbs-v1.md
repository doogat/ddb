---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 206409f48a3d
source_prd: 00169-poison-file-reindex-resilience-v1.md
severity: high
---

# Triage: skip-warning-never-reaches-read-verbs

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `settled-deferral`, topic `skip-warning-never-reaches-read-verbs`, cycle `1`, consensus `3/5`, raised against `00169-poison-file-reindex-resilience-v1.md`.

Issue: The PRD's Phase 1 acceptance criterion names search explicitly ('after a poison file lands, a create AND a search both succeed and each carries a skip AppWarning'), but only create, update and apply_schema capture ensure_fresh's return value. Every read verb (search, read, sql, discovery, batch, delete, utility) calls self.ensure_fresh()?; as a bare statement and discards the warnings, so a search that is the first verb after a poison commit silently returns a short result set with no structured signal.

Detail: Raised by Blake at CRITICAL, Bob at HIGH, Alice as R9 fail, and placed in KNOWN by Eve. Severity adjudicated HIGH rather than CRITICAL: the three reviewers who actually read the implementation all rated it below Blake's blind CRITICAL, the skip is not silent at the log level (per-file tracing::warn! still fires), and the index is a derived, rebuildable cache with git as source of truth. Deferred because search has no AppOutput envelope to carry a warning - SearchCommand/AppSearchResult exist in app_contract but are never constructed - and building one is precisely the subject of backlog PRD 00176 route-search-through-contract. Eve additionally proved the criterion is unimplementable as literally written: the warning is consume-once, because the first verb's ensure_fresh reindexes and leaves the index fresh, so a create AND a subsequent search cannot both carry it for a single poison event. What IS being fixed this cycle is the documentation that overstated the guarantee (invariants.md I5 claimed 'the verb envelope' generally). THIRD CONSECUTIVE PRD in this batch where the PRD text asserts a transport/verb surface that does not exist - see the 00167 prd-transport-impact-overclaim and 00168 ffi-import-bundle-discards-syncreport entries above. That pattern now deserves one batch-end decision about how Transport Impact and acceptance criteria are authored, not a fourth per-PRD patch.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `206409f48a3d`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `206409f48a3d` from `00169-poison-file-reindex-resilience-v1.md` until a human triages it
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
