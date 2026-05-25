use super::comparator::{compare_per_step, DiffClass};
use super::driver_cli::CliDriver;
use super::driver_graphql::GraphqlDriver;
use super::workflows;

fn run_validation_error() -> (Vec<super::result::ConformanceResult>, Vec<super::result::ConformanceResult>) {
    let fixture = workflows::validation_error();
    let cli = CliDriver::new();
    let graphql = GraphqlDriver::new();
    (cli.run_workflow(&fixture), graphql.run_workflow(&fixture))
}

#[test]
fn validation_error_result_count_matches_step_count() {
    let fixture = workflows::validation_error();
    let (cli_results, graphql_results) = run_validation_error();
    let diffs = compare_per_step(&cli_results, &graphql_results);
    assert_eq!(diffs.len(), fixture.steps.len());
}

// VariantMismatch here would mean one driver succeeded where the other failed,
// which is the real conformance gap (PRD 00148 success criterion 5).
#[test]
fn validation_error_no_step_has_variant_mismatch() {
    let (cli_results, graphql_results) = run_validation_error();
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

// Test 3: error code/message strings may differ between CLI and GraphQL
// (existing contract gap, same as ListDoogats format gap in cross_driver_crud).
// Strict cross-interface error-code equality is deferred to a later PRD.
// Assert the single step's diff is Match OR ContentDiff (NOT VariantMismatch).
#[test]
fn validation_error_diff_class_is_match_or_content_diff() {
    let (cli_results, graphql_results) = run_validation_error();
    let diffs = compare_per_step(&cli_results, &graphql_results);
    assert_eq!(diffs.len(), 1);
    let acceptable = matches!(diffs[0], DiffClass::Match | DiffClass::ContentDiff);
    assert!(
        acceptable,
        "validation_error step yielded VariantMismatch; strict cross-driver error-code equality is deferred but Err/Err outcome agreement is required\n  cli={:?}\n  graphql={:?}",
        cli_results[0],
        graphql_results[0]
    );
}
