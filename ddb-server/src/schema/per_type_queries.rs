//! Per-type dynamic query field registration: the per-typedef loop that
//! builds typed objects/inputs and the plural, aggregate, and singleton
//! query fields for every registered `TableSchema`. Split out of
//! `queries.rs` (PRD 00183 round 2) to keep that file under the size limit;
//! the discovery-field registrars (search, tags, etc.) stay there.

use async_graphql::dynamic::*;
use async_graphql::{Name, Value as GqlValue};
use ddb_core::types::TableSchema;
use indexmap::IndexMap;

use std::collections::{HashMap, HashSet};

use crate::error::to_graphql_error;
use crate::read_pool::ReadPool;

use super::base_types::*;

const RESERVED_QUERY_FIELD_NAMES: &[&str] = &[
    "doogat",
    "doogats",
    "search",
    "normalizeSearchQuery",
    "typeDefs",
    "sql",
    "schemaVersion",
    "checkboxItems",
    "openActions",
    "unlinkedMentions",
    "suggestions",
    "staleDoogats",
    "orphanDoogats",
    "sequenceInfo",
    "sequenceChildren",
    "sequenceBreadcrumb",
    "brokenSequences",
    "tags",
    "tagEntries",
];

/// Mutable accumulator state threaded through the per-typedef registration
/// loop. Bundled into one struct so `register_fields_for_schema` takes a
/// handful of arguments instead of one per accumulator.
struct TypeRegistrationState<'a> {
    known_types: &'a HashMap<String, String>,
    schema_lookup: &'a crate::filter::SchemaLookup,
    dynamic_types: &'a mut Vec<Object>,
    dynamic_inputs: &'a mut Vec<InputObject>,
    emitted_query_fields: &'a mut HashSet<String>,
}

// Discovery queries plus the dynamic per-type query fields, the per-type
// object/input types they register, and the forward-relation filter inputs.
pub(super) fn register_dynamic_type_fields(
    query: Object,
    type_schemas: &[TableSchema],
    known_types: &mut HashMap<String, String>,
    dynamic_types: &mut Vec<Object>,
    dynamic_inputs: &mut Vec<InputObject>,
) -> Result<(Object, Object, Object, Object), String> {
    // -- Discovery queries (orphans, unlinked mentions, sequences, etc.) --
    let super::discovery_queries::DiscoveryOutput {
        query: q,
        sequence_node_type,
        sequence_info_type,
        broken_sequence_type,
    } = super::discovery_queries::register_discovery_fields(query);
    let mut query = q;

    *known_types = collect_known_types(type_schemas);
    // Lookup of every registered type schema, threaded into the where-builder
    // for relation resolution (consumed by later relation tasks). Built once.
    let schema_lookup = build_schema_lookup(type_schemas, known_types);
    // PRD 00139 §6 / T16: track per-typedef-loop Query field names so the
    // singleton singular field can detect collisions and fall back to
    // `<base>_singleton`; a fallback collision is a hard error at
    // schema-build time (see `register_per_type_singleton_field_if_applicable`).
    let mut emitted_query_fields: HashSet<String> = RESERVED_QUERY_FIELD_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();

    let mut state = TypeRegistrationState {
        known_types,
        schema_lookup: &schema_lookup,
        dynamic_types,
        dynamic_inputs,
        emitted_query_fields: &mut emitted_query_fields,
    };
    for schema in type_schemas {
        query = register_fields_for_schema(query, schema, type_schemas, &mut state)?;
    }

    // Forward-relation `{Target}RelationFilter` inputs for every REFERENCES
    // column whose target type is registered. Keyed off `known_types` so it
    // agrees with `build_where_input` on which targets get a RelationFilter.
    register_relation_filter_inputs(type_schemas, state.known_types, state.dynamic_inputs);

    Ok((query, sequence_node_type, sequence_info_type, broken_sequence_type))
}

fn collect_known_types(type_schemas: &[TableSchema]) -> HashMap<String, String> {
    let mut known_types = HashMap::new();
    let mut seen_gql_names: HashSet<String> = HashSet::new();
    for s in type_schemas {
        let gql_name = sanitize_type_name(&s.table_name);
        if !seen_gql_names.insert(gql_name.clone()) {
            tracing::warn!(
                "skipping typedef '{}': GraphQL name '{}' collides with another type",
                s.table_name,
                gql_name
            );
            continue;
        }
        known_types.insert(s.table_name.clone(), gql_name);
    }
    known_types
}

