use crate::common::{DdbTestRepo, ServerGuard};

struct CompletenessFixture {
    cat_rust_id: String,
    link1_id: String,
}

/// Fixture for `typed_where_filter_completeness`.
/// Creates `relcat` (title, slug) and `rellink` (url, category -> relcat),
/// inserts Rust/Python/Empty categories and rust/py/orphan links.
fn setup_completeness_fixture(server: &ServerGuard) -> CompletenessFixture {
    exec_sql(server, "CREATE TABLE relcat (title TEXT, slug TEXT)");
    exec_sql(
        server,
        "CREATE TABLE rellink (url TEXT, category TEXT REFERENCES relcat)",
    );
    std::thread::sleep(std::time::Duration::from_secs(1));

    let cat_rust_id = exec_sql_then_sleep(
        server,
        "INSERT INTO relcat (title, slug) VALUES ('Rust', 'rust')",
    );
    let cat_py_id = exec_sql_then_sleep(
        server,
        "INSERT INTO relcat (title, slug) VALUES ('Python', 'py')",
    );
    exec_sql_then_sleep(
        server,
        "INSERT INTO relcat (title, slug) VALUES ('Empty', 'empty')",
    );

    let link1_id = exec_sql(
        server,
        &format!(
            "INSERT INTO rellink (url, category) VALUES ('https://rust.example', '{cat_rust_id}')"
        ),
    );
    exec_sql_then_sleep(
        server,
        &format!(
            "INSERT INTO rellink (url, category) VALUES ('https://py.example', '{cat_py_id}')"
        ),
    );
    exec_sql(server, "INSERT INTO rellink (title) VALUES ('orphan-link')");
    CompletenessFixture { cat_rust_id, link1_id }
}

fn assert_negation_filters(server: &ServerGuard, link1_id: &str) {
    let r = server.graphql(&format!(
        r#"{{ rellinks(where: {{ id: {{ notIn: ["{link1_id}"] }} }}) {{ totalCount }} }}"#
    ));
    assert!(r.get("errors").is_none(), "notIn query failed: {r}");
    assert_eq!(r["data"]["rellinks"]["totalCount"].as_i64().unwrap(), 2, "notIn totalCount: {r}");

    let r = server.graphql(&format!(
        r#"{{ rellinks(where: {{ id: {{ nin: ["{link1_id}"] }} }}) {{ totalCount }} }}"#
    ));
    assert!(r.get("errors").is_none(), "nin query failed: {r}");
    assert_eq!(r["data"]["rellinks"]["totalCount"].as_i64().unwrap(), 2, "nin totalCount: {r}");
}

