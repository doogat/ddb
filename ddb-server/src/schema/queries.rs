use async_graphql::dynamic::*;
use async_graphql::{Name, Value as GqlValue};
use ddb_core::app_contract::{PageRequest, SearchCommand};
use ddb_core::error::DoogatError;
use ddb_core::search_query;
use ddb_core::types::{ListFilter, SearchFieldFilter, SearchFieldOp, SearchFilters, TableSchema};
use indexmap::IndexMap;

use std::collections::HashMap;
use std::sync::Arc;

use crate::actor::ActorHandle;
use crate::error::to_graphql_error;
use crate::read_pool::ReadPool;
use crate::reload::SchemaReloader;
use crate::warning_extension::forward_warnings;

use super::base_types::*;

/// Auxiliary types and collections produced by the query builder that must be
/// registered on the schema alongside the Query object itself.
pub(crate) struct QueryOutput {
    pub query: Object,
    pub known_types: HashMap<String, String>,
    pub dynamic_types: Vec<Object>,
    pub dynamic_inputs: Vec<InputObject>,
    pub sequence_node_type: Object,
    pub sequence_info_type: Object,
    pub broken_sequence_type: Object,
}

pub(crate) fn build_query_fields(type_schemas: &[TableSchema]) -> Result<QueryOutput, String> {
    let mut query = Object::new("Query");

    query = register_doogat_field(query);
    query = register_doogats_field(query);
    query = register_search_field(query);
    query = register_normalize_search_query_field(query);
    query = register_type_defs_field(query);
    query = register_sql_field(query);
    query = register_schema_version_field(query);
    query = register_checkbox_items_field(query);
    query = register_open_actions_field(query);

    let mut known_types: HashMap<String, String> = HashMap::new();
    let mut dynamic_types: Vec<Object> = Vec::new();
    let mut dynamic_inputs: Vec<InputObject> = Vec::new();
    let (mut query, sequence_node_type, sequence_info_type, broken_sequence_type) =
        super::per_type_queries::register_dynamic_type_fields(
            query,
            type_schemas,
            &mut known_types,
            &mut dynamic_types,
            &mut dynamic_inputs,
        )?;

    query = register_tags_field(query);
    query = register_tag_entries_field(query);

    Ok(QueryOutput {
        query,
        known_types,
        dynamic_types,
        dynamic_inputs,
        sequence_node_type,
        sequence_info_type,
        broken_sequence_type,
    })
}

// doogat(id)
fn register_doogat_field(query: Object) -> Object {
    query.field(
        Field::new("doogat", TypeRef::named("Doogat"), |ctx| {
            FieldFuture::new(async move {
                let pool = ctx.data::<ReadPool>()?;
                let id = ctx.args.try_get("id")?.string()?.to_string();
                let z = pool.get_doogat(id).await.map_err(to_graphql_error)?;
                Ok(Some(FieldValue::owned_any(doogat_to_value(&z))))
            })
        })
        .argument(
            InputValue::new("id", TypeRef::named_nn(TypeRef::ID))
                .description("The 14-digit timestamp ID of the doogat to fetch."),
        )
        .description("Fetch a single doogat by its 14-digit timestamp ID."),
    )
}

