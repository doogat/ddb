use async_graphql::dynamic::*;
use async_graphql::{Name, Value as GqlValue};
use ddb_core::error::DoogatError;
use ddb_core::search_query;
use ddb_core::types::{ListFilter, SearchFieldFilter, SearchFieldOp, SearchFilters, TableSchema};
use indexmap::IndexMap;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::actor::ActorHandle;
use crate::error::to_graphql_error;
use crate::read_pool::ReadPool;
use crate::reload::SchemaReloader;

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

    // doogat(id)
    {
        query = query.field(
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
        );
    }

    // doogats(type, tag, backlinksOf, limit, offset)
    {
        query = query.field(
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
                    let limit = validate_limit(&ctx)?;
                    let offset = validate_offset(&ctx)?;
                    let doogats = pool
                        .list_doogats(ListFilter {
                            doogat_type,
                            tag,
                            backlinks_of,
                            limit,
                            offset,
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
                    .description("Maximum results to return."),
            )
            .argument(
                InputValue::new("offset", TypeRef::named(TypeRef::INT))
                    .description("Number of results to skip for pagination."),
            )
            .description("List doogats with optional type, tag, and backlink filters."),
        );
    }

    // search(query, limit?, offset?, types?, tag?, where?)
    {
        query = query.field(
            Field::new("search", TypeRef::named_nn("SearchConnection"), |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let q = ctx.args.try_get("query")?.string()?.to_string();
                    let limit = validate_limit(&ctx)?.unwrap_or(20) as usize;
                    let offset = validate_offset(&ctx)?.unwrap_or(0) as usize;

                    let types = ctx
                        .args
                        .get("types")
                        .and_then(|v| v.list().ok())
                        .map(|list| {
                            list.iter()
                                .filter_map(|v| v.string().ok().map(|s| s.to_string()))
                                .collect::<Vec<_>>()
                        });

                    let tag = ctx
                        .args
                        .get("tag")
                        .and_then(|v| v.string().ok().map(|s| s.to_string()));

                    let where_filters = ctx
                        .args
                        .get("where")
                        .and_then(|v| v.list().ok())
                        .map(|list| {
                            list.iter()
                                .filter_map(|v| {
                                    let obj = v.object().ok()?;
                                    let field =
                                        obj.get("field")?.string().ok()?.to_string();
                                    if let Some(eq) = obj.get("eq") {
                                        let val = eq.string().ok()?.to_string();
                                        Some(SearchFieldFilter {
                                            field,
                                            op: SearchFieldOp::Eq(val),
                                        })
                                    } else if let Some(contains) = obj.get("contains") {
                                        let val = contains.string().ok()?.to_string();
                                        Some(SearchFieldFilter {
                                            field,
                                            op: SearchFieldOp::Contains(val),
                                        })
                                    } else if let Some(in_val) = obj.get("in") {
                                        let vals = in_val.list().ok()?
                                            .iter()
                                            .filter_map(|v| v.string().ok().map(|s| s.to_string()))
                                            .collect();
                                        Some(SearchFieldFilter {
                                            field,
                                            op: SearchFieldOp::In(vals),
                                        })
                                    } else {
                                        None
                                    }
                                })
                                .collect::<Vec<_>>()
                        });

                    let filters = SearchFilters {
                        types,
                        tag,
                        where_filters,
                    };

                    let normalized = search_query::normalize(&q);
                    let result = pool
                        .search(q, limit, offset, filters)
                        .await
                        .map_err(to_graphql_error)?;
                    let mut obj = IndexMap::new();
                    obj.insert(
                        Name::new("hits"),
                        GqlValue::List(result.hits.iter().map(search_hit_to_value).collect()),
                    );
                    obj.insert(
                        Name::new("totalCount"),
                        GqlValue::from(result.total_count as i64),
                    );
                    obj.insert(
                        Name::new("queryNormalized"),
                        GqlValue::from(normalized),
                    );
                    Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
                })
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
            .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)).description("Maximum results to return. Default 20."))
            .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)).description("Number of results to skip for pagination. Default 0."))
            .description("Full-text search with optional structured where filters. Returns paginated hits with BM25 ranking."),
        );
    }

    // normalizeSearchQuery(query)
    {
        query = query.field(
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
        );
    }

    // typeDefs
    {
        query = query.field(
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
        );
    }

    // sql(query, format?)
    {
        query = query.field(
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
        );
    }

    // schemaVersion
    {
        query = query.field(Field::new(
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
        ).description("Current schema version counter. Increments on typedef creation, modification, or deletion."));
    }

    // checkboxItems(state?, doogatId?, limit?, offset?)
    {
        query = query.field(
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
        );
    }

    // openActions(limit?)
    {
        query = query.field(
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
        );
    }

    // -- Discovery queries (orphans, unlinked mentions, sequences, etc.) --
    let super::discovery_queries::DiscoveryOutput {
        query: q,
        sequence_node_type,
        sequence_info_type,
        broken_sequence_type,
    } = super::discovery_queries::register_discovery_fields(query);
    query = q;

    // -- Dynamic per-type queries --
    let mut known_types: HashMap<String, String> = HashMap::new();
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
    let mut dynamic_types: Vec<Object> = Vec::new();
    let mut dynamic_inputs: Vec<InputObject> = Vec::new();
    // PRD 00139 §6 / T16: track per-typedef-loop Query field names so the
    // singleton singular field can detect collisions and fall back to
    // `<base>_singleton`. The fallback also collides? -> hard error at
    // schema-build time so a future operator notices instead of silently
    // dropping the singular field.
    let mut emitted_query_fields: HashSet<String> = RESERVED_QUERY_FIELD_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    // Lookup of every registered type schema, threaded into the where-builder
    // for relation resolution (consumed by later relation tasks). Built once.
    let schema_lookup: crate::filter::SchemaLookup = type_schemas
        .iter()
        .map(|s| (s.table_name.clone(), s.clone()))
        .collect();
    for schema in type_schemas {
        let type_name = match known_types.get(&schema.table_name) {
            Some(name) => name.clone(),
            None => continue,
        };
        let field_base = sanitize_field_name(&schema.table_name);
        let plural = pluralize_preserving_case(&field_base);

        // Create typed object
        let typed_obj = build_typed_object(&type_name, schema, &known_types);
        dynamic_types.push(typed_obj);

        // Create per-type Where, OrderBy inputs, Connection and Aggregate types
        let where_input =
            crate::filter::build_where_input(&type_name, schema, type_schemas, &known_types);
        let order_by_input = crate::filter::build_order_by_input(&type_name, schema);
        let connection_type = crate::filter::build_connection_type(&type_name);
        let aggregate_type = crate::filter::build_aggregate_type(&type_name, schema);
        let aggregate_group_type = crate::filter::build_aggregate_group_type(&type_name, schema);
        dynamic_inputs.push(where_input);
        dynamic_inputs.push(order_by_input);
        dynamic_types.push(connection_type);
        dynamic_types.push(aggregate_type);
        dynamic_types.push(aggregate_group_type);

        // Add per-type query field
        let schema_clone = schema.clone();
        let table_name = schema.table_name.clone();
        let where_type_name = format!("{type_name}Where");
        let order_by_type_name = format!("{type_name}OrderBy");
        let connection_type_name = format!("{type_name}Connection");

        // Per-type query returning Connection (items + totalCount)
        let query_desc = format!(
            "List all {} doogats with optional where, orderBy, tag, and pagination filters.",
            type_name
        );
        let agg_desc = format!(
            "Aggregate {} doogats. Returns count with optional groupBy breakdown.",
            type_name
        );
        {
            let schema_clone = schema_clone.clone();
            let table_name = table_name.clone();
            let schema_lookup = schema_lookup.clone();
            query = query.field(
                Field::new(
                    &plural,
                    TypeRef::named_nn(&connection_type_name),
                    move |ctx| {
                        let schema_clone = schema_clone.clone();
                        let table_name = table_name.clone();
                        let schema_lookup = schema_lookup.clone();
                        FieldFuture::new(async move {
                            let pool = ctx.data::<ReadPool>()?;
                            let tag = ctx
                                .args
                                .get("tag")
                                .and_then(|v| v.string().ok())
                                .map(|s| s.to_string());
                            let limit = validate_limit(&ctx)?;
                            let offset = validate_offset(&ctx)?;
                            let distinct = ctx
                                .args
                                .get("distinct")
                                .and_then(|v| v.string().ok())
                                .and_then(|col| {
                                    resolve_column(&schema_clone.columns, col)
                                        .map(|s| s.to_string())
                                });

                            // Parse optional orderBy
                            let order_sql = ctx
                                .args
                                .get("orderBy")
                                .and_then(|v| v.deserialize::<GqlValue>().ok())
                                .and_then(|v| crate::filter::build_order_sql(&v, &schema_clone));

                            // Build where clause. Validation errors (e.g.
                            // empty `tags` filter, issue #11) bubble out
                            // as GraphQL errors instead of silently
                            // matching no rows.
                            let where_val =
                                ctx.args.get("where").map(|v| v.deserialize::<GqlValue>());
                            let wc = match &where_val {
                                Some(Ok(ref filter_input)) => {
                                    crate::filter::try_build_where_sql(
                                        filter_input,
                                        &schema_clone,
                                        &schema_lookup,
                                    )
                                    .map_err(|msg| async_graphql::ServerError::new(msg, None))?
                                }
                                _ => crate::filter::WhereClause::empty(),
                            };

                            // Fetch items (always use filtered_list)
                            let doogats = pool
                                .filtered_list(ddb_core::types::TypedListQuery {
                                    table_name: table_name.clone(),
                                    where_sql: wc.sql.clone(),
                                    params: wc.params.clone(),
                                    order_sql,
                                    tag: tag.clone(),
                                    limit,
                                    offset,
                                    distinct: distinct.clone(),
                                })
                                .await
                                .map_err(to_graphql_error)?;

                            // Fetch totalCount (same where + tag filters, no limit/offset)
                            let mut count_conditions = Vec::new();
                            if !wc.sql.is_empty() {
                                count_conditions.push(wc.sql.clone());
                            }
                            if let Some(ref t) = tag {
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
                            let count_sql = if let Some(ref col) = distinct {
                                let escaped = col.replace('"', "\"\"");
                                format!(
                                    "SELECT COUNT(DISTINCT \"{escaped}\") FROM \"{table_name}\"{count_where}"
                                )
                            } else {
                                format!("SELECT COUNT(*) FROM \"{table_name}\"{count_where}")
                            };
                            let count_row = pool
                                .aggregate_query(count_sql, wc.params)
                                .await
                                .map_err(to_graphql_error)?;
                            let total_count: i64 =
                                count_row.first().and_then(|s| s.parse().ok()).unwrap_or(0);

                            let items = GqlValue::List(
                                doogats
                                    .iter()
                                    .map(|z| typed_doogat_to_value(z, &schema_clone))
                                    .collect(),
                            );
                            let mut conn = IndexMap::new();
                            conn.insert(Name::new("items"), items);
                            conn.insert(Name::new("totalCount"), GqlValue::from(total_count));

                            Ok(Some(FieldValue::owned_any(GqlValue::Object(conn))))
                        })
                    },
                )
                .argument(InputValue::new("where", TypeRef::named(&where_type_name)).description("Filter conditions."))
                .argument(InputValue::new(
                    "orderBy",
                    TypeRef::named(&order_by_type_name),
                ).description("Sort order specification."))
                .argument(InputValue::new("tag", TypeRef::named(TypeRef::STRING)).description("Filter by tag name."))
                .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)).description("Maximum results to return."))
                .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)).description("Number of results to skip."))
                .argument(InputValue::new("distinct", TypeRef::named(TypeRef::STRING)).description("Column to deduplicate results by."))
                .description(&query_desc),
            );
            // T16: track plural field name for singleton-collision detection.
            emitted_query_fields.insert(plural.clone());
        }

        // Per-type aggregate query
        {
            let agg_type_name = format!("{type_name}Aggregate");
            let schema_clone2 = schema_clone.clone();
            let table_name2 = table_name.clone();
            let schema_lookup2 = schema_lookup.clone();
            query = query.field(
                Field::new(
                    format!("{plural}Aggregate"),
                    TypeRef::named_nn(&agg_type_name),
                    move |ctx| {
                        let schema_clone = schema_clone2.clone();
                        let table_name = table_name2.clone();
                        let schema_lookup = schema_lookup2.clone();
                        FieldFuture::new(async move {
                            let pool = ctx.data::<ReadPool>()?;
                            // Aggregate `where` shares the issue-#11
                            // validation; bubble errors out cleanly.
                            let wc = match ctx
                                .args
                                .get("where")
                                .and_then(|v| v.deserialize::<GqlValue>().ok())
                            {
                                Some(v) => crate::filter::try_build_where_sql(
                                    &v,
                                    &schema_clone,
                                    &schema_lookup,
                                )
                                .map_err(|msg| async_graphql::ServerError::new(msg, None))?,
                                None => crate::filter::WhereClause::empty(),
                            };

                            let group_by = ctx
                                .args
                                .get("groupBy")
                                .and_then(|v| v.string().ok())
                                .and_then(|col| {
                                    resolve_column(&schema_clone.columns, col)
                                        .map(|s| s.to_string())
                                });

                            let (sql, names) = crate::filter::build_aggregate_sql_grouped(
                                &table_name,
                                &schema_clone,
                                &wc,
                                group_by.as_deref(),
                            );

                            if group_by.is_some() {
                                let rows = pool
                                    .aggregate_query_rows(sql, wc.params)
                                    .await
                                    .map_err(to_graphql_error)?;
                                let groups: Vec<GqlValue> = rows
                                    .iter()
                                    .map(|row| crate::filter::aggregate_row_to_value(row, &names))
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
                            } else {
                                let row = pool
                                    .aggregate_query(sql, wc.params)
                                    .await
                                    .map_err(to_graphql_error)?;
                                let val = crate::filter::aggregate_row_to_value(&row, &names);
                                Ok(Some(FieldValue::owned_any(val)))
                            }
                        })
                    },
                )
                .argument(
                    InputValue::new("where", TypeRef::named(&where_type_name))
                        .description("Filter conditions for aggregation."),
                )
                .argument(
                    InputValue::new("groupBy", TypeRef::named(TypeRef::STRING))
                        .description("Column to group results by."),
                )
                .description(&agg_desc),
            );
        }

        // PRD 00139 §6: SINGLETON typedefs gain a per-type singular query
        // field. The field returns the single materialized row or null
        // (when the typedef is not yet auto-seeded). The plural field
        // already added above stays generated for backward compat.
        if schema.singleton {
            let schema_clone3 = schema_clone.clone();
            let table_name3 = table_name.clone();
            let singular_desc = format!(
                "Fetch the singleton {} row, or null when the typedef is empty.",
                type_name
            );
            let singleton_field_base = singleton_field_base(&schema.table_name);
            // PRD 00139 §6 / T16: name-collision fallback. If the bare
            // SINGLETON field base (e.g. `foo_bar`) already shipped as
            // another Query field, fall back to `<field_base>_singleton`.
            // If the fallback ALSO collides, fail schema build so the
            // operator sees the bad schema instead of silently losing the
            // field.
            let singular_field_name = if emitted_query_fields.contains(&singleton_field_base) {
                let fallback = format!("{singleton_field_base}_singleton");
                if emitted_query_fields.contains(&fallback) {
                    return Err(format!(
                        "cannot build SINGLETON singular field for typedef '{}': '{}' collides on Query and fallback '{}' is also already in use",
                        schema.table_name, singleton_field_base, fallback
                    ));
                }
                tracing::warn!(
                    "SINGLETON typedef '{}' singular field '{}' collides with another Query field; emitting as '{}' instead",
                    schema.table_name,
                    singleton_field_base,
                    fallback
                );
                fallback
            } else {
                singleton_field_base
            };
            emitted_query_fields.insert(singular_field_name.clone());
            query = query.field(
                Field::new(
                    &singular_field_name,
                    TypeRef::named(&type_name),
                    move |ctx| {
                        let schema_clone = schema_clone3.clone();
                        let table_name = table_name3.clone();
                        FieldFuture::new(async move {
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
                                    d,
                                    &schema_clone,
                                )))),
                                None => Ok(None),
                            }
                        })
                    },
                )
                .description(&singular_desc),
            );
        }
    }

    // tags
    {
        query = query.field(
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
        );
    }

    // tagEntries
    {
        query = query.field(
            Field::new(
                "tagEntries",
                TypeRef::named_nn("TagEntryConnection"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;

                        let mut filter = ddb_core::types::TagQueryFilter::default();

                        if let Some(Ok(GqlValue::Object(map))) =
                            ctx.args.get("where").map(|v| v.deserialize::<GqlValue>())
                        {
                            if let Some(GqlValue::Object(id_filter)) = map.get("doogatId") {
                                if let Some(GqlValue::String(eq)) = id_filter.get("eq") {
                                    filter.doogat_id_eq = Some(eq.to_string());
                                }
                                if let Some(GqlValue::List(vals)) = id_filter.get("in") {
                                    filter.doogat_id_in = Some(
                                        vals.iter()
                                            .filter_map(|v| {
                                                if let GqlValue::String(s) = v {
                                                    Some(s.to_string())
                                                } else {
                                                    None
                                                }
                                            })
                                            .collect(),
                                    );
                                }
                            }
                            if let Some(GqlValue::Object(tag_filter)) = map.get("tag") {
                                if let Some(GqlValue::String(eq)) = tag_filter.get("eq") {
                                    filter.tag_eq = Some(eq.to_string());
                                }
                                if let Some(GqlValue::String(c)) = tag_filter.get("contains") {
                                    filter.tag_contains = Some(c.to_string());
                                }
                                if let Some(GqlValue::List(vals)) = tag_filter.get("in") {
                                    filter.tag_in = Some(
                                        vals.iter()
                                            .filter_map(|v| {
                                                if let GqlValue::String(s) = v {
                                                    Some(s.to_string())
                                                } else {
                                                    None
                                                }
                                            })
                                            .collect(),
                                    );
                                }
                            }
                        }

                        let entries = pool.query_tags(filter).await.map_err(to_graphql_error)?;
                        let total_count = entries.len() as i64;
                        let items =
                            GqlValue::List(entries.iter().map(tag_entry_to_value).collect());
                        let mut conn = IndexMap::new();
                        conn.insert(Name::new("items"), items);
                        conn.insert(Name::new("totalCount"), GqlValue::from(total_count));
                        Ok(Some(FieldValue::owned_any(GqlValue::Object(conn))))
                    })
                },
            )
            .argument(
                InputValue::new("where", TypeRef::named("TagEntriesWhere"))
                    .description("Filter conditions for tag entries."),
            )
            .description(
                "Query individual tag assignments with where filters on doogatId and tag name.",
            ),
        );
    }

    // Forward-relation `{Target}RelationFilter` inputs for every REFERENCES
    // column whose target type is registered. Keyed off `known_types` so it
    // agrees with `build_where_input` on which targets get a RelationFilter.
    for relation_input in
        crate::relation_filter::relation_input_objects(type_schemas, &known_types)
    {
        dynamic_inputs.push(relation_input);
    }

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
