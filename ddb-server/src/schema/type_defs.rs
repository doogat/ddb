use async_graphql::dynamic::*;
use async_graphql::Value as GqlValue;

use super::base_types::*;

/// All shared GraphQL type definitions (objects, inputs, enums) that are
/// registered on the schema alongside the Query/Mutation/Subscription roots.
pub(crate) struct TypeDefs {
    pub inline_field_type: Object,
    pub link_type: Object,
    pub search_hit_type: Object,
    pub search_connection_type: Object,
    pub column_info_type: Object,
    pub typedef_type: Object,
    pub sql_result_type: Object,
    pub attachment_type: Object,
    pub checkbox_item_type: Object,
    pub tag_info_type: Object,
    pub tag_entry_type: Object,
    pub tag_entries_connection_type: Object,
    pub tag_entries_where_input: InputObject,
    pub unlinked_mention_type: Object,
    pub suggestion_type: Object,
    pub stale_doogat_type: Object,
    pub orphan_doogat_type: Object,
    pub doogat_type: Object,
    pub create_input: InputObject,
    pub create_many_item_input: InputObject,
    pub conflict_action_enum: Enum,
    pub update_input: InputObject,
    pub search_field_filter_input: InputObject,
}

pub(crate) fn build_type_defs() -> TypeDefs {
    let inline_field_type = Object::new("InlineField")
        .description("A key-value pair from a doogat's frontmatter, body, or reference zone.")
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
        .description("A wikilink or reference from one doogat to another.")
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
        .description("A single search result with metadata and relevance score.")
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("title", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("path", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("snippet", TypeRef::named_nn(TypeRef::STRING)).description("FTS5-generated snippet with <b> highlight tags. Empty string for all-negative queries."))
        .field(Field::new(
            "rank",
            TypeRef::named_nn(TypeRef::FLOAT),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "rank"))
                })
            },
        ).description("BM25 relevance score. Lower values indicate better matches. 0.0 for non-FTS queries."))
        .field(simple_field("updated_at", TypeRef::named(TypeRef::STRING)))
        .field(simple_field("tags", TypeRef::named_nn_list_nn(TypeRef::STRING)))
        .field(simple_field("type", TypeRef::named(TypeRef::STRING)))
        .field(Field::new("fields", TypeRef::named("JSON"), |ctx| {
            FieldFuture::new(async move {
                let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                match obj {
                    GqlValue::Object(map) => match map.get("fields") {
                        Some(val) => Ok(Some(FieldValue::value(val.clone()))),
                        None => Ok(None),
                    },
                    _ => Ok(None),
                }
            })
        }).description("Type-specific frontmatter fields as a JSON object (key-value pairs). Null for untyped doogats."))
        .field(simple_field("created_at", TypeRef::named(TypeRef::STRING)));

    let search_connection_type = Object::new("SearchConnection")
        .description("Paginated search results with total count and normalized query.")
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
        ))
        .field(simple_field(
            "queryNormalized",
            TypeRef::named_nn(TypeRef::STRING),
        ).description("Canonical form of the search query. Use for deduplication and saved search comparison."));

    let column_info_type = Object::new("ColumnInfo")
        .description("Schema definition for a single column in a type definition.")
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
        .description("A registered type definition with its column schema and CRDT strategy.")
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
        .description("Result of a SQL query or statement execution.")
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
        .description("A file attached to a doogat.")
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
        .description("A checkbox or task item extracted from a doogat's body.")
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
        .description("A tag with its usage count across all doogats.")
        .field(simple_field("name", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("count", TypeRef::named_nn(TypeRef::INT)));

    let tag_entry_type = Object::new("TagEntry")
        .description("A single tag-to-doogat assignment with its source (frontmatter or body).")
        .field(simple_field("doogatId", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("tag", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("source", TypeRef::named_nn(TypeRef::STRING)));

    let tag_entries_connection_type = crate::filter::build_connection_type("TagEntry");

    let tag_entries_where_input = InputObject::new("TagEntriesWhere")
        .description("Filter conditions for querying tag entries.")
        .field(InputValue::new("doogatId", TypeRef::named("StringFilter")))
        .field(InputValue::new("tag", TypeRef::named("StringFilter")));

    let unlinked_mention_type = Object::new("UnlinkedMention")
        .description("A doogat that mentions another doogat's title as plain text without a wikilink.")
        .field(simple_field("sourceId", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field(
            "sourceTitle",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(simple_field("snippet", TypeRef::named_nn(TypeRef::STRING)));

    let suggestion_type = Object::new("Suggestion")
        .description("A suggested doogat to link based on shared tags and content similarity.")
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
        .description("A typed doogat that hasn't been updated within its staleness threshold.")
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
        .description("A typed doogat with no inbound links from other doogats.")
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

    let doogat_type = doogat_object("Doogat")
        .description("A doogat (document/note) with metadata, content, and relationships.");

    let create_input = InputObject::new("CreateDoogatInput")
        .description("Input for creating a new doogat.")
        .field(InputValue::new("title", TypeRef::named(TypeRef::STRING)).description("Title for the new doogat. Omit (or pass null) to render server-side from the typedef's title_template; rejected with NOT_NULL_VIOLATION when no template is declared."))
        .field(InputValue::new("content", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new(
            "tags",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new("type", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("fields", TypeRef::named(TypeRef::STRING)).description("JSON object of frontmatter key-value pairs for typed columns."));

    let create_many_item_input = InputObject::new("CreateManyItemInput")
        .description("Input for a single item in a batch create operation.")
        .field(InputValue::new("title", TypeRef::named(TypeRef::STRING)).description("Title for the new doogat. Omit (or pass null) to render server-side from the typedef's title_template; rejected with NOT_NULL_VIOLATION when no template is declared."))
        .field(InputValue::new("content", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new(
            "tags",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new("type", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("fields", TypeRef::named(TypeRef::STRING)).description("JSON object of frontmatter key-value pairs for typed columns."));

    let conflict_action_enum = Enum::new("ConflictAction")
        .description("Action to take when a unique constraint violation occurs during creation.")
        .item(EnumItem::new("ERROR").description("Fail with an error (default)."))
        .item(EnumItem::new("IGNORE").description("Skip creation and return the existing doogat."));

    let update_input = InputObject::new("UpdateDoogatInput")
        .description("Input for updating an existing doogat. Omitted fields are left unchanged.")
        .field(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)))
        .field(InputValue::new("title", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("content", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new(
            "tags",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new("type", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("fields", TypeRef::named(TypeRef::STRING)).description("JSON object of type-specific field key-value pairs to set."))
        .field(InputValue::new("unsetFields", TypeRef::named_list(TypeRef::STRING)).description("Type-specific field names to remove."));

    let search_field_filter_input = InputObject::new("SearchFieldFilter")
        .description("Filter condition for structured field-based search filtering.")
        .field(InputValue::new(
            "field",
            TypeRef::named_nn(TypeRef::STRING),
        ).description("Field name to filter on. Resolution order: 'tag' routes to tag index, then materialized type columns, then frontmatter key-value store fallback."))
        .field(InputValue::new("eq", TypeRef::named(TypeRef::STRING)).description("Exact match. For tags, matches the tag name exactly."))
        .field(InputValue::new("contains", TypeRef::named(TypeRef::STRING)).description("Case-insensitive substring match (SQL LIKE %value%)."))
        .field(InputValue::new("in", TypeRef::named_list(TypeRef::STRING)).description("Set membership filter. Matches if field value equals any of the provided strings."));

    TypeDefs {
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
    }
}