// doogats(type, tag, backlinksOf, limit, offset)
fn register_doogats_field(query: Object) -> Object {
    query.field(
        Field::new("doogats", TypeRef::named_nn_list_nn("Doogat"), |ctx| {
            FieldFuture::new(async move {
                let pool = ctx.data::<ReadPool>()?;
                let doogat_type = ctx
                    .args
                    .get("type")
                    .and_then(|v| v.string().ok())
                    .map(|s| s.to_string());
                let tag = ctx
                    .args
                    .get("tag")
                    .and_then(|v| v.string().ok())
                    .map(|s| s.to_string());
                let backlinks_of = ctx
                    .args
                    .get("backlinksOf")
                    .and_then(|v| v.string().ok())
                    .map(|s| s.to_string());
                let page = PageRequest::decode(validate_limit(&ctx)?, validate_offset(&ctx)?)
                    .map_err(to_graphql_error)?;
                let doogats = pool
                    .list_doogats(ListFilter {
                        doogat_type,
                        tag,
                        backlinks_of,
                        limit: Some(page.limit as i64),
                        offset: Some(page.offset as i64),
                        ..Default::default()
                    })
                    .await
                    .map_err(to_graphql_error)?;
                Ok(Some(FieldValue::list(
                    doogats
                        .iter()
                        .map(|z| FieldValue::owned_any(doogat_to_value(z))),
                )))
            })
        })
        .argument(
            InputValue::new("type", TypeRef::named(TypeRef::STRING))
                .description("Filter by doogat type name."),
        )
        .argument(
            InputValue::new("tag", TypeRef::named(TypeRef::STRING))
                .description("Filter by tag name."),
        )
        .argument(
            InputValue::new("backlinksOf", TypeRef::named(TypeRef::ID))
                .description("Return doogats that link to this ID."),
        )
        .argument(
            InputValue::new("limit", TypeRef::named(TypeRef::INT))
                .description("Maximum results to return. 1-10000. Default 50."),
        )
        .argument(
            InputValue::new("offset", TypeRef::named(TypeRef::INT))
                .description("Number of results to skip for pagination. Default 0."),
        )
        .description("List doogats with optional type, tag, and backlink filters."),
    )
}

// search(query, limit?, offset?, types?, tag?, where?)
fn register_search_field(query: Object) -> Object {
    query.field(
        Field::new("search", TypeRef::named_nn("SearchConnection"), |ctx| {
            FieldFuture::new(resolve_search(ctx))
        })
        .argument(InputValue::new("query", TypeRef::named_nn(TypeRef::STRING)).description("FTS5 query string. Supports AND, OR, NOT, quoted phrases, and field:value syntax."))
        .argument(InputValue::new(
            "types",
            TypeRef::named_list(TypeRef::STRING),
        ).description("Restrict results to these doogat types. Also limits where filter column resolution to matching type schemas."))
        .argument(InputValue::new("tag", TypeRef::named(TypeRef::STRING)).description("Shorthand for where: [{field: \"tag\", eq: value}]."))
        .argument(InputValue::new(
            "where",
            TypeRef::named_list("SearchFieldFilter"),
        ).description("Structured field filters. Each entry specifies a field name and an eq, contains, or in operator."))
        .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)).description("Maximum results to return (1-10000). Default 50."))
        .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)).description("Number of results to skip for pagination. Default 0."))
        .description("Full-text search with optional structured where filters. Returns paginated hits with BM25 ranking."),
    )
}

async fn resolve_search<'a>(
    ctx: async_graphql::dynamic::ResolverContext<'a>,
) -> async_graphql::Result<Option<FieldValue<'a>>> {
    let pool = ctx.data::<ReadPool>()?;
    let q = ctx.args.try_get("query")?.string()?.to_string();
    let page = PageRequest::decode(validate_limit(&ctx)?, validate_offset(&ctx)?)
        .map_err(to_graphql_error)?;

    let types = parse_search_types(&ctx);
    let tag = parse_search_tag(&ctx);
    let where_filters = parse_search_where_filters(&ctx);

    let normalized = search_query::normalize(&q);
    let (hits, total) =
        fetch_search_results(&ctx, pool, q, page, types, tag, where_filters).await?;

    let mut obj = IndexMap::new();
    obj.insert(
        Name::new("hits"),
        GqlValue::List(hits.iter().map(search_hit_to_value).collect()),
    );
    obj.insert(Name::new("totalCount"), GqlValue::from(total as i64));
    obj.insert(Name::new("queryNormalized"), GqlValue::from(normalized));
    Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
}

fn parse_search_types(ctx: &async_graphql::dynamic::ResolverContext) -> Option<Vec<String>> {
    ctx.args.get("types").and_then(|v| v.list().ok()).map(|list| {
        list.iter()
            .filter_map(|v| v.string().ok().map(|s| s.to_string()))
            .collect::<Vec<_>>()
    })
}

