mod base_types;
mod mutations;
mod queries;
mod subscriptions;
mod type_defs;
pub(crate) use base_types::{parse_fields_json, resolve_column, sanitize_field_name, sanitize_type_name};
use base_types::TypeSchemaMap;

use async_graphql::dynamic::*;
use ddb_core::types::TableSchema;

use std::collections::HashMap;
use std::sync::Arc;

use crate::actor::ActorHandle;
use crate::read_pool::ReadPool;
use crate::reload::SchemaReloader;

// -- Schema builder --

pub fn build_schema(
    actor: ActorHandle,
    read_pool: ReadPool,
    type_schemas: Vec<TableSchema>,
    reloader: Option<Arc<SchemaReloader>>,
) -> Result<Schema, String> {
    // -- Shared type definitions --
    let type_defs::TypeDefs {
        inline_field_type,
        link_type,
        search_hit_type,
        search_connection_type,
        column_info_type,
        typedef_type,
        sql_result_type,
        attachment_type,
        checkbox_item_type,
        tag_info_type,
        tag_entry_type,
        tag_entries_connection_type,
        tag_entries_where_input,
        unlinked_mention_type,
        suggestion_type,
        stale_doogat_type,
        orphan_doogat_type,
        doogat_type,
        create_input,
        create_many_item_input,
        conflict_action_enum,
        update_input,
        search_field_filter_input,
    } = type_defs::build_type_defs();

    // -- Query fields (including dynamic per-type queries) --
    let queries::QueryOutput {
        query,
        known_types,
        dynamic_types,
        mut dynamic_inputs,
        sequence_node_type,
        sequence_info_type,
        broken_sequence_type,
    } = queries::build_query_fields(&type_schemas);

    // -- Mutation fields --
    let mutations::MutationOutput {
        mutation,
        sync_result_type,
        compact_result_type,
        git_maintenance_result_type,
        attach_input,
    } = mutations::build_mutation_fields();
    dynamic_inputs.push(attach_input);

    // -- Subscription fields --
    let subscriptions::SubscriptionOutput {
        subscription,
        change_event_type,
    } = subscriptions::build_subscription_fields(&known_types, &type_schemas);

    // -- Build schema --
    let mut builder = Schema::build(
        query.type_name(),
        Some(mutation.type_name()),
        Some(subscription.type_name()),
    )
    .register(Scalar::new("JSON"))
    .register(doogat_type)
    .register(inline_field_type)
    .register(link_type)
    .register(search_hit_type)
    .register(search_connection_type)
    .register(column_info_type)
    .register(typedef_type)
    .register(sql_result_type)
    .register(create_input)
    .register(create_many_item_input)
    .register(update_input)
    .register(search_field_filter_input)
    .register(conflict_action_enum)
    .register(attachment_type)
    .register(checkbox_item_type)
    .register(unlinked_mention_type)
    .register(suggestion_type)
    .register(stale_doogat_type)
    .register(orphan_doogat_type)
    .register(tag_info_type)
    .register(tag_entry_type)
    .register(tag_entries_connection_type)
    .register(tag_entries_where_input)
    .register(sequence_node_type)
    .register(sequence_info_type)
    .register(broken_sequence_type)
    .register(change_event_type)
    .register(sync_result_type)
    .register(compact_result_type)
    .register(git_maintenance_result_type)
    // Shared filter/sort types
    .register(crate::filter::string_filter())
    .register(crate::filter::int_filter())
    .register(crate::filter::float_filter())
    .register(crate::filter::bool_filter())
    .register(crate::filter::id_filter())
    .register(crate::filter::sort_order_enum())
    .register(query)
    .register(mutation)
    .register(subscription)
    .data(actor)
    .data(read_pool)
    .data(TypeSchemaMap(Arc::new(
        type_schemas
            .iter()
            .map(|s| (s.table_name.clone(), s.clone()))
            .collect::<HashMap<_, _>>(),
    )));

    for typed_obj in dynamic_types {
        builder = builder.register(typed_obj);
    }
    for input in dynamic_inputs {
        builder = builder.register(input);
    }

    if let Some(reloader) = reloader {
        builder = builder.data(reloader);
    }

    builder.finish().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventBus;
    use ddb_core::service::DoogatService;
    use ddb_core::types::{ColumnDef, TableSchema, Zone};

    /// Helper: spin up ActorHandle + ReadPool backed by a temp ddb repo.
    fn test_actor_and_pool(dir: &std::path::Path) -> (ActorHandle, ReadPool) {
        DoogatService::init(dir).expect("init repo");
        let event_bus = EventBus::new();
        let actor = ActorHandle::spawn(dir.to_path_buf(), event_bus)
            .expect("spawn actor");
        let pool = ReadPool::new(dir.to_path_buf(), 1).expect("read pool");
        (actor, pool)
    }

    fn make_table_schema(name: &str, columns: Vec<ColumnDef>) -> TableSchema {
        TableSchema {
            table_name: name.into(),
            columns,
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
            unique_together: None,
        }
    }

    fn simple_column(name: &str) -> ColumnDef {
        ColumnDef {
            name: name.into(),
            data_type: "TEXT".into(),
            references: None,
            zone: Some(Zone::Frontmatter),
            required: false,
            search_boost: None,
            allowed_values: None,
            default_value: None,
        }
    }

    #[tokio::test]
    async fn build_schema_includes_hyphenated_type() {
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let schemas = vec![make_table_schema(
            "test-item",
            vec![simple_column("status")],
        )];

        let schema = build_schema(actor, pool, schemas, None)
            .expect("schema should build successfully");
        let sdl = schema.sdl();

        // Type name: kebab-case → PascalCase
        assert!(
            sdl.contains("type TestItem"),
            "SDL should contain 'type TestItem', got:\n{sdl}"
        );

        // Query field: camelCase + pluralized
        assert!(
            sdl.contains("testItems"),
            "SDL should contain query field 'testItems', got:\n{sdl}"
        );

        // Subscription field: camelCase + "Changed"
        assert!(
            sdl.contains("testItemChanged"),
            "SDL should contain subscription field 'testItemChanged', got:\n{sdl}"
        );
    }

    #[tokio::test]
    async fn build_schema_detects_name_collision() {
        // "my-type" and "myType" both sanitize to PascalCase "MyType".
        // The second should be skipped (collision detection).
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let schemas = vec![
            make_table_schema("my-type", vec![simple_column("a")]),
            make_table_schema("myType", vec![simple_column("b")]),
        ];

        let schema = build_schema(actor, pool, schemas, None)
            .expect("schema should build despite collision");
        let sdl = schema.sdl();

        // First type registered (match with " {" to avoid substring-matching MyTypeConnection etc.)
        assert!(
            sdl.contains("type MyType {"),
            "SDL should contain 'type MyType' from the first registrant"
        );

        // Count occurrences - there should be exactly one 'type MyType {'
        let count = sdl.matches("type MyType {").count();
        assert_eq!(
            count, 1,
            "expected exactly 1 'type MyType {{' but found {count} (collision not detected)"
        );
    }

    #[tokio::test]
    async fn all_query_fields_have_descriptions() {
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let schemas = vec![make_table_schema(
            "contact",
            vec![simple_column("email")],
        )];

        let schema = build_schema(actor, pool, schemas, None)
            .expect("schema should build");

        let res = schema
            .execute(r#"{ __type(name: "Query") { fields { name description } } }"#)
            .await;
        let data = res.data.into_json().unwrap();
        let fields = data["__type"]["fields"].as_array().unwrap();

        let missing: Vec<&str> = fields
            .iter()
            .filter(|f| {
                f["description"].is_null()
                    || f["description"].as_str().unwrap_or("").is_empty()
            })
            .map(|f| f["name"].as_str().unwrap_or("?"))
            .collect();

        assert!(
            missing.is_empty(),
            "Query fields missing descriptions: {missing:?}"
        );
    }

    #[tokio::test]
    async fn all_mutation_fields_have_descriptions() {
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let schemas = vec![make_table_schema(
            "contact",
            vec![simple_column("email")],
        )];

        let schema = build_schema(actor, pool, schemas, None)
            .expect("schema should build");

        let res = schema
            .execute(r#"{ __type(name: "Mutation") { fields { name description } } }"#)
            .await;
        let data = res.data.into_json().unwrap();
        let fields = data["__type"]["fields"].as_array().unwrap();

        let missing: Vec<&str> = fields
            .iter()
            .filter(|f| {
                f["description"].is_null()
                    || f["description"].as_str().unwrap_or("").is_empty()
            })
            .map(|f| f["name"].as_str().unwrap_or("?"))
            .collect();

        assert!(
            missing.is_empty(),
            "Mutation fields missing descriptions: {missing:?}"
        );
    }

    #[tokio::test]
    async fn search_field_filter_inputs_have_descriptions() {
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let schema = build_schema(actor, pool, vec![], None)
            .expect("schema should build");

        let res = schema
            .execute(
                r#"{ __type(name: "SearchFieldFilter") { inputFields { name description } } }"#,
            )
            .await;
        let data = res.data.into_json().unwrap();
        let fields = data["__type"]["inputFields"].as_array().unwrap();

        let missing: Vec<&str> = fields
            .iter()
            .filter(|f| {
                f["description"].is_null()
                    || f["description"].as_str().unwrap_or("").is_empty()
            })
            .map(|f| f["name"].as_str().unwrap_or("?"))
            .collect();

        assert!(
            missing.is_empty(),
            "SearchFieldFilter input fields missing descriptions: {missing:?}"
        );
    }

    #[tokio::test]
    async fn build_schema_hyphenated_query_and_aggregate() {
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let schemas = vec![
            make_table_schema(
                "test-widget",
                vec![
                    simple_column("status"),
                    ColumnDef {
                        name: "priority".into(),
                        data_type: "INTEGER".into(),
                        references: None,
                        zone: Some(Zone::Frontmatter),
                        required: false,
                        search_boost: None,
                        allowed_values: None,
                        default_value: None,
                    },
                ],
            ),
            make_table_schema("bookmark", vec![simple_column("url")]),
        ];

        let schema = build_schema(actor, pool, schemas, None)
            .expect("schema should build with mixed hyphenated and plain types");
        let sdl = schema.sdl();

        // Query fields: camelCase + pluralized
        assert!(
            sdl.contains("testWidgets"),
            "SDL should contain query field 'testWidgets', got:\n{sdl}"
        );
        assert!(
            sdl.contains("bookmarks"),
            "SDL should contain query field 'bookmarks', got:\n{sdl}"
        );

        // Aggregate query field
        assert!(
            sdl.contains("testWidgetsAggregate"),
            "SDL should contain 'testWidgetsAggregate', got:\n{sdl}"
        );

        // Input types
        assert!(
            sdl.contains("TestWidgetWhere"),
            "SDL should contain input 'TestWidgetWhere', got:\n{sdl}"
        );
        assert!(
            sdl.contains("TestWidgetOrderBy"),
            "SDL should contain input 'TestWidgetOrderBy', got:\n{sdl}"
        );

        // Subscription field
        assert!(
            sdl.contains("testWidgetChanged"),
            "SDL should contain subscription 'testWidgetChanged', got:\n{sdl}"
        );
    }
}

