use crate::common::{DdbTestRepo, ServerGuard};

/// Helper: create a typedef via SQL DDL, patch it to add unique_together on `code`,
/// git commit, and return the repo ready for server start.
fn setup_repo_with_unique_constraint() -> DdbTestRepo {
    let repo = DdbTestRepo::init();

    // Create typedef via SQL
    repo.ddb()
        .args(["query", "CREATE TABLE product (code TEXT, label TEXT)"])
        .assert()
        .success();

    // Patch the typedef to add unique_together: [code]
    let typedef_dir = repo.path().join("ddb/_typedef");
    let typedef_file = std::fs::read_dir(&typedef_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| {
            let content = std::fs::read_to_string(e.path()).unwrap_or_default();
            content.contains("title: product")
        })
        .expect("product typedef not found");

    let content = std::fs::read_to_string(typedef_file.path()).unwrap();
    let patched = content.replace(
        "type: _typedef",
        "type: _typedef\nunique_together:\n  - - code",
    );
    std::fs::write(typedef_file.path(), &patched).unwrap();

    // Commit the change
    std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["add", "-A"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["commit", "-m", "add unique_together to product typedef"])
        .output()
        .unwrap();

    repo
}

#[test]
fn create_doogat_accepts_on_conflict_argument() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Verify the onConflict argument is accepted on createDoogat
    let r = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) {
            createDoogat(input: $input, onConflict: IGNORE) { id title }
        }"#,
        serde_json::json!({
            "input": { "title": "Test Doogat" }
        }),
    );
    assert!(
        r.get("errors").is_none(),
        "createDoogat with onConflict: IGNORE should succeed: {r}"
    );
    assert!(
        r["data"]["createDoogat"]["id"].as_str().is_some(),
        "should return created doogat: {r}"
    );
}

#[test]
fn create_many_on_conflict_ignore_returns_existing() {
    let repo = setup_repo_with_unique_constraint();
    let server = ServerGuard::start(&repo);

    // Seed a product with code=ABC via createMany
    let r1 = server.graphql(
        r#"mutation {
            createMany(inputs: [
                {title: "Widget ABC", type: "product", fields: "{\"code\":\"ABC\",\"label\":\"First\"}"}
            ]) { id title }
        }"#,
    );
    assert!(r1.get("errors").is_none(), "seed create failed: {r1}");
    let first_id = r1["data"]["createMany"]
        .as_array()
        .unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Create another with same code=ABC, using onConflict: IGNORE
    let r2 = server.graphql(
        r#"mutation {
            createMany(
                inputs: [{title: "Widget ABC Dup", type: "product", fields: "{\"code\":\"ABC\",\"label\":\"Second\"}"}],
                onConflict: IGNORE
            ) { id title }
        }"#,
    );
    assert!(
        r2.get("errors").is_none(),
        "createMany with IGNORE should not error: {r2}"
    );
    let results = r2["data"]["createMany"].as_array().unwrap();
    assert_eq!(results.len(), 1, "should return 1 result: {r2}");
    assert_eq!(
        results[0]["id"].as_str().unwrap(),
        first_id,
        "onConflict: IGNORE should return the existing doogat ID"
    );
    assert_eq!(
        results[0]["title"].as_str().unwrap(),
        "Widget ABC",
        "onConflict: IGNORE should return the original title"
    );
}

#[test]
fn create_many_without_on_conflict_errors_on_duplicate() {
    let repo = setup_repo_with_unique_constraint();
    let server = ServerGuard::start(&repo);

    // Seed a product with code=XYZ
    let r1 = server.graphql(
        r#"mutation {
            createMany(inputs: [
                {title: "Widget XYZ", type: "product", fields: "{\"code\":\"XYZ\",\"label\":\"First\"}"}
            ]) { id }
        }"#,
    );
    assert!(r1.get("errors").is_none(), "seed create failed: {r1}");

    // Try duplicate without onConflict (defaults to ERROR)
    let r2 = server.graphql(
        r#"mutation {
            createMany(inputs: [
                {title: "Widget XYZ Dup", type: "product", fields: "{\"code\":\"XYZ\",\"label\":\"Second\"}"}
            ]) { id }
        }"#,
    );
    assert!(
        r2.get("errors").is_some(),
        "duplicate without onConflict should error: {r2}"
    );
}

#[test]
fn create_many_on_conflict_ignore_mixed_new_and_existing() {
    let repo = setup_repo_with_unique_constraint();
    let server = ServerGuard::start(&repo);

    // Seed a product with code=AAA
    let r1 = server.graphql(
        r#"mutation {
            createMany(inputs: [
                {title: "Product AAA", type: "product", fields: "{\"code\":\"AAA\",\"label\":\"Original\"}"}
            ]) { id title }
        }"#,
    );
    assert!(r1.get("errors").is_none(), "seed create failed: {r1}");
    let original_id = r1["data"]["createMany"]
        .as_array()
        .unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Batch with one duplicate (AAA) and one new (BBB)
    let r2 = server.graphql(
        r#"mutation {
            createMany(
                inputs: [
                    {title: "Product AAA dup", type: "product", fields: "{\"code\":\"AAA\",\"label\":\"Dup\"}"},
                    {title: "Product BBB", type: "product", fields: "{\"code\":\"BBB\",\"label\":\"New\"}"}
                ],
                onConflict: IGNORE
            ) { id title }
        }"#,
    );
    assert!(
        r2.get("errors").is_none(),
        "createMany with IGNORE should not error: {r2}"
    );
    let results = r2["data"]["createMany"].as_array().unwrap();
    assert_eq!(results.len(), 2, "should return 2 results: {r2}");

    // First result should be the existing doogat (AAA)
    assert_eq!(
        results[0]["id"].as_str().unwrap(),
        original_id,
        "duplicate should return existing ID"
    );
    assert_eq!(
        results[0]["title"].as_str().unwrap(),
        "Product AAA",
        "duplicate should return original title"
    );

    // Second result should be newly created (BBB)
    assert_ne!(
        results[1]["id"].as_str().unwrap(),
        original_id,
        "new item should get a new ID"
    );
    assert_eq!(results[1]["title"].as_str().unwrap(), "Product BBB");
}