fn build_schema_lookup(
    type_schemas: &[TableSchema],
    known_types: &HashMap<String, String>,
) -> crate::filter::SchemaLookup {
    type_schemas
        .iter()
        .filter(|s| known_types.contains_key(&s.table_name))
        .map(|s| (s.table_name.clone(), s.clone()))
        .collect()
}

fn register_relation_filter_inputs(
    type_schemas: &[TableSchema],
    known_types: &HashMap<String, String>,
    dynamic_inputs: &mut Vec<InputObject>,
) {
    for relation_input in crate::relation_filter::relation_input_objects(type_schemas, known_types)
    {
        dynamic_inputs.push(relation_input);
    }
}

// Registers one typedef's typed object/inputs plus its plural, aggregate,
// and (if applicable) singleton query fields.
fn register_fields_for_schema(
    mut query: Object,
    schema: &TableSchema,
    type_schemas: &[TableSchema],
    state: &mut TypeRegistrationState,
) -> Result<Object, String> {
    let type_name = match state.known_types.get(&schema.table_name) {
        Some(name) => name.clone(),
        None => return Ok(query),
    };
    let field_base = sanitize_field_name(&schema.table_name);
    let plural = pluralize_preserving_case(&field_base);

    // Create typed object and per-type Where/OrderBy inputs plus the
    // Connection and Aggregate types.
    let (
        typed_obj,
        where_input,
        order_by_input,
        connection_type,
        aggregate_type,
        aggregate_group_type,
    ) = register_typed_object_and_inputs(&type_name, schema, type_schemas, state.known_types);
    state.dynamic_types.push(typed_obj);
    state.dynamic_inputs.push(where_input);
    state.dynamic_inputs.push(order_by_input);
    state.dynamic_types.push(connection_type);
    state.dynamic_types.push(aggregate_type);
    state.dynamic_types.push(aggregate_group_type);

    query = register_per_type_list_field(query, schema, &type_name, state.schema_lookup, &plural);
    // T16: track plural field name for singleton-collision detection.
    state.emitted_query_fields.insert(plural.clone());

    query =
        register_per_type_aggregate_field(query, schema, &type_name, state.schema_lookup, &plural);

    // PRD 00139 §6: SINGLETON typedefs gain a per-type singular query
    // field. The field returns the single materialized row or null
    // (when the typedef is not yet auto-seeded). The plural field
    // already added above stays generated for backward compat.
    register_per_type_singleton_field_if_applicable(
        query,
        schema,
        &type_name,
        state.emitted_query_fields,
    )
}

// Builds the per-type object and its companion input/object types.
fn register_typed_object_and_inputs(
    type_name: &str,
    schema: &TableSchema,
    type_schemas: &[TableSchema],
    known_types: &HashMap<String, String>,
) -> (Object, InputObject, InputObject, Object, Object, Object) {
    // Create typed object
    let typed_obj = build_typed_object(type_name, schema, known_types);

    // Create per-type Where, OrderBy inputs, Connection and Aggregate types
    let where_input = crate::filter::build_where_input(type_name, schema, type_schemas, known_types);
    let order_by_input = crate::filter::build_order_by_input(type_name, schema);
    let connection_type = crate::filter::build_connection_type(type_name);
    let aggregate_type = crate::filter::build_aggregate_type(type_name, schema);
    let aggregate_group_type = crate::filter::build_aggregate_group_type(type_name, schema);

    (
        typed_obj,
        where_input,
        order_by_input,
        connection_type,
        aggregate_type,
        aggregate_group_type,
    )
}

// Per-type query returning Connection (items + totalCount)
fn register_per_type_list_field(
    query: Object,
    schema: &TableSchema,
    type_name: &str,
    schema_lookup: &crate::filter::SchemaLookup,
    plural: &str,
) -> Object {
    let schema_clone = schema.clone();
    let table_name = schema.table_name.clone();
    let schema_lookup = schema_lookup.clone();
    let query_desc = format!(
        "List all {} doogats with optional where, orderBy, tag, and pagination filters.",
        type_name
    );
    query.field(
        Field::new(
            plural,
            TypeRef::named_nn(format!("{type_name}Connection")),
            move |ctx| {
                FieldFuture::new(resolve_per_type_list(
                    ctx,
                    schema_clone.clone(),
                    table_name.clone(),
                    schema_lookup.clone(),
                ))
            },
        )
        .argument(InputValue::new("where", TypeRef::named(format!("{type_name}Where"))).description("Filter conditions."))
        .argument(InputValue::new(
            "orderBy",
            TypeRef::named(format!("{type_name}OrderBy")),
        ).description("Sort order specification."))
        .argument(InputValue::new("tag", TypeRef::named(TypeRef::STRING)).description("Filter by tag name."))
        .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)).description("Maximum results to return."))
        .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)).description("Number of results to skip."))
        .argument(InputValue::new("distinct", TypeRef::named(TypeRef::STRING)).description("Column to deduplicate results by."))
        .description(&query_desc),
    )
}

