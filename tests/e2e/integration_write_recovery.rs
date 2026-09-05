use crate::common::{select_scalar, DdbTestRepo, ServerGuard};

#[test]
fn integration_45_a1_cross_mutation_parity_after_unique_failure() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE a1item (title VARCHAR(255) NOT NULL, name VARCHAR(255) NOT NULL, UNIQUE(name))") { message } }"#,
    );
    assert!(
        create.get("errors").is_none(),
        "CREATE a1item failed: {create}"
    );

    let valid = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO a1item (title, name) VALUES (\"a\", \"unique1\")") { message } }"#,
    );
    assert!(
        valid.get("errors").is_none(),
        "valid insert failed: {valid}"
    );
    let valid_id = valid["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    let dup = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO a1item (title, name) VALUES (\"b\", \"unique1\")") { message } }"#,
    );
    assert!(
        dup.get("errors").is_some(),
        "duplicate name insert should error: {dup}"
    );
    assert!(
        dup["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("UNIQUE"),
        "error message mismatch: {dup}"
    );

    // (a) updateDoogat still works on the same table
    let updated = server.graphql(&format!(
        r#"mutation {{ updateDoogat(input: {{ id: "{valid_id}", tags: ["a1-recovered"] }}) {{ id tags }} }}"#
    ));
    assert!(
        updated.get("errors").is_none(),
        "updateDoogat after UNIQUE failure should succeed: {updated}"
    );
    let tags = updated["data"]["updateDoogat"]["tags"].as_array().unwrap();
    assert!(
        tags.iter().any(|t| t.as_str() == Some("a1-recovered")),
        "updated tags should contain a1-recovered: {updated}"
    );

    // (b) createDoogat still works on the same table
    let created = server.graphql(
        r#"mutation { createDoogat(input: { type: "a1item", title: "created-after-rollback", fields: "{\"name\":\"unique2\"}" }) { id title } }"#,
    );
    assert!(
        created.get("errors").is_none(),
        "createDoogat after UNIQUE failure should succeed: {created}"
    );
    let created_id = created["data"]["createDoogat"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // (c) deleteDoogat still works on the same table
    let deleted = server.graphql(&format!(
        r#"mutation {{ deleteDoogat(id: "{created_id}") }}"#
    ));
    assert!(
        deleted.get("errors").is_none(),
        "deleteDoogat after UNIQUE failure should succeed: {deleted}"
    );
    assert_eq!(deleted["data"]["deleteDoogat"], serde_json::json!(true));
}

#[test]
fn integration_45_a3_cross_table_isolation_after_unique_failure() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let c1 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE a3thing (title VARCHAR(255) NOT NULL)") { message } }"#,
    );
    assert!(c1.get("errors").is_none(), "CREATE a3thing failed: {c1}");
    let c2 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE a3item (title VARCHAR(255) NOT NULL, name VARCHAR(255) NOT NULL, UNIQUE(name))") { message } }"#,
    );
    assert!(c2.get("errors").is_none(), "CREATE a3item failed: {c2}");

    let thing = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO a3thing (title) VALUES (\"thing1\")") { message } }"#,
    );
    assert!(
        thing.get("errors").is_none(),
        "insert a3thing failed: {thing}"
    );
    let thing_id = thing["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    let valid = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO a3item (title, name) VALUES (\"a\", \"u1\")") { message } }"#,
    );
    assert!(
        valid.get("errors").is_none(),
        "valid a3item insert failed: {valid}"
    );

    let dup = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO a3item (title, name) VALUES (\"b\", \"u1\")") { message } }"#,
    );
    assert!(
        dup.get("errors").is_some(),
        "duplicate name insert should error: {dup}"
    );
    assert!(
        dup["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("UNIQUE"),
        "error message mismatch: {dup}"
    );

    let updated = server.graphql(&format!(
        r#"mutation {{ updateDoogat(input: {{ id: "{thing_id}", tags: ["a3-isolated"] }}) {{ id tags }} }}"#
    ));
    assert!(
        updated.get("errors").is_none(),
        "updateDoogat on the sibling table should succeed: {updated}"
    );
    let tags = updated["data"]["updateDoogat"]["tags"].as_array().unwrap();
    assert!(
        tags.iter().any(|t| t.as_str() == Some("a3-isolated")),
        "updated tags should contain a3-isolated: {updated}"
    );
}

