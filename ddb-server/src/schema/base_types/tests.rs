use std::collections::HashMap;

use async_graphql::dynamic::*;
use async_graphql::{Name, Value as GqlValue};
use ddb_core::sql_engine::SqlResult;
use ddb_core::types::{ColumnDef, Value, Zone};
use indexmap::IndexMap;

use super::naming::*;
use super::schema_object::*;
use super::to_gql::*;
use super::validation::*;

#[test]
fn valid_graphql_names() {
    assert!(is_valid_graphql_name("title"));
    assert!(is_valid_graphql_name("_private"));
    assert!(is_valid_graphql_name("camelCase"));
    assert!(is_valid_graphql_name("snake_case"));
    assert!(is_valid_graphql_name("Field123"));
}

#[test]
fn invalid_graphql_names() {
    assert!(!is_valid_graphql_name(""));
    assert!(!is_valid_graphql_name("123start"));
    assert!(!is_valid_graphql_name("has space"));
    assert!(!is_valid_graphql_name("has-dash"));
    assert!(!is_valid_graphql_name("has.dot"));
    assert!(!is_valid_graphql_name("special!"));
}

#[test]
fn singleton_field_base_strips_leading_hyphens_underscores() {
    // PRD 00139 blind m2 follow-up: typedef YAML loaded from hand-written
    // git files can carry leading-hyphen names that bypass DDL validation.
    // The derived GraphQL field base must remain a valid identifier
    // (start with [A-Za-z_], not `_` followed by digits etc).
    assert_eq!(singleton_field_base("app_config"), "app_config");
    assert_eq!(singleton_field_base("app-config"), "app_config");
    assert_eq!(singleton_field_base("--foo--"), "foo__");
    assert_eq!(singleton_field_base("-foo"), "foo");
}

#[test]
fn typed_doogat_multi_ref_list_field() {
    use ddb_core::types::ParsedDoogat;
    use ddb_core::types::{ColumnDef, DoogatMeta, InlineField, TableSchema, Zone};

    let z = ParsedDoogat {
        meta: DoogatMeta {
            id: Some(ddb_core::types::DoogatId("20260301140100".into())),
            title: Some("Test".into()),
            date: None,
            tags: vec![],
            doogat_type: Some("bookmark".into()),
            extra: std::collections::BTreeMap::new(),
        },
        body: String::new(),
        sections: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        inline_fields: vec![
            InlineField {
                key: "category".into(),
                value: "20260301120100".into(),
                zone: Zone::Reference,
            },
            InlineField {
                key: "category".into(),
                value: "20260301120101".into(),
                zone: Zone::Reference,
            },
        ],
        reference_section: String::new(),
        path: "ddb/20260301140100.md".into(),
        updated_at: None,
    };

    let schema = TableSchema {
        table_name: "bookmark".into(),
        columns: vec![ColumnDef {
            name: "category".into(),
            data_type: "TEXT".into(),
            references: Some("category".into()),
            zone: Some(Zone::Reference),
            required: false,
            search_boost: None,
            allowed_values: None,
            default_value: None,
            on_delete: ddb_core::types::OnDeleteAction::Restrict,
        }],
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
        search_key: None,
        singleton: false,
    };

    let val = typed_doogat_to_value(&z, &schema);
    let obj = match &val {
        GqlValue::Object(o) => o,
        _ => panic!("expected object"),
    };

    // Singular field stores raw ID (resolver fetches the object at query time)
    let scalar = obj.get("category").unwrap();
    assert_eq!(scalar, &GqlValue::from("20260301120100".to_string()));

    // List field should have both values
    let list = obj.get("categories").unwrap();
    match list {
        GqlValue::List(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], GqlValue::from("20260301120100".to_string()));
            assert_eq!(items[1], GqlValue::from("20260301120101".to_string()));
        }
        _ => panic!("expected list, got {list:?}"),
    }
}

