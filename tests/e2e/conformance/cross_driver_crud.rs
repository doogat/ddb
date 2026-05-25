use super::comparator::{compare_per_step, DiffClass};
use super::driver_cli::CliDriver;
use super::driver_graphql::GraphqlDriver;
use super::workflows;

fn run_crud_baseline() -> (Vec<super::result::ConformanceResult>, Vec<super::result::ConformanceResult>) {
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