fn parse_search_tag(ctx: &async_graphql::dynamic::ResolverContext) -> Option<String> {
    ctx.args
        .get("tag")
        .and_then(|v| v.string().ok().map(|s| s.to_string()))
}

fn parse_search_where_filters(
    ctx: &async_graphql::dynamic::ResolverContext,
) -> Option<Vec<SearchFieldFilter>> {
    ctx.args.get("where").and_then(|v| v.list().ok()).map(|list| {
        list.iter()
            .filter_map(parse_single_search_field_filter)
            .collect::<Vec<_>>()
    })
}

fn parse_single_search_field_filter(
    v: async_graphql::dynamic::ValueAccessor,
) -> Option<SearchFieldFilter> {
    let obj = v.object().ok()?;
    let field = obj.get("field")?.string().ok()?.to_string();
    if let Some(eq) = obj.get("eq") {
        let val = eq.string().ok()?.to_string();
        return Some(SearchFieldFilter {
            field,
            op: SearchFieldOp::Eq(val),
        });
    }
    if let Some(contains) = obj.get("contains") {
        let val = contains.string().ok()?.to_string();
        return Some(SearchFieldFilter {
            field,
            op: SearchFieldOp::Contains(val),
        });
    }
    if let Some(in_val) = obj.get("in") {
        let vals = in_val
            .list()
            .ok()?
            .iter()
            .filter_map(|v| v.string().ok().map(|s| s.to_string()))
            .collect();
        return Some(SearchFieldFilter {
            field,
            op: SearchFieldOp::In(vals),
        });
    }
    None
}

// SearchCommand has no slot for structured `where` filters, so only the
// types/tag case routes through the app-contract facade.
async fn fetch_search_results<'a>(
    ctx: &async_graphql::dynamic::ResolverContext<'a>,
    pool: &ReadPool,
    q: String,
    page: PageRequest,
    types: Option<Vec<String>>,
    tag: Option<String>,
    where_filters: Option<Vec<SearchFieldFilter>>,
) -> async_graphql::Result<(Vec<ddb_core::types::SearchResult>, usize)> {
    if where_filters.is_some() {
        let filters = SearchFilters {
            types,
            tag,
            where_filters,
        };
        let result = pool
            .search(q, page.limit, page.offset, filters)
            .await
            .map_err(to_graphql_error)?;
        Ok((result.hits, result.total_count))
    } else {
        let output = pool
            .search_command(SearchCommand { query: q, page, types, tag })
            .await
            .map_err(to_graphql_error)?;
        forward_warnings(ctx, &output.warnings);
        Ok((output.value.items, output.value.total))
    }
}

// normalizeSearchQuery(query)
fn register_normalize_search_query_field(query: Object) -> Object {
    query.field(
        Field::new(
            "normalizeSearchQuery",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let q = ctx.args.try_get("query")?.string()?.to_string();
                    // PRD 00121 invariant: normalizeSearchQuery must agree
                    // with search() on the set of valid inputs.
                    // validate_and_compile rejects bare wildcards,
                    // unparseable input, empty queries, and non-tag
                    // negated field filters.
                    search_query::validate_and_compile(&q).map_err(|_| {
                        to_graphql_error(DoogatError::BadRequest(format!(
                            "invalid search query: {q}"
                        )))
                    })?;
                    let normalized = search_query::normalize(&q);
                    Ok(Some(FieldValue::from(GqlValue::from(normalized))))
                })
            },
        )
        .argument(InputValue::new("query", TypeRef::named_nn(TypeRef::STRING)).description("The search query string to normalize."))
        .description("Return the canonical form of a search query without executing it. Useful for deduplication and saved searches. Rejects the same malformed inputs as search() so the two endpoints agree on validity."),
    )
}