#[test]
fn sql_result_rows_includes_columns() {
    let result = SqlResult::Rows {
        columns: vec!["id".into(), "title".into()],
        rows: vec![vec!["123".into(), "hello".into()]],
        column_types: None,
    };
    let val = sql_result_to_value(&result, "array");
    let obj = match &val {
        GqlValue::Object(o) => o,
        _ => panic!("expected object"),
    };
    let cols = obj.get("columns").unwrap();
    match cols {
        GqlValue::List(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], GqlValue::from("id"));
            assert_eq!(items[1], GqlValue::from("title"));
        }
        _ => panic!("expected list, got {cols:?}"),
    }
    assert!(obj.get("rows").is_some());
    assert_eq!(obj.get("affected").unwrap(), &GqlValue::Null);
}

#[test]
fn sql_result_affected_has_empty_columns() {
    let result = SqlResult::Affected(3);
    let val = sql_result_to_value(&result, "array");
    let obj = match &val {
        GqlValue::Object(o) => o,
        _ => panic!("expected object"),
    };
    assert_eq!(obj.get("columns").unwrap(), &GqlValue::List(vec![]));
    assert_eq!(obj.get("rows").unwrap(), &GqlValue::List(vec![]));
    assert_eq!(obj.get("affected").unwrap(), &GqlValue::from(3i64));
}

#[test]
fn sql_result_ok_has_empty_columns() {
    let result = SqlResult::Ok("done".into());
    let val = sql_result_to_value(&result, "array");
    let obj = match &val {
        GqlValue::Object(o) => o,
        _ => panic!("expected object"),
    };
    assert_eq!(obj.get("columns").unwrap(), &GqlValue::List(vec![]));
    assert_eq!(obj.get("rows").unwrap(), &GqlValue::List(vec![]));
    assert_eq!(obj.get("message").unwrap(), &GqlValue::from("done"));
}

#[test]
fn sql_result_rows_objects_format() {
    let result = SqlResult::Rows {
        columns: vec!["id".into(), "title".into()],
        rows: vec![vec!["123".into(), "hello".into()]],
        column_types: None,
    };
    let val = sql_result_to_value(&result, "objects");
    let obj = match &val {
        GqlValue::Object(o) => o,
        _ => panic!("expected object"),
    };
    let rows = match obj.get("rows").unwrap() {
        GqlValue::List(items) => items,
        other => panic!("expected list, got {other:?}"),
    };
    assert_eq!(rows.len(), 1);
    let row_json = match &rows[0] {
        GqlValue::String(s) => s.clone(),
        other => panic!("expected string, got {other:?}"),
    };
    let parsed: serde_json::Value = serde_json::from_str(&row_json).unwrap();
    assert!(parsed.is_object(), "row should be JSON object");
    assert_eq!(parsed["id"].as_str().unwrap(), "123");
    assert_eq!(parsed["title"].as_str().unwrap(), "hello");
}

#[test]
fn doogat_to_value_merges_body_tags_deduplicated() {
    use ddb_core::types::{DoogatMeta, ParsedDoogat};

    let z = ParsedDoogat {
        meta: DoogatMeta {
            id: Some(ddb_core::types::DoogatId("20260301140200".into())),
            title: Some("TagTest".into()),
            date: None,
            tags: vec!["shared".into(), "fm-only".into()],
            doogat_type: None,
            extra: std::collections::BTreeMap::new(),
        },
        body: String::new(),
        sections: vec![],
        links: vec![],
        body_tags: vec!["shared".into(), "body-only".into()],
        checkboxes: vec![],
        inline_fields: vec![],
        reference_section: String::new(),
        path: "ddb/20260301140200.md".into(),
        updated_at: None,
    };

    let val = doogat_to_value(&z);
    let obj = match &val {
        GqlValue::Object(o) => o,
        _ => panic!("expected object"),
    };

    let tags = obj.get("tags").unwrap();
    match tags {
        GqlValue::List(items) => {
            let strs: Vec<&str> = items
                .iter()
                .map(|v| match v {
                    GqlValue::String(s) => s.as_str(),
                    _ => panic!("expected string in tags"),
                })
                .collect();
            assert_eq!(strs, vec!["shared", "fm-only", "body-only"]);
        }
        _ => panic!("expected list, got {tags:?}"),
    }
}

