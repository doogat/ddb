---
catchup: force
default_model: opus
model_tier_rationale: cross-cutting shape change - GitRemote and sync_manager token threading plus a new FFI sync surface
---

# FFI git sync: mobile devices as full replicas

**Priority:** P1 · **Roadmap:** Phase 3 (purpose gaps) · **Depends on:** 00163/00165/00167 (merge-path data safety — mobile sync exercises exactly those paths; all done), 00174 (installs the `FetchOptions`/`PushOptions` + `RemoteCallbacks` scaffolding the credentials callback rides on), 00177 (FFI outputs on the app contract) · **Source:** maintainer decision 2026-07-13 ("FFI git sync, full replica"), `dev/local/audit-results/2026-07-12-purpose-fit-critique.md` §3e

## Overview

### Problem Statement
The product purpose says "distributed thanks to git, mobile included", but mobile is not in the distribution:

- **FFI exposes no sync.** `DoogatDriver` has no fetch/push/sync/add-remote; the only replication path is offline bundle export/import (`ddb-core/src/ffi/driver.rs:249-276`).
- **The core capability exists, unbridged — and carries no credentials.** `GitRemote::fetch/push` live in `ddb-core/src/traits.rs:383-395` (`fetch` `:388`, `push` `:391`) and drive CLI/server sync via `sync_manager`; FFI simply never surfaces them. Nor does anything else pass credentials: `git_ops/remote.rs:14,:22` call `fetch`/`push` with `None` options and no `RemoteCallbacks`; `sync_manager::sync(remote, branch)` and the CLI `Sync { remote, branch }` have no token argument — HTTPS-token remotes are a NEW core capability this PRD adds on top of the `FetchOptions`/`PushOptions` scaffolding 00174 installs, not a thin bridge (grounded 2026-09-05).
- **The design already commits to the model.** Full-clone semantics on every device are a stated contract (`design-decisions.md`, "Sparse Index Not Applicable"), and git remotes are the sole sync transport (LAN discovery explicitly rejected). A mobile app is therefore meant to be a replica; today it cannot be one.

Maintainer decision (2026-07-13): mobile is a full git replica via FFI sync — HTTPS-token remotes first; bundles remain the offline fallback; hosted thin-client access can layer on later via 00180.

### Target Users
Downstream mobile apps (jink on iOS/Android via the Swift/Kotlin bindings) that need multi-device sync without shuttling bundle files; any embedded consumer of `DoogatDriver` that today cannot participate in the git distribution.

### Success Metrics
- A `DoogatDriver` instance can configure an HTTPS remote and run a full sync cycle (fetch, merge with conflict resolution, push) against it — pinned by an FFI-level test using a local remote.
- Sync through FFI returns the same structured report the CLI sync surfaces (the seven core `SyncReport` fields: direction, commits transferred, conflicts resolved, resurrected, collisions reassigned, singleton conflicts resolved + their entries) — no silent outcomes.
- Two `DoogatDriver` replicas syncing through one remote converge: each sees the other's creates/updates/deletes — pinned by a two-replica test.
- Swift and Kotlin bindings for the new surface generate cleanly (`uniffi-bindgen`, headless); the xcframework/.aar platform builds are verified by 00194's CI jobs, not on this host (decided 2026-09-05: no Xcode/NDK/cargo-ndk/kotlinc here).
- Docs state the mobile operating envelope: full-clone size expectations, host-app-scheduled sync (no background magic), bundles as the offline fallback.

## Functional Decomposition

### Capability: Remote configuration over FFI
#### Feature: Configure and inspect the sync remote
- **Description**: set, replace, and read the git remote a driver replica syncs with.
- **Inputs**: remote name (default `origin`) and URL (HTTPS first; local paths accepted, which the tests use).
- **Outputs**: persisted remote config in the replica's repo; a getter returning the configured remote.
- **Behavior**: add core remote replacement and inspection alongside add_remote, then adapt SetRemoteCommand/GetRemoteCommand through DoogatService and AppOutput. Validate HTTPS/local-path URLs at the boundary; SSH returns the documented unsupported-scheme error.

### Capability: Sync execution over FFI
#### Feature: Credentials callback in core
- **Description**: the core remote plumbing learns to present a per-call token to libgit2.
- **Inputs**: `token: Option<String>` threaded through `GitRemote::fetch`/`push` (`traits.rs:388,:391`) and `sync_manager::sync`.
- **Outputs**: a `RemoteCallbacks::credentials` callback installed on the `FetchOptions`/`PushOptions` 00174 introduces, supplying the token when `Some`; CLI and server callers pass `None` and keep today's behavior.
- **Behavior**: the callback answers libgit2's credential request with the token (HTTPS token/password scheme); `None` installs no credentials; a rejected token surfaces as a typed error through the unified policy; nothing is ever persisted by core.

