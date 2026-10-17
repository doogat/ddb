---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 98ca37eb6470
source_prd: 00170-id-throughput-ceiling-decision-v1.md
severity: high
---

# Triage: node-validation-fallback-creates-second-node-rendering

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, topic `node-validation-fallback-creates-second-node-rendering`, cycle `2`, consensus `1/4`, raised against `00170-id-throughput-ceiling-decision-v1.md`.

Issue: The malformed-node fallback creates a second node-identity rendering: IDs hash invalid values while the explicitly read-only HlcClock still uses raw truncate_node, contradicting the recorded rationale for keeping one shared representation

Detail: Raised by Bob (codex, doubt lens) at HIGH; Alice, Blake and Carl did not raise it. Orchestrator-verified against source: HlcClock::load (ddb-core/src/git_ops/hlc_clock.rs:42-46) stores the RAW trimmed non-empty .git/ddb-node value as node_id, and the HLC's node field is truncate_node(raw). PRD 00201 lists hlc_clock.rs as 'Read-only: the node-id source reused by the discriminator' (:108), so HlcClock keeps emitting truncate_node(raw) - while 00201's new 'Validated node field' feature (:52-57) HASHES a non-conforming raw value into a different 8-hex string. For a malformed node file (short, non-hex, dash-only) the ID's node field and the HLC trailer's node field would therefore disagree. docs/src/architecture/design-decisions.md:52 rejects a wider discriminator precisely to avoid 'two different renderings of node identity - the ID's and HlcClock's'. The fallback reintroduces on the malformed path the exact drift the decision's own rationale rejects. NOTE: this is a regression finding against the CYCLE-1 REWORK - cycle 1 asked for node-file validation, task 4 added it, and this is a gap in the thing that was added. Materiality: it is a gap in a PLAN DOCUMENT held in dev/local/prds/hold/, which autopilot never drains and which a maintainer must clear via /review-prd-backlog before any batch picks it up. Nothing ships from it today. Fix direction (Bob's): extract one shared node-field resolver used by BOTH HlcClock and ID minting, or explicitly document and justify the divergence in 00201.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `98ca37eb6470`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `98ca37eb6470` from `00170-id-throughput-ceiling-decision-v1.md` until a human triages it
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