#[test]
fn ref_target_gql_type_handles_hyphenated() {
    let col = ColumnDef {
        name: "membership".into(),
        data_type: "TEXT".into(),
        references: Some("category-membership".into()),
        zone: Some(Zone::Reference),
        required: false,
        search_boost: None,
        allowed_values: None,
        default_value: None,
        on_delete: ddb_core::types::OnDeleteAction::Restrict,
    };

    // After the change, known_types is HashMap<String, String>
    // mapping original table_name → sanitized PascalCase name.
    let mut known_types: HashMap<String, String> = HashMap::new();
    known_types.insert("category-membership".into(), "CategoryMembership".into());

    let result = ref_target_gql_type(&col, &known_types);
    assert_eq!(
        result, "CategoryMembership",
        "ref_target_gql_type should return sanitized PascalCase for hyphenated target"
    );
}

#[test]
fn ref_target_gql_type_falls_back_to_doogat() {
    let col = ColumnDef {
        name: "owner".into(),
        data_type: "TEXT".into(),
        references: Some("unknown-type".into()),
        zone: Some(Zone::Reference),
        required: false,
        search_boost: None,
        allowed_values: None,
        default_value: None,
        on_delete: ddb_core::types::OnDeleteAction::Restrict,
    };

    let known_types: HashMap<String, String> = HashMap::new();
    let result = ref_target_gql_type(&col, &known_types);
    assert_eq!(
        result, "Doogat",
        "ref_target_gql_type should fall back to 'Doogat' for unknown targets"
    );
}

#[test]
fn test_sanitize_type_name() {
    // Kebab-case to PascalCase
    assert_eq!(
        sanitize_type_name("category-membership"),
        "CategoryMembership"
    );
    assert_eq!(sanitize_type_name("saved-search"), "SavedSearch");
    assert_eq!(sanitize_type_name("pinned-result"), "PinnedResult");
    assert_eq!(sanitize_type_name("jink-config"), "JinkConfig");

    // Single word - capitalize first char
    assert_eq!(sanitize_type_name("contact"), "Contact");
    assert_eq!(sanitize_type_name("bookmark"), "Bookmark");

    // Already PascalCase - pass through
    assert_eq!(sanitize_type_name("MyType"), "MyType");

    // Underscored names - capitalize first char only, underscores preserved
    assert_eq!(sanitize_type_name("my_type"), "My_type");

    // Empty string
    assert_eq!(sanitize_type_name(""), "");

    // Multi-segment hyphens
    assert_eq!(sanitize_type_name("a-b-c-d"), "ABCD");
}

#[test]
fn test_sanitize_field_name() {
    // Kebab-case to camelCase
    assert_eq!(
        sanitize_field_name("category-membership"),
        "categoryMembership"
    );
    assert_eq!(sanitize_field_name("saved-search"), "savedSearch");
    assert_eq!(sanitize_field_name("jink-config"), "jinkConfig");

    // Single word - no change
    assert_eq!(sanitize_field_name("title"), "title");

    // Already capitalized first char - lowercase it
    assert_eq!(sanitize_field_name("MyField"), "myField");

    // Empty string
    assert_eq!(sanitize_field_name(""), "");

    // Multi-segment hyphens
    assert_eq!(sanitize_field_name("a-b-c-d"), "aBCD");
}

