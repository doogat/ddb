use crate::common::{DdbTestRepo, ServerGuard};

/// §42b base-field orderBy (created_at/updated_at/id) + deterministic
/// pagination (PRD 00158), exercised through a live `ddb serve` process via
/// `ServerGuard`. Mirrors tests/integration.sh:1529-1601. Rows are inserted
/// with distinct dates scrambled relative to insertion order so that
/// created_at ASC, created_at DESC, and id ASC each produce a different
/// ordering, binding the sort direction.
///
///   subject | date       | insertion order (id ASC)
///   n0      | 2026-03-01 | 1st  (renamed to n0e after update)
///   n1      | 2026-01-01 | 2nd
///   n2      | 2026-04-01 | 3rd
///   n3      | 2026-02-01 | 4th
#[test]
fn integration_42b_orderby_base_fields_deterministic_pagination() {
    fn subjects(ob: &serde_json::Value) -> Vec<&str> {
        ob["data"]["obnotes"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["subject"].as_str().unwrap())
            .collect()
    }

    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE obnote (subject TEXT NOT NULL)") { message } }"#,
    );
    assert!(
        create.get("errors").is_none(),
        "CREATE obnote failed: {create}"
    );
    assert!(
        create["data"]["executeSql"]["message"]
            .as_str()
            .unwrap()
            .contains("table obnote created"),
        "unexpected CREATE TABLE message: {create}"
    );

    let n0 = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO obnote (subject, date) VALUES ('n0', '2026-03-01')") { message } }"#,
    );
    assert!(n0.get("errors").is_none(), "insert n0 failed: {n0}");
    std::thread::sleep(std::time::Duration::from_secs(1));

    let n1 = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO obnote (subject, date) VALUES ('n1', '2026-01-01')") { message } }"#,
    );
    assert!(n1.get("errors").is_none(), "insert n1 failed: {n1}");
    std::thread::sleep(std::time::Duration::from_secs(1));

    let n2 = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO obnote (subject, date) VALUES ('n2', '2026-04-01')") { message } }"#,
    );
    assert!(n2.get("errors").is_none(), "insert n2 failed: {n2}");
    std::thread::sleep(std::time::Duration::from_secs(1));

    let n3 = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO obnote (subject, date) VALUES ('n3', '2026-02-01')") { message } }"#,
    );
    assert!(n3.get("errors").is_none(), "insert n3 failed: {n3}");
    std::thread::sleep(std::time::Duration::from_secs(1));

    // n0 -> n0e: re-indexed last (UPDATE), so it becomes the newest by updated_at.
    let rename = server.graphql(
        r#"mutation { executeSql(sql: "UPDATE obnote SET subject = 'n0e' WHERE subject = 'n0'") { message } }"#,
    );
    assert!(
        rename.get("errors").is_none(),
        "rename n0->n0e failed: {rename}"
    );

    // created_at ASC: date order Jan < Feb < Mar < Apr = [n1, n3, n0e, n2].
    let ob = server.graphql(
        "{ obnotes(orderBy: { created_at: ASC }) { items { subject } totalCount } }",
    );
    assert!(
        ob.get("errors").is_none(),
        "orderBy created_at ASC failed: {ob}"
    );
    assert_eq!(subjects(&ob), vec!["n1", "n3", "n0e", "n2"]);
    assert_eq!(ob["data"]["obnotes"]["totalCount"], serde_json::json!(4));

    // created_at DESC: date order Apr > Mar > Feb > Jan = [n2, n0e, n3, n1].
    let ob = server.graphql("{ obnotes(orderBy: { created_at: DESC }) { items { subject } } }");
    assert!(
        ob.get("errors").is_none(),
        "orderBy created_at DESC failed: {ob}"
    );
    assert_eq!(subjects(&ob), vec!["n2", "n0e", "n3", "n1"]);

    // updated_at DESC: n0e was re-indexed last (UPDATE), so it is newest.
    let ob = server.graphql("{ obnotes(orderBy: { updated_at: DESC }) { items { subject } } }");
    assert!(
        ob.get("errors").is_none(),
        "orderBy updated_at DESC failed: {ob}"
    );
    assert_eq!(subjects(&ob), vec!["n0e", "n3", "n2", "n1"]);

    // updated_at ASC: n1 has the oldest updated_at, n0e the newest.
    let ob = server.graphql("{ obnotes(orderBy: { updated_at: ASC }) { items { subject } } }");
    assert!(
        ob.get("errors").is_none(),
        "orderBy updated_at ASC failed: {ob}"
    );
    assert_eq!(subjects(&ob), vec!["n1", "n2", "n3", "n0e"]);

    // id ASC: insertion order.
    let ob = server.graphql("{ obnotes(orderBy: { id: ASC }) { items { subject } } }");
    assert!(ob.get("errors").is_none(), "orderBy id ASC failed: {ob}");
    assert_eq!(subjects(&ob), vec!["n0e", "n1", "n2", "n3"]);

    // id DESC: reverse insertion order.
    let ob = server.graphql("{ obnotes(orderBy: { id: DESC }) { items { subject } } }");
    assert!(ob.get("errors").is_none(), "orderBy id DESC failed: {ob}");
    assert_eq!(subjects(&ob), vec!["n3", "n2", "n1", "n0e"]);

    // Deterministic pagination over created_at ASC: page1 + page2 reconstruct
    // the full order with no gaps or dupes (the id tiebreaker guarantees a
    // total order).
    let p1 = server.graphql(
        "{ obnotes(orderBy: { created_at: ASC }, limit: 2, offset: 0) { items { subject } } }",
    );
    assert!(p1.get("errors").is_none(), "pagination page1 failed: {p1}");
    assert_eq!(subjects(&p1), vec!["n1", "n3"]);

    let p2 = server.graphql(
        "{ obnotes(orderBy: { created_at: ASC }, limit: 2, offset: 2) { items { subject } } }",
    );
    assert!(p2.get("errors").is_none(), "pagination page2 failed: {p2}");
    assert_eq!(subjects(&p2), vec!["n0e", "n2"]);

    let drop = server.graphql(
        r#"mutation { executeSql(sql: "DROP TABLE obnote CASCADE") { message } }"#,
    );
    assert!(
        drop.get("errors").is_none(),
        "DROP TABLE obnote failed: {drop}"
    );
}
