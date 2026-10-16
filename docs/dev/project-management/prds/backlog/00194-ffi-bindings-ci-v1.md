---
design: skip
default_model: sonnet
model_tier_rationale: CI wiring only; packaging row
---

# FFI Bindings CI

Depends on: 00191

(`design: skip`: CI wiring on the minimal template; no implementation design to review.)

Source: `dev/local/audit-results/2026-07-13-goal-gap-analysis.md` (gap G3).

## Problem

No GitHub workflow references uniffi, swift, kotlin, xcframework, or android: the Swift/Kotlin bindings are never generated in CI, the XCFramework/.aar builds run only via local scripts (`dev/bin/build-xcframework`, `dev/bin/build-android`), and the bindings-level test projects that already exist (`tests/swift` — SwiftPM; `tests/kotlin` — Gradle) are never executed. Releases ship only desktop artifacts (the `ddb` binary plus `libddb_core.{so,dylib}` per target, `release.yml:121-127`); no mobile artifact is ever built. The mobile surface — the distribution moat that 00191 turns into a full git replica — can rot silently between local builds. Sequencing note: numerically this runs after 00191 (the numeric-order contract has no free slot below it, and 00191 validates local binding generation; this PRD owns platform builds on CI runners); this PRD locks in the continuous protection from then on.

## Solution

One CI leg (a job set in `full-validation.yml`, so it runs nightly and on `workflow_dispatch`, not per-push — the toolchains are heavy) that: generates Swift and Kotlin bindings via `ddb-uniffi-bindgen`; builds the XCFramework on a macOS runner and the Android .aar on a Linux runner (pinned NDK + cargo-ndk) using the existing `dev/bin` scripts; runs the `tests/swift` and `tests/kotlin` suites against the freshly generated bindings; uploads both artifacts as short-retention CI artifacts for inspection. Publishing (SwiftPM/CocoaPods/Maven, release attachment) stays out of scope — building is what catches rot.

## Requirements

### Must have
- Bindings generation step for both languages; a generation error fails the job.
- XCFramework build on macOS runner and .aar build on Linux runner via the existing `dev/bin` scripts (scripts adapted only if CI paths require it; behavior parity with local runs).
- Premise: `tests/swift` and `tests/kotlin` are runnable binding-level suites (both carry READMEs and build manifests). Re-check at execution; if either turns out to be scaffolding, reduce that leg to generate+build plus a minimal load-the-library smoke assertion, and report the reduction.
- Jobs wired into `full-validation.yml` (nightly + manual dispatch); failures surface like the other Tier 2 legs.
- Artifacts uploaded with short retention; explicitly NOT attached to releases.

### Nice to have
- Toolchain/cargo/Gradle caching to keep the nightly runtime reasonable.

## Implementation

### Module: bindings-ci workflow
- **Location**: `.github/workflows/full-validation.yml` (new jobs) + minimal `dev/bin` script adjustments if needed
- **Responsibility**: generate bindings, build platform artifacts, run bindings-level suites, upload artifacts
- **Exports**: n/a (CI configuration)

### Dependencies
- bindings-ci workflow: No code dependency on 00191's features (it builds whatever FFI surface exists); Depends-on 00191 is sequencing — 00191 checks local bindgen only; pinned CI toolchains here provide continuous platform-artifact verification.

## Tasks

### Phase 0: Foundation
- [ ] Add bindings-generation jobs (Swift + Kotlin) to `full-validation.yml` - Acceptance: both `ddb-uniffi-bindgen` invocations succeed in CI and fail the job on error.
- [ ] Pin toolchains (Xcode version on macOS runner; NDK + cargo-ndk on Linux) - Acceptance: versions recorded in the workflow, not floating.

### Phase 1: Core
- [ ] XCFramework + .aar build jobs via `dev/bin/build-xcframework` / `dev/bin/build-android` (depends on: Phase 0) - Acceptance: both artifacts produced and uploaded with short retention; the new jobs are added to `mark-validated.needs` (`full-validation.yml:128-133`) so a red bindings leg blocks the validated stamp. These CI builds are the ONLY verification of the platform artifacts: 00191 verifies bindgen locally and defers the xcframework/.aar builds here (decided 2026-09-05 — this host has no Xcode/NDK/cargo-ndk/kotlinc).
- [ ] Run `tests/swift` and `tests/kotlin` suites against generated bindings (depends on: Phase 0) - Acceptance: suites execute and pass in CI, or the premise-reduction path is taken and reported.
- [ ] Docs note in `docs/src/technical/ffi.md` (depends on: Phase 1 builds) - Acceptance: states that CI guards binding generation and platform builds nightly.

## Success Criteria

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

- Nightly CI fails when binding generation, the XCFramework build, or the .aar build breaks.
- The existing Swift/Kotlin test suites run in CI (or the documented smoke fallback per the premise re-check).
- No publishing side effects: release artifacts unchanged.

## Transport Impact

FFI only, and build-infrastructure only — no runtime behavior change on any transport. CLI/GraphQL/REST/PgWire/NoSQL HTTP: no impact. Conformance plan: not applicable (no behavior surface); the FFI behavioral parity leg is 00193.