#[test]
fn test_pluralize_preserving_case() {
    assert_eq!(pluralize_preserving_case("contact"), "contacts");
    assert_eq!(pluralize_preserving_case("testItem"), "testItems");
    assert_eq!(pluralize_preserving_case("savedSearch"), "savedSearches");
    assert_eq!(pluralize_preserving_case("testClass"), "testClasses");
    assert_eq!(pluralize_preserving_case("testBox"), "testBoxes");
    assert_eq!(pluralize_preserving_case("testBuzz"), "testBuzzes");
    assert_eq!(
        pluralize_preserving_case("categoryMembership"),
        "categoryMemberships"
    );
}

#[test]
fn search_hit_to_value_includes_enriched_fields() {
    use ddb_core::types::SearchResult;
    use std::collections::BTreeMap;

    let mut fields = BTreeMap::new();
    fields.insert("url".into(), "https://example.com".into());
    fields.insert("description".into(), "Example".into());

    let r = SearchResult {
        id: "20260301120000".into(),
        title: "Test".into(),
        path: "ddb/20260301120000.md".into(),
        snippet: "test snippet".into(),
        rank: -1.5,
        updated_at: "2026-03-01T12:00:00Z".into(),
        tags: vec!["rust".into(), "cli".into()],
        doogat_type: Some("link".into()),
        fields: Some(fields),
        created_at: Some("2026-03-01".into()),
    };

    let val = search_hit_to_value(&r);
    let obj = match &val {
        GqlValue::Object(o) => o,
        _ => panic!("expected object"),
    };

    // tags
    let tags = obj.get("tags").unwrap();
    match tags {
        GqlValue::List(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], GqlValue::from("rust"));
            assert_eq!(items[1], GqlValue::from("cli"));
        }
        _ => panic!("expected list for tags"),
    }

    // type
    assert_eq!(obj.get("type").unwrap(), &GqlValue::from("link"));

    // fields (JSON object)
    match obj.get("fields").unwrap() {
        GqlValue::Object(map) => {
            assert_eq!(
                map.get(&Name::new("url")).unwrap(),
                &GqlValue::String("https://example.com".into())
            );
            assert_eq!(
                map.get(&Name::new("description")).unwrap(),
                &GqlValue::String("Example".into())
            );
        }
        _ => panic!("expected object for fields"),
    }

    // created_at
    assert_eq!(
        obj.get("created_at").unwrap(),
        &GqlValue::from("2026-03-01")
    );
}

#[test]
fn search_hit_to_value_fields_with_non_standard_keys() {
    use ddb_core::types::SearchResult;
    use std::collections::BTreeMap;

    let mut fields = BTreeMap::new();
    fields.insert("my-field".into(), "value1".into());
    fields.insert("field with spaces".into(), "value2".into());

    let r = SearchResult {
        id: "20260301120000".into(),
        title: "Test".into(),
        path: "ddb/20260301120000.md".into(),
        snippet: "test snippet".into(),
        rank: -1.5,
        updated_at: String::new(),
        tags: vec![],
        doogat_type: Some("custom".into()),
        fields: Some(fields),
        created_at: None,
    };

    let val = search_hit_to_value(&r);
    let obj = match &val {
        GqlValue::Object(o) => o,
        _ => panic!("expected object"),
    };

    // Verify Name::new() accepts non-standard strings without panicking (no validation
    // in async-graphql-value 7.x - Name is just Arc<str>). Wire-level serialization
    // through the JSON scalar is covered by E2E tests; this test confirms the
    // GqlValue::Object is built correctly.
    match obj.get("fields").unwrap() {
        GqlValue::Object(map) => {
            assert_eq!(
                map.get(&Name::new("my-field")).unwrap(),
                &GqlValue::String("value1".into())
            );
            assert_eq!(
                map.get(&Name::new("field with spaces")).unwrap(),
                &GqlValue::String("value2".into())
            );
        }
        _ => panic!("expected object for fields"),
    }
}

