use super::comparator::{compare_per_step, DiffClass};
use super::driver_cli::CliDriver;
use super::driver_graphql::GraphqlDriver;
use super::result::ConformanceResult;
use super::workflows;

/// Whether this step's `SchemaApply` result reports a real migration
/// (applied=true / at least one op) as opposed to a converged no-op, read
/// from whichever driver's value shape produced it (PRD 00184 task 7):
/// the GraphQL driver carries an `Object{applied, ops}` value; the CLI
/// driver carries the raw rendered stdout ("no changes" on a no-op, one
/// line per op otherwise -- ddb-cli/src/commands/schema.rs).
fn schema_apply_applied(result: &ConformanceResult) -> bool {
    match result {
        ConformanceResult::Ok {
            value: super::result::ConformanceValue::Object(fields),
            ..
        } => matches!(
            fields.get("applied"),
            Some(super::result::ConformanceValue::Bool(true))
        ),
        ConformanceResult::Ok {
            value: super::result::ConformanceValue::String(stdout),
            ..
        } => stdout != "no changes",
        other => panic!("expected an Ok schema-apply result, got: {other:?}"),
    }
}

/// Whether this step's `SchemaApply` result's op list names `kind`, read
/// from either driver's value shape (see `schema_apply_applied`).
fn schema_apply_has_op_kind(result: &ConformanceResult, kind: &str) -> bool {
    match result {
        ConformanceResult::Ok {
            value: super::result::ConformanceValue::Object(fields),
            ..
        } => match fields.get("ops") {
            Some(super::result::ConformanceValue::Array(ops)) => ops.iter().any(|op| {
                matches!(op, super::result::ConformanceValue::String(s) if s == kind)
            }),
            _ => false,
        },
        ConformanceResult::Ok {
            value: super::result::ConformanceValue::String(stdout),
            ..
        } => stdout.lines().any(|line| line.starts_with(kind)),
        other => panic!("expected an Ok schema-apply result, got: {other:?}"),
    }
}

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

// Regression (PRD 00184 task 7, BLAKE): assert `applied`/op-kind value-level
// outcomes, not just "every step is Ok", on both drivers. A buggy
// implementation that always returns applied=true (or never reports
// drop_unique_constraint) passed the variant-level checks above but must
// fail this one.
#[test]
fn index_evolution_migration_reports_applied_and_op_kinds_on_both_drivers() {
    let (cli_results, graphql_results) = run_index_evolution_migration();
    for results in [&cli_results, &graphql_results] {
        assert!(
            schema_apply_applied(&results[0]) && schema_apply_has_op_kind(&results[0], "create_type"),
            "step 0 (create) must report applied=true with a create_type op: {:?}",
            results[0]
        );
        assert!(
            schema_apply_applied(&results[1])
                && schema_apply_has_op_kind(&results[1], "add_unique_constraint"),
            "step 1 (migrate) must report applied=true with an add_unique_constraint op: {:?}",
            results[1]
        );
        assert!(
            !schema_apply_applied(&results[2]),
            "step 2 (re-apply) must report applied=false (converged no-op): {:?}",
            results[2]
        );
        assert!(
            schema_apply_applied(&results[3])
                && schema_apply_has_op_kind(&results[3], "drop_unique_constraint"),
            "step 3 (drop) must report applied=true with a drop_unique_constraint op: {:?}",
            results[3]
        );
    }
}

fn run_index_evolution_duplicate_enforcement_and_drop() -> (
    Vec<super::result::ConformanceResult>,
    Vec<super::result::ConformanceResult>,
) {
    let fixture = workflows::index_evolution_duplicate_enforcement_and_drop();
    let cli = CliDriver::new();
    let graphql = GraphqlDriver::new();
    (cli.run_workflow(&fixture), graphql.run_workflow(&fixture))
}

// Regression (PRD 00184 task 7, BLAKE/BOB): the migration fixture's "every
// step is Ok" assertions cannot see whether the unique constraint is
// actually enforced, or whether a drop actually lifts it (as opposed to
// just updating the typedef). This fixture's step 3 (duplicate insert right
// after the migration) must error on BOTH drivers, and step 5 (the same
// insert, after the drop) must succeed on BOTH -- with no variant mismatch
// at either point.
#[test]
fn index_evolution_duplicate_enforcement_and_drop_no_step_has_variant_mismatch() {
    let (cli_results, graphql_results) = run_index_evolution_duplicate_enforcement_and_drop();
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
fn index_evolution_duplicate_enforcement_and_drop_duplicate_is_rejected_then_allowed() {
    let (cli_results, graphql_results) = run_index_evolution_duplicate_enforcement_and_drop();
    for (name, results) in [("cli", &cli_results), ("graphql", &graphql_results)] {
        for i in [0usize, 1, 2, 4] {
            assert!(
                matches!(results[i], super::result::ConformanceResult::Ok { .. }),
                "{name} step {i} must succeed: {:?}",
                results[i]
            );
        }
        assert!(
            matches!(results[3], super::result::ConformanceResult::Err(_)),
            "{name} step 3 (duplicate insert post-migration) must be rejected: {:?}",
            results[3]
        );
        assert!(
            matches!(results[5], super::result::ConformanceResult::Ok { .. }),
            "{name} step 5 (duplicate insert post-drop) must succeed now the constraint is gone: {:?}",
            results[5]
        );
    }
}
