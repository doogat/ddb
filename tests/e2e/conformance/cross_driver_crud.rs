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

// PRD 00150 D1 #17: pin the Search step in crud_baseline at the
// cross-driver variant level. The Search step probes the live row
// created in step 0; both CLI and GraphQL drivers must return Ok.
// Value-level ExpectedBehavior enforcement on Search content is
// deferred until the comparator gains value-level support.
#[test]
fn crud_baseline_search_returns_ok_on_both_drivers() {
    let fixture = workflows::crud_baseline();
    let search_idx = fixture
        .steps
        .iter()
        .position(|s| s.op == super::fixture::StepOp::Search)
        .expect("crud_baseline fixture has a Search step");
    let (cli_results, graphql_results) = run_crud_baseline();
    assert!(
        matches!(
            cli_results[search_idx],
            super::result::ConformanceResult::Ok { .. }
        ),
        "CLI search should return Ok, got: {:?}",
        cli_results[search_idx]
    );
    assert!(
        matches!(
            graphql_results[search_idx],
            super::result::ConformanceResult::Ok { .. }
        ),
        "GraphQL search should return Ok, got: {:?}",
        graphql_results[search_idx]
    );
}

// The not-found contract is shape-level for now: both drivers must
// return Err, even though their error codes still differ.
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
}

/// Expected hit titles for each `Search` step of `workflows::filtered_search`,
/// in fixture order: type only, tag only, type + tag, unfiltered control.
const FILTERED_SEARCH_EXPECTED_TITLES: [&[&str]; 4] = [
    &["Thai soup"],
    &["Lime cake", "Plain note"],
    &["Lime cake"],
    &["Lime cake", "Plain note", "Thai soup"],
];

/// `(step index, id)` for every `CreateDoogat` step that returned an id.
fn created_ids(
    fixture: &super::fixture::WorkflowFixture,
    results: &[super::result::ConformanceResult],
) -> Vec<(usize, String)> {
    fixture
        .steps
        .iter()
        .zip(results)
        .enumerate()
        .filter(|(_, (step, _))| step.op == super::fixture::StepOp::CreateDoogat)
        .filter_map(|(i, (_, result))| match result {
            super::result::ConformanceResult::Ok {
                value: super::result::ConformanceValue::String(id),
                ..
            } => Some((i, id.clone())),
            _ => None,
        })
        .collect()
}

fn search_value<'a>(driver: &str, step: usize, result: &'a super::result::ConformanceResult) -> &'a str {
    match result {
        super::result::ConformanceResult::Ok {
            value: super::result::ConformanceValue::String(text),
            ..
        } => text,
        other => panic!("{driver} search step {step} should return Ok(String), got: {other:?}"),
    }
}

// PRD 00176: SearchCommand's `types` / `tag` filters must narrow the same way
// on CLI and GraphQL, and both must return the same hits with all ten core
// `SearchResult` fields equal. Each driver runs in its own repo, so ids and
// wall-clock timestamps are anonymized first (`anonymize_search_hits`); every
// other field (title, path shape, snippet, tags, type, fields, and whether
// created_at / updated_at are set) must match exactly, and the BM25 rank
// within a 1e-9 relative tolerance (`search_hits_match`).
#[test]
fn filtered_search_returns_same_hits_and_full_field_set_on_cli_and_graphql() {
    use super::comparator::{
        anonymize_search_hits, parse_cli_search_hits, parse_graphql_search_hits,
        search_hits_match,
    };

    let fixture = workflows::filtered_search();
    let cli_results = CliDriver::new().run_workflow(&fixture);
    let graphql_results = GraphqlDriver::new().run_workflow(&fixture);
    assert_eq!(cli_results.len(), fixture.steps.len());
    assert_eq!(graphql_results.len(), fixture.steps.len());

    // Seeding (typedefs + creates) must succeed on both before search parity
    // means anything.
    for (i, step) in fixture.steps.iter().enumerate() {
        if step.op != super::fixture::StepOp::Search {
            for (driver, results) in [("CLI", &cli_results), ("GraphQL", &graphql_results)] {
                assert!(
                    matches!(results[i], super::result::ConformanceResult::Ok { .. }),
                    "{driver} seed step {i} ({:?}) should return Ok, got: {:?}",
                    step.op,
                    results[i]
                );
            }
        }
    }

    let cli_created = created_ids(&fixture, &cli_results);
    let graphql_created = created_ids(&fixture, &graphql_results);
    let search_steps: Vec<usize> = fixture
        .steps
        .iter()
        .enumerate()
        .filter(|(_, s)| s.op == super::fixture::StepOp::Search)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        search_steps.len(),
        FILTERED_SEARCH_EXPECTED_TITLES.len(),
        "filtered_search Search steps and expected-title table are out of sync"
    );

    for (&step, expected_titles) in search_steps.iter().zip(FILTERED_SEARCH_EXPECTED_TITLES) {
        let args = &fixture.steps[step].args;
        let cli_hits = parse_cli_search_hits(search_value("CLI", step, &cli_results[step]))
            .unwrap_or_else(|e| panic!("CLI search step {step} ({args}): {e}"));
        let graphql_hits =
            parse_graphql_search_hits(search_value("GraphQL", step, &graphql_results[step]))
                .unwrap_or_else(|e| panic!("GraphQL search step {step} ({args}): {e}"));
        let cli_hits = anonymize_search_hits(&cli_hits, &cli_created)
            .unwrap_or_else(|e| panic!("CLI search step {step} ({args}): {e}"));
        let graphql_hits = anonymize_search_hits(&graphql_hits, &graphql_created)
            .unwrap_or_else(|e| panic!("GraphQL search step {step} ({args}): {e}"));

        let mut titles: Vec<&str> = cli_hits.iter().map(|h| h.title.as_str()).collect();
        titles.sort_unstable();
        assert_eq!(
            titles, expected_titles,
            "search step {step} ({args}) returned the wrong doogats"
        );
        assert!(
            search_hits_match(&cli_hits, &graphql_hits),
            "search step {step} ({args}): CLI and GraphQL hits differ\n  cli={cli_hits:?}\n  graphql={graphql_hits:?}"
        );

        // Typed hits must carry their type and typed fields; this keeps the
        // field-set comparison from passing on two all-null transports.
        for hit in &cli_hits {
            if hit.title != "Plain note" {
                assert!(
                    hit.doogat_type.is_some() && hit.fields.is_some() && !hit.tags.is_empty(),
                    "search step {step}: typed hit lost type/fields/tags: {hit:?}"
                );
            }
        }
    }
}