#### Feature: Full sync cycle
- **Description**: one call runs fetch → merge/resolve → push, the same pipeline the CLI uses.
- **Inputs**: optional credentials for the HTTPS remote — DECIDED (2026-08-14 maintainer decision): a per-call token parameter (`Option<String>`) on `sync`/`fetch`/`push`; the host app owns secure storage and refresh (re-reads its keychain per call). A token expiring mid-sync fails that sync with a typed error and the host retries with a fresh token; username/password schemes and a credential-provider callback interface are explicitly deferred to a v2 (recorded so the deferral is deliberate).
- **Outputs**: a `SyncReportRecord` — a lossless mirror of the core `SyncReport` (`ddb-core/src/types/doogat.rs:218-232`: `direction`, `commits_transferred`, `conflicts_resolved`, `resurrected`, `collisions_reassigned`, `singleton_conflicts_resolved`, `singleton_conflicts`). The conflict/collision/resurrection entries ARE the structured signal; the core report carries no separate `warnings` field and no per-step fetched/applied/pushed counts, and none are invented here (DECIDED 2026-09-05 backlog review). Errors map through the unified policy (00173) to the FFI variants.
- **Behavior**: SyncCommand runs through DoogatService → AppOutput<SyncReport>, delegating merge policy to sync_manager and credentials/deadlines to the shared 00174 remote implementation. Hold the repo lock for actual tree/ref/config writes, not blocked network I/O. The host schedules the blocking call; explicit host cancellation is deferred, while 00174's remote timeout still applies. After a merge commits, push/auth failure preserves local commits and reports the failed stage for retry; no rollback or untouched-repo promise.

#### Feature: Fetch-only and push-only steps
- **Description**: expose the halves for hosts that stage sync around connectivity windows.
- **Inputs**: same remote + credentials as full sync.
- **Outputs**: the same seven-field SyncReportRecord. Core half-operation direction is fetch or push when its destination ref changes, otherwise up-to-date; commits_transferred counts commits newly reachable at the selected destination ref relative to its previous tip, not fabricated merge-operation counts. A zero count alone does not mean no ref change; no-op requires up-to-date and zero. Non-merge conflict/resurrection/collision/singleton fields are zero/empty.
- **Behavior**: core FetchCommand/PushCommand methods produce SyncReport before FFI adaptation; low-level GitRemote methods currently return Result<()> and must gain outcome production. Fetch updates remote-tracking state without merging into the worktree; push reports the acknowledged remote update. Full sync retains its existing CLI-equivalent seven-field semantics; document the half-operation count semantics explicitly.