/// Parsed per-type list field arguments (tag/limit/offset/distinct/orderBy).
struct ListArgs {
    tag: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    distinct: Option<String>,
    order_sql: Option<String>,
}

fn parse_list_args(ctx: &ResolverContext, schema: &TableSchema) -> async_graphql::Result<ListArgs> {
    let tag = ctx
        .args
        .get("tag")
        .and_then(|v| v.string().ok())
        .map(|s| s.to_string());
    let limit = validate_limit(ctx)?;
    let offset = validate_offset(ctx)?;
    let distinct = ctx
        .args
        .get("distinct")
        .and_then(|v| v.string().ok())
        .and_then(|col| resolve_column(&schema.columns, col).map(|s| s.to_string()));
    // Parse optional orderBy
    let order_sql = ctx
        .args
        .get("orderBy")
        .and_then(|v| v.deserialize::<GqlValue>().ok())
        .and_then(|v| crate::filter::build_order_sql(&v, schema));
    Ok(ListArgs {
        tag,
        limit,
        offset,
        distinct,
        order_sql,
    })
}

async fn resolve_per_type_list<'a>(
    ctx: ResolverContext<'a>,
    schema: TableSchema,
    table_name: String,
    schema_lookup: crate::filter::SchemaLookup,
) -> async_graphql::Result<Option<FieldValue<'a>>> {
    let pool = ctx.data::<ReadPool>()?;
    let args = parse_list_args(&ctx, &schema)?;
    // Build where clause. Validation errors (e.g. empty `tags` filter,
    // issue #11) bubble out as GraphQL errors instead of silently
    // matching no rows.
    let wc = build_list_where_clause(&ctx, &schema, &schema_lookup)?;

    // Fetch items (always use filtered_list)
    let doogats = pool
        .filtered_list(ddb_core::types::TypedListQuery {
            table_name: table_name.clone(),
            where_sql: wc.sql.clone(),
            params: wc.params.clone(),
            order_sql: args.order_sql.clone(),
            tag: args.tag.clone(),
            limit: args.limit,
            offset: args.offset,
            distinct: args.distinct.clone(),
        })
        .await
        .map_err(to_graphql_error)?;

    let total_count = fetch_list_total_count(
        pool,
        &table_name,
        &wc,
        args.tag.as_deref(),
        args.distinct.as_deref(),
    )
    .await?;

    Ok(Some(FieldValue::owned_any(build_list_connection(
        &doogats,
        &schema,
        total_count,
    ))))
}

fn build_list_connection(
    doogats: &[ddb_core::types::ParsedDoogat],
    schema: &TableSchema,
    total_count: i64,
) -> GqlValue {
    let items = GqlValue::List(
        doogats
            .iter()
            .map(|z| typed_doogat_to_value(z, schema))
            .collect(),
    );
    let mut conn = IndexMap::new();
    conn.insert(Name::new("items"), items);
    conn.insert(Name::new("totalCount"), GqlValue::from(total_count));
    GqlValue::Object(conn)
}

fn build_list_where_clause(
    ctx: &ResolverContext,
    schema: &TableSchema,
    schema_lookup: &crate::filter::SchemaLookup,
) -> async_graphql::Result<crate::filter::WhereClause> {
    let where_val = ctx.args.get("where").map(|v| v.deserialize::<GqlValue>());
    match &where_val {
        Some(Ok(ref filter_input)) => {
            crate::filter::try_build_where_sql(filter_input, schema, schema_lookup)
                .map_err(|msg| async_graphql::ServerError::new(msg, None).into())
        }
        _ => Ok(crate::filter::WhereClause::empty()),
    }
}

