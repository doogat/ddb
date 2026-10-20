use super::comparator::{compare_per_step, DiffClass};
use super::driver_cli::CliDriver;
use super::driver_graphql::GraphqlDriver;
use super::workflows;

fn run_index_evolution_migration() -> (
    Vec<super::result::ConformanceResult>,
    Vec<super::result::ConformanceResult>,
) {
    let fixture = workflows::index_evolution_migration();
    let cli = CliDriver::new();
    let graphql = GraphqlDriver::new();
    (cli.run_workflow(&fixture), graphql.run_workflow(&fixture))
}

#[test]
fn index_evolution_migration_result_count_matches_step_count() {
    let fixture = workflows::index_evolution_migration();
    let (cli_results, graphql_results) = run_index_evolution_migration();
    let diffs = compare_per_step(&cli_results, &graphql_results);
    assert_eq!(diffs.len(), fixture.steps.len());
}

// Every step — the initial create, the add, and the no-op re-apply — must
// succeed on BOTH drivers. A VariantMismatch here means one interface
// reported the unique_together migration as unsupported (or errored) while
// the other applied it: the exact regression PRD 00184 fixes.
#[test]
fn index_evolution_migration_no_step_has_variant_mismatch() {
    let (cli_results, graphql_results) = run_index_evolution_migration();
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

#[test]
fn index_evolution_migration_all_steps_succeed_on_cli() {
    let (cli_results, _) = run_index_evolution_migration();
    for (i, result) in cli_results.iter().enumerate() {
        assert!(
            matches!(result, super::result::ConformanceResult::Ok { .. }),
            "step {i} failed on CLI: {result:?}"
        );
    }
}

#[test]
fn index_evolution_migration_all_steps_succeed_on_graphql() {
    let (_, graphql_results) = run_index_evolution_migration();
    for (i, result) in graphql_results.iter().enumerate() {
        assert!(
            matches!(result, super::result::ConformanceResult::Ok { .. }),
            "step {i} failed on GraphQL: {result:?}"
        );
    }
}
