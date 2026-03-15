use crate::common::{ServerGuard, ZdbTestRepo};
use predicates::prelude::*;

fn create_with_checkboxes(repo: &ZdbTestRepo, title: &str, body: &str) {
    let body_arg = format!("--body={body}");
    repo.zdb()
        .args(["create", "--title", title, &body_arg])
        .assert()
        .success();
}

#[test]
fn checkbox_items_indexed_via_sql() {
    let repo = ZdbTestRepo::init();
    create_with_checkboxes(
        &repo,
        "tasks",
        "- [ ] buy groceries\n- [x] send email\n- [i] meeting notes",
    );

    repo.zdb()
        .args([
            "query",
            "SELECT state, content FROM _zdb_checkboxes ORDER BY line_number",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("open"))
        .stdout(predicate::str::contains("buy groceries"))
        .stdout(predicate::str::contains("done"))
        .stdout(predicate::str::contains("send email"))
        .stdout(predicate::str::contains("info"))
        .stdout(predicate::str::contains("meeting notes"));
}

#[test]
fn checkbox_filter_by_state() {
    let repo = ZdbTestRepo::init();
    create_with_checkboxes(
        &repo,
        "mixed",
        "- [ ] task one\n- [x] task two\n- [ ] task three",
    );

    repo.zdb()
        .args([
            "query",
            "SELECT content FROM _zdb_checkboxes WHERE state = 'open' ORDER BY line_number",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("task one"))
        .stdout(predicate::str::contains("task three"))
        .stdout(predicate::str::contains("task two").not());
}

#[test]
fn checkbox_graphql_queries() {
    let repo = ZdbTestRepo::init();
    create_with_checkboxes(
        &repo,
        "gql test",
        "- [ ] open item\n- [x] done item\n- [i] info item",
    );

    let server = ServerGuard::start(&repo);

    // checkboxItems — all states
    let result = server.graphql("{ checkboxItems { state content zettelTitle } }");
    let items = &result["data"]["checkboxItems"];
    assert!(items.is_array());
    assert_eq!(items.as_array().unwrap().len(), 3);

    // openActions — only open
    let result = server.graphql("{ openActions { state content } }");
    let items = &result["data"]["openActions"];
    assert!(items.is_array());
    let items = items.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["state"], "open");
    assert_eq!(items[0]["content"], "open item");

    // checkboxItems with state filter
    let result = server.graphql("{ checkboxItems(state: \"done\") { content } }");
    let items = result["data"]["checkboxItems"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["content"], "done item");
}
