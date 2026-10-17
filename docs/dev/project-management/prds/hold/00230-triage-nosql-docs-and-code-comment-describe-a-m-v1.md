---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 0ab419d412c3
source_prd: 00174-decouple-maintenance-from-write-actor-v1.md
severity: high
---

# Triage: NoSQL docs and code comment describe a mechanism that does not exist. docs/sr...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, cycle `2`, consensus `1/4`, raised against `00174-decouple-maintenance-from-write-actor-v1.md`.

Issue: NoSQL docs and code comment describe a mechanism that does not exist. docs/src/technical/server.md:36 and ddb-core/src/nosql.rs:232 say NoSQL reads are serialized against mirror writes via `NoSqlMirrorPort::lock_for_read`. That method exists nowhere (rg finds only those two comments); the trait at ddb-core/src/traits.rs:352 has only the two mirror methods. Commit 92571f5 replaced it with a bounded retry (20 x 10 ms) on the read side only (ddb-core/src/service/utility.rs:121); docs were not updated. The "never two opens competing" claim is false

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `0ab419d412c3`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `0ab419d412c3` from `00174-decouple-maintenance-from-write-actor-v1.md` until a human triages it
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