#[test]
fn search_hit_to_value_nulls_for_untyped() {
    use ddb_core::types::SearchResult;

    let r = SearchResult {
        id: "20260301120000".into(),
        title: "Test".into(),
        path: "ddb/20260301120000.md".into(),
        snippet: "test snippet".into(),
        rank: -1.5,
        updated_at: String::new(),
        tags: vec![],
        doogat_type: None,
        fields: None,
        created_at: None,
    };

    let val = search_hit_to_value(&r);
    let obj = match &val {
        GqlValue::Object(o) => o,
        _ => panic!("expected object"),
    };

    // empty tags list
    match obj.get("tags").unwrap() {
        GqlValue::List(items) => assert!(items.is_empty()),
        _ => panic!("expected list for tags"),
    }

    // nulls
    assert_eq!(obj.get("type").unwrap(), &GqlValue::Null);
    assert_eq!(obj.get("fields").unwrap(), &GqlValue::Null);
    assert_eq!(obj.get("created_at").unwrap(), &GqlValue::Null);
    assert_eq!(obj.get("updated_at").unwrap(), &GqlValue::Null);
}

#[test]
fn test_strip_id_suffix() {
    // Column name with _id suffix: strip it
    assert_eq!(strip_id_suffix("link_id"), "link");
    // Multi-word with _id suffix
    assert_eq!(
        strip_id_suffix("category_membership_id"),
        "category_membership"
    );
    // Bare "id" should NOT be stripped (no prefix before _id)
    assert_eq!(strip_id_suffix("id"), "id");
    // No _id suffix: return as-is
    assert_eq!(strip_id_suffix("category"), "category");
    // Underscore but not _id suffix
    assert_eq!(strip_id_suffix("link_name"), "link_name");
    // Ends with "id" but not "_id"
    assert_eq!(strip_id_suffix("valid"), "valid");
}

#[test]
fn check_limit_valid() {
    assert_eq!(check_limit(0).unwrap(), 0);
    assert_eq!(check_limit(100).unwrap(), 100);
    assert_eq!(check_limit(10_000).unwrap(), 10_000);
}

#[test]
fn check_limit_negative() {
    let err = check_limit(-1).unwrap_err();
    assert!(err.message.contains("non-negative"));
}

#[test]
fn check_limit_exceeds_max() {
    let err = check_limit(10_001).unwrap_err();
    assert!(err.message.contains("exceed"));
}

#[test]
fn check_offset_valid() {
    assert_eq!(check_offset(0).unwrap(), 0);
    assert_eq!(check_offset(500).unwrap(), 500);
}

#[test]
fn check_offset_negative() {
    let err = check_offset(-1).unwrap_err();
    assert!(err.message.contains("non-negative"));
}

#[test]
fn check_format_valid() {
    assert!(check_format("array").is_ok());
    assert!(check_format("objects").is_ok());
}

#[test]
fn check_format_invalid() {
    let err = check_format("csv").unwrap_err();
    assert!(err.message.contains("invalid format"));
    assert!(err.message.contains("csv"));
}

fn gql_map(pairs: Vec<(&str, GqlValue)>) -> IndexMap<Name, GqlValue> {
    pairs.into_iter().map(|(k, v)| (Name::new(k), v)).collect()
}

fn gql_num(n: f64) -> GqlValue {
    GqlValue::Number(async_graphql::Number::from_f64(n).unwrap())
}

fn gql_str(v: &str) -> GqlValue {
    GqlValue::String(v.to_string())
}

