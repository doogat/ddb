use crate::common::{select_scalar, DdbTestRepo, ServerGuard};

#[test]
fn integration_44_e1_join_pin_graphql() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let c1 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE e1_link (url VARCHAR(255))") { message } }"#,
    );
    assert!(c1.get("errors").is_none(), "CREATE e1_link failed: {c1}");
    let c2 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE e1_num (count INTEGER)") { message } }"#,
    );
    assert!(c2.get("errors").is_none(), "CREATE e1_num failed: {c2}");

    let i1 = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO e1_link (title, url) VALUES (\"a\", \"https://a.com\")") { message } }"#,
    );
    assert!(i1.get("errors").is_none(), "insert e1_link failed: {i1}");
    let i2 = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO e1_num (title, count) VALUES (\"a\", 1)") { message } }"#,
    );
    assert!(i2.get("errors").is_none(), "insert e1_num failed: {i2}");

    let joined = server.graphql(
        r#"{ sql(query: "SELECT l.title, n.count FROM e1_link l JOIN e1_num n ON l.title = n.title") { rows } }"#,
    );
    assert!(
        joined.get("errors").is_none(),
        "JOIN query failed: {joined}"
    );
    let rows = joined["data"]["sql"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "expected exactly one joined row: {joined}");
    let row_str = rows[0].as_str().unwrap();
    assert!(
        row_str.contains('a'),
        "joined row should contain title 'a': {row_str}"
    );
    assert!(
        row_str.contains('1'),
        "joined row should contain count 1: {row_str}"
    );
}

#[test]
fn integration_44_j_auto_junction_atomic_graphql() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let c1 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE j134_cat (label VARCHAR(100))") { message } }"#,
    );
    assert!(c1.get("errors").is_none(), "CREATE j134_cat failed: {c1}");
    let c2 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE j134_bm (url TEXT, category TEXT REFERENCES j134_cat)") { message } }"#,
    );
    assert!(c2.get("errors").is_none(), "CREATE j134_bm failed: {c2}");

    let cat_a = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO j134_cat (title, label) VALUES (\"alpha\", \"alpha\")") { message } }"#,
    );
    assert!(cat_a.get("errors").is_none(), "insert cat_a failed: {cat_a}");
    let cat_a_id = cat_a["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    let cat_b = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO j134_cat (title, label) VALUES (\"beta\", \"beta\")") { message } }"#,
    );
    assert!(cat_b.get("errors").is_none(), "insert cat_b failed: {cat_b}");
    let cat_b_id = cat_b["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    let bm = server.graphql(&format!(
        r#"mutation {{ executeSql(sql: "INSERT INTO j134_bm (url, category) VALUES ('https://j134.example', '{cat_a_id}')") {{ message }} }}"#
    ));
    assert!(bm.get("errors").is_none(), "insert bookmark failed: {bm}");
    let bm_id = bm["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    let junction_cat = select_scalar(
        &server,
        &format!("SELECT category_id FROM j134_bm_category WHERE j134_bm_id = '{bm_id}'"),
    );
    assert_eq!(
        junction_cat, cat_a_id,
        "junction row should reference cat_a"
    );

    let update = server.graphql(&format!(
        r#"mutation {{ executeSql(sql: "UPDATE j134_bm SET category = '{cat_b_id}' WHERE id = '{bm_id}'") {{ message }} }}"#
    ));
    assert!(
        update.get("errors").is_none(),
        "update category failed: {update}"
    );

    let old_count = select_scalar(
        &server,
        &format!(
            "SELECT COUNT(*) FROM j134_bm_category WHERE j134_bm_id = '{bm_id}' AND category_id = '{cat_a_id}'"
        ),
    );
    assert_eq!(
        old_count, "0",
        "old junction row should be removed after update"
    );

    let new_count = select_scalar(
        &server,
        &format!(
            "SELECT COUNT(*) FROM j134_bm_category WHERE j134_bm_id = '{bm_id}' AND category_id = '{cat_b_id}'"
        ),
    );
    assert_eq!(
        new_count, "1",
        "new junction row should be present after update"
    );
}

#[test]
fn integration_44_k_create_doogat_auto_junction() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let c1 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE j134_cat (label VARCHAR(100))") { message } }"#,
    );
    assert!(c1.get("errors").is_none(), "CREATE j134_cat failed: {c1}");
    let c2 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE j134_bm (url TEXT, category TEXT REFERENCES j134_cat)") { message } }"#,
    );
    assert!(c2.get("errors").is_none(), "CREATE j134_bm failed: {c2}");

    let cat = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO j134_cat (title, label) VALUES (\"gamma\", \"gamma\")") { message } }"#,
    );
    assert!(cat.get("errors").is_none(), "insert category failed: {cat}");
    let cat_id = cat["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    let created = server.graphql(&format!(
        r#"mutation {{ createDoogat(input: {{ type: "j134_bm", title: "K", fields: "{{\"url\":\"https://k.example\",\"category\":\"{cat_id}\"}}" }}) {{ id }} }}"#
    ));
    assert!(
        created.get("errors").is_none(),
        "createDoogat failed: {created}"
    );
    let bm_id = created["data"]["createDoogat"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let count = select_scalar(
        &server,
        &format!(
            "SELECT COUNT(*) FROM j134_bm_category WHERE j134_bm_id = '{bm_id}' AND category_id = '{cat_id}'"
        ),
    );
    assert_eq!(
        count, "1",
        "auto-junction row should exist after createDoogat"
    );
}

#[test]
fn integration_44_m_parent_junction_cleanup_graphql() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let c1 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE pdc_cat (label VARCHAR(100))") { message } }"#,
    );
    assert!(c1.get("errors").is_none(), "CREATE pdc_cat failed: {c1}");
    let c2 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE pdc_bm (url TEXT, pdc_cat TEXT REFERENCES pdc_cat)") { message } }"#,
    );
    assert!(c2.get("errors").is_none(), "CREATE pdc_bm failed: {c2}");

    let cat = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO pdc_cat (title, label) VALUES (\"cat1\", \"cat1\")") { message } }"#,
    );
    assert!(
        cat.get("errors").is_none(),
        "insert pdc_cat row failed: {cat}"
    );
    let cat_id = cat["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    let bm = server.graphql(&format!(
        r#"mutation {{ executeSql(sql: "INSERT INTO pdc_bm (url, pdc_cat) VALUES ('https://pdc.example', '{cat_id}')") {{ message }} }}"#
    ));
    assert!(
        bm.get("errors").is_none(),
        "insert pdc_bm row failed: {bm}"
    );
    let bm_id = bm["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    let count_before = select_scalar(
        &server,
        &format!("SELECT COUNT(*) FROM pdc_bm_pdc_cat WHERE pdc_bm_id = '{bm_id}'"),
    );
    assert_eq!(count_before, "1", "junction row should exist after insert");

    let delete = server.graphql(&format!(
        r#"mutation {{ executeSql(sql: "DELETE FROM pdc_bm WHERE id = '{bm_id}'") {{ message }} }}"#
    ));
    assert!(
        delete.get("errors").is_none(),
        "delete pdc_bm row failed: {delete}"
    );

    let count_after_scoped = select_scalar(
        &server,
        &format!("SELECT COUNT(*) FROM pdc_bm_pdc_cat WHERE pdc_bm_id = '{bm_id}'"),
    );
    assert_eq!(
        count_after_scoped, "0",
        "junction row should be cleaned up on parent delete"
    );

    let count_after_total = select_scalar(&server, "SELECT COUNT(*) FROM pdc_bm_pdc_cat");
    assert_eq!(count_after_total, "0", "junction table should be empty");
}
