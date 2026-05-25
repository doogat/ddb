# Conformance Harness

The conformance harness proves that interfaces described as equivalent behave identically from a downstream developer's perspective. It runs the same golden workflows against multiple interfaces and classifies any differences using a structured result model.

This is the proof layer for PRD 00143: an equivalence claim is only valid if the same workflow passes against every interface listed in the promise matrix.

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
| `step_refs.rs` | `resolve_refs()` — cross-step `$N.id` reference resolution |
| `cross_driver_crud.rs` | Cross-driver tests for the `crud_baseline` workflow |
| `cross_driver_validation.rs` | Cross-driver tests for the `validation_error` workflow |

## Fixture format

A `WorkflowFixture` declares what a workflow does and what it expects:

```rust
WorkflowFixture {
    id: "crud_baseline",          // stable identifier
    title: "CRUD baseline",       // human-readable name
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

`AuthMode` variants: `None`, `Token { env_var }`, `Embedded`.

`StepOp` variants: `CreateDoogat`, `ReadDoogat`, `UpdateDoogat`, `DeleteDoogat`, `ListDoogats`, `Search`.

## Step reference resolution

Steps can reference the string output of an earlier step using `$N.id` syntax. For example, `args: json!({"id": "$0.id"})` in step 1 is resolved to the id string returned by step 0's `Ok` result. This lets multi-step workflows chain operations (create then read) without hardcoded ids.

References to non-existent or non-Ok steps pass through as literal strings, which causes the downstream operation to fail with an application error rather than silently succeeding.

## Result model and comparison

Each step produces a `ConformanceResult`:

| Variant | Meaning |
|---------|---------|
| `Ok { value, warnings }` | Step succeeded; value is the normalized output |
| `Err(code, message, context)` | Step failed with an application error |
| `Unsupported { reason }` | Driver does not implement this operation |
| `SetupFailed { reason }` | Driver could not execute the step (timeout, binary missing, missing arg) |

`compare_per_step(left, right)` pairs up two result slices and classifies each pair as:

| `DiffClass` | Meaning |
|-------------|---------|
| `Match` | Results are identical |
| `VariantMismatch` | Enum discriminants differ — one driver returned `Ok` where another returned `Err` |
| `ContentDiff` | Same discriminant but content differs (different value, error code, etc.) |

`VariantMismatch` is the real contract gap: the two interfaces disagree on whether the operation succeeded. `ContentDiff` records known format differences that do not constitute a conformance failure under the current promise matrix.

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

The GraphQL driver starts `ddb serve` internally and connects on a random port. No external server is required.

Tests run with `AuthMode::None`; no environment variables need to be set for the current workflows.

## Interpreting failures

**`VariantMismatch`** — one driver returned `Ok` where another returned `Err`. This is a contract gap. Check whether the operation is `Guaranteed` on both interfaces in the promise matrix (PRD 00143). If it is, the gap must be fixed. If it is not, update the fixture's `interfaces` list to exclude the non-guaranteeing interface.

**`ContentDiff`** — same outcome type but different content. This is expected for known format differences (e.g., CLI returns plain text where GraphQL returns JSON). The cross-driver tests assert only on `VariantMismatch` for gaps not covered by the current promise matrix.

**`SetupFailed`** — the driver could not execute the step. Common causes: `ddb` binary not built (`cargo build -p ddb-cli`), GraphQL server failed to start, or a step's `$N.id` reference could not be resolved.

**`Unsupported`** — the driver does not implement the requested `StepOp`. Add a `ConformanceResult::Unsupported` return in the driver's step dispatcher.

## Adding a workflow

1. Define the fixture in `workflows.rs` as a `pub fn` returning `WorkflowFixture`.
2. Add cross-driver tests in a new `cross_driver_<name>.rs` file. Pair each fixture step with `validation_error_result_count_matches_step_count`-style count assertions and a `no_step_has_variant_mismatch` assertion.
3. Register the new module in `mod.rs`.
4. Do not assert `ContentDiff` vs `Match` for fields where the promise matrix does not say `Guaranteed` — this hides legitimate transport-format differences behind fragile equality checks.
