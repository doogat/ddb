---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 900c25649c9e
source_prd: 00178-structured-fields-input-v1.md
severity: high
---

# Triage: [cycle 2] R1: the GraphQL VALIDATION_ERROR mapping added in task 6 is unteste...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `deferred_decision`, cycle `2`, consensus `1/3`, raised against `00178-structured-fields-input-v1.md`.

Issue: [cycle 2] R1: the GraphQL VALIDATION_ERROR mapping added in task 6 is untested - no test asserts extensions.code == VALIDATION_ERROR on any GraphQL path, so reverting the four to_graphql_error call sites in ddb-server/src/schema/mutations/operations.rs to ServerError::new(msg, None) would still pass every test. Alice also found the same gap unfixed in the singleton update/upsert resolvers (ddb-server/src/schema/mutations/singleton.rs:45 and :99).

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `900c25649c9e`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `900c25649c9e` from `00178-structured-fields-input-v1.md` until a human triages it
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
