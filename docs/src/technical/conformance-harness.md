# Conformance Harness

The conformance harness proves that interfaces described as equivalent behave identically from a downstream developer's perspective. It runs the same golden workflows against multiple interfaces and classifies any differences using a structured result model.

This is the proof layer for the PRD 00143 application contract: an equivalence claim is only valid if the same workflow passes against every interface listed in the promise matrix.

## Module structure

All conformance code lives under `tests/e2e/conformance/`:

| File | Role |
|------|------|
| `fixture.rs` | `WorkflowFixture`, `Step`, `StepOp`, `SetupExpectation`, `AuthMode`, `InterfaceId` |
| `workflows.rs` | Concrete workflow definitions (`crud_baseline`, `validation_error`) |
| `result.rs` | `ConformanceResult`, `ConformanceValue`, `ConformanceError`, `ConformanceWarning` |
| `comparator.rs` | `DiffClass`, `compare()`, `compare_per_step()` |
| `driver_cli.rs` | `CliDriver` — executes steps via the `ddb` binary |
| `driver_graphql.rs` | `GraphqlDriver` — executes steps via `ddb serve` GraphQL API |
| `step_refs.rs` | `resolve_refs()` and `resolve_step()` — cross-step `$N.id` reference resolution |
| `args.rs` | `require_string()` / `optional_string()` — shared arg-extraction helpers |
| `setup.rs` | `check_setup_supported()` — shared driver gate for `auth_mode` / `setup_steps` enforcement |
| `cross_driver_crud.rs` | Cross-driver tests for the `crud_baseline` workflow |
| `cross_driver_validation.rs` | Cross-driver tests for the `validation_error` workflow |

## Fixture format

A `WorkflowFixture` declares what a workflow does and what it expects:

```rust
WorkflowFixture {
    id: "crud_baseline".into(),
    title: "CRUD baseline".into(),
    setup: SetupExpectation {
        auth_mode: AuthMode::None,
        timeout_ms: 30_000,
        setup_steps: vec![],
    },
    steps: vec![
        Step { op: StepOp::CreateDoogat, args: json!({"title": "Test"}) },
        Step { op: StepOp::ListDoogats,  args: json!({}) },
    ],
    expected: ExpectedBehavior {
        value: None,
        warnings: vec![],
        error: None,
    },
    interfaces: vec![InterfaceId::Cli, InterfaceId::Graphql],
}
```

`AuthMode` variants: `None`, `Token { env_var: String }`, `Embedded`.

`StepOp` variants: `CreateDoogat`, `ReadDoogat`, `UpdateDoogat`, `DeleteDoogat`, `ListDoogats`, `Search`.

## Step reference resolution

Steps can reference the string output of an earlier step using `$N.id` syntax. For example, `args: json!({"id": "$0.id"})` in step 1 is resolved to the id string returned by step 0's `Ok` result. This lets multi-step workflows chain operations (create then read) without hardcoded ids.

References to non-existent or non-Ok steps pass through as literal strings, so id-consuming operations surface the unresolved value through the driver instead of the resolver inventing one.

## Result model and comparison

Each step produces a `ConformanceResult`:

| Variant | Meaning |
|---------|---------|
| `Ok { value, warnings }` | Step succeeded; value is the normalized output |
| `Err(ConformanceError)` | Step failed with an application error |
| `Unsupported { reason }` | Driver does not implement this operation |
| `SetupFailed { reason }` | Driver could not execute the step (timeout, binary missing, missing arg) |

`compare_per_step(left, right)` pairs up two result slices and classifies each pair as:

| `DiffClass` | Meaning |
|-------------|---------|
| `Match` | Results are identical |
| `ValueMismatch` | Both `Ok`; the `value` fields differ (warnings may also differ; the value diff dominates) |
| `WarningMismatch` | Both `Ok`; `value` matches but `warnings` differ |
| `ErrorMismatch` | Both `Err`; the `ConformanceError` shapes differ (code / message / context) |
| `UnsupportedOperation` | At least one driver returned `Unsupported` and the results do not match |
| `SetupFailure` | At least one driver returned `SetupFailed` and the results do not match |
| `MissingField` | `compare_per_step` only: one slice ran out of results before the other (length divergence) |
| `VariantMismatch` | Catch-all for incompatible variant pairs not covered above (most commonly `Ok` vs `Err`) |

`VariantMismatch` is the real contract gap: the two interfaces disagree on whether the operation succeeded. The other categories let downstream readers distinguish "two drivers returned `Ok` but the values differ" from "both errored differently" or "one timed out". Known format differences (e.g. CLI returns text where GraphQL returns JSON) surface as `ValueMismatch` and the cross-driver tests assert only on `VariantMismatch` for gaps not covered by the current promise matrix.

## Running conformance tests

Conformance tests require the `ddb` binary to be built first:

```bash
cargo build -p ddb-cli
cargo test -p ddb-e2e
```

To run only conformance tests:

```bash
cargo build -p ddb-cli
cargo test -p ddb-e2e conformance
```

The GraphQL driver starts `ddb serve` internally and connects on an available local port. No external server is required.

Tests run with `AuthMode::None`; no environment variables need to be set for the current workflows.

## Interpreting failures

