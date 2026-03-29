mod base_types;
pub use base_types::is_valid_graphql_name;
use base_types::*;

use async_graphql::dynamic::*;
use async_graphql::{Name, Value as GqlValue};
use base64::engine::general_purpose as base64_engine;
use base64::Engine as _;
use futures_util::StreamExt;
use indexmap::IndexMap;
use ddb_core::types::{SearchFieldFilter, SearchFieldOp, SearchFilters, TableSchema};

use std::sync::Arc;

use crate::actor::ActorHandle;
use crate::error::to_server_error;
use crate::events::EventKind;
use crate::read_pool::ReadPool;
use crate::reload::SchemaReloader;

// -- Schema builder --

pub fn build_schema(
    actor: ActorHandle,
    read_pool: ReadPool,
    type_schemas: Vec<TableSchema>,
    reloader: Option<Arc<SchemaReloader>>,
) -> Result<Schema, String> {
    let inline_field_type = Object::new("InlineField")
        .field(Field::new(
            "key",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "key"))
                })
            },
        ))
        .field(Field::new(
            "value",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "value"))
                })
            },
        ))
        .field(Field::new(
            "zone",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "zone"))
                })
            },
        ));

    let link_type = Object::new("Link")
        .field(Field::new(
            "target",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "target"))
                })
            },
        ))
        .field(Field::new(
            "display",
            TypeRef::named(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "display"))
                })
            },
        ))
        .field(Field::new(
            "zone",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "zone"))
                })
            },
        ))
        .field(Field::new(
            "kind",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "kind"))
                })
            },
        ))
        .field(Field::new(
            "section",
            TypeRef::named(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "section"))
                })
            },
        ));

    let search_hit_type = Object::new("SearchHit")
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("title", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("path", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("snippet", TypeRef::named_nn(TypeRef::STRING)))
        .field(Field::new(
            "rank",
            TypeRef::named_nn(TypeRef::FLOAT),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "rank"))
                })
            },
        ));

    let search_connection_type = Object::new("SearchConnection")
        .field(Field::new(
            "hits",
            TypeRef::named_nn_list_nn("SearchHit"),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "hits"))
                })
            },
        ))
        .field(Field::new(
            "totalCount",
            TypeRef::named_nn(TypeRef::INT),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "totalCount"))
                })
            },
        ));

    let column_info_type = Object::new("ColumnInfo")
        .field(simple_field("name", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("dataType", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("zone", TypeRef::named(TypeRef::STRING)))
        .field(Field::new(
            "required",
            TypeRef::named_nn(TypeRef::BOOLEAN),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "required"))
                })
            },
        ))
        .field(simple_field("references", TypeRef::named(TypeRef::STRING)))
        .field(Field::new(
            "allowedValues",
            TypeRef::named_list(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "allowedValues"))
                })
            },
        ))
        .field(simple_field(
            "defaultValue",
            TypeRef::named(TypeRef::STRING),
        ));

    let typedef_type = Object::new("TypeDef")
        .field(simple_field("name", TypeRef::named_nn(TypeRef::STRING)))
        .field(Field::new(
            "columns",
            TypeRef::named_nn_list_nn("ColumnInfo"),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "columns"))
                })
            },
        ))
        .field(simple_field(
            "crdtStrategy",
            TypeRef::named(TypeRef::STRING),
        ))
        .field(Field::new(
            "templateSections",
            TypeRef::named_nn_list_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "templateSections"))
                })
            },
        ));

    let sql_result_type = Object::new("SqlResult")
        .field(Field::new(
            "columns",
            TypeRef::named_nn_list(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "columns"))
                })
            },
        ))
        .field(Field::new(
            "rows",
            TypeRef::named_nn_list(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "rows"))
                })
            },
        ))
        .field(Field::new(
            "affected",
            TypeRef::named(TypeRef::INT),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "affected"))
                })
            },
        ))
        .field(simple_field("message", TypeRef::named(TypeRef::STRING)));

    let attachment_type = Object::new("Attachment")
        .field(simple_field("name", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("mime", TypeRef::named_nn(TypeRef::STRING)))
        .field(Field::new("size", TypeRef::named_nn(TypeRef::INT), |ctx| {
            FieldFuture::new(async move {
                let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                Ok(obj_field(obj, "size"))
            })
        }))
        .field(simple_field("url", TypeRef::named_nn(TypeRef::STRING)));

    let checkbox_item_type = Object::new("CheckboxItem")
        .field(simple_field("doogatId", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("doogatTitle", TypeRef::named(TypeRef::STRING)))
        .field(simple_field("state", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("content", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("date", TypeRef::named(TypeRef::STRING)))
        .field(simple_field("dueDate", TypeRef::named(TypeRef::STRING)))
        .field(Field::new(
            "lineNumber",
            TypeRef::named(TypeRef::INT),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "lineNumber"))
                })
            },
        ))
        .field(Field::new(
            "indentLevel",
            TypeRef::named(TypeRef::INT),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "indentLevel"))
                })
            },
        ));

    let tag_info_type = Object::new("TagInfo")
        .field(simple_field("name", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("count", TypeRef::named_nn(TypeRef::INT)));

    // -- Discovery output types --
    let unlinked_mention_type = Object::new("UnlinkedMention")
        .field(simple_field("sourceId", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field(
            "sourceTitle",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(simple_field("snippet", TypeRef::named_nn(TypeRef::STRING)));

    let suggestion_type = Object::new("Suggestion")
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("title", TypeRef::named_nn(TypeRef::STRING)))
        .field(Field::new(
            "score",
            TypeRef::named_nn(TypeRef::FLOAT),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "score"))
                })
            },
        ))
        .field(Field::new(
            "sharedTags",
            TypeRef::named_nn_list_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "sharedTags"))
                })
            },
        ));

    let stale_doogat_type = Object::new("StaleDoogat")
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("title", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field(
            "doogatType",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(simple_field(
            "lastUpdated",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(simple_field(
            "dateSource",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(Field::new(
            "daysStale",
            TypeRef::named_nn(TypeRef::INT),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "daysStale"))
                })
            },
        ))
        .field(Field::new(
            "thresholdDays",
            TypeRef::named_nn(TypeRef::INT),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "thresholdDays"))
                })
            },
        ));

    let orphan_doogat_type = Object::new("OrphanDoogat")
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("title", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field(
            "doogatType",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(Field::new(
            "outgoingLinks",
            TypeRef::named_nn(TypeRef::INT),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "outgoingLinks"))
                })
            },
        ));

    // Base Doogat type
    let doogat_type = doogat_object("Doogat");

    // Input types
    let create_input = InputObject::new("CreateDoogatInput")
        .field(InputValue::new("title", TypeRef::named_nn(TypeRef::STRING)))
        .field(InputValue::new("content", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new(
            "tags",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new("type", TypeRef::named(TypeRef::STRING)));

    let update_input = InputObject::new("UpdateDoogatInput")
        .field(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)))
        .field(InputValue::new("title", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("content", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new(
            "tags",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new("type", TypeRef::named(TypeRef::STRING)));

    let search_field_filter_input = InputObject::new("SearchFieldFilter")
        .field(InputValue::new(
            "field",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(InputValue::new("eq", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("contains", TypeRef::named(TypeRef::STRING)));

    // -- Query fields --
    let mut query = Object::new("Query");

    // doogat(id)
    {
        query = query.field(
            Field::new("doogat", TypeRef::named("Doogat"), |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let id = ctx.args.try_get("id")?.string()?.to_string();
                    let z = pool.get_doogat(id).await.map_err(to_server_error)?;
                    Ok(Some(FieldValue::owned_any(doogat_to_value(&z))))
                })
            })
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID))),
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
                    let limit = ctx.args.get("limit").and_then(|v| v.i64().ok());
                    let offset = ctx.args.get("offset").and_then(|v| v.i64().ok());
                    let doogats = pool
                        .list_doogats(doogat_type, tag, backlinks_of, vec![], limit, offset)
                        .await
                        .map_err(to_server_error)?;
                    Ok(Some(FieldValue::list(
                        doogats
                            .iter()
                            .map(|z| FieldValue::owned_any(doogat_to_value(z))),
                    )))
                })
            })
            .argument(InputValue::new("type", TypeRef::named(TypeRef::STRING)))
            .argument(InputValue::new("tag", TypeRef::named(TypeRef::STRING)))
            .argument(InputValue::new("backlinksOf", TypeRef::named(TypeRef::ID)))
            .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
            .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT))),
        );
    }

    // search(query, limit?, offset?, types?, tag?, where?)
    {
        query = query.field(
            Field::new("search", TypeRef::named_nn("SearchConnection"), |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let q = ctx.args.try_get("query")?.string()?.to_string();
                    let limit = ctx
                        .args
                        .get("limit")
                        .and_then(|v| v.i64().ok())
                        .unwrap_or(20) as usize;
                    let offset = ctx
                        .args
                        .get("offset")
                        .and_then(|v| v.i64().ok())
                        .unwrap_or(0) as usize;

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

                    let result = pool
                        .search(q, limit, offset, filters)
                        .await
                        .map_err(to_server_error)?;
                    let mut obj = IndexMap::new();
                    obj.insert(
                        Name::new("hits"),
                        GqlValue::List(result.hits.iter().map(search_hit_to_value).collect()),
                    );
                    obj.insert(
                        Name::new("totalCount"),
                        GqlValue::from(result.total_count as i64),
                    );
                    Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
                })
            })
            .argument(InputValue::new("query", TypeRef::named_nn(TypeRef::STRING)))
            .argument(InputValue::new(
                "types",
                TypeRef::named_list_nn(TypeRef::STRING),
            ))
            .argument(InputValue::new("tag", TypeRef::named(TypeRef::STRING)))
            .argument(InputValue::new(
                "where",
                TypeRef::named_list_nn("SearchFieldFilter"),
            ))
            .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
            .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT))),
        );
    }

    // typeDefs
    {
        query = query.field(Field::new(
            "typeDefs",
            TypeRef::named_nn_list_nn("TypeDef"),
            |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let schemas = pool.get_type_schemas().await.map_err(to_server_error)?;
                    Ok(Some(FieldValue::list(
                        schemas
                            .iter()
                            .map(|s| FieldValue::owned_any(typedef_to_value(s))),
                    )))
                })
            },
        ));
    }

    // sql(query) — SELECT via ReadPool, non-SELECT via actor
    {
        query = query.field(
            Field::new("sql", TypeRef::named_nn("SqlResult"), |ctx| {
                FieldFuture::new(async move {
                    let q = ctx.args.try_get("query")?.string()?.to_string();
                    let result = if crate::pgwire::is_select_only(&q) {
                        let pool = ctx.data::<ReadPool>()?;
                        pool.execute_select(q).await.map_err(to_server_error)?
                    } else {
                        let a = ctx.data::<ActorHandle>()?;
                        a.execute_sql(q).await.map_err(to_server_error)?
                    };
                    Ok(Some(FieldValue::owned_any(sql_result_to_value(&result))))
                })
            })
            .argument(InputValue::new("query", TypeRef::named_nn(TypeRef::STRING))),
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
        ));
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
                        let limit = ctx.args.get("limit").and_then(|v| v.i64().ok());
                        let offset = ctx.args.get("offset").and_then(|v| v.i64().ok());
                        let rows = pool
                            .query_checkboxes(state, doogat_id, limit, offset)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(rows.iter().map(|row| {
                            FieldValue::owned_any(checkbox_row_to_value(row))
                        }))))
                    })
                },
            )
            .argument(InputValue::new("state", TypeRef::named(TypeRef::STRING)))
            .argument(InputValue::new("doogatId", TypeRef::named(TypeRef::ID)))
            .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
            .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT))),
        );
    }

    // openActions(limit?) — convenience alias for checkboxItems(state: "open")
    {
        query = query.field(
            Field::new(
                "openActions",
                TypeRef::named_nn_list_nn("CheckboxItem"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let limit = ctx.args.get("limit").and_then(|v| v.i64().ok());
                        let rows = pool
                            .query_checkboxes(Some("open".to_string()), None, limit, None)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(rows.iter().map(|row| {
                            FieldValue::owned_any(checkbox_row_to_value(row))
                        }))))
                    })
                },
            )
            .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT))),
        );
    }

    // unlinkedMentions(id: ID!): [UnlinkedMention!]!
    {
        query = query.field(
            Field::new(
                "unlinkedMentions",
                TypeRef::named_nn_list_nn("UnlinkedMention"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let id = ctx.args.try_get("id")?.string()?.to_string();
                        let mentions = pool.unlinked_mentions(id).await.map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(mentions.iter().map(|m| {
                            let mut obj = IndexMap::new();
                            obj.insert(Name::new("sourceId"), GqlValue::from(m.source_id.as_str()));
                            obj.insert(
                                Name::new("sourceTitle"),
                                GqlValue::from(m.source_title.as_str()),
                            );
                            obj.insert(Name::new("snippet"), GqlValue::from(m.snippet.as_str()));
                            FieldValue::owned_any(GqlValue::Object(obj))
                        }))))
                    })
                },
            )
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID))),
        );
    }

    // suggestions(id: ID!, limit: Int): [Suggestion!]!
    {
        query = query.field(
            Field::new(
                "suggestions",
                TypeRef::named_nn_list_nn("Suggestion"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let id = ctx.args.try_get("id")?.string()?.to_string();
                        let limit = ctx
                            .args
                            .get("limit")
                            .and_then(|v| v.i64().ok())
                            .unwrap_or(10) as usize;
                        let suggestions = pool
                            .suggest_links(id, limit)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(suggestions.iter().map(|s| {
                            let tags: Vec<GqlValue> = s
                                .shared_tags
                                .iter()
                                .map(|t| GqlValue::from(t.as_str()))
                                .collect();
                            let mut obj = IndexMap::new();
                            obj.insert(Name::new("id"), GqlValue::from(s.id.as_str()));
                            obj.insert(Name::new("title"), GqlValue::from(s.title.as_str()));
                            obj.insert(Name::new("score"), GqlValue::from(s.score));
                            obj.insert(Name::new("sharedTags"), GqlValue::List(tags));
                            FieldValue::owned_any(GqlValue::Object(obj))
                        }))))
                    })
                },
            )
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)))
            .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT))),
        );
    }

    // staleDoogats(type: String): [StaleDoogat!]!
    {
        query = query.field(
            Field::new(
                "staleDoogats",
                TypeRef::named_nn_list_nn("StaleDoogat"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let type_filter = ctx
                            .args
                            .get("type")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let stale = pool
                            .stale_doogats(type_filter)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(stale.iter().map(|s| {
                            let mut obj = IndexMap::new();
                            obj.insert(Name::new("id"), GqlValue::from(s.id.as_str()));
                            obj.insert(Name::new("title"), GqlValue::from(s.title.as_str()));
                            obj.insert(
                                Name::new("doogatType"),
                                GqlValue::from(s.doogat_type.as_str()),
                            );
                            obj.insert(
                                Name::new("lastUpdated"),
                                GqlValue::from(s.last_updated.as_str()),
                            );
                            obj.insert(
                                Name::new("dateSource"),
                                GqlValue::from(s.date_source.to_string()),
                            );
                            obj.insert(Name::new("daysStale"), GqlValue::from(s.days_stale as i64));
                            obj.insert(
                                Name::new("thresholdDays"),
                                GqlValue::from(s.threshold_days as i64),
                            );
                            FieldValue::owned_any(GqlValue::Object(obj))
                        }))))
                    })
                },
            )
            .argument(InputValue::new("type", TypeRef::named(TypeRef::STRING))),
        );
    }

    // orphanDoogats(type: String): [OrphanDoogat!]!
    {
        query = query.field(
            Field::new(
                "orphanDoogats",
                TypeRef::named_nn_list_nn("OrphanDoogat"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let type_filter = ctx
                            .args
                            .get("type")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let orphans = pool
                            .orphan_doogats(type_filter)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(orphans.iter().map(|o| {
                            let mut obj = IndexMap::new();
                            obj.insert(Name::new("id"), GqlValue::from(o.id.as_str()));
                            obj.insert(Name::new("title"), GqlValue::from(o.title.as_str()));
                            obj.insert(
                                Name::new("doogatType"),
                                GqlValue::from(o.doogat_type.as_str()),
                            );
                            obj.insert(
                                Name::new("outgoingLinks"),
                                GqlValue::from(o.outgoing_links as i64),
                            );
                            FieldValue::owned_any(GqlValue::Object(obj))
                        }))))
                    })
                },
            )
            .argument(InputValue::new("type", TypeRef::named(TypeRef::STRING))),
        );
    }

    // -- Sequence output types --
    let sequence_node_type = Object::new("SequenceNode")
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("title", TypeRef::named_nn(TypeRef::STRING)));

    let sequence_info_type = Object::new("SequenceInfo")
        .field(Field::new(
            "parent",
            TypeRef::named("SequenceNode"),
            |ctx| {
                FieldFuture::new(async move {
                    let info = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    if let GqlValue::Object(obj) = info {
                        if let Some(p) = obj.get("parent") {
                            if matches!(p, GqlValue::Null) {
                                return Ok(None);
                            }
                            return Ok(Some(FieldValue::owned_any(p.clone())));
                        }
                    }
                    Ok(None)
                })
            },
        ))
        .field(Field::new(
            "children",
            TypeRef::named_nn_list_nn("SequenceNode"),
            |ctx| {
                FieldFuture::new(async move {
                    let info = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    if let GqlValue::Object(obj) = info {
                        if let Some(GqlValue::List(children)) = obj.get("children") {
                            return Ok(Some(FieldValue::list(
                                children.iter().map(|c| FieldValue::owned_any(c.clone())),
                            )));
                        }
                    }
                    Ok(Some(FieldValue::list(std::iter::empty::<FieldValue>())))
                })
            },
        ))
        .field(Field::new(
            "breadcrumb",
            TypeRef::named_nn_list_nn("SequenceNode"),
            |ctx| {
                FieldFuture::new(async move {
                    let info = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    if let GqlValue::Object(obj) = info {
                        if let Some(GqlValue::List(bc)) = obj.get("breadcrumb") {
                            return Ok(Some(FieldValue::list(
                                bc.iter().map(|c| FieldValue::owned_any(c.clone())),
                            )));
                        }
                    }
                    Ok(Some(FieldValue::list(std::iter::empty::<FieldValue>())))
                })
            },
        ));

    let broken_sequence_type = Object::new("BrokenSequence")
        .field(simple_field("doogatId", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field(
            "brokenParentId",
            TypeRef::named_nn(TypeRef::ID),
        ));

    fn seq_node_to_gql(n: &ddb_core::types::SequenceNode) -> GqlValue {
        let mut obj = IndexMap::new();
        obj.insert(Name::new("id"), GqlValue::from(n.id.as_str()));
        obj.insert(Name::new("title"), GqlValue::from(n.title.as_str()));
        GqlValue::Object(obj)
    }

    // sequenceInfo(id: ID!): SequenceInfo!
    {
        query = query.field(
            Field::new("sequenceInfo", TypeRef::named_nn("SequenceInfo"), |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let id = ctx.args.try_get("id")?.string()?.to_string();
                    let info = pool.sequence_info(id).await.map_err(to_server_error)?;

                    let parent_val = match &info.parent {
                        Some(p) => seq_node_to_gql(p),
                        None => GqlValue::Null,
                    };
                    let children_val =
                        GqlValue::List(info.children.iter().map(seq_node_to_gql).collect());
                    let bc_val =
                        GqlValue::List(info.breadcrumb.iter().map(seq_node_to_gql).collect());

                    let mut obj = IndexMap::new();
                    obj.insert(Name::new("parent"), parent_val);
                    obj.insert(Name::new("children"), children_val);
                    obj.insert(Name::new("breadcrumb"), bc_val);
                    Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
                })
            })
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID))),
        );
    }

    // sequenceChildren(id: ID!): [SequenceNode!]!
    {
        query = query.field(
            Field::new(
                "sequenceChildren",
                TypeRef::named_nn_list_nn("SequenceNode"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let id = ctx.args.try_get("id")?.string()?.to_string();
                        let children = pool.sequence_children(id).await.map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(
                            children
                                .iter()
                                .map(|n| FieldValue::owned_any(seq_node_to_gql(n))),
                        )))
                    })
                },
            )
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID))),
        );
    }

    // sequenceBreadcrumb(id: ID!): [SequenceNode!]!
    {
        query = query.field(
            Field::new(
                "sequenceBreadcrumb",
                TypeRef::named_nn_list_nn("SequenceNode"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let id = ctx.args.try_get("id")?.string()?.to_string();
                        let bc = pool
                            .sequence_breadcrumb(id)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(
                            bc.iter().map(|n| FieldValue::owned_any(seq_node_to_gql(n))),
                        )))
                    })
                },
            )
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID))),
        );
    }

    // brokenSequences: [BrokenSequence!]!
    {
        query = query.field(Field::new(
            "brokenSequences",
            TypeRef::named_nn_list_nn("BrokenSequence"),
            |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let broken = pool.broken_sequences().await.map_err(to_server_error)?;
                    Ok(Some(FieldValue::list(broken.iter().map(|b| {
                        let mut obj = IndexMap::new();
                        obj.insert(Name::new("doogatId"), GqlValue::from(b.doogat_id.as_str()));
                        obj.insert(
                            Name::new("brokenParentId"),
                            GqlValue::from(b.broken_parent_id.as_str()),
                        );
                        FieldValue::owned_any(GqlValue::Object(obj))
                    }))))
                })
            },
        ));
    }

    // -- Dynamic per-type queries --
    let mut dynamic_types: Vec<Object> = Vec::new();
    let mut dynamic_inputs: Vec<InputObject> = Vec::new();
    for schema in &type_schemas {
        if !is_valid_graphql_name(&schema.table_name) {
            tracing::warn!(
                "skipping typedef '{}': not a valid GraphQL identifier",
                schema.table_name
            );
            continue;
        }
        let type_name = capitalize(&schema.table_name);
        let plural = pluralize(&schema.table_name);

        // Create typed object
        let typed_obj = build_typed_object(&type_name, schema);
        dynamic_types.push(typed_obj);

        // Create per-type Where, OrderBy inputs, Connection and Aggregate types
        let where_input = crate::filter::build_where_input(&type_name, schema);
        let order_by_input = crate::filter::build_order_by_input(&type_name, schema);
        let connection_type = crate::filter::build_connection_type(&type_name);
        let aggregate_type = crate::filter::build_aggregate_type(&type_name, schema);
        dynamic_inputs.push(where_input);
        dynamic_inputs.push(order_by_input);
        dynamic_types.push(connection_type);
        dynamic_types.push(aggregate_type);

        // Add per-type query field
        let schema_clone = schema.clone();
        let table_name = schema.table_name.clone();
        let where_type_name = format!("{type_name}Where");
        let order_by_type_name = format!("{type_name}OrderBy");
        let connection_type_name = format!("{type_name}Connection");

        // Per-type query returning Connection (items + totalCount)
        {
            let schema_clone = schema_clone.clone();
            let type_name_clone = type_name.clone();
            let table_name = table_name.clone();
            query = query.field(
                Field::new(
                    &plural,
                    TypeRef::named_nn(&connection_type_name),
                    move |ctx| {
                        let schema_clone = schema_clone.clone();
                        let _type_name = type_name_clone.clone();
                        let table_name = table_name.clone();
                        FieldFuture::new(async move {
                            let pool = ctx.data::<ReadPool>()?;
                            let tag = ctx
                                .args
                                .get("tag")
                                .and_then(|v| v.string().ok())
                                .map(|s| s.to_string());
                            let limit = ctx.args.get("limit").and_then(|v| v.i64().ok());
                            let offset = ctx.args.get("offset").and_then(|v| v.i64().ok());

                            // Parse optional orderBy
                            let order_sql = ctx
                                .args
                                .get("orderBy")
                                .and_then(|v| v.deserialize::<GqlValue>().ok())
                                .and_then(|v| crate::filter::build_order_sql(&v, &schema_clone));

                            // Build where clause
                            let where_val =
                                ctx.args.get("where").map(|v| v.deserialize::<GqlValue>());
                            let wc = match &where_val {
                                Some(Ok(ref filter_input)) => {
                                    crate::filter::build_where_sql(filter_input, &schema_clone)
                                }
                                _ => crate::filter::WhereClause::empty(),
                            };

                            // Fetch items (always use filtered_list — supports where + tag + orderBy)
                            let doogats = pool
                                .filtered_list(ddb_core::types::TypedListQuery {
                                    table_name: table_name.clone(),
                                    where_sql: wc.sql.clone(),
                                    params: wc.params.clone(),
                                    order_sql,
                                    tag: tag.clone(),
                                    limit,
                                    offset,
                                })
                                .await
                                .map_err(to_server_error)?;

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
                            let count_sql =
                                format!("SELECT COUNT(*) FROM \"{table_name}\"{count_where}");
                            let count_row = pool
                                .aggregate_query(count_sql, wc.params)
                                .await
                                .map_err(to_server_error)?;
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
                .argument(InputValue::new("where", TypeRef::named(&where_type_name)))
                .argument(InputValue::new(
                    "orderBy",
                    TypeRef::named(&order_by_type_name),
                ))
                .argument(InputValue::new("tag", TypeRef::named(TypeRef::STRING)))
                .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
                .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT))),
            );
        }

        // Per-type aggregate query
        {
            let agg_type_name = format!("{type_name}Aggregate");
            let schema_clone2 = schema_clone.clone();
            let table_name2 = table_name.clone();
            query = query.field(
                Field::new(
                    format!("{plural}Aggregate"),
                    TypeRef::named_nn(&agg_type_name),
                    move |ctx| {
                        let schema_clone = schema_clone2.clone();
                        let table_name = table_name2.clone();
                        FieldFuture::new(async move {
                            let pool = ctx.data::<ReadPool>()?;
                            let wc = ctx
                                .args
                                .get("where")
                                .and_then(|v| v.deserialize::<GqlValue>().ok())
                                .map(|v| crate::filter::build_where_sql(&v, &schema_clone))
                                .unwrap_or_else(crate::filter::WhereClause::empty);

                            let (sql, names) =
                                crate::filter::build_aggregate_sql(&table_name, &schema_clone, &wc);
                            let row = pool
                                .aggregate_query(sql, wc.params)
                                .await
                                .map_err(to_server_error)?;
                            let val = crate::filter::aggregate_row_to_value(&row, &names);
                            Ok(Some(FieldValue::owned_any(val)))
                        })
                    },
                )
                .argument(InputValue::new("where", TypeRef::named(&where_type_name))),
            );
        }
    }

    // tags
    {
        query = query.field(Field::new(
            "tags",
            TypeRef::named_nn_list_nn("TagInfo"),
            |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let tags = pool.list_tags().await.map_err(to_server_error)?;
                    Ok(Some(FieldValue::list(
                        tags.iter()
                            .map(|(name, count)| FieldValue::owned_any(tag_info_to_value(name, *count))),
                    )))
                })
            },
        ));
    }

    // -- Mutation fields --
    let mut mutation = Object::new("Mutation");

    // createDoogat
    {
        mutation = mutation.field(
            Field::new("createDoogat", TypeRef::named_nn("Doogat"), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let input = ctx.args.try_get("input")?;
                    let input = input.object()?;
                    let title = input.try_get("title")?.string()?.to_string();
                    let content = input
                        .get("content")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
                    let tags = input
                        .get("tags")
                        .and_then(|v| v.list().ok())
                        .map(|l| {
                            l.iter()
                                .filter_map(|v| v.string().ok().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let doogat_type = input
                        .get("type")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
                    let z = a
                        .create_doogat(title, content, tags, doogat_type)
                        .await
                        .map_err(to_server_error)?;
                    Ok(Some(FieldValue::owned_any(doogat_to_value(&z))))
                })
            })
            .argument(InputValue::new(
                "input",
                TypeRef::named_nn("CreateDoogatInput"),
            )),
        );
    }

    // updateDoogat
    {
        mutation = mutation.field(
            Field::new("updateDoogat", TypeRef::named_nn("Doogat"), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let input = ctx.args.try_get("input")?;
                    let input = input.object()?;
                    let id = input.try_get("id")?.string()?.to_string();
                    let title = input
                        .get("title")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
                    let content = input
                        .get("content")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
                    let tags = input.get("tags").and_then(|v| v.list().ok()).map(|l| {
                        l.iter()
                            .filter_map(|v| v.string().ok().map(|s| s.to_string()))
                            .collect()
                    });
                    let doogat_type = input
                        .get("type")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
                    let z = a
                        .update_doogat(id, title, content, tags, doogat_type)
                        .await
                        .map_err(to_server_error)?;
                    Ok(Some(FieldValue::owned_any(doogat_to_value(&z))))
                })
            })
            .argument(InputValue::new(
                "input",
                TypeRef::named_nn("UpdateDoogatInput"),
            )),
        );
    }

    // deleteDoogat
    {
        mutation = mutation.field(
            Field::new("deleteDoogat", TypeRef::named_nn(TypeRef::BOOLEAN), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let id = ctx.args.try_get("id")?.string()?.to_string();
                    a.delete_doogat(id).await.map_err(to_server_error)?;
                    Ok(Some(FieldValue::value(GqlValue::from(true))))
                })
            })
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID))),
        );
    }

    // attachFile(doogatId, filename, dataBase64, mime?)
    {
        let attach_input = InputObject::new("AttachFileInput")
            .field(InputValue::new("doogatId", TypeRef::named_nn(TypeRef::ID)))
            .field(InputValue::new(
                "filename",
                TypeRef::named_nn(TypeRef::STRING),
            ))
            .field(InputValue::new(
                "dataBase64",
                TypeRef::named_nn(TypeRef::STRING),
            ))
            .field(InputValue::new("mime", TypeRef::named(TypeRef::STRING)));

        mutation = mutation.field(
            Field::new("attachFile", TypeRef::named_nn("Attachment"), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let input = ctx.args.try_get("input")?;
                    let input = input.object()?;
                    let doogat_id = input.try_get("doogatId")?.string()?.to_string();
                    let filename = input.try_get("filename")?.string()?.to_string();
                    let data_b64 = input.try_get("dataBase64")?.string()?.to_string();
                    let bytes = base64_engine::STANDARD.decode(&data_b64).map_err(|e| {
                        async_graphql::ServerError::new(format!("invalid base64: {e}"), None)
                    })?;
                    let mime = input
                        .get("mime")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| {
                            ddb_core::types::AttachmentInfo::mime_from_filename(&filename)
                                .to_string()
                        });
                    let info = a
                        .attach_file(doogat_id, filename, bytes, mime)
                        .await
                        .map_err(to_server_error)?;
                    let zid = &info.path.split('/').nth(1).unwrap_or("");
                    let url = format!("/attachments/{}/{}", zid, info.name);
                    let mut obj = IndexMap::new();
                    obj.insert(Name::new("name"), GqlValue::from(info.name.as_str()));
                    obj.insert(Name::new("mime"), GqlValue::from(info.mime.as_str()));
                    obj.insert(Name::new("size"), GqlValue::from(info.size as i64));
                    obj.insert(Name::new("url"), GqlValue::from(url.as_str()));
                    Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
                })
            })
            .argument(InputValue::new(
                "input",
                TypeRef::named_nn("AttachFileInput"),
            )),
        );

        // Register attach input type
        // (will be registered with builder below)
        // Store for later registration
        dynamic_inputs.push(attach_input);
    }

    // detachFile(doogatId, filename)
    {
        mutation = mutation.field(
            Field::new("detachFile", TypeRef::named_nn(TypeRef::BOOLEAN), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let doogat_id = ctx.args.try_get("doogatId")?.string()?.to_string();
                    let filename = ctx.args.try_get("filename")?.string()?.to_string();
                    a.detach_file(doogat_id, filename)
                        .await
                        .map_err(to_server_error)?;
                    Ok(Some(FieldValue::value(GqlValue::from(true))))
                })
            })
            .argument(InputValue::new("doogatId", TypeRef::named_nn(TypeRef::ID)))
            .argument(InputValue::new(
                "filename",
                TypeRef::named_nn(TypeRef::STRING),
            )),
        );
    }

    // executeSql
    {
        mutation = mutation.field(
            Field::new("executeSql", TypeRef::named_nn("SqlResult"), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let sql = ctx.args.try_get("sql")?.string()?.to_string();
                    let result = a.execute_sql(sql.clone()).await.map_err(to_server_error)?;

                    // Await schema reload if this was a typedef-mutating statement
                    let upper = sql.to_uppercase();
                    if upper.contains("CREATE TABLE")
                        || upper.contains("DROP TABLE")
                        || upper.contains("ALTER TABLE")
                    {
                        if let Ok(reloader) = ctx.data::<Arc<SchemaReloader>>() {
                            reloader.trigger_reload_and_wait().await;
                        }
                    }

                    Ok(Some(FieldValue::owned_any(sql_result_to_value(&result))))
                })
            })
            .argument(InputValue::new("sql", TypeRef::named_nn(TypeRef::STRING))),
        );
    }

    // -- SyncResult output type --
    let sync_result_type = Object::new("SyncResult")
        .field(simple_field(
            "direction",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(simple_field(
            "commitsTransferred",
            TypeRef::named_nn(TypeRef::INT),
        ))
        .field(simple_field(
            "conflictsResolved",
            TypeRef::named_nn(TypeRef::INT),
        ))
        .field(simple_field("resurrected", TypeRef::named_nn(TypeRef::INT)))
        .field(simple_field(
            "collisionsReassigned",
            TypeRef::named_nn(TypeRef::INT),
        ));

    // -- CompactResult output type --
    let compact_result_type = Object::new("CompactResult")
        .field(simple_field(
            "filesRemoved",
            TypeRef::named_nn(TypeRef::INT),
        ))
        .field(simple_field(
            "crdtDocsCompacted",
            TypeRef::named_nn(TypeRef::INT),
        ))
        .field(simple_field(
            "gcSuccess",
            TypeRef::named_nn(TypeRef::BOOLEAN),
        ))
        .field(simple_field(
            "crdtTempBytesBefore",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(simple_field(
            "crdtTempBytesAfter",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(simple_field(
            "crdtTempFilesBefore",
            TypeRef::named_nn(TypeRef::INT),
        ))
        .field(simple_field(
            "crdtTempFilesAfter",
            TypeRef::named_nn(TypeRef::INT),
        ))
        .field(simple_field(
            "repoBytesBefore",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(simple_field(
            "repoBytesAfter",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(simple_field("backupPath", TypeRef::named(TypeRef::STRING)));

    // sync mutation
    {
        mutation = mutation.field(
            Field::new("sync", TypeRef::named_nn("SyncResult"), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let remote = ctx
                        .args
                        .get("remote")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "origin".to_string());
                    let branch = ctx
                        .args
                        .get("branch")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "master".to_string());
                    let report = a.sync(remote, branch).await.map_err(to_server_error)?;
                    let mut obj = IndexMap::new();
                    obj.insert(
                        Name::new("direction"),
                        GqlValue::from(report.direction.as_str()),
                    );
                    obj.insert(
                        Name::new("commitsTransferred"),
                        GqlValue::from(report.commits_transferred as i64),
                    );
                    obj.insert(
                        Name::new("conflictsResolved"),
                        GqlValue::from(report.conflicts_resolved as i64),
                    );
                    obj.insert(
                        Name::new("resurrected"),
                        GqlValue::from(report.resurrected as i64),
                    );
                    obj.insert(
                        Name::new("collisionsReassigned"),
                        GqlValue::from(report.collisions_reassigned as i64),
                    );
                    Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
                })
            })
            .argument(InputValue::new("remote", TypeRef::named(TypeRef::STRING)))
            .argument(InputValue::new("branch", TypeRef::named(TypeRef::STRING))),
        );
    }

    // compact mutation
    {
        mutation = mutation.field(
            Field::new("compact", TypeRef::named_nn("CompactResult"), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let force = ctx
                        .args
                        .get("force")
                        .and_then(|v| v.boolean().ok())
                        .unwrap_or(false);
                    let no_backup = ctx
                        .args
                        .get("noBackup")
                        .and_then(|v| v.boolean().ok())
                        .unwrap_or(false);
                    let backup_path = ctx
                        .args
                        .get("backupPath")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
                    let report = a
                        .compact(force, no_backup, backup_path)
                        .await
                        .map_err(to_server_error)?;
                    let mut obj = IndexMap::new();
                    obj.insert(
                        Name::new("filesRemoved"),
                        GqlValue::from(report.files_removed as i64),
                    );
                    obj.insert(
                        Name::new("crdtDocsCompacted"),
                        GqlValue::from(report.crdt_docs_compacted as i64),
                    );
                    obj.insert(Name::new("gcSuccess"), GqlValue::from(report.gc_success));
                    obj.insert(
                        Name::new("crdtTempBytesBefore"),
                        GqlValue::from(report.crdt_temp_bytes_before.to_string()),
                    );
                    obj.insert(
                        Name::new("crdtTempBytesAfter"),
                        GqlValue::from(report.crdt_temp_bytes_after.to_string()),
                    );
                    obj.insert(
                        Name::new("crdtTempFilesBefore"),
                        GqlValue::from(report.crdt_temp_files_before as i64),
                    );
                    obj.insert(
                        Name::new("crdtTempFilesAfter"),
                        GqlValue::from(report.crdt_temp_files_after as i64),
                    );
                    obj.insert(
                        Name::new("repoBytesBefore"),
                        GqlValue::from(report.repo_bytes_before.to_string()),
                    );
                    obj.insert(
                        Name::new("repoBytesAfter"),
                        GqlValue::from(report.repo_bytes_after.to_string()),
                    );
                    if let Some(bp) = report.backup_path {
                        obj.insert(
                            Name::new("backupPath"),
                            GqlValue::from(bp.display().to_string()),
                        );
                    }
                    Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
                })
            })
            .argument(InputValue::new("force", TypeRef::named(TypeRef::BOOLEAN)))
            .argument(InputValue::new(
                "noBackup",
                TypeRef::named(TypeRef::BOOLEAN),
            ))
            .argument(InputValue::new(
                "backupPath",
                TypeRef::named(TypeRef::STRING),
            )),
        );
    }

    // -- GitMaintenanceResult output type --
    let git_maintenance_result_type = Object::new("GitMaintenanceResult")
        .field(simple_field("success", TypeRef::named_nn(TypeRef::BOOLEAN)))
        .field(simple_field("durationMs", TypeRef::named_nn(TypeRef::INT)))
        .field(simple_field(
            "fallbackUsed",
            TypeRef::named_nn(TypeRef::BOOLEAN),
        ))
        .field(Field::new(
            "tasksRun",
            TypeRef::named_nn_list_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "tasksRun"))
                })
            },
        ));

    // maintenance mutation
    {
        mutation = mutation.field(
            Field::new(
                "maintenance",
                TypeRef::named_nn("GitMaintenanceResult"),
                |ctx| {
                    FieldFuture::new(async move {
                        let a = ctx.data::<ActorHandle>()?;
                        let task = ctx
                            .args
                            .get("task")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let report = a.run_maintenance(task).await.map_err(to_server_error)?;
                        let tasks_run: Vec<GqlValue> = report
                            .tasks_run
                            .iter()
                            .map(|t| GqlValue::from(t.as_str()))
                            .collect();
                        let mut obj = IndexMap::new();
                        obj.insert(Name::new("success"), GqlValue::from(report.success));
                        obj.insert(
                            Name::new("durationMs"),
                            GqlValue::from(report.duration_ms as i64),
                        );
                        obj.insert(
                            Name::new("fallbackUsed"),
                            GqlValue::from(report.fallback_used),
                        );
                        obj.insert(Name::new("tasksRun"), GqlValue::List(tasks_run));
                        Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
                    })
                },
            )
            .argument(InputValue::new("task", TypeRef::named(TypeRef::STRING))),
        );
    }

    // -- DoogatChangeEvent type --
    let change_event_type = Object::new("DoogatChangeEvent")
        .field(simple_field("action", TypeRef::named_nn(TypeRef::STRING)))
        .field(Field::new("doogat", TypeRef::named("Doogat"), |ctx| {
            FieldFuture::new(async move {
                let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                Ok(obj_field(obj, "doogat"))
            })
        }))
        .field(simple_field("doogatId", TypeRef::named_nn(TypeRef::ID)));

    // -- Subscription fields --
    let mut subscription = Subscription::new("Subscription");

    // doogatChanged: DoogatChangeEvent! — all events
    subscription = subscription.field(SubscriptionField::new(
        "doogatChanged",
        TypeRef::named_nn("DoogatChangeEvent"),
        |ctx| {
            let handle = ctx.data::<ActorHandle>().cloned();
            SubscriptionFieldFuture::new(async move {
                let handle = handle?;
                let event_bus = handle.event_bus().clone();
                let actor = handle;
                let rx = event_bus.subscribe();
                let stream = event_stream(rx).then(move |result| {
                    let actor = actor.clone();
                    async move {
                        let event = result?;
                        let action = match event.kind {
                            EventKind::Created => "created",
                            EventKind::Updated => "updated",
                            EventKind::Deleted => "deleted",
                        };
                        let doogat = if event.kind != EventKind::Deleted {
                            actor
                                .get_doogat(event.doogat_id.clone())
                                .await
                                .ok()
                                .map(|z| doogat_to_value(&z))
                        } else {
                            None
                        };
                        let mut map = IndexMap::new();
                        map.insert(Name::new("action"), GqlValue::from(action));
                        map.insert(
                            Name::new("doogatId"),
                            GqlValue::from(event.doogat_id.as_str()),
                        );
                        if let Some(z) = doogat {
                            map.insert(Name::new("doogat"), z);
                        }
                        Ok(FieldValue::owned_any(GqlValue::Object(map)))
                    }
                });
                Ok(stream)
            })
        },
    ));

    // doogatCreated: Doogat! — only Created events
    subscription = subscription.field(SubscriptionField::new(
        "doogatCreated",
        TypeRef::named_nn("Doogat"),
        |ctx| {
            let handle = ctx.data::<ActorHandle>().cloned();
            SubscriptionFieldFuture::new(async move {
                let handle = handle?;
                let event_bus = handle.event_bus().clone();
                let actor = handle;
                let rx = event_bus.subscribe();
                let stream = event_stream(rx).filter_map(move |result| {
                    let actor = actor.clone();
                    async move {
                        let event = result.ok()?;
                        if event.kind != EventKind::Created {
                            return None;
                        }
                        let z = actor.get_doogat(event.doogat_id).await.ok()?;
                        Some(Ok(FieldValue::owned_any(doogat_to_value(&z))))
                    }
                });
                Ok(stream)
            })
        },
    ));

    // doogatUpdated: Doogat! — only Updated events
    subscription = subscription.field(SubscriptionField::new(
        "doogatUpdated",
        TypeRef::named_nn("Doogat"),
        |ctx| {
            let handle = ctx.data::<ActorHandle>().cloned();
            SubscriptionFieldFuture::new(async move {
                let handle = handle?;
                let event_bus = handle.event_bus().clone();
                let actor = handle;
                let rx = event_bus.subscribe();
                let stream = event_stream(rx).filter_map(move |result| {
                    let actor = actor.clone();
                    async move {
                        let event = result.ok()?;
                        if event.kind != EventKind::Updated {
                            return None;
                        }
                        let z = actor.get_doogat(event.doogat_id).await.ok()?;
                        Some(Ok(FieldValue::owned_any(doogat_to_value(&z))))
                    }
                });
                Ok(stream)
            })
        },
    ));

    // doogatDeleted: ID! — only Deleted events
    subscription = subscription.field(SubscriptionField::new(
        "doogatDeleted",
        TypeRef::named_nn(TypeRef::ID),
        |ctx| {
            let handle = ctx.data::<ActorHandle>().cloned();
            SubscriptionFieldFuture::new(async move {
                let handle = handle?;
                let event_bus = handle.event_bus().clone();
                let rx = event_bus.subscribe();
                let stream = event_stream(rx).filter_map(|result| async move {
                    let event = result.ok()?;
                    if event.kind != EventKind::Deleted {
                        return None;
                    }
                    Some(Ok(FieldValue::value(GqlValue::from(
                        event.doogat_id.as_str(),
                    ))))
                });
                Ok(stream)
            })
        },
    ));

    // Per-type subscription fields (e.g., contactChanged, bookmarkChanged)
    for schema in &type_schemas {
        if !is_valid_graphql_name(&schema.table_name) {
            // Already warned in the query/mutation loop above.
            continue;
        }
        let field_name = format!("{}Changed", schema.table_name);
        let table_name = schema.table_name.clone();
        subscription = subscription.field(SubscriptionField::new(
            &field_name,
            TypeRef::named_nn("DoogatChangeEvent"),
            move |ctx| {
                let handle = ctx.data::<ActorHandle>().cloned();
                let table_name = table_name.clone();
                SubscriptionFieldFuture::new(async move {
                    let handle = handle?;
                    let event_bus = handle.event_bus().clone();
                    let actor = handle;
                    let rx = event_bus.subscribe();
                    let stream = event_stream(rx).filter_map(move |result| {
                        let actor = actor.clone();
                        let table_name = table_name.clone();
                        async move {
                            let event = result.ok()?;
                            if event.doogat_type.as_deref() != Some(&table_name) {
                                return None;
                            }
                            let action = match event.kind {
                                EventKind::Created => "created",
                                EventKind::Updated => "updated",
                                EventKind::Deleted => "deleted",
                            };
                            let doogat = if event.kind != EventKind::Deleted {
                                actor
                                    .get_doogat(event.doogat_id.clone())
                                    .await
                                    .ok()
                                    .map(|z| doogat_to_value(&z))
                            } else {
                                None
                            };
                            let mut map = IndexMap::new();
                            map.insert(Name::new("action"), GqlValue::from(action));
                            map.insert(
                                Name::new("doogatId"),
                                GqlValue::from(event.doogat_id.as_str()),
                            );
                            if let Some(z) = doogat {
                                map.insert(Name::new("doogat"), z);
                            }
                            Some(Ok(FieldValue::owned_any(GqlValue::Object(map))))
                        }
                    });
                    Ok(stream)
                })
            },
        ));
    }

    // -- Build schema --
    let mut builder = Schema::build(
        query.type_name(),
        Some(mutation.type_name()),
        Some(subscription.type_name()),
    )
    .register(doogat_type)
    .register(inline_field_type)
    .register(link_type)
    .register(search_hit_type)
    .register(search_connection_type)
    .register(column_info_type)
    .register(typedef_type)
    .register(sql_result_type)
    .register(create_input)
    .register(update_input)
    .register(search_field_filter_input)
    .register(attachment_type)
    .register(checkbox_item_type)
    .register(unlinked_mention_type)
    .register(suggestion_type)
    .register(stale_doogat_type)
    .register(orphan_doogat_type)
    .register(tag_info_type)
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
    .data(read_pool);

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

