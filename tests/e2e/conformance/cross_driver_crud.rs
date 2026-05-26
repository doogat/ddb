use super::comparator::{compare_per_step, DiffClass};
use super::driver_cli::CliDriver;
use super::driver_graphql::GraphqlDriver;
use super::workflows;

fn run_crud_baseline() -> (
    Vec<super::result::ConformanceResult>,
    Vec<super::result::ConformanceResult>,
) {
    let fixture = workflows::crud_baseline();
    let cli = CliDriver::new();
    let graphql = GraphqlDriver::new();
    (cli.run_workflow(&fixture), graphql.run_workflow(&fixture))
}

// Result count must equal step count for both drivers.
#[test]
fn crud_baseline_result_count_matches_step_count() {
    let fixture = workflows::crud_baseline();
    let (cli_results, graphql_results) = run_crud_baseline();
    let diffs = compare_per_step(&cli_results, &graphql_results);
    assert_eq!(diffs.len(), fixture.steps.len());
}

// Both drivers must produce the same outcome type per step (both Ok or both Err).
// ContentDiff is acceptable: each driver runs in its own repo, so IDs and
// formatted values differ by design. VariantMismatch would mean one driver
// succeeded where the other failed — a real conformance gap.
#[test]
fn crud_baseline_no_step_has_variant_mismatch() {
    let (cli_results, graphql_results) = run_crud_baseline();
    let diffs = compare_per_step(&cli_results, &graphql_results);
    for (i, diff) in diffs.iter().enumerate() {
        assert_ne!(
            *diff,
            DiffClass::VariantMismatch,
            "step {i}: one driver succeeded where the other failed\n  cli={:?}\n  graphql={:?}",
            cli_results[i],
            graphql_results[i]
        );
    }
}

// ListDoogats output formats diverge: CLI returns "ID | Title" text;
// GraphQL returns a JSON array. This is a known contract gap classified
// as ValueMismatch (both Ok, value fields differ). Strict output
// equivalence is deferred to a later PRD. Looks up the List step by
// StepOp rather than positional index so the assertion survives
// crud_baseline gaining new steps (PRD 00148 cycle-2 F15).
#[test]
fn crud_baseline_list_doogats_format_diverges_across_cli_and_graphql() {
    let fixture = workflows::crud_baseline();
    let list_idx = fixture
        .steps
        .iter()
        .position(|s| s.op == super::fixture::StepOp::ListDoogats)
        .expect("crud_baseline fixture has a ListDoogats step");
    let (cli_results, graphql_results) = run_crud_baseline();
    let diffs = compare_per_step(&cli_results, &graphql_results);
    assert_eq!(
        diffs[list_idx],
        DiffClass::ValueMismatch,
        "ListDoogats format gap no longer present — update or remove this test"
    );
}

// Read of a nonexistent doogat must return Err on BOTH drivers —
// PRD 00148 Phase 1 explicitly requires not-found behavior in the
// CRUD baseline (surfaced as the blind-review I4 gap). The not-found
// step is identified by its literal id "99999999999999" so the
// lookup survives further fixture growth. The asserted contract is
// shape-level (both drivers return Err), not code-level: CLI returns
// CLI_ERROR while GraphQL returns a GraphQL-shaped error code; the
// resulting ErrorMismatch is documented as a contract gap deferred to
// PRD 00149 (cross-interface error-code equality).
#[test]
fn crud_baseline_not_found_read_returns_err_on_both_drivers() {
    let fixture = workflows::crud_baseline();
    let nf_idx = fixture
        .steps
        .iter()
        .position(|s| {
            s.op == super::fixture::StepOp::ReadDoogat
                && s.args.get("id").and_then(|v| v.as_str()) == Some("99999999999999")
        })
        .expect("crud_baseline fixture has a not-found ReadDoogat step");
    let (cli_results, graphql_results) = run_crud_baseline();
    assert!(
        matches!(
            cli_results[nf_idx],
            super::result::ConformanceResult::Err(_)
        ),
        "CLI not-found read should return Err, got: {:?}",
        cli_results[nf_idx]
    );
    assert!(
        matches!(
            graphql_results[nf_idx],
            super::result::ConformanceResult::Err(_)
        ),
        "GraphQL not-found read should return Err, got: {:?}",
        graphql_results[nf_idx]
    );
    let diffs = compare_per_step(&cli_results, &graphql_results);
    assert_ne!(
        diffs[nf_idx],
        DiffClass::VariantMismatch,
        "not-found step: one driver succeeded where the other failed"
    );
}
