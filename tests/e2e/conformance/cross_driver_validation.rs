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

/// Variant-level parity: the malformed request is rejected as an error on BOTH
/// interfaces. This is the CRUD-baseline error contract — neither interface may
/// silently accept input the other rejects. (Code-level parity is out of reach
/// because CLI errors are `Specialized`; see the GraphQL/CLI tests below.)
#[test]
fn validation_error_both_drivers_return_err() {
    let (cli_results, graphql_results) = run_validation_error();
    assert!(
        matches!(cli_results[0], ConformanceResult::Err(_)),
        "CLI should reject the malformed request, got: {:?}",
        cli_results[0]
    );
    assert!(
        matches!(graphql_results[0], ConformanceResult::Err(_)),
        "GraphQL should reject the malformed request, got: {:?}",
        graphql_results[0]
    );
}

/// GraphQL is the flagship network interface (D-1): it carries the
/// machine-readable code in `extensions.code`, and it must match the code this
/// fixture promises (`BAD_REQUEST`).
#[test]
fn validation_error_graphql_reports_expected_code() {
    let fixture = workflows::validation_error();
    let expected = fixture
        .expected
        .error
        .as_ref()
        .expect("validation_error fixture declares an expected error");
    let (_, graphql_results) = run_validation_error();
    match &graphql_results[0] {
        ConformanceResult::Err(e) => assert_eq!(
            e.code, expected.code,
            "GraphQL machine-readable code must match the fixture contract"
        ),
        other => panic!("GraphQL should return Err, got: {other:?}"),
    }
}

/// CLI structured errors are `Specialized` (D-CLI-1): the CLI surfaces failures
/// as text + a non-zero exit code, so the driver reports the catch-all
/// `CLI_ERROR` rather than the shared machine code. This test pins that the CLI
/// rejection is Specialized (an error, but not the GraphQL code), documenting
/// why code-level parity is intentionally not asserted across the two.
#[test]
fn validation_error_cli_error_is_specialized() {
    let (cli_results, _) = run_validation_error();
    match &cli_results[0] {
        ConformanceResult::Err(e) => assert_eq!(
            e.code, "CLI_ERROR",
            "CLI errors are Specialized (text + exit code), so the driver reports CLI_ERROR"
        ),
        other => panic!("CLI should return Err, got: {other:?}"),
    }
}
