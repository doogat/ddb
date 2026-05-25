//! GraphqlDriver tests, split out from `driver_graphql.rs` to keep the
//! production module under the 500-line file limit (PRD 00148 cycle-2 F2).

use super::driver_graphql::GraphqlDriver;
use super::fixture::{
    AuthMode, ExpectedBehavior, InterfaceId, SetupExpectation, Step, StepOp, WorkflowFixture,
};
use super::result::{ConformanceError, ConformanceResult, ConformanceValue};

fn fx(steps: Vec<Step>, timeout_ms: u64) -> WorkflowFixture {
    WorkflowFixture {
        id: "test".into(),
        title: "Test".into(),
        setup: SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms,
            setup_steps: vec![],
        },
        steps,
        expected: ExpectedBehavior {
            value: None,
            warnings: vec![],
            error: None,
        },
        interfaces: vec![InterfaceId::Graphql],
    }
}

#[test]
fn missing_required_arg_returns_setup_failed() {
    let driver = GraphqlDriver::new();
    let fixture = fx(
        vec![Step {
            op: StepOp::ReadDoogat,
            args: serde_json::json!({}),
        }],
        30_000,
    );
    let results = driver.run_workflow(&fixture);
    assert_eq!(results.len(), 1);
    match &results[0] {
        ConformanceResult::SetupFailed { reason } => {
            assert!(!reason.is_empty(), "reason should be non-empty");
        }
        other => panic!("expected SetupFailed, got: {other:?}"),
    }
}

/// Transport-level failure (server unreachable) must surface as
/// `SetupFailed` with a `reason` that names the transport error class,
/// not panic and not return `Ok` or `Err`. Mirrors `CliDriver`'s
/// `RunError::Timeout -> SetupFailed` contract and covers the
/// `post_graphql` `.send()` arm added in c4b9d4e plus the `.json()`
/// timeout arm added in b9699dd (PRD 00148 C3-F1/F2).
#[test]
fn transport_error_returns_setup_failed() {
    let mut driver = GraphqlDriver::new();
    driver.kill_server_for_test();
    let fixture = fx(
        vec![Step {
            op: StepOp::ListDoogats,
            args: serde_json::json!({}),
        }],
        5_000,
    );
    let results = driver.run_workflow(&fixture);
    assert_eq!(results.len(), 1);
    match &results[0] {
        ConformanceResult::SetupFailed { reason } => {
            let expected_prefixes = [
                "graphql request failed",
                "graphql request timed out",
                "graphql response not valid json",
            ];
            assert!(
                expected_prefixes.iter().any(|p| reason.starts_with(p)),
                "expected transport-error reason prefix, got: {reason}"
            );
        }
        other => panic!("expected SetupFailed, got: {other:?}"),
    }
}

#[test]
fn create_returns_id_in_value() {
    let driver = GraphqlDriver::new();
    let fixture = fx(
        vec![Step {
            op: StepOp::CreateDoogat,
            args: serde_json::json!({"title": "Test doogat"}),
        }],
        30_000,
    );
    let results = driver.run_workflow(&fixture);
    assert_eq!(results.len(), 1);
    match &results[0] {
        ConformanceResult::Ok { value, warnings: _ } => match value {
            ConformanceValue::String(id) => {
                assert_eq!(id.len(), 14, "id should be 14 digits, got: {id}");
                assert!(
                    id.chars().all(|c| c.is_ascii_digit()),
                    "id should be all ASCII digits, got: {id}"
                );
            }
            other => panic!("expected String(id), got: {other:?}"),
        },
        other => panic!("expected Ok, got: {other:?}"),
    }
}

#[test]
fn create_then_read_round_trip_shows_title() {
    let driver = GraphqlDriver::new();
    let title = "Round trip title";
    let create_step = Step {
        op: StepOp::CreateDoogat,
        args: serde_json::json!({"title": title}),
    };
    let create_fixture = fx(vec![create_step], 30_000);
    let create_results = driver.run_workflow(&create_fixture);
    assert_eq!(create_results.len(), 1);
    let id = match &create_results[0] {
        ConformanceResult::Ok {
            value: ConformanceValue::String(id),
            ..
        } => id.clone(),
        other => panic!("expected Ok(String(id)), got: {other:?}"),
    };

    let read_fixture = fx(
        vec![Step {
            op: StepOp::ReadDoogat,
            args: serde_json::json!({"id": id}),
        }],
        30_000,
    );
    let read_results = driver.run_workflow(&read_fixture);
    assert_eq!(read_results.len(), 1);
    match &read_results[0] {
        ConformanceResult::Ok {
            value: ConformanceValue::String(text),
            ..
        } => {
            assert!(!text.is_empty(), "read text should be non-empty");
            assert!(
                text.contains(title),
                "read text should contain title {title}, got: {text}"
            );
        }
        other => panic!("expected Ok(String(text)), got: {other:?}"),
    }
}

#[test]
fn read_nonexistent_returns_err() {
    let driver = GraphqlDriver::new();
    let fixture = fx(
        vec![Step {
            op: StepOp::ReadDoogat,
            args: serde_json::json!({"id": "99999999999999"}),
        }],
        30_000,
    );
    let results = driver.run_workflow(&fixture);
    assert_eq!(results.len(), 1);
    match &results[0] {
        ConformanceResult::Err(ConformanceError { code, .. }) => {
            assert!(!code.is_empty(), "error code should be non-empty");
        }
        other => panic!("expected Err, got: {other:?}"),
    }
}