fn assert_null_filters(server: &ServerGuard) {
    let r = server.graphql(r#"{ rellinks(where: { url: { isNull: true } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "isNull true query failed: {r}");
    assert_eq!(r["data"]["rellinks"]["totalCount"].as_i64().unwrap(), 1, "isNull true: {r}");

    let r = server.graphql(r#"{ rellinks(where: { url: { isNull: false } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "isNull false query failed: {r}");
    assert_eq!(r["data"]["rellinks"]["totalCount"].as_i64().unwrap(), 2, "isNull false: {r}");
}

fn assert_forward_filters(server: &ServerGuard, cat_rust_id: &str) {
    let r = server.graphql(
        r#"{ rellinks(where: { category: { title: { eq: "Rust" } } }) { totalCount items { id url } } }"#,
    );
    assert!(r.get("errors").is_none(), "inlined title query failed: {r}");
    assert_eq!(r["data"]["rellinks"]["totalCount"].as_i64().unwrap(), 1, "inlined title count: {r}");
    let items = r["data"]["rellinks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "inlined title items: {r}");
    assert_eq!(items[0]["url"].as_str().unwrap(), "https://rust.example", "inlined title url: {r}");

    let r = server.graphql(r#"{ rellinks(where: { category: { slug: { eq: "py" } } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "inlined slug query failed: {r}");
    assert_eq!(r["data"]["rellinks"]["totalCount"].as_i64().unwrap(), 1, "inlined slug count: {r}");

    let r = server.graphql(&format!(
        r#"{{ rellinks(where: {{ category: {{ eq: "{cat_rust_id}" }} }}) {{ totalCount }} }}"#
    ));
    assert!(r.get("errors").is_none(), "category eq query failed: {r}");
    assert_eq!(r["data"]["rellinks"]["totalCount"].as_i64().unwrap(), 1, "category eq count: {r}");
}

fn assert_quantifier_filters(server: &ServerGuard) {
    let r = server.graphql(r#"{ rellinks(where: { category: { none: {} } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "none query failed: {r}");
    assert_eq!(r["data"]["rellinks"]["totalCount"].as_i64().unwrap(), 1, "none count: {r}");

    let r = server.graphql(r#"{ rellinks(where: { category: { some: {} } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "some query failed: {r}");
    assert_eq!(r["data"]["rellinks"]["totalCount"].as_i64().unwrap(), 2, "some count: {r}");
}

fn assert_reverse_filters(server: &ServerGuard) {
    let r = server.graphql(
        r#"{ relcats(where: { rellinks: { some: { url: { contains: "rust" } } } }) { totalCount items { id title } } }"#,
    );
    assert!(r.get("errors").is_none(), "reverse some query failed: {r}");
    assert_eq!(r["data"]["relcats"]["totalCount"].as_i64().unwrap(), 1, "reverse some count: {r}");
    let items = r["data"]["relcats"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "reverse some items: {r}");
    assert_eq!(items[0]["title"].as_str().unwrap(), "Rust", "reverse some title: {r}");

    let r = server.graphql(r#"{ relcats(where: { rellinks: { none: {} } }) { totalCount items { id title } } }"#);
    assert!(r.get("errors").is_none(), "reverse none query failed: {r}");
    assert_eq!(r["data"]["relcats"]["totalCount"].as_i64().unwrap(), 1, "reverse none count: {r}");
    let items = r["data"]["relcats"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "reverse none items: {r}");
    assert_eq!(items[0]["title"].as_str().unwrap(), "Empty", "reverse none title: {r}");
}

fn assert_pagination(server: &ServerGuard) {
    let r = server.graphql(
        r#"{ rellinks(where: { category: { some: {} } }, orderBy: { id: ASC }, limit: 1, offset: 0) { items { id url } totalCount } }"#,
    );
    assert!(r.get("errors").is_none(), "pagination page 0 failed: {r}");
    let items = r["data"]["rellinks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "page 0 item count: {r}");
    assert_eq!(r["data"]["rellinks"]["totalCount"].as_i64().unwrap(), 2, "page 0 totalCount: {r}");
    let p0_url = items[0]["url"].as_str().unwrap().to_string();

    let r = server.graphql(
        r#"{ rellinks(where: { category: { some: {} } }, orderBy: { id: ASC }, limit: 1, offset: 1) { items { id url } totalCount } }"#,
    );
    assert!(r.get("errors").is_none(), "pagination page 1 failed: {r}");
    let items = r["data"]["rellinks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "page 1 item count: {r}");
    assert_eq!(r["data"]["rellinks"]["totalCount"].as_i64().unwrap(), 2, "page 1 totalCount: {r}");
    let p1_url = items[0]["url"].as_str().unwrap().to_string();

    assert_ne!(p0_url, p1_url, "page 0 and page 1 should have different URLs");
}

/// Exhaustive test of the typed per-type `where:` filter on GraphQL
/// connection fields (rellinks / relcats), exercising negation, null,
/// inlined target columns, junction quantifiers, reverse membership,
/// and pagination + totalCount correlation.
#[test]
fn typed_where_filter_completeness() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let fixture = setup_completeness_fixture(&server);

    assert_negation_filters(&server, &fixture.link1_id);
    assert_null_filters(&server);
    assert_forward_filters(&server, &fixture.cat_rust_id);
    assert_quantifier_filters(&server);
    assert_reverse_filters(&server);
    assert_pagination(&server);
}

/// Helper: run a `CREATE TABLE`/`INSERT` (or any `executeSql`) mutation and
/// return the trimmed `message` (the new doogat id for INSERTs). Panics with
/// the full response on a GraphQL error.
fn exec_sql(server: &ServerGuard, sql: &str) -> String {
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": sql }),
    );
    assert!(r.get("errors").is_none(), "SQL failed ({sql}): {r}");
    r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Helper: `exec_sql` followed by a 1-second pause for index propagation.
fn exec_sql_then_sleep(server: &ServerGuard, sql: &str) -> String {
    let id = exec_sql(server, sql);
    std::thread::sleep(std::time::Duration::from_secs(1));
    id
}

/// Helper: collect item IDs from `r["data"][collection]["items"][*]["id"]`.
fn extract_item_ids(r: &serde_json::Value, collection: &str) -> Vec<String> {
    r["data"][collection]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["id"].as_str().unwrap().to_string())
        .collect()
}

/// Fixture for `every_treats_null_target_column_as_non_counterexample`.
/// Returns `(cat_null, cat_red, cat_blue, link_null, link_red, link_blue)`.
fn setup_every_fixture(server: &ServerGuard) -> (String, String, String, String, String, String) {
    exec_sql(server, "CREATE TABLE evcat (title TEXT, color TEXT)");
    exec_sql(
        server,
        "CREATE TABLE evlink (url TEXT, evcat TEXT REFERENCES evcat)",
    );
    std::thread::sleep(std::time::Duration::from_secs(1));

    let cat_null = exec_sql_then_sleep(server, "INSERT INTO evcat (title) VALUES ('NoColor')");
    let cat_red =
        exec_sql_then_sleep(server, "INSERT INTO evcat (title, color) VALUES ('Red', 'red')");
    let cat_blue =
        exec_sql_then_sleep(server, "INSERT INTO evcat (title, color) VALUES ('Blue', 'blue')");

    let link_null = exec_sql_then_sleep(
        server,
        &format!("INSERT INTO evlink (url, evcat) VALUES ('null.example', '{cat_null}')"),
    );
    let link_red = exec_sql_then_sleep(
        server,
        &format!("INSERT INTO evlink (url, evcat) VALUES ('red.example', '{cat_red}')"),
    );
    let link_blue = exec_sql(
        server,
        &format!("INSERT INTO evlink (url, evcat) VALUES ('blue.example', '{cat_blue}')"),
    );
    (cat_null, cat_red, cat_blue, link_null, link_red, link_blue)
}

/// Fixture for `id_op_vs_junction_some_diverge_on_multi_reference`.
/// Returns `(cat_a, cat_b, link)`.
fn setup_multi_ref_fixture(server: &ServerGuard) -> (String, String, String) {
    exec_sql(server, "CREATE TABLE mrcat (title TEXT)");
    exec_sql(
        server,
        "CREATE TABLE mrlink (url TEXT, mrcat TEXT REFERENCES mrcat)",
    );
    std::thread::sleep(std::time::Duration::from_secs(1));

    let cat_a = exec_sql_then_sleep(server, "INSERT INTO mrcat (title) VALUES ('A')");
    let cat_b = exec_sql_then_sleep(server, "INSERT INTO mrcat (title) VALUES ('B')");

    let link = exec_sql(
        server,
        &format!("INSERT INTO mrlink (url, mrcat) VALUES ('multi.example', '{cat_a}')"),
    );
    exec_sql(
        server,
        &format!("INSERT INTO mrlink_mrcat (mrlink_id, mrcat_id) VALUES ('{link}', '{cat_b}')"),
    );
    (cat_a, cat_b, link)
}

/// `every` over a junction relation must follow SQLite three-valued logic: a
/// related row whose filtered (inlinable) target column is NULL is NOT a
/// counterexample, because `NOT (<col> = ?)` evaluates to NULL (not TRUE) and
/// so never satisfies the inner `NOT EXISTS (... AND NOT (<sub>))`. The parent
/// is therefore still returned by `every`, as if the NULL row "passes".
///
/// This characterizes design non-blocker #4: `every` + a NULL target column.
/// The target type DECLARES the filtered column (`color`) so it is inlinable
/// (base doogat fields are never inlined). A genuine non-matching related row
/// (`color = 'blue'`) IS excluded, proving the test binds to the 3VL rule and
/// not to a trivially-passing query.
#[test]
fn every_treats_null_target_column_as_non_counterexample() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // catNull: NULL color (3VL non-counterexample); catRed: matches `every`;
    // catBlue: genuine counterexample. linkNull/linkRed pass, linkBlue excluded.
    let (_cat_null, _cat_red, _cat_blue, link_null, link_red, link_blue) =
        setup_every_fixture(&server);

    // `every: { color: { eq: "red" } }`
    let r = server.graphql(
        r#"{ evlinks(where: { evcat: { every: { color: { eq: "red" } } } }) { totalCount items { id } } }"#,
    );
    assert!(r.get("errors").is_none(), "every query failed: {r}");
    let ids = extract_item_ids(&r, "evlinks");

    // 3VL: the NULL-color related row is NOT a counterexample, so linkNull is
    // returned. linkRed matches outright. linkBlue ('blue' != 'red') is the
    // genuine counterexample and is excluded.
    assert!(
        ids.contains(&link_null),
        "linkNull (NULL target column) must pass `every` as a 3VL non-counterexample: {r}"
    );
    assert!(
        ids.contains(&link_red),
        "linkRed (color = 'red') must pass `every`: {r}"
    );
    assert!(
        !ids.contains(&link_blue),
        "linkBlue (color = 'blue') is a real counterexample and must be excluded: {r}"
    );
    assert_eq!(
        r["data"]["evlinks"]["totalCount"].as_i64().unwrap(),
        2,
        "exactly linkNull + linkRed should match `every`: {r}"
    );
}

/// A single stored REFERENCES column and its junction are different
/// truth-sources for a multi-reference row. The forward id-op
/// (`evcat: { eq: B }`) compares the parent's STORED column directly, while
/// `evcat: { some: { id: { eq: B } } }` traverses the full junction. When a
/// row stores A but the junction also holds B, the two operators return
/// DIFFERENT result sets for that row: id-op does NOT match B, the junction
/// `some` DOES.
///
/// This characterizes design non-blocker #6: id-op vs junction `some`
/// divergence on a multi-reference column. The extra junction row (B) is
/// inserted directly via SQL (same idiom as the plural-resolver e2e fixtures);
/// the test performs no reindex, so the manual junction row persists.
#[test]
fn id_op_vs_junction_some_diverge_on_multi_reference() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // cat_a: stored first-value reference; cat_b: junction-only. link stores A,
    // junction holds both A and B (B inserted directly).
    let (_cat_a, cat_b, link) = setup_multi_ref_fixture(&server);

    // id-op: direct stored-column compare. Stored column is A, so `eq: B`
    // does NOT match.
    let r = server.graphql(&format!(
        r#"{{ mrlinks(where: {{ mrcat: {{ eq: "{cat_b}" }} }}) {{ totalCount items {{ id }} }} }}"#
    ));
    assert!(r.get("errors").is_none(), "id-op eq B query failed: {r}");
    let id_op_ids = extract_item_ids(&r, "mrlinks");
    assert!(
        !id_op_ids.contains(&link),
        "id-op `eq: B` must NOT match the row (stored column holds A, not B): {r}"
    );

    // junction `some`: traverses the full junction, which holds B, so it
    // DOES match.
    let r = server.graphql(&format!(
        r#"{{ mrlinks(where: {{ mrcat: {{ some: {{ id: {{ eq: "{cat_b}" }} }} }} }}) {{ totalCount items {{ id }} }} }}"#
    ));
    assert!(
        r.get("errors").is_none(),
        "junction some id eq B query failed: {r}"
    );
    let junction_ids = extract_item_ids(&r, "mrlinks");
    assert!(
        junction_ids.contains(&link),
        "junction `some: {{ id: {{ eq: B }} }}` MUST match the row (junction holds B): {r}"
    );

    // The divergence is the point: the two operators disagree on this row.
    assert_ne!(
        id_op_ids, junction_ids,
        "id-op and junction `some` must diverge on a multi-reference row: {r}"
    );
}
