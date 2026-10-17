---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 89b8a44d2eab
source_prd: 00174-decouple-maintenance-from-write-actor-v1.md
severity: high
---

# Triage: Documentation names an API that does not exist. The new `nosql.rs` doc commen...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, cycle `2`, consensus `1/4`, raised against `00174-decouple-maintenance-from-write-actor-v1.md`.

Issue: Documentation names an API that does not exist. The new `nosql.rs` doc comment and `docs/src/technical/server.md:36` say NoSQL reads are serialized against mirror writes via `NoSqlMirrorPort::lock_for_read`, "which `DoogatService` holds for the read's full duration". `rg lock_for_read` finds only those two doc mentions. The code instead retries `RedbIndex::open` 20 times at 10 ms (`ddb-core/src/service/utility.rs:121`). The rework turned the earlier overclaim (R6/R8) into a false claim about a lock that is not there. Rewrite both texts to describe what the code does, or implement the lock.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `89b8a44d2eab`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `89b8a44d2eab` from `00174-decouple-maintenance-from-write-actor-v1.md` until a human triages it
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
