use crate::common::{DdbTestRepo, ServerGuard};

#[test]
fn integration_38f_execute_batch_multi_statement() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE batchfoo (bar TEXT, baz INTEGER)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE batchfoo failed: {r}");

    let result = server.graphql_with_vars(
        r#"mutation($stmts: [String!]!) { executeBatch(statements: $stmts) { message } }"#,
        serde_json::json!({ "stmts": [
            "INSERT INTO batchfoo (bar, baz) VALUES ('b1', 10)",
            "INSERT INTO batchfoo (bar, baz) VALUES ('b2', 20)"
        ] }),
    );
    assert!(
        result.get("errors").is_none(),
        "multi-statement INSERT batch failed: {result}"
    );

    // DDL-triggers-reload: CREATE TABLE inside executeBatch should hot-reload the schema
    let result = server.graphql_with_vars(
        r#"mutation($stmts: [String!]!) { executeBatch(statements: $stmts) { message } }"#,
        serde_json::json!({ "stmts": ["CREATE TABLE battest2 (col1 TEXT)"] }),
    );
    assert!(
        result.get("errors").is_none(),
        "DDL executeBatch failed: {result}"
    );
    let result = server.graphql(r#"{ battest2s { totalCount } }"#);
    assert!(
        result.get("errors").is_none(),
        "battest2s should resolve after DDL executeBatch: {result}"
    );
    assert_eq!(
        result["data"]["battest2s"]["totalCount"].as_i64().unwrap(),
        0
    );

    // Failure-rollback: a batch with a bad statement must fail entirely, leaving no partial writes
    let pre = server.graphql(r#"{ batchfoosAggregate { count } }"#);
    assert!(pre.get("errors").is_none(), "pre-count query failed: {pre}");
    let pre_count = pre["data"]["batchfoosAggregate"]["count"]
        .as_i64()
        .unwrap();

    let result = server.graphql_with_vars(
        r#"mutation($stmts: [String!]!) { executeBatch(statements: $stmts) { message } }"#,
        serde_json::json!({ "stmts": [
            "INSERT INTO batchfoo (bar, baz) VALUES ('rollback_test', 99)",
            "INSERT INTO no_such_table (bar) VALUES ('bad')"
        ] }),
    );
    assert!(
        result["errors"].is_array(),
        "batch with a bad statement should return errors: {result}"
    );

    let post = server.graphql(r#"{ batchfoosAggregate { count } }"#);
    assert!(
        post.get("errors").is_none(),
        "post-count query failed: {post}"
    );
    let post_count = post["data"]["batchfoosAggregate"]["count"]
        .as_i64()
        .unwrap();
    assert_eq!(
        post_count, pre_count,
        "failed batch should not persist any statement (all-or-nothing rollback)"
    );
}
