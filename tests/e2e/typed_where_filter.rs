use crate::common::{DdbTestRepo, ServerGuard};

/// Exhaustive test of the typed per-type `where:` filter on GraphQL
/// connection fields (rellinks / relcats), exercising negation, null,
/// inlined target columns, junction quantifiers, reverse membership,
/// and pagination + totalCount correlation.
#[test]
fn typed_where_filter_completeness() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // ── Fixture: create types ──
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE relcat (title TEXT, slug TEXT)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE relcat failed: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE rellink (url TEXT, category TEXT REFERENCES relcat)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE rellink failed: {r}");

    std::thread::sleep(std::time::Duration::from_secs(1));

    // ── Insert categories ──
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO relcat (title, slug) VALUES ('Rust', 'rust')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT Rust category failed: {r}");
    let cat_rust_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!cat_rust_id.is_empty(), "cat_rust_id empty: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO relcat (title, slug) VALUES ('Python', 'py')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT Python category failed: {r}");
    let cat_py_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!cat_py_id.is_empty(), "cat_py_id empty: {r}");

    std::thread::sleep(std::time::Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO relcat (title, slug) VALUES ('Empty', 'empty')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT Empty category failed: {r}");
    let cat_empty_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!cat_empty_id.is_empty(), "cat_empty_id empty: {r}");

    std::thread::sleep(std::time::Duration::from_secs(1));

    // ── Insert links ──
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({
            "sql": format!(
                "INSERT INTO rellink (url, category) VALUES ('https://rust.example', '{cat_rust_id}')"
            )
        }),
    );
    assert!(r.get("errors").is_none(), "INSERT rust link failed: {r}");
    let link1_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!link1_id.is_empty(), "link1_id empty: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({
            "sql": format!(
                "INSERT INTO rellink (url, category) VALUES ('https://py.example', '{cat_py_id}')"
            )
        }),
    );
    assert!(r.get("errors").is_none(), "INSERT py link failed: {r}");
    let link2_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!link2_id.is_empty(), "link2_id empty: {r}");

    std::thread::sleep(std::time::Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO rellink (title) VALUES ('orphan-link')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT orphan link failed: {r}");
    let link3_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!link3_id.is_empty(), "link3_id empty: {r}");

    // ═══════════════════════════════════════════════════════════
    // ASSERTIONS
    // ═══════════════════════════════════════════════════════════

    // 1. NEGATION: notIn
    let r = server.graphql(&format!(
        r#"{{ rellinks(where: {{ id: {{ notIn: ["{id}"] }} }}) {{ totalCount }} }}"#,
        id = link1_id
    ));
    assert!(r.get("errors").is_none(), "notIn query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        2,
        "notIn totalCount should be 2: {r}"
    );

    // 1b. NEGATION: nin (exact alias of notIn)
    let r = server.graphql(&format!(
        r#"{{ rellinks(where: {{ id: {{ nin: ["{id}"] }} }}) {{ totalCount }} }}"#,
        id = link1_id
    ));
    assert!(r.get("errors").is_none(), "nin query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        2,
        "nin totalCount should be 2: {r}"
    );

    // 2. NULL: isNull true
    let r = server.graphql(r#"{ rellinks(where: { url: { isNull: true } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "isNull true query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        1,
        "isNull true totalCount should be 1: {r}"
    );

    // 2b. NULL: isNull false
    let r = server.graphql(r#"{ rellinks(where: { url: { isNull: false } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "isNull false query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        2,
        "isNull false totalCount should be 2: {r}"
    );

    // 3. FORWARD inlined col: category.title eq "Rust"
    let r = server.graphql(
        r#"{ rellinks(where: { category: { title: { eq: "Rust" } } }) { totalCount items { id url } } }"#,
    );
    assert!(r.get("errors").is_none(), "inlined title query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        1,
        "inlined title totalCount should be 1: {r}"
    );
    let items = r["data"]["rellinks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "inlined title should return 1 item: {r}");
    assert_eq!(
        items[0]["url"].as_str().unwrap(),
        "https://rust.example",
        "inlined title url mismatch: {r}"
    );

    // 4. FORWARD inlined custom col: category.slug eq "py"
    let r = server.graphql(
        r#"{ rellinks(where: { category: { slug: { eq: "py" } } }) { totalCount } }"#,
    );
    assert!(r.get("errors").is_none(), "inlined slug query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        1,
        "inlined slug totalCount should be 1: {r}"
    );

    // 5. FORWARD id-ops (back-compat): category eq cat_rust_id
    let r = server.graphql(&format!(
        r#"{{ rellinks(where: {{ category: {{ eq: "{id}" }} }}) {{ totalCount }} }}"#,
        id = cat_rust_id
    ));
    assert!(r.get("errors").is_none(), "category eq query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        1,
        "category eq totalCount should be 1: {r}"
    );

    // 6. NONE: no relation (orphan link)
    let r = server.graphql(r#"{ rellinks(where: { category: { none: {} } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "none query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        1,
        "none totalCount should be 1: {r}"
    );

    // 6b. SOME: has relation
    let r = server.graphql(r#"{ rellinks(where: { category: { some: {} } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "some query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        2,
        "some totalCount should be 2: {r}"
    );

    // 7. REVERSE some: relcats where rellinks has url containing "rust"
    let r = server.graphql(
        r#"{ relcats(where: { rellinks: { some: { url: { contains: "rust" } } } }) { totalCount items { id title } } }"#,
    );
    assert!(r.get("errors").is_none(), "reverse some query failed: {r}");
    assert_eq!(
        r["data"]["relcats"]["totalCount"].as_i64().unwrap(),
        1,
        "reverse some totalCount should be 1: {r}"
    );
    let items = r["data"]["relcats"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "reverse some should return 1 item: {r}");
    assert_eq!(
        items[0]["title"].as_str().unwrap(),
        "Rust",
        "reverse some title mismatch: {r}"
    );

    // 8. REVERSE none: relcats with no rellinks
    let r = server.graphql(r#"{ relcats(where: { rellinks: { none: {} } }) { totalCount items { id title } } }"#);
    assert!(r.get("errors").is_none(), "reverse none query failed: {r}");
    assert_eq!(
        r["data"]["relcats"]["totalCount"].as_i64().unwrap(),
        1,
        "reverse none totalCount should be 1: {r}"
    );
    let items = r["data"]["relcats"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "reverse none should return 1 item: {r}");
    assert_eq!(
        items[0]["title"].as_str().unwrap(),
        "Empty",
        "reverse none title mismatch: {r}"
    );

    // 9. PAGINATION + totalCount correlation
    // Page 0
    let r = server.graphql(
        r#"{ rellinks(where: { category: { some: {} } }, orderBy: { id: ASC }, limit: 1, offset: 0) { items { id url } totalCount } }"#,
    );
    assert!(r.get("errors").is_none(), "pagination page 0 failed: {r}");
    let items = r["data"]["rellinks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "page 0 should have 1 item: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        2,
        "page 0 totalCount should be 2: {r}"
    );
    let p0_url = items[0]["url"].as_str().unwrap().to_string();

    // Page 1
    let r = server.graphql(
        r#"{ rellinks(where: { category: { some: {} } }, orderBy: { id: ASC }, limit: 1, offset: 1) { items { id url } totalCount } }"#,
    );
    assert!(r.get("errors").is_none(), "pagination page 1 failed: {r}");
    let items = r["data"]["rellinks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "page 1 should have 1 item: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        2,
        "page 1 totalCount should be 2: {r}"
    );
    let p1_url = items[0]["url"].as_str().unwrap().to_string();

    // No duplicates across pages
    assert_ne!(p0_url, p1_url, "page 0 and page 1 should have different URLs: {r}");
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

    // Target type declares `color` (inlinable). Parent references it.
    exec_sql(&server, "CREATE TABLE evcat (title TEXT, color TEXT)");
    exec_sql(
        &server,
        "CREATE TABLE evlink (url TEXT, evcat TEXT REFERENCES evcat)",
    );
    std::thread::sleep(std::time::Duration::from_secs(1));

    // catNull: color absent -> stored color is NULL.
    let cat_null = exec_sql(&server, "INSERT INTO evcat (title) VALUES ('NoColor')");
    std::thread::sleep(std::time::Duration::from_secs(1));
    // catRed: color = 'red' (the value `every` filters on).
    let cat_red = exec_sql(&server, "INSERT INTO evcat (title, color) VALUES ('Red', 'red')");
    std::thread::sleep(std::time::Duration::from_secs(1));
    // catBlue: color = 'blue' (genuine counterexample to color = 'red').
    let cat_blue = exec_sql(
        &server,
        "INSERT INTO evcat (title, color) VALUES ('Blue', 'blue')",
    );
    std::thread::sleep(std::time::Duration::from_secs(1));

    // linkNull -> catNull (NULL color): must pass `every` via 3VL.
    let link_null = exec_sql(
        &server,
        &format!("INSERT INTO evlink (url, evcat) VALUES ('null.example', '{cat_null}')"),
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
    // linkRed -> catRed ('red'): matches, passes `every`.
    let link_red = exec_sql(
        &server,
        &format!("INSERT INTO evlink (url, evcat) VALUES ('red.example', '{cat_red}')"),
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
    // linkBlue -> catBlue ('blue'): genuine counterexample, excluded by `every`.
    let link_blue = exec_sql(
        &server,
        &format!("INSERT INTO evlink (url, evcat) VALUES ('blue.example', '{cat_blue}')"),
    );

    // `every: { color: { eq: "red" } }`
    let r = server.graphql(
        r#"{ evlinks(where: { evcat: { every: { color: { eq: "red" } } } }) { totalCount items { id } } }"#,
    );
    assert!(r.get("errors").is_none(), "every query failed: {r}");

    let ids: Vec<String> = r["data"]["evlinks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["id"].as_str().unwrap().to_string())
        .collect();

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

    exec_sql(&server, "CREATE TABLE mrcat (title TEXT)");
    exec_sql(
        &server,
        "CREATE TABLE mrlink (url TEXT, mrcat TEXT REFERENCES mrcat)",
    );
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Two categories: A is the stored first-value reference, B lives only in
    // the junction.
    let cat_a = exec_sql(&server, "INSERT INTO mrcat (title) VALUES ('A')");
    std::thread::sleep(std::time::Duration::from_secs(1));
    let cat_b = exec_sql(&server, "INSERT INTO mrcat (title) VALUES ('B')");
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Link stores A in its `mrcat` column (and auto-populates junction row A).
    let link = exec_sql(
        &server,
        &format!("INSERT INTO mrlink (url, mrcat) VALUES ('multi.example', '{cat_a}')"),
    );

    // Add B to the junction directly: the junction now holds A and B, but the
    // stored `mrcat` column still holds only A. Junction for column `mrcat` on
    // table `mrlink` is `mrlink_mrcat(mrlink_id, mrcat_id)`.
    exec_sql(
        &server,
        &format!("INSERT INTO mrlink_mrcat (mrlink_id, mrcat_id) VALUES ('{link}', '{cat_b}')"),
    );

    // id-op: direct stored-column compare. Stored column is A, so `eq: B`
    // does NOT match.
    let r = server.graphql(&format!(
        r#"{{ mrlinks(where: {{ mrcat: {{ eq: "{cat_b}" }} }}) {{ totalCount items {{ id }} }} }}"#
    ));
    assert!(r.get("errors").is_none(), "id-op eq B query failed: {r}");
    let id_op_ids: Vec<String> = r["data"]["mrlinks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["id"].as_str().unwrap().to_string())
        .collect();
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
    let junction_ids: Vec<String> = r["data"]["mrlinks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["id"].as_str().unwrap().to_string())
        .collect();
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