#[test]
fn decode_json_fields_object_keeps_native_variants_and_marks_null_as_none() {
    let input = gql_map(vec![
        ("s", gql_str("42")),
        ("t", gql_str("true")),
        ("n", gql_num(42.25)),
        ("i", gql_num(7.0)),
        ("b", GqlValue::Boolean(false)),
        ("l", GqlValue::List(vec![gql_str("a"), gql_num(2.0), GqlValue::Boolean(true)])),
        ("m", GqlValue::Object(gql_map(vec![("inner", gql_num(-1.5))]))),
        ("gone", GqlValue::Null),
    ]);
    let decoded = decode_json_fields_object(&input).expect("plain JSON values must decode");

    let expected: ddb_core::app_contract::RawFieldsInput = [
        ("s", Some(Value::String("42".into()))),
        ("t", Some(Value::String("true".into()))),
        ("n", Some(Value::Number(42.25))),
        ("i", Some(Value::Number(7.0))),
        ("b", Some(Value::Bool(false))),
        (
            "l",
            Some(Value::List(vec![
                Value::String("a".into()),
                Value::Number(2.0),
                Value::Bool(true),
            ])),
        ),
        (
            "m",
            Some(Value::Map(
                [("inner".to_string(), Value::Number(-1.5))].into_iter().collect(),
            )),
        ),
        ("gone", None),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    assert_eq!(
        decoded, expected,
        "strings stay strings, numbers/bools stay native, top-level null is None"
    );
}

#[test]
fn decode_json_fields_object_keeps_empty_values_present() {
    let input = gql_map(vec![
        ("empty_s", gql_str("")),
        ("empty_l", GqlValue::List(vec![])),
        ("empty_m", GqlValue::Object(IndexMap::new())),
    ]);
    let decoded = decode_json_fields_object(&input).expect("empty values must decode");
    assert_eq!(decoded.get("empty_s"), Some(&Some(Value::String(String::new()))));
    assert_eq!(decoded.get("empty_l"), Some(&Some(Value::List(vec![]))));
    assert_eq!(
        decoded.get("empty_m"),
        Some(&Some(Value::Map(std::collections::BTreeMap::new()))),
        "empty values are not null and must not decode to None"
    );
}

#[test]
fn decode_json_fields_object_empty_map_yields_empty_input() {
    let decoded = decode_json_fields_object(&IndexMap::new()).expect("empty object decodes");
    assert!(decoded.is_empty(), "got {decoded:?}");
}

#[test]
fn decode_json_fields_object_rejects_null_inside_list() {
    let input = gql_map(vec![(
        "tags_list",
        GqlValue::List(vec![gql_str("a"), GqlValue::Null]),
    )]);
    let err = decode_json_fields_object(&input)
        .expect_err("null is only a top-level per-field signal, never a list element");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("tags_list"),
        "the error must name the offending key 'tags_list', got {msg:?}"
    );
}

#[test]
fn decode_json_fields_object_rejects_null_inside_nested_object() {
    let input = gql_map(vec![(
        "m",
        GqlValue::Object(gql_map(vec![("ok", gql_num(1.0)), ("bad", GqlValue::Null)])),
    )]);
    let err = decode_json_fields_object(&input)
        .expect_err("null is only a top-level per-field signal, never a nested object value");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("bad"),
        "the error must name the offending key 'bad', got {msg:?}"
    );
}

#[test]
fn decode_json_fields_object_rejects_null_deep_inside_list_of_objects() {
    let input = gql_map(vec![(
        "l",
        GqlValue::List(vec![GqlValue::Object(gql_map(vec![("bad", GqlValue::Null)]))]),
    )]);
    let err = decode_json_fields_object(&input)
        .expect_err("a null nested at any depth below the top level must be rejected");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("bad"),
        "the error must name the offending key 'bad', got {msg:?}"
    );
}

#[test]
fn decode_json_fields_object_decodes_integer_above_2_pow_53_without_panic() {
    let input = gql_map(vec![("big", gql_num(9_007_199_254_740_993.0))]);
    let decoded = decode_json_fields_object(&input)
        .expect("a large integer must decode, lossy f64 precision is acceptable");
    assert!(
        matches!(decoded.get("big"), Some(Some(Value::Number(_)))),
        "a large integer must decode to Value::Number, got {decoded:?}"
    );
}