// Fetch totalCount (same where + tag filters, no limit/offset)
async fn fetch_list_total_count(
    pool: &ReadPool,
    table_name: &str,
    wc: &crate::filter::WhereClause,
    tag: Option<&str>,
    distinct: Option<&str>,
) -> async_graphql::Result<i64> {
    let mut count_conditions = Vec::new();
    if !wc.sql.is_empty() {
        count_conditions.push(wc.sql.clone());
    }
    if let Some(t) = tag {
        count_conditions.push(format!(
            "id IN (SELECT doogat_id FROM _ddb_tags WHERE tag = '{}')",
            t.replace('\'', "''")
        ));
    }
    let count_where = if count_conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", count_conditions.join(" AND "))
    };
    let count_sql = if let Some(col) = distinct {
        let escaped = col.replace('"', "\"\"");
        format!("SELECT COUNT(DISTINCT \"{escaped}\") FROM \"{table_name}\"{count_where}")
    } else {
        format!("SELECT COUNT(*) FROM \"{table_name}\"{count_where}")
    };
    let count_row = pool
        .aggregate_query(count_sql, wc.params.clone())
        .await
        .map_err(to_graphql_error)?;
    Ok(count_row.first().and_then(|s| s.parse().ok()).unwrap_or(0))
}

// Per-type aggregate query
fn register_per_type_aggregate_field(
    query: Object,
    schema: &TableSchema,
    type_name: &str,
    schema_lookup: &crate::filter::SchemaLookup,
    plural: &str,
) -> Object {
    let agg_type_name = format!("{type_name}Aggregate");
    let schema_clone = schema.clone();
    let table_name = schema.table_name.clone();
    let schema_lookup = schema_lookup.clone();
    let agg_desc = format!(
        "Aggregate {} doogats. Returns count with optional groupBy breakdown.",
        type_name
    );
    query.field(
        Field::new(
            format!("{plural}Aggregate"),
            TypeRef::named_nn(&agg_type_name),
            move |ctx| {
                FieldFuture::new(resolve_per_type_aggregate(
                    ctx,
                    schema_clone.clone(),
                    table_name.clone(),
                    schema_lookup.clone(),
                ))
            },
        )
        .argument(
            InputValue::new("where", TypeRef::named(format!("{type_name}Where")))
                .description("Filter conditions for aggregation."),
        )
        .argument(
            InputValue::new("groupBy", TypeRef::named(TypeRef::STRING))
                .description("Column to group results by."),
        )
        .description(&agg_desc),
    )
}

async fn resolve_per_type_aggregate<'a>(
    ctx: ResolverContext<'a>,
    schema: TableSchema,
    table_name: String,
    schema_lookup: crate::filter::SchemaLookup,
) -> async_graphql::Result<Option<FieldValue<'a>>> {
    let pool = ctx.data::<ReadPool>()?;
    // Aggregate `where` shares the issue-#11 validation; bubble errors out
    // cleanly.
    let wc = build_aggregate_where_clause(&ctx, &schema, &schema_lookup)?;

    let group_by = ctx
        .args
        .get("groupBy")
        .and_then(|v| v.string().ok())
        .and_then(|col| resolve_column(&schema.columns, col).map(|s| s.to_string()));

    let (sql, names) =
        crate::filter::build_aggregate_sql_grouped(&table_name, &schema, &wc, group_by.as_deref());

    if group_by.is_some() {
        run_grouped_aggregate(pool, sql, wc.params, &names).await
    } else {
        run_scalar_aggregate(pool, sql, wc.params, &names).await
    }
}

fn build_aggregate_where_clause(
    ctx: &ResolverContext,
    schema: &TableSchema,
    schema_lookup: &crate::filter::SchemaLookup,
) -> async_graphql::Result<crate::filter::WhereClause> {
    match ctx
        .args
        .get("where")
        .and_then(|v| v.deserialize::<GqlValue>().ok())
    {
        Some(v) => crate::filter::try_build_where_sql(&v, schema, schema_lookup)
            .map_err(|msg| async_graphql::ServerError::new(msg, None).into()),
        None => Ok(crate::filter::WhereClause::empty()),
    }
}