### Capability: Mobile operating envelope
#### Feature: Documented replica lifecycle for mobile hosts
- **Description**: the honest written story a mobile downstream builds against.
- **Inputs**: the shipped behavior above; existing full-clone and compaction docs.
- **Outputs**: an FFI sync section in `docs/src/technical/ffi.md` (which already has a "Mobile Integration Model" section at `:25`) + `sync.md` (which has no FFI/mobile mention today).
- **Behavior**: documents initial clone/bootstrap cost (full history by contract), sync scheduling responsibility (host app), credential ownership (host app), TLS provenance (libgit2 build flavor), bundle fallback interplay, and index rebuild cost after large syncs.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/ffi/
├── driver.rs          # Maps to: Remote configuration; Sync execution (new exported methods)
├── records.rs         # Maps to: SyncReportRecord (lossless mirror of core SyncReport) + unsupported-scheme error
└── tests.rs           # Maps to: FFI-level sync tests (local remote, two replicas)
ddb-core/src/
├── traits.rs          # Maps to: Credentials callback — GitRemote::fetch/push gain the `token: Option<String>` parameter (:383-395)
├── git_ops/remote.rs  # Maps to: Remote configuration (add, replace, inspect); Credentials callback on the FetchOptions/PushOptions 00174 installs (new)
└── sync_manager/      # Maps to: Sync execution (existing merge pipeline plus core report-producing fetch/push halves; token parameter added)
docs/src/technical/
├── ffi.md             # Maps to: Mobile operating envelope
└── sync.md            # Maps to: Mobile operating envelope (FFI section)
```

### Module: ffi::driver (ddb-core/src/ffi/driver.rs)
- **Maps to capability**: Remote configuration; Sync execution
- **Responsibility**: bridge remote config + sync calls to core; no business logic in the bridge.
- **Exports**: `set_remote(...)`, `get_remote()`, `sync(remote, branch, token: Option<String>)`, `fetch(...)`, `push(...)` (uniffi-exported).

### Module: ffi::records (ddb-core/src/ffi/records.rs)
- **Maps to capability**: Sync execution
- **Responsibility**: FFI-safe `SyncReportRecord` (named `*Record` so it does not collide with core `SyncReport` in the same crate — the `RebuildReport` precedent notwithstanding) and error variants derived from the unified policy (00173/00177), no lossy re-mapping.
- **Exports**: `SyncReportRecord`, extended `DdbError` variants as the policy dictates.

### Module: core remote + sync (ddb-core/src/traits.rs, git_ops/remote.rs, sync_manager/, app_contract/commands.rs, app_contract/results.rs)
- **Maps to capability**: Sync execution (Credentials callback)
- **Responsibility**: thread the token from the FFI call to libgit2; CLI/server pass `None`.
- **Exports**: core remote commands/results, report-producing fetch/push, and sync_manager::sync with per-call credentials; FFI only adapts these results.

### Module: docs (docs/src/technical/ffi.md, sync.md)
- **Maps to capability**: Mobile operating envelope
- **Responsibility**: the documented contract mobile downstreams rely on.
- **Exports**: FFI sync + mobile envelope sections.

### Module: FFI verification (ddb-core/src/ffi/tests.rs)
- **Maps to capability**: Remote configuration over FFI; Sync execution over FFI; Mobile operating envelope
- **Responsibility**: exercise local remote configuration, two-replica convergence, report fidelity, failures by stage, and generated bindings.
- **Exports**: FFI regression fixtures; no additional public API.

### Module: capability publication (ddb-server/src/capabilities.rs)
- **Maps to capability**: Sync execution over FFI
- **Responsibility**: publish ffi_git_sync from the existing 00186 registry and verify its declared interface/build scope.
- **Exports**: the registry entry and matching handshake fixtures.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies within this PRD - built first (external deps: 00163/00165/00167 landed; 00174 options scaffolding and 00177 shape available).

- **records: SyncReportRecord + errors**: the FFI-safe shapes everything returns.

### Core Layer (Phase 1)
- **core credentials callback**: Depends on [00174 options scaffolding].
- **driver: remote config + sync/fetch/push**: Depends on [records, core credentials callback].

### Integration Layer (Phase 2)
- **two-replica tests, bindgen verification, docs**: Depends on [driver methods].

## Implementation Phases

### Phase 0: Foundation
**Goal**: contract-true FFI shapes for sync.

**Tasks**:
- [ ] Define `SyncReportRecord` in `records.rs` as a lossless mirror of core `SyncReport`'s seven fields (plus any new error variants from the unified error policy — no hand-rolled subsets); document the decided credential contract (per-call token, host-owned storage/refresh, mid-sync expiry fails that sync, callback interface deferred to v2) in the driver doc comment (no deps) - Acceptance: unit test maps a core `SyncReport` to the FFI record losslessly (all seven fields); error variants come from the 00173 policy surface; the credential contract is stated in the exported API docs.

**Exit Criteria**: FFI shapes exist; the credential contract (per-call token — decided 2026-08-14) is documented on the surface.

### Phase 1: Core
**Goal**: a driver replica can sync.

**Tasks**:
- [ ] Thread `token: Option<String>` through `GitRemote::fetch`/`push` (`traits.rs:388,:391`) via a `RemoteCallbacks::credentials` callback on the `FetchOptions`/`PushOptions` 00174 installs, and through `sync_manager::sync`; CLI and server callers pass `None` (depends on: Phase 0) - Acceptance: a unit test proves the callback supplies the token to libgit2 when `Some` and installs no credentials when `None`; existing CLI/server sync tests stay green unchanged.
- [ ] Implement core add/replace/inspect remote operations with SetRemoteCommand/GetRemoteCommand, then export set_remote/get_remote adapters with the documented SSH rejection (depends on: Phase 0) - Acceptance: tests set, inspect, and replace HTTPS/local-path remotes; invalid schemes fail without changing config; actual config writes are locked.
- [ ] Implement core FetchCommand/PushCommand result producers and SyncCommand wrappers through DoogatService → AppOutput before FFI adaptation (depends on: token threading) - Acceptance: local-remote tests pin first transfer, repeat no-op, changed/rewound refs, and exact newly-reachable commit counts for halves; directions distinguish ref changes from up-to-date even when count is zero, and non-merge fields are zero/empty. Full sync still matches the CLI core report. Register all five remote/sync commands for 00193's disposition completeness check.
- [ ] Export sync/fetch/push as command/result adapters returning the lossless SyncReportRecord (depends on: core command/report producers) - Acceptance: FFI tests pin all seven fields for normal and conflict-resolving sync, initial-fetch failure without a merge, and push-auth failure after a successful local merge; the latter preserves local commits, reports the push failure stage, and succeeds on retry with refreshed credentials.

**Exit Criteria**: full cycle + halves work through `DoogatDriver` with structured reporting.

### Phase 2: Integration
**Goal**: convergence proven, bindings verified, envelope documented.

**Tasks**:
- [ ] Two-replica convergence test: A creates/updates/deletes, syncs; B syncs and sees all three; then the reverse (depends on: Phase 1) - Acceptance: both replicas converge; the delete does not resurrect (regression tie-in to 00163).
- [ ] Verify the Swift + Kotlin bindings generate for the new surface via `cargo run -p ddb-uniffi-bindgen --bin uniffi-bindgen -- generate` (headless; no Xcode/NDK needed) and `cargo build -p ddb-core` stays green on the host target; author a Rust e2e scenario (`tests/e2e/`; the shell harness is removed by 00171) driving sync via the FFI test surface (depends on: Phase 1) - Acceptance: bindgen output (Swift + Kotlin sources) contains `set_remote`/`get_remote`/`sync`/`fetch`/`push`; e2e scenario green. The xcframework/.aar platform builds are NOT run here (DECIDED 2026-09-05: this host has no Xcode/NDK/cargo-ndk/kotlinc) — they are proven by 00194's nightly CI jobs on GitHub runners. No CLI command or server endpoint is added, so no showboat walkthrough is required.
- [ ] Write the mobile operating envelope docs in `ffi.md` + `sync.md` (clone cost, host-scheduled sync, credential ownership, TLS build flavor, bundle fallback, reindex cost) and a CHANGELOG entry (depends on: Phase 1) - Acceptance: docs landed; CHANGELOG updated; push `master`. Include the `ffi_git_sync` registry addition and update exact-set plus health/GraphQL agreement fixtures; Acceptance also requires the flag to match compiled availability and the implemented workflow.

**Exit Criteria**: convergence + bindgen + docs landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: replica A creates a doogat, syncs; replica B syncs → Expected: B serves the doogat; reports on both sides show the transfer.
- **Edge case**: both replicas create different doogats in the same wall-clock second, then sync → Expected: collision reassignment runs (00167 semantics), both documents survive, the report says so — nothing silent.
- **Error case**: bad credentials/unreachable remote returns a typed, stage-specific failure. Before merge, HEAD/worktree stay unchanged though bootstrap metadata or fetched objects may exist; after a committed merge, push failure preserves that progress for retry. Never promise whole-repository rollback.

## Risks

- **libgit2 TLS on mobile targets**: HTTPS transport depends on the git2 crate's TLS build flavor per platform. Mitigation: pin the vendored TLS features; the xcframework/android build verification runs in 00194's CI (the local toolchains are absent by decision, 2026-09-05).
- **`GitRemote`/`sync_manager` signature change is cross-cutting**: CLI and server call sites change. Mitigation: they pass `None`; existing sync tests are the oracle; the classifier treats this as a `contract_edit`.
- **Merge-path defects amplified**: mobile sync exercises resurrection/divergence paths. Mitigation: hard dependency on 00163/00165/00167 landing first; the convergence test re-pins deletion safety.
- **Long-blocking sync on the driver mutex**: `DoogatDriver` wraps one `Mutex<DoogatService>`; a slow sync blocks other FFI calls. Mitigation: document the blocking model for v1 (host schedules sync off the UI thread); revisit split locks only if a downstream measures pain.
- **Credential misuse**: tokens must never be persisted by ddb. Mitigation: the decided credential shape keeps storage in the host app; docs state it; no credential ever written to repo or config by core.

**Capability registry (depends on 00186):** add `ffi_git_sync` to the existing sorted registry only with this implemented surface; document the applicable interface/workflow and compile-time availability. Internal refactors and tests are not separate flags.

## Transport Impact

- **CLI**: Guaranteed — sync already exists; passes `None` for the token; no user-facing change.
- **GraphQL**: Guaranteed — server sync mutation already exists; passes `None`; no change.
- **REST**: Guaranteed — no new surface needed; sync stays a server-level operation where applicable.
- **PgWire**: Intentionally absent — sync is not a SQL-surface operation on any transport; parity is judged against the app capability set, where sync is exposed on CLI/GraphQL/FFI hosts that own a replica.
- **FFI**: Guaranteed — this PRD's subject; reaches parity with CLI/server sync including the structured `SyncReportRecord`.
- **NoSQL HTTP**: Intentionally absent — same reasoning as PgWire; NoSQL HTTP serves documents, it is not a replica-management surface.
- **Cross-interface conformance plan**: the two-replica FFI convergence test mirrors the existing core `sync_test` scenarios so CLI-sync and FFI-sync verify the same workflow; report-shape parity with CLI sync output asserted in a test (same seven fields). jink impact: the jink mobile app can be a first-class offline replica instead of a bundle-shuttling client.