#[test]
fn integration_45_r10_restrict_blocks_sql_and_graphql_delete() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let c1 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE r10link (url VARCHAR(255) NOT NULL)") { message } }"#,
    );
    assert!(c1.get("errors").is_none(), "CREATE r10link failed: {c1}");
    let c2 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE r10cat (name VARCHAR(255) NOT NULL)") { message } }"#,
    );
    assert!(c2.get("errors").is_none(), "CREATE r10cat failed: {c2}");
    let c3 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE \"r10-mem\" (link_id VARCHAR(255) NOT NULL REFERENCES r10link(id), cat_id VARCHAR(255) NOT NULL REFERENCES r10cat(id), UNIQUE(link_id, cat_id))") { message } }"#,
    );
    assert!(c3.get("errors").is_none(), "CREATE r10-mem failed: {c3}");

    let link = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO r10link (url) VALUES (\"https://r10.example\")") { message } }"#,
    );
    assert!(
        link.get("errors").is_none(),
        "insert r10link failed: {link}"
    );
    let link_id = link["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    let cat = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO r10cat (name) VALUES (\"cat1\")") { message } }"#,
    );
    assert!(cat.get("errors").is_none(), "insert r10cat failed: {cat}");
    let cat_id = cat["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    let mem = server.graphql(&format!(
        r#"mutation {{ executeSql(sql: "INSERT INTO \"r10-mem\" (link_id, cat_id) VALUES ('{link_id}', '{cat_id}')") {{ message }} }}"#
    ));
    assert!(mem.get("errors").is_none(), "insert r10-mem failed: {mem}");

    let sql_delete = server.graphql(&format!(
        r#"mutation {{ executeSql(sql: "DELETE FROM r10link WHERE id = \"{link_id}\"") {{ message }} }}"#
    ));
    assert!(
        sql_delete.get("errors").is_some(),
        "SQL DELETE should be blocked by RESTRICT: {sql_delete}"
    );
    let sql_msg = sql_delete["errors"][0]["message"].as_str().unwrap();
    assert!(
        sql_msg.contains("NOT NULL REFERENCES"),
        "error message mismatch: {sql_msg}"
    );
    assert!(
        sql_msg.contains("r10-mem"),
        "error message should name the blocking child table: {sql_msg}"
    );

    let gql_delete = server.graphql(&format!(r#"mutation {{ deleteDoogat(id: "{link_id}") }}"#));
    assert!(
        gql_delete.get("errors").is_some(),
        "deleteDoogat should be blocked by RESTRICT: {gql_delete}"
    );
    assert!(
        gql_delete["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("NOT NULL REFERENCES"),
        "error message mismatch: {gql_delete}"
    );

    let link_count = select_scalar(
        &server,
        &format!("SELECT COUNT(*) FROM r10link WHERE id = \"{link_id}\""),
    );
    assert_eq!(link_count, "1", "parent row should still exist");
    let mem_count = select_scalar(
        &server,
        &format!("SELECT COUNT(*) FROM \"r10-mem\" WHERE link_id = \"{link_id}\""),
    );
    assert_eq!(mem_count, "1", "child row should still exist");

    let delete_child = server.graphql(&format!(
        r#"mutation {{ executeSql(sql: "DELETE FROM \"r10-mem\" WHERE link_id = \"{link_id}\"") {{ message }} }}"#
    ));
    assert!(
        delete_child.get("errors").is_none(),
        "deleting the child row should succeed: {delete_child}"
    );

    let delete_parent = server.graphql(&format!(
        r#"mutation {{ executeSql(sql: "DELETE FROM r10link WHERE id = \"{link_id}\"") {{ affected }} }}"#
    ));
    assert!(
        delete_parent.get("errors").is_none(),
        "parent delete should now succeed: {delete_parent}"
    );
    assert_eq!(
        delete_parent["data"]["executeSql"]["affected"],
        serde_json::json!(1)
    );
}

#[test]
fn integration_45_a2_ghost_row_survives_server_restart() {
    let repo = DdbTestRepo::init();
    let mut server = ServerGuard::start(&repo);

    let create = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE a2persist (title VARCHAR(255) NOT NULL, name VARCHAR(255) NOT NULL, UNIQUE(name))") { message } }"#,
    );
    assert!(
        create.get("errors").is_none(),
        "CREATE a2persist failed: {create}"
    );

    let valid = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO a2persist (title, name) VALUES (\"a\", \"uniq_a2\")") { message } }"#,
    );
    assert!(
        valid.get("errors").is_none(),
        "valid insert failed: {valid}"
    );
    let valid_id = valid["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    let dup = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO a2persist (title, name) VALUES (\"dup\", \"uniq_a2\")") { message } }"#,
    );
    assert!(
        dup.get("errors").is_some(),
        "duplicate name insert should error while the server is still up: {dup}"
    );

    server.kill();
    let server = ServerGuard::start(&repo);

    let updated = server.graphql(&format!(
        r#"mutation {{ updateDoogat(input: {{ id: "{valid_id}", tags: ["restart-survived"] }}) {{ id tags }} }}"#
    ));
    assert!(
        updated.get("errors").is_none(),
        "updateDoogat after restart should succeed: {updated}"
    );
    let tags = updated["data"]["updateDoogat"]["tags"].as_array().unwrap();
    assert!(
        tags.iter().any(|t| t.as_str() == Some("restart-survived")),
        "updated tags should contain restart-survived: {updated}"
    );

    let fresh = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO a2persist (title, name) VALUES (\"fresh\", \"uniq_a2_post\")") { message } }"#,
    );
    assert!(
        fresh.get("errors").is_none(),
        "fresh insert on the restarted server should succeed: {fresh}"
    );
    assert!(
        !fresh["data"]["executeSql"]["message"]
            .as_str()
            .unwrap()
            .is_empty(),
        "fresh INSERT must return an id: {fresh}"
    );
}