// typeDefs
fn register_type_defs_field(query: Object) -> Object {
    query.field(
        Field::new("typeDefs", TypeRef::named_nn_list_nn("TypeDef"), |ctx| {
            FieldFuture::new(async move {
                let pool = ctx.data::<ReadPool>()?;
                let schemas = pool.get_type_schemas().await.map_err(to_graphql_error)?;
                Ok(Some(FieldValue::list(
                    schemas
                        .iter()
                        .map(|s| FieldValue::owned_any(typedef_to_value(s))),
                )))
            })
        })
        .description("List all registered type definitions with their column schemas."),
    )
}

// sql(query, format?)
fn register_sql_field(query: Object) -> Object {
    query.field(
        Field::new("sql", TypeRef::named_nn("SqlResult"), |ctx| {
            FieldFuture::new(async move {
                let q = ctx.args.try_get("query")?.string()?.to_string();
                let fmt = validate_format(&ctx)?;
                let result = if crate::pgwire::is_select_only(&q) {
                    let pool = ctx.data::<ReadPool>()?;
                    pool.execute_select(q).await.map_err(to_graphql_error)?
                } else {
                    let a = ctx.data::<ActorHandle>()?;
                    a.execute_sql(q).await.map_err(to_graphql_error)?
                };
                Ok(Some(FieldValue::owned_any(sql_result_to_value(&result, fmt))))
            })
        })
        .argument(InputValue::new("query", TypeRef::named_nn(TypeRef::STRING)).description("SQL SELECT query to execute."))
        .argument(InputValue::new("format", TypeRef::named(TypeRef::STRING)).description("Response format: 'array' (default) or 'objects'."))
        .description("Execute a read-only SQL SELECT query. Non-SELECT statements route through the mutation actor."),
    )
}

// schemaVersion
fn register_schema_version_field(query: Object) -> Object {
    query.field(Field::new(
        "schemaVersion",
        TypeRef::named_nn(TypeRef::INT),
        |ctx| {
            FieldFuture::new(async move {
                let reloader = ctx.data::<Arc<SchemaReloader>>()?;
                Ok(Some(FieldValue::value(GqlValue::from(
                    reloader.version() as i64
                ))))
            })
        },
    ).description("Current schema version counter. Increments on typedef creation, modification, or deletion."))
}

// checkboxItems(state?, doogatId?, limit?, offset?)
fn register_checkbox_items_field(query: Object) -> Object {
    query.field(
        Field::new(
            "checkboxItems",
            TypeRef::named_nn_list_nn("CheckboxItem"),
            |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let state = ctx
                        .args
                        .get("state")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
                    let doogat_id = ctx
                        .args
                        .get("doogatId")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
                    let limit = validate_limit(&ctx)?;
                    let offset = validate_offset(&ctx)?;
                    let rows = pool
                        .query_checkboxes(state, doogat_id, limit, offset)
                        .await
                        .map_err(to_graphql_error)?;
                    Ok(Some(FieldValue::list(rows.iter().map(|row| {
                        FieldValue::owned_any(checkbox_row_to_value(row))
                    }))))
                })
            },
        )
        .argument(InputValue::new("state", TypeRef::named(TypeRef::STRING)).description("Filter by checkbox state (open, done, cancelled)."))
        .argument(InputValue::new("doogatId", TypeRef::named(TypeRef::ID)).description("Filter to a specific doogat's checkboxes."))
        .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)).description("Maximum results to return."))
        .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)).description("Number of results to skip."))
        .description("Query checkbox/task items across doogats. Filter by state (open/done/cancelled) or doogat ID."),
    )
}

// openActions(limit?)
fn register_open_actions_field(query: Object) -> Object {
    query.field(
        Field::new(
            "openActions",
            TypeRef::named_nn_list_nn("CheckboxItem"),
            |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let limit = validate_limit(&ctx)?;
                    let rows = pool
                        .query_checkboxes(Some("open".to_string()), None, limit, None)
                        .await
                        .map_err(to_graphql_error)?;
                    Ok(Some(FieldValue::list(rows.iter().map(|row| {
                        FieldValue::owned_any(checkbox_row_to_value(row))
                    }))))
                })
            },
        )
        .argument(
            InputValue::new("limit", TypeRef::named(TypeRef::INT))
                .description("Maximum results to return."),
        )
        .description(
            "Shorthand for checkboxItems(state: \"open\"). Returns uncompleted action items.",
        ),
    )
}

