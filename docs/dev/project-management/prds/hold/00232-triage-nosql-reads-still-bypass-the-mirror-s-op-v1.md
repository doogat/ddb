---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 7896926a7dfc
source_prd: 00174-decouple-maintenance-from-write-actor-v1.md
severity: high
---

# Triage: NoSQL reads still bypass the mirror's `open_lock`. Retrying reads for 190ms d...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, cycle `2`, consensus `1/4`, raised against `00174-decouple-maintenance-from-write-actor-v1.md`.

Issue: NoSQL reads still bypass the mirror's `open_lock`. Retrying reads for 190ms does not serialize access: a read can acquire redb first and make a concurrent mirror write fail immediately, or exhaust retries during a longer write. The new documentation incorrectly claims `lock_for_read` provides serialization. Hold the shared mirror lock throughout each read's handle lifetime and add a read/write concurrency regression.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `7896926a7dfc`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `7896926a7dfc` from `00174-decouple-maintenance-from-write-actor-v1.md` until a human triages it
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
