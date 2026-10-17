---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: dbcc514947b7
source_prd: 00174-decouple-maintenance-from-write-actor-v1.md
severity: high
---

# Triage: NoSQL mirror writes can now be silently lost. RedbMirror::with_index (ddb-cor...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, cycle `2`, consensus `1/4`, raised against `00174-decouple-maintenance-from-write-actor-v1.md`.

Issue: NoSQL mirror writes can now be silently lost. RedbMirror::with_index (ddb-core/src/nosql.rs:261) opens redb with no retry, and nosql_index_doogat discards the error (ddb-core/src/service/write_helpers.rs:254, `let _ =`). A NoSQL read on the write-actor thread holds the exclusive redb handle for its whole rebuild (utility.rs:141; open_nosql_fresh rebuilds on every read). A maintenance-thread mirror write landing in that window fails with DatabaseAlreadyOpen and is dropped. Conversely a read can fail after 200 ms of retries. Single-threading used to make this impossible. Mitigated only because each read rebuilds the whole mirror

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `dbcc514947b7`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `dbcc514947b7` from `00174-decouple-maintenance-from-write-actor-v1.md` until a human triages it
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
