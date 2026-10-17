---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: b052b1166214
source_prd: 00171-migrate-shell-scenarios-to-e2e-v1.md
severity: high
---

# Triage: Nightly macOS ConnectionReset in a migrated e2e test, cause unconfirmed. inte...

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `maintainer-accepted-deferral`, cycle `2`, consensus `1/4`, raised against `00171-migrate-shell-scenarios-to-e2e-v1.md`.

Issue: Nightly macOS ConnectionReset in a migrated e2e test, cause unconfirmed. integration_warnings_update::integration_58_a_graphql_update_warnings_empty failed once at c8b72bd in Nightly 34020494801 at tests/e2e/common.rs:186. At the reviewed SHA it passed on all three platforms with no code change, no retry, no skip. Eve traced the readiness path and found it sound for HTTP; cause remains unconfirmed. Alice established that task 15 moved the full e2e suite from nightly-only to every push, so this flake exposure increased as a direct result of this PRD rework, with no resilience added. Severity contested: Blake (blind) 1/4 rates it high; Alice, Bob and Eve rate it low with PRD 00188 named as the durable home.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `b052b1166214`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `b052b1166214` from `00171-migrate-shell-scenarios-to-e2e-v1.md` until a human triages it
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