// tags
fn register_tags_field(query: Object) -> Object {
    query.field(
        Field::new("tags", TypeRef::named_nn_list_nn("TagInfo"), |ctx| {
            FieldFuture::new(async move {
                let pool = ctx.data::<ReadPool>()?;
                let tags = pool.list_tags().await.map_err(to_graphql_error)?;
                Ok(Some(FieldValue::list(tags.iter().map(|(name, count)| {
                    FieldValue::owned_any(tag_info_to_value(name, *count))
                }))))
            })
        })
        .description("List all tags with their usage counts."),
    )
}

// tagEntries
fn register_tag_entries_field(query: Object) -> Object {
    query.field(
        Field::new(
            "tagEntries",
            TypeRef::named_nn("TagEntryConnection"),
            |ctx| FieldFuture::new(resolve_tag_entries(ctx)),
        )
        .argument(
            InputValue::new("where", TypeRef::named("TagEntriesWhere"))
                .description("Filter conditions for tag entries."),
        )
        .description(
            "Query individual tag assignments with where filters on doogatId and tag name.",
        ),
    )
}

async fn resolve_tag_entries<'a>(
    ctx: async_graphql::dynamic::ResolverContext<'a>,
) -> async_graphql::Result<Option<FieldValue<'a>>> {
    let pool = ctx.data::<ReadPool>()?;
    let filter = parse_tag_entries_filter(&ctx);
    let entries = pool.query_tags(filter).await.map_err(to_graphql_error)?;
    let total_count = entries.len() as i64;
    let items = GqlValue::List(entries.iter().map(tag_entry_to_value).collect());
    let mut conn = IndexMap::new();
    conn.insert(Name::new("items"), items);
    conn.insert(Name::new("totalCount"), GqlValue::from(total_count));
    Ok(Some(FieldValue::owned_any(GqlValue::Object(conn))))
}

fn parse_tag_entries_filter(
    ctx: &async_graphql::dynamic::ResolverContext,
) -> ddb_core::types::TagQueryFilter {
    let mut filter = ddb_core::types::TagQueryFilter::default();
    let Some(Ok(GqlValue::Object(map))) =
        ctx.args.get("where").map(|v| v.deserialize::<GqlValue>())
    else {
        return filter;
    };
    apply_tag_entries_doogat_id_filter(&map, &mut filter);
    apply_tag_entries_tag_filter(&map, &mut filter);
    filter
}

fn apply_tag_entries_doogat_id_filter(
    map: &IndexMap<Name, GqlValue>,
    filter: &mut ddb_core::types::TagQueryFilter,
) {
    let Some(GqlValue::Object(id_filter)) = map.get("doogatId") else {
        return;
    };
    if let Some(GqlValue::String(eq)) = id_filter.get("eq") {
        filter.doogat_id_eq = Some(eq.to_string());
    }
    if let Some(GqlValue::List(vals)) = id_filter.get("in") {
        filter.doogat_id_in = Some(gql_string_list_values(vals));
    }
}

fn apply_tag_entries_tag_filter(
    map: &IndexMap<Name, GqlValue>,
    filter: &mut ddb_core::types::TagQueryFilter,
) {
    let Some(GqlValue::Object(tag_filter)) = map.get("tag") else {
        return;
    };
    if let Some(GqlValue::String(eq)) = tag_filter.get("eq") {
        filter.tag_eq = Some(eq.to_string());
    }
    if let Some(GqlValue::String(c)) = tag_filter.get("contains") {
        filter.tag_contains = Some(c.to_string());
    }
    if let Some(GqlValue::List(vals)) = tag_filter.get("in") {
        filter.tag_in = Some(gql_string_list_values(vals));
    }
}

fn gql_string_list_values(vals: &[GqlValue]) -> Vec<String> {
    vals.iter()
        .filter_map(|v| {
            if let GqlValue::String(s) = v {
                Some(s.to_string())
            } else {
                None
            }
        })
        .collect()
}