async fn run_grouped_aggregate<'a>(
    pool: &ReadPool,
    sql: String,
    params: Vec<ddb_core::types::QueryValue>,
    names: &[String],
) -> async_graphql::Result<Option<FieldValue<'a>>> {
    let rows = pool
        .aggregate_query_rows(sql, params)
        .await
        .map_err(to_graphql_error)?;
    let groups: Vec<GqlValue> = rows
        .iter()
        .map(|row| crate::filter::aggregate_row_to_value(row, names))
        .collect();
    // Sum group counts for top-level count
    let total_count: i64 = groups
        .iter()
        .map(|g| {
            if let GqlValue::Object(map) = g {
                if let Some(GqlValue::Number(n)) = map.get("count") {
                    return n.as_i64().unwrap_or(0);
                }
            }
            0
        })
        .sum();
    let mut val = IndexMap::new();
    val.insert(Name::new("count"), GqlValue::from(total_count));
    val.insert(Name::new("groups"), GqlValue::List(groups));
    Ok(Some(FieldValue::owned_any(GqlValue::Object(val))))
}

async fn run_scalar_aggregate<'a>(
    pool: &ReadPool,
    sql: String,
    params: Vec<ddb_core::types::QueryValue>,
    names: &[String],
) -> async_graphql::Result<Option<FieldValue<'a>>> {
    let row = pool
        .aggregate_query(sql, params)
        .await
        .map_err(to_graphql_error)?;
    let val = crate::filter::aggregate_row_to_value(&row, names);
    Ok(Some(FieldValue::owned_any(val)))
}

// PRD 00139 §6 / T16: SINGLETON typedefs gain a per-type singular query
// field with name-collision fallback. Returns the updated Query object, or an
// error when both the base name and the `_singleton` fallback collide.
fn register_per_type_singleton_field_if_applicable(
    query: Object,
    schema: &TableSchema,
    type_name: &str,
    emitted_query_fields: &mut HashSet<String>,
) -> Result<Object, String> {
    if !schema.singleton {
        return Ok(query);
    }
    let singular_field_name = resolve_singleton_field_name(schema, emitted_query_fields)?;
    let schema_clone = schema.clone();
    let table_name = schema.table_name.clone();
    let singular_desc = format!(
        "Fetch the singleton {} row, or null when the typedef is empty.",
        type_name
    );
    let type_name_owned = type_name.to_string();
    Ok(query.field(
        Field::new(
            &singular_field_name,
            TypeRef::named(&type_name_owned),
            move |ctx| {
                FieldFuture::new(resolve_per_type_singleton(
                    ctx,
                    schema_clone.clone(),
                    table_name.clone(),
                ))
            },
        )
        .description(&singular_desc),
    ))
}

// PRD 00139 §6 / T16: name-collision fallback. If the bare SINGLETON field
// base (e.g. `foo_bar`) already shipped as another Query field, fall back to
// `<field_base>_singleton`. If the fallback ALSO collides, fail schema build
// so the operator sees the bad schema instead of silently losing the field.
fn resolve_singleton_field_name(
    schema: &TableSchema,
    emitted_query_fields: &mut HashSet<String>,
) -> Result<String, String> {
    let base = singleton_field_base(&schema.table_name);
    let name = if emitted_query_fields.contains(&base) {
        let fallback = format!("{base}_singleton");
        if emitted_query_fields.contains(&fallback) {
            return Err(format!(
                "cannot build SINGLETON singular field for typedef '{}': '{}' collides on Query and fallback '{}' is also already in use",
                schema.table_name, base, fallback
            ));
        }
        tracing::warn!(
            "SINGLETON typedef '{}' singular field '{}' collides with another Query field; emitting as '{}' instead",
            schema.table_name,
            base,
            fallback
        );
        fallback
    } else {
        base
    };
    emitted_query_fields.insert(name.clone());
    Ok(name)
}

async fn resolve_per_type_singleton<'a>(
    ctx: ResolverContext<'a>,
    schema: TableSchema,
    table_name: String,
) -> async_graphql::Result<Option<FieldValue<'a>>> {
    let pool = ctx.data::<ReadPool>()?;
    let doogats = pool
        .filtered_list(ddb_core::types::TypedListQuery {
            table_name,
            where_sql: String::new(),
            params: Vec::new(),
            order_sql: None,
            tag: None,
            limit: Some(1),
            offset: None,
            distinct: None,
        })
        .await
        .map_err(to_graphql_error)?;
    match doogats.first() {
        Some(d) => Ok(Some(FieldValue::owned_any(typed_doogat_to_value(
            d, &schema,
        )))),
        None => Ok(None),
    }
}
