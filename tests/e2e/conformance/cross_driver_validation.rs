use super::driver_cli::CliDriver;
use super::driver_graphql::GraphqlDriver;
use super::result::ConformanceResult;
use super::workflows;

fn run_validation_error() -> (Vec<ConformanceResult>, Vec<ConformanceResult>) {
    let fixture = workflows::validation_error();
    let cli = CliDriver::new();
    let graphql = GraphqlDriver::new();
    (cli.run_workflow(&fixture), graphql.run_workflow(&fixture))
}

#[test]
fn validation_error_result_count_matches_step_count() {
    let fixture = workflows::validation_error();
    let (cli_results, graphql_results) = run_validation_error();
    assert_eq!(cli_results.len(), fixture.steps.len());
    assert_eq!(graphql_results.len(), fixture.steps.len());
}