**`VariantMismatch`** — one driver returned `Ok` where another returned `Err`. This is a contract gap. Check whether the operation is `Guaranteed` on both interfaces in the promise matrix (PRD 00143). If it is, the gap must be fixed. If it is not, update the fixture's `interfaces` list to exclude the non-guaranteeing interface.

**`ValueMismatch`** — both drivers reported `Ok` but the `value` fields differ. Expected for known format differences (e.g. CLI returns plain text where GraphQL returns JSON). Strict cross-interface equality is deferred; see "Deferred scope" below.

**`WarningMismatch`** — both drivers reported `Ok`, values match, but warnings differ. Typically a contract gap when warnings are part of the promise.

**`ErrorMismatch`** — both drivers reported `Err` but the `code` / `message` / `context` differ. Today the cross-driver tests do not assert error-code equivalence (deferred; see below); a `ErrorMismatch` only indicates the drivers' error shapes are not byte-identical.

**`SetupFailure`** — at least one driver returned `SetupFailed`. Common causes: `ddb` binary not built (`cargo build -p ddb-cli`), GraphQL server failed to start, request timeout, missing fixture arguments, or the fixture's setup expectation names a capability the driver does not yet implement (see "Deferred scope").

**`UnsupportedOperation`** — at least one driver returned `Unsupported`. New drivers that cannot support a `StepOp` should return `ConformanceResult::Unsupported`.

**`MissingField`** — `compare_per_step` saw a length divergence between the two driver result slices. The unmatched trailing entries are reported as `MissingField` so a driver returning fewer (or more) results than the other is surfaced rather than silently truncated.

## Adding a workflow

1. Define the fixture in `workflows.rs` as a `pub fn` returning `WorkflowFixture`.
2. Add cross-driver tests in a new `cross_driver_<name>.rs` file. Assert each driver returns one result per fixture step and no step has a `VariantMismatch`.
3. Register the new module in `mod.rs`.
4. Do not assert byte-equality on values where the promise matrix does not say `Guaranteed` — that hides legitimate transport-format differences behind fragile checks.

## Deferred scope

The v1 harness intentionally defers the following items. Future PRDs pick each up.

- **FFI driver** — deferred. The intended landing spot is the `uniffi` `DoogatDriver` facade in `ddb-core::ffi`. Will follow the same `Driver::run_workflow(&fixture) -> Vec<ConformanceResult>` shape as `CliDriver` / `GraphqlDriver`.
- **PgWire driver** — deferred. Targets the `ddb serve --pg-port` Postgres-wire endpoint. Belongs in the same `tests/e2e/conformance/` tree.
- **REST driver** — deferred. Targets the `/rest/*` endpoints already served by `ddb serve`.
- **NoSQL HTTP driver** — deferred. Targets the document-style HTTP API.
- **Cross-interface error-code equality** — deferred. The `validation_error` workflow does not yet produce a real validation error, so the current cross-driver test only checks that both drivers run the fixture to completion. It does not assert that the two drivers returned the same `code` / `message` / `context`. Strict equality across transports awaits the application contract model work in PRD 00147; once that contract is ready, the `validation_error` cross-driver tests should re-assert `DiffClass::Match` (not just "no `VariantMismatch`") for each step.
- **`validation_error` fixture trigger** — deferred. The fixture's `CreateDoogat { title: "" }` step does not actually trigger validation in ddb today: both `CliDriver` and `GraphqlDriver` accept the empty title and return `Ok` after generating a fallback title. **SC4** ("Structured errors can be compared across at least two interfaces") awaits one of: (a) the fixture switching to input ddb genuinely rejects (e.g. a non-existent type reference, malformed primary key, or a schema-validated typedef field when typed-doogat validation lands), or (b) ddb adding validation that rejects empty titles when no template is set.
- **Driver timeout-conformance tests (CLI and GraphQL)** — deferred. For CLI, a previous `#[ignore]`'d `timeout_yields_setup_failed` test tried to exceed a 1ms timeout via `ddb list`, but clap exits in ~2ms before the timer fires; no always-fast no-op subcommand exists that reliably exceeds short timeouts on every machine. For GraphQL, the `transport_error_returns_setup_failed` test in `driver_graphql_test.rs` already covers the connection-refused transport-error path (server killed before request), but a dedicated test that asserts `SetupFailed { reason: contains "timed out" }` against an actually-slow handler is still deferred — current fixtures use `timeout_ms: 30_000`, so the timeout-specific reason variant is exercised only by the unit-level `is_timeout()` branch, not by an end-to-end test that triggers a slow response. When a reliable slow-handler fixture exists for either driver, add the corresponding timeout-conformance test.
- **`SetupExpectation.auth_mode` enforcement beyond `None`** — deferred. Drivers reject `Token` / `Embedded` via `setup::check_setup_supported`; honoring them requires teaching each driver to source the relevant credentials before running steps.
- **`SetupExpectation.setup_steps` interpreter** — deferred. Drivers reject any non-empty `setup_steps` via the same gate; honoring them requires defining the string-action vocabulary (e.g. `create_baseline_typedef`) and wiring an interpreter into each driver.
- **`ExpectedBehavior` enforcement against driver results** — deferred. `WorkflowFixture::expected.{value,warnings,error}` are present in the fixture model but not yet enforced by the comparator. Today the cross-driver tests compare driver outputs against each other, not against the fixture's expected shape. When enforcement lands, the comparator should fold `ExpectedBehavior` into per-step classification.
