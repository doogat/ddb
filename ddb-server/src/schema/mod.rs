mod base_types;
mod discovery_queries;
mod mutations;
mod queries;
mod subscriptions;
mod type_defs;
use base_types::TypeSchemaMap;
pub(crate) use base_types::{
    parse_fields_json, resolve_column, sanitize_field_name, sanitize_type_name,
};

use async_graphql::dynamic::*;
use ddb_core::types::TableSchema;

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
    let mut type_defs = type_defs::build_type_defs();
    let q = queries::build_query_fields(&type_schemas)?;
    let m = mutations::build_mutation_fields(&type_schemas);
    let s = subscriptions::build_subscription_fields(&q.known_types, &type_schemas);

    // PRD 00129 §4 (Option B): extend the base `Doogat` GraphQL object
    // with one nested accessor per registered typedef. e.g. `Doogat.link`
    // resolves to the `Link` typed object when the row's type is `link`,
    // and null otherwise. Available on every mutation response and read
    // path that returns a Doogat — eliminates the round-trip jink does
    // today through `links(where:{id:eq:...})` after every mutation.
    type_defs.doogat_type = base_types::add_typed_doogat_accessors(
        type_defs.doogat_type,
        &type_schemas,
        &q.known_types,
    );

    let mut dynamic_inputs = q.dynamic_inputs;
    dynamic_inputs.push(m.attach_input);

    let mut builder = Schema::build(
        q.query.type_name(),
        Some(m.mutation.type_name()),
        Some(s.subscription.type_name()),
    );

    builder = register_shared_types(builder, type_defs)
        .register(q.sequence_node_type)
        .register(q.sequence_info_type)
        .register(q.broken_sequence_type)
        .register(s.change_event_type)
        .register(m.sync_result_type)
        .register(m.compact_result_type)
        .register(m.git_maintenance_result_type)
        .register(m.upsert_result_type)
        .register(q.query)
        .register(m.mutation)
        .register(s.subscription)
        .data(actor)
        .data(read_pool)
        .data(TypeSchemaMap(Arc::new(
            type_schemas
                .iter()
                .map(|ts| (ts.table_name.clone(), ts.clone()))
                .collect(),
        )));

    for typed_obj in q.dynamic_types {
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

/// Register core type definitions and shared filter types onto the schema builder.
fn register_shared_types(builder: SchemaBuilder, td: type_defs::TypeDefs) -> SchemaBuilder {
    builder
        .register(Scalar::new("JSON"))
        .register(td.doogat_type)
        .register(td.inline_field_type)
        .register(td.link_type)
        .register(td.search_hit_type)
        .register(td.search_connection_type)
        .register(td.column_info_type)
        .register(td.typedef_type)
        .register(td.sql_result_type)
        .register(td.create_input)
        .register(td.create_many_item_input)
        .register(td.update_input)
        .register(td.search_field_filter_input)
        .register(td.conflict_action_enum)
        .register(td.attachment_type)
        .register(td.checkbox_item_type)
        .register(td.unlinked_mention_type)
        .register(td.suggestion_type)
        .register(td.stale_doogat_type)
        .register(td.orphan_doogat_type)
        .register(td.tag_info_type)
        .register(td.tag_entry_type)
        .register(td.tag_entries_connection_type)
        .register(td.tag_entries_where_input)
        .register(crate::filter::string_filter())
        .register(crate::filter::int_filter())
        .register(crate::filter::float_filter())
        .register(crate::filter::bool_filter())
        .register(crate::filter::id_filter())
        .register(crate::filter::tags_filter())
        .register(crate::filter::sort_order_enum())
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
        let actor = ActorHandle::spawn(dir.to_path_buf(), event_bus).expect("spawn actor");
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
            search_key: None,
            singleton: false,
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
            on_delete: ddb_core::types::OnDeleteAction::Restrict,
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

        let schema =
            build_schema(actor, pool, schemas, None).expect("schema should build successfully");
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
    async fn doogat_object_includes_typed_accessor_per_typedef_prd_00129() {
        // PRD 00129 §4 (Option B): every registered typedef gets a
        // matching nested accessor on the base `Doogat` GraphQL type so
        // mutation responses and read paths can pull typed fields in
        // one round trip. The field name is the camelCased table name.
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let schemas = vec![
            make_table_schema("link", vec![simple_column("url")]),
            make_table_schema("category-membership", vec![simple_column("link")]),
        ];

        let schema =
            build_schema(actor, pool, schemas, None).expect("schema should build successfully");
        let sdl = schema.sdl();

        // Doogat picks up `link: Link` and `categoryMembership: CategoryMembership`.
        assert!(
            sdl.contains("link: Link"),
            "Doogat must expose `link: Link` accessor, got:\n{sdl}"
        );
        assert!(
            sdl.contains("categoryMembership: CategoryMembership"),
            "Doogat must expose camelCased `categoryMembership` accessor, got:\n{sdl}"
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

        let schemas = vec![make_table_schema("contact", vec![simple_column("email")])];

        let schema = build_schema(actor, pool, schemas, None).expect("schema should build");

        let res = schema
            .execute(r#"{ __type(name: "Query") { fields { name description } } }"#)
            .await;
        let data = res.data.into_json().unwrap();
        let fields = data["__type"]["fields"].as_array().unwrap();

        let missing: Vec<&str> = fields
            .iter()
            .filter(|f| {
                f["description"].is_null() || f["description"].as_str().unwrap_or("").is_empty()
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

        let schemas = vec![make_table_schema("contact", vec![simple_column("email")])];

        let schema = build_schema(actor, pool, schemas, None).expect("schema should build");

        let res = schema
            .execute(r#"{ __type(name: "Mutation") { fields { name description } } }"#)
            .await;
        let data = res.data.into_json().unwrap();
        let fields = data["__type"]["fields"].as_array().unwrap();

        let missing: Vec<&str> = fields
            .iter()
            .filter(|f| {
                f["description"].is_null() || f["description"].as_str().unwrap_or("").is_empty()
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

        let schema = build_schema(actor, pool, vec![], None).expect("schema should build");

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
                f["description"].is_null() || f["description"].as_str().unwrap_or("").is_empty()
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
                        on_delete: ddb_core::types::OnDeleteAction::Restrict,
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

    #[tokio::test]
    async fn singleton_singular_field_uses_bare_name_when_no_collision_prd_00139() {
        // PRD 00139 §6 / T16: in the common no-collision case, the
        // singular SINGLETON field uses the bare snake_case name. The
        // collision-fallback path (`<field_base>_singleton`) lives in
        // queries.rs and fires when `emitted_query_fields` already
        // contains the bare name; constructing that case via the public
        // API is awkward because the type-name sanitizer at queries.rs:392
        // dedups duplicate `table_name` values before the singleton-field
        // emission loop runs. The fallback is a pure-code property
        // documented in the queries.rs comment block; this test pins the
        // happy path.
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());
        let mut singleton_schema = make_table_schema("app_config", vec![simple_column("theme")]);
        singleton_schema.singleton = true;

        let schema = build_schema(actor, pool, vec![singleton_schema], None).unwrap();
        let sdl = schema.sdl();
        // Bare name `app_config` (snake_case mirrors typedef name).
        assert!(
            sdl.contains("\tapp_config: App_config\n"),
            "bare singleton singular field must be present, got:\n{sdl}"
        );
        // No `_singleton` suffix should appear (no collision triggered).
        assert!(
            !sdl.contains("app_config_singleton"),
            "fallback name must NOT appear when no collision; got:\n{sdl}"
        );
    }

    #[tokio::test]
    async fn singleton_hyphenated_type_uses_snake_case_query_and_mutation_names_prd_00139() {
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());
        let mut singleton_schema = make_table_schema("foo-bar", vec![simple_column("theme")]);
        singleton_schema.singleton = true;

        let schema = build_schema(actor, pool, vec![singleton_schema], None).unwrap();
        let sdl = schema.sdl();

        assert!(
            sdl.contains("\tfoo_bar: FooBar\n"),
            "hyphenated singleton singular field must use snake_case, got:\n{sdl}"
        );
        assert!(
            sdl.contains("update_foo_bar("),
            "hyphenated singleton update mutation must use snake_case, got:\n{sdl}"
        );
        assert!(
            sdl.contains("upsert_foo_bar("),
            "hyphenated singleton upsert mutation must use snake_case, got:\n{sdl}"
        );
    }

    #[tokio::test]
    async fn singleton_singular_field_falls_back_to_snake_case_singleton_suffix_prd_00139() {
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());
        let mut singleton_schema = make_table_schema("tags", vec![simple_column("label")]);
        singleton_schema.singleton = true;

        let schema = build_schema(actor, pool, vec![singleton_schema], None)
            .expect("schema should build with fallback singleton name");
        let sdl = schema.sdl();

        assert!(
            sdl.contains("\ttags_singleton: Tags\n"),
            "singleton singular field must fall back to tags_singleton, got:\n{sdl}"
        );
        assert!(
            !sdl.contains("\ttags: Tags\n"),
            "colliding bare singleton field must not be emitted, got:\n{sdl}"
        );
    }

    #[tokio::test]
    async fn singleton_double_collision_fails_schema_build_prd_00139() {
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let mut tags = make_table_schema("tags", vec![simple_column("label")]);
        tags.singleton = true;

        let mut reserved_fallback =
            make_table_schema("tags_singleton_singleton", vec![simple_column("label")]);
        reserved_fallback.singleton = true;

        let mut colliding = make_table_schema("tags-singleton", vec![simple_column("label")]);
        colliding.singleton = true;

        let err = build_schema(actor, pool, vec![tags, reserved_fallback, colliding], None)
            .expect_err("double singleton-field collision must fail schema build");

        assert!(
            err.contains("tags-singleton"),
            "error must name the typedef that failed, got: {err}"
        );
        assert!(
            err.contains("tags_singleton"),
            "error must mention the bare colliding field, got: {err}"
        );
        assert!(
            err.contains("tags_singleton_singleton"),
            "error must mention the fallback colliding field, got: {err}"
        );
    }

    #[tokio::test]
    async fn singleton_alter_changes_emit_singular_field_after_rebuild_prd_00139() {
        // PRD 00139 §8 / T15: ALTER TABLE x SET SINGLETON flips the flag
        // on the typedef; the next schema rebuild surfaces the singular
        // query field. The reverse holds for DROP SINGLETON. Wired by:
        //   1. `executeSql` resolver triggers schema reload after any
        //      `ALTER TABLE` (`mutations.rs:411-418`).
        //   2. Reload calls `build_schema(...)` with fresh `type_schemas`
        //      loaded from disk, which now reflect the post-ALTER state.
        //   3. T12's per-type loop emits the singular field when
        //      `schema.singleton == true`.
        // This test pins the third leg by simulating a rebuild manually:
        // build the schema once with `singleton: false`, then rebuild with
        // `singleton: true`, and assert the singular field shows up.
        let tmp = tempfile::tempdir().unwrap();
        let (actor1, pool1) = test_actor_and_pool(tmp.path());

        // Phase 1: typedef is non-singleton — no singular field.
        let plain = make_table_schema("app_config", vec![simple_column("theme")]);
        let schema = build_schema(actor1, pool1, vec![plain], None).unwrap();
        let sdl_before = schema.sdl();
        assert!(
            !sdl_before.contains("Fetch the singleton App_config row"),
            "non-singleton typedef must not have singular field, got:\n{sdl_before}"
        );

        // Phase 2: same typedef, singleton flipped on — singular field appears.
        let tmp2 = tempfile::tempdir().unwrap();
        let (actor2, pool2) = test_actor_and_pool(tmp2.path());
        let mut singleton_schema = make_table_schema("app_config", vec![simple_column("theme")]);
        singleton_schema.singleton = true;
        let schema = build_schema(actor2, pool2, vec![singleton_schema], None).unwrap();
        let sdl_after = schema.sdl();
        assert!(
            sdl_after.contains("Fetch the singleton App_config row"),
            "after SET SINGLETON the singular field must appear, got:\n{sdl_after}"
        );
    }

    #[tokio::test]
    async fn singleton_typedef_emits_singular_query_field_prd_00139() {
        // PRD 00139 §6: SINGLETON typedef gains a per-type singular query
        // field alongside the existing plural field.
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let mut singleton_schema = make_table_schema("app_config", vec![simple_column("theme")]);
        singleton_schema.singleton = true;

        let schema = build_schema(actor, pool, vec![singleton_schema], None)
            .expect("schema should build with singleton typedef");
        let sdl = schema.sdl();

        // Singular field: bare snake_case (preserves typedef name shape),
        // returns nullable typed object. There are TWO occurrences: the
        // typed accessor on `type Doogat` (PRD 00129) and the singular
        // SINGLETON query field this PRD adds. Asserting the singular's
        // description string disambiguates.
        assert!(
            sdl.contains("Fetch the singleton App_config row"),
            "SDL should contain singular SINGLETON field with its description, got:\n{sdl}"
        );
        // Plural field still emitted (PRD 00139 §6 backward compat).
        assert!(
            sdl.contains("app_configs"),
            "SDL should also contain plural field 'app_configs', got:\n{sdl}"
        );
    }

    #[tokio::test]
    async fn non_singleton_typedef_does_not_emit_singular_query_field_prd_00139() {
        // PRD 00139 §6 regression: non-singleton typedefs only emit the
        // plural field, never the singular.
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let plain = make_table_schema("link", vec![simple_column("url")]);
        let schema = build_schema(actor, pool, vec![plain], None).unwrap();
        let sdl = schema.sdl();

        assert!(sdl.contains("links"), "plural field 'links' must exist");
        // The "Fetch the singleton" description string is unique to the
        // SINGLETON-emitted singular field. A non-singleton typedef must
        // not emit it.
        assert!(
            !sdl.contains("Fetch the singleton Link row"),
            "non-singleton typedef must not emit a singular SINGLETON Query field, got:\n{sdl}"
        );
    }

    #[tokio::test]
    async fn singleton_typedef_emits_update_mutation_field_prd_00139() {
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let mut singleton_schema = make_table_schema("app_config", vec![simple_column("theme")]);
        singleton_schema.singleton = true;

        let schema = build_schema(actor, pool, vec![singleton_schema], None)
            .expect("schema should build with singleton typedef");
        let sdl = schema.sdl();

        assert!(
            sdl.contains("update_app_config("),
            "SDL should contain singleton update mutation signature, got:\n{sdl}"
        );
        assert!(
            sdl.contains("input: String!"),
            "SDL should use String input for singleton update mutation, got:\n{sdl}"
        );
        assert!(
            sdl.contains("): App_config!"),
            "SDL should return the typed singleton object, got:\n{sdl}"
        );
        assert!(
            sdl.contains("Update the App_config singleton row"),
            "SDL should contain singleton update mutation description, got:\n{sdl}"
        );
    }

    #[tokio::test]
    async fn singleton_typedef_emits_upsert_mutation_field_prd_00139() {
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let mut singleton_schema = make_table_schema("app_config", vec![simple_column("theme")]);
        singleton_schema.singleton = true;

        let schema = build_schema(actor, pool, vec![singleton_schema], None)
            .expect("schema should build with singleton typedef");
        let sdl = schema.sdl();

        assert!(
            sdl.contains("upsert_app_config("),
            "SDL should contain singleton upsert mutation signature, got:\n{sdl}"
        );
        assert!(
            sdl.contains("input: String!"),
            "SDL should use String input for singleton upsert mutation, got:\n{sdl}"
        );
        assert!(
            sdl.contains("): UpsertResult!"),
            "SDL should return UpsertResult for singleton upsert mutation, got:\n{sdl}"
        );
        assert!(
            sdl.contains("Upsert the App_config singleton row"),
            "SDL should contain singleton upsert mutation description, got:\n{sdl}"
        );
    }

    #[tokio::test]
    async fn non_singleton_typedef_does_not_emit_update_or_upsert_prd_00139() {
        let tmp = tempfile::tempdir().unwrap();
        let (actor, pool) = test_actor_and_pool(tmp.path());

        let plain = make_table_schema("link", vec![simple_column("url")]);
        let schema = build_schema(actor, pool, vec![plain], None).unwrap();
        let sdl = schema.sdl();

        assert!(
            !sdl.contains("Update the Link singleton row"),
            "non-singleton typedef must not emit singleton update mutation, got:\n{sdl}"
        );
        assert!(
            !sdl.contains("Upsert the Link singleton row"),
            "non-singleton typedef must not emit singleton upsert mutation, got:\n{sdl}"
        );
    }
}