#[test]
fn create_then_delete_succeeds_with_null_value() {
    let driver = GraphqlDriver::new();
    let create_fixture = fx(
        vec![Step {
            op: StepOp::CreateDoogat,
            args: serde_json::json!({"title": "To delete"}),
        }],
        30_000,
    );
    let create_results = driver.run_workflow(&create_fixture);
    assert_eq!(create_results.len(), 1);
    let id = match &create_results[0] {
        ConformanceResult::Ok {
            value: ConformanceValue::String(id),
            ..
        } => id.clone(),
        other => panic!("expected Ok(String(id)), got: {other:?}"),
    };

    let delete_fixture = fx(
        vec![Step {
            op: StepOp::DeleteDoogat,
            args: serde_json::json!({"id": id}),
        }],
        30_000,
    );
    let delete_results = driver.run_workflow(&delete_fixture);
    assert_eq!(delete_results.len(), 1);
    match &delete_results[0] {
        ConformanceResult::Ok { value, warnings: _ } => match value {
            ConformanceValue::Null => {}
            other => panic!("expected Null, got: {other:?}"),
        },
        other => panic!("expected Ok, got: {other:?}"),
    }
}

#[test]
fn list_after_create_contains_created_title() {
    let driver = GraphqlDriver::new();
    let title = "Listed title";
    let create_fixture = fx(
        vec![Step {
            op: StepOp::CreateDoogat,
            args: serde_json::json!({"title": title}),
        }],
        30_000,
    );
    let create_results = driver.run_workflow(&create_fixture);
    assert_eq!(create_results.len(), 1);
    assert!(
        matches!(&create_results[0], ConformanceResult::Ok { .. }),
        "create should succeed, got: {:?}",
        create_results[0]
    );

    let list_fixture = fx(
        vec![Step {
            op: StepOp::ListDoogats,
            args: serde_json::json!({}),
        }],
        30_000,
    );
    let list_results = driver.run_workflow(&list_fixture);
    assert_eq!(list_results.len(), 1);
    match &list_results[0] {
        ConformanceResult::Ok {
            value: ConformanceValue::String(text),
            ..
        } => {
            assert!(
                text.contains(title),
                "list output should contain title {title}, got: {text}"
            );
        }
        other => panic!("expected Ok(String(text)), got: {other:?}"),
    }
}

#[test]
fn resolves_dollar_zero_id_in_second_step_arg() {
    let driver = GraphqlDriver::new();
    let fixture = fx(
        vec![
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({"title": "T9 test"}),
            },
            Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({"id": "$0.id"}),
            },
        ],
        30_000,
    );
    let results = driver.run_workflow(&fixture);
    assert_eq!(results.len(), 2);

    let id = match &results[0] {
        ConformanceResult::Ok {
            value: ConformanceValue::String(id),
            ..
        } => id.clone(),
        other => panic!("step 0 expected Ok(String(id)), got: {other:?}"),
    };
    assert_eq!(id.len(), 14, "step 0 id should be 14 digits, got: {id}");

    match &results[1] {
        ConformanceResult::Ok {
            value: ConformanceValue::String(text),
            ..
        } => {
            assert!(
                text.contains("T9 test"),
                "step 1 text should contain title 'T9 test', got: {text}"
            );
            assert!(
                text.contains(&id),
                "step 1 text should contain id {id}, got: {text}"
            );
        }
        other => panic!("step 1 expected Ok(String(text)), got: {other:?}"),
    }
}

#[test]
fn unresolvable_ref_passes_literal_and_graphql_rejects_it() {
    let driver = GraphqlDriver::new();
    let fixture = fx(
        vec![
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({"title": "T9 unresolvable ref"}),
            },
            Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({"id": "$99.id"}),
            },
        ],
        30_000,
    );
    let results = driver.run_workflow(&fixture);
    assert_eq!(results.len(), 2);

    match &results[0] {
        ConformanceResult::Ok { .. } => {}
        other => panic!("step 0 expected Ok, got: {other:?}"),
    }

    match &results[1] {
        ConformanceResult::Err(_) => {}
        other => panic!("step 1 expected Err (invalid id), got: {other:?}"),
    }
}

#[test]
fn resolves_step_zero_ref_in_step_two_skipping_step_one() {
    let driver = GraphqlDriver::new();
    let fixture = fx(
        vec![
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({"title": "T9 indexed ref"}),
            },
            Step {
                op: StepOp::ListDoogats,
                args: serde_json::json!({}),
            },
            Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({"id": "$0.id"}),
            },
        ],
        30_000,
    );
    let results = driver.run_workflow(&fixture);
    assert_eq!(results.len(), 3);

    let id = match &results[0] {
        ConformanceResult::Ok {
            value: ConformanceValue::String(id),
            ..
        } => id.clone(),
        other => panic!("step 0 expected Ok(String(id)), got: {other:?}"),
    };

    match &results[1] {
        ConformanceResult::Ok { .. } => {}
        other => panic!("step 1 expected Ok, got: {other:?}"),
    }

    match &results[2] {
        ConformanceResult::Ok {
            value: ConformanceValue::String(text),
            ..
        } => {
            assert!(
                text.contains("T9 indexed ref"),
                "step 2 text should contain title 'T9 indexed ref', got: {text}"
            );
            assert!(
                text.contains(&id),
                "step 2 text should contain id {id} from step 0, got: {text}"
            );
        }
        other => panic!("step 2 expected Ok(String(text)), got: {other:?}"),
    }
}
