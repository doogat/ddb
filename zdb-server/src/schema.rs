use async_graphql::dynamic::*;
use async_graphql::{Name, Value as GqlValue};
use base64::engine::general_purpose as base64_engine;
use base64::Engine as _;
use futures_util::StreamExt;
use indexmap::IndexMap;
use tokio::sync::broadcast;
use zdb_core::sql_engine::SqlResult;
use zdb_core::types::{ColumnDef, ParsedZettel, SearchResult, TableSchema, Zone};

use std::sync::Arc;

use crate::actor::ActorHandle;
use crate::error::to_server_error;
use crate::events::EventKind;
use crate::read_pool::ReadPool;
use crate::reload::SchemaReloader;

// -- Helper: convert ParsedZettel to GqlValue (FieldValue) for the base Zettel type --

fn zettel_to_value(z: &ParsedZettel) -> GqlValue {
    let id = z.meta.id.as_ref().map(|i| i.0.as_str()).unwrap_or("");
    let title = z.meta.title.as_deref().unwrap_or("");
    let date = z.meta.date.as_deref().unwrap_or("");
    let ztype = z.meta.zettel_type.as_deref().unwrap_or("");
    let tags: Vec<GqlValue> = z
        .meta
        .tags
        .iter()
        .map(|t| GqlValue::from(t.as_str()))
        .collect();

    let fields: Vec<GqlValue> = z
        .inline_fields
        .iter()
        .map(|f| {
            let zone = match f.zone {
                Zone::Frontmatter => "frontmatter",
                Zone::Body => "body",
                Zone::Reference => "reference",
            };
            GqlValue::Object(
                [
                    (Name::new("key"), GqlValue::from(f.key.as_str())),
                    (Name::new("value"), GqlValue::from(f.value.as_str())),
                    (Name::new("zone"), GqlValue::from(zone)),
                ]
                .into_iter()
                .collect(),
            )
        })
        .collect();

    let links: Vec<GqlValue> = z
        .links
        .iter()
        .map(|l| {
            let zone = match l.zone {
                Zone::Frontmatter => "frontmatter",
                Zone::Body => "body",
                Zone::Reference => "reference",
            };
            let mut obj = IndexMap::new();
            obj.insert(Name::new("target"), GqlValue::from(l.target.as_str()));
            obj.insert(
                Name::new("display"),
                l.display
                    .as_deref()
                    .map(GqlValue::from)
                    .unwrap_or(GqlValue::Null),
            );
            obj.insert(Name::new("zone"), GqlValue::from(zone));
            let kind = l.kind.as_str();
            obj.insert(Name::new("kind"), GqlValue::from(kind));
            obj.insert(
                Name::new("section"),
                l.section
                    .as_deref()
                    .map(GqlValue::from)
                    .unwrap_or(GqlValue::Null),
            );
            GqlValue::Object(obj)
        })
        .collect();

    let mut obj = IndexMap::new();
    obj.insert(Name::new("id"), GqlValue::from(id));
    obj.insert(Name::new("title"), GqlValue::from(title));
    obj.insert(Name::new("date"), GqlValue::from(date));
    obj.insert(Name::new("type"), GqlValue::from(ztype));
    obj.insert(Name::new("tags"), GqlValue::List(tags));
    obj.insert(Name::new("body"), GqlValue::from(z.body.as_str()));
    obj.insert(Name::new("path"), GqlValue::from(z.path.as_str()));
    obj.insert(Name::new("fields"), GqlValue::List(fields));
    obj.insert(Name::new("links"), GqlValue::List(links));

    // Attachments from frontmatter extra
    let attachments: Vec<GqlValue> = {
        use zdb_core::types::Value;
        match z.meta.extra.get("attachments") {
            Some(Value::List(items)) => items
                .iter()
                .filter_map(|item| {
                    let Value::Map(map) = item else { return None };
                    let name = map.get("name")?.as_str()?;
                    let mime = map
                        .get("mime")
                        .and_then(|v| v.as_str())
                        .unwrap_or("application/octet-stream");
                    let size = map.get("size").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
                    let zid = z.meta.id.as_ref().map(|i| i.0.as_str()).unwrap_or("");
                    let url = format!("/attachments/{}/{}", zid, name);
                    let mut a = IndexMap::new();
                    a.insert(Name::new("name"), GqlValue::from(name));
                    a.insert(Name::new("mime"), GqlValue::from(mime));
                    a.insert(Name::new("size"), GqlValue::from(size));
                    a.insert(Name::new("url"), GqlValue::from(url.as_str()));
                    Some(GqlValue::Object(a))
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    obj.insert(Name::new("attachments"), GqlValue::List(attachments));

    GqlValue::Object(obj)
}

fn checkbox_row_to_value(row: &[String]) -> GqlValue {
    let zettel_id = row.first().map(|s| s.as_str()).unwrap_or("");
    let zettel_title = row.get(1).map(|s| s.as_str()).unwrap_or("");
    let state = row.get(2).map(|s| s.as_str()).unwrap_or("");
    let content = row.get(3).map(|s| s.as_str()).unwrap_or("");
    let date = row.get(4).filter(|s| !s.is_empty());
    let due_date = row.get(5).filter(|s| !s.is_empty());
    let line_number = row.get(6).and_then(|s| s.parse::<i64>().ok());
    let indent_level = row.get(7).and_then(|s| s.parse::<i64>().ok());

    let mut obj = IndexMap::new();
    obj.insert(Name::new("zettelId"), GqlValue::from(zettel_id));
    obj.insert(Name::new("zettelTitle"), GqlValue::from(zettel_title));
    obj.insert(Name::new("state"), GqlValue::from(state));
    obj.insert(Name::new("content"), GqlValue::from(content));
    obj.insert(
        Name::new("date"),
        date.map(|d| GqlValue::from(d.as_str()))
            .unwrap_or(GqlValue::Null),
    );
    obj.insert(
        Name::new("dueDate"),
        due_date
            .map(|d| GqlValue::from(d.as_str()))
            .unwrap_or(GqlValue::Null),
    );
    obj.insert(
        Name::new("lineNumber"),
        line_number.map(GqlValue::from).unwrap_or(GqlValue::Null),
    );
    obj.insert(
        Name::new("indentLevel"),
        indent_level.map(GqlValue::from).unwrap_or(GqlValue::Null),
    );
    GqlValue::Object(obj)
}

fn search_hit_to_value(r: &SearchResult) -> GqlValue {
    let mut obj = IndexMap::new();
    obj.insert(Name::new("id"), GqlValue::from(r.id.as_str()));
    obj.insert(Name::new("title"), GqlValue::from(r.title.as_str()));
    obj.insert(Name::new("path"), GqlValue::from(r.path.as_str()));
    obj.insert(Name::new("snippet"), GqlValue::from(r.snippet.as_str()));
    obj.insert(Name::new("rank"), GqlValue::from(r.rank));
    GqlValue::Object(obj)
}

fn sql_result_to_value(r: &SqlResult) -> GqlValue {
    let mut obj = IndexMap::new();
    match r {
        SqlResult::Rows { rows, .. } => {
            // Encode each row as a JSON string to avoid nested list limitation
            let gql_rows: Vec<GqlValue> = rows
                .iter()
                .map(|row| {
                    let json = serde_json::to_string(row).unwrap_or_default();
                    GqlValue::from(json)
                })
                .collect();
            obj.insert(Name::new("rows"), GqlValue::List(gql_rows));
            obj.insert(Name::new("affected"), GqlValue::Null);
            obj.insert(Name::new("message"), GqlValue::Null);
        }
        SqlResult::Affected(n) => {
            obj.insert(Name::new("rows"), GqlValue::Null);
            obj.insert(Name::new("affected"), GqlValue::from(*n as i64));
            obj.insert(Name::new("message"), GqlValue::Null);
        }
        SqlResult::Ok(msg) => {
            obj.insert(Name::new("rows"), GqlValue::Null);
            obj.insert(Name::new("affected"), GqlValue::Null);
            obj.insert(Name::new("message"), GqlValue::from(msg.as_str()));
        }
    }
    GqlValue::Object(obj)
}

fn typedef_to_value(s: &TableSchema) -> GqlValue {
    let columns: Vec<GqlValue> = s
        .columns
        .iter()
        .map(|c| {
            let mut obj = IndexMap::new();
            obj.insert(Name::new("name"), GqlValue::from(c.name.as_str()));
            obj.insert(Name::new("dataType"), GqlValue::from(c.data_type.as_str()));
            obj.insert(
                Name::new("zone"),
                c.zone
                    .as_ref()
                    .map(|z| {
                        GqlValue::from(match z {
                            Zone::Frontmatter => "frontmatter",
                            Zone::Body => "body",
                            Zone::Reference => "reference",
                        })
                    })
                    .unwrap_or(GqlValue::Null),
            );
            obj.insert(Name::new("required"), GqlValue::from(c.required));
            obj.insert(
                Name::new("references"),
                c.references
                    .as_deref()
                    .map(GqlValue::from)
                    .unwrap_or(GqlValue::Null),
            );
            obj.insert(
                Name::new("allowedValues"),
                c.allowed_values
                    .as_ref()
                    .map(|vals| {
                        GqlValue::List(vals.iter().map(|v| GqlValue::from(v.as_str())).collect())
                    })
                    .unwrap_or(GqlValue::Null),
            );
            obj.insert(
                Name::new("defaultValue"),
                c.default_value
                    .as_deref()
                    .map(GqlValue::from)
                    .unwrap_or(GqlValue::Null),
            );
            GqlValue::Object(obj)
        })
        .collect();

    let sections: Vec<GqlValue> = s
        .template_sections
        .iter()
        .map(|s| GqlValue::from(s.as_str()))
        .collect();

    let mut obj = IndexMap::new();
    obj.insert(Name::new("name"), GqlValue::from(s.table_name.as_str()));
    obj.insert(Name::new("columns"), GqlValue::List(columns));
    obj.insert(
        Name::new("crdtStrategy"),
        s.crdt_strategy
            .as_deref()
            .map(GqlValue::from)
            .unwrap_or(GqlValue::Null),
    );
    obj.insert(Name::new("templateSections"), GqlValue::List(sections));
    GqlValue::Object(obj)
}

/// Convert a ParsedZettel into a typed GraphQL value with native typed fields from its schema.
fn typed_zettel_to_value(z: &ParsedZettel, schema: &TableSchema) -> GqlValue {
    // Start with base zettel fields
    let base = zettel_to_value(z);
    let mut obj = match base {
        GqlValue::Object(o) => o,
        _ => return base,
    };

    // Add typed columns
    for col in &schema.columns {
        let val = extract_typed_field(z, col);
        obj.insert(Name::new(&col.name), val);
    }

    GqlValue::Object(obj)
}

/// Extract a typed field value from a ParsedZettel based on the column definition.
fn extract_typed_field(z: &ParsedZettel, col: &ColumnDef) -> GqlValue {
    let zone = col.zone.as_ref().unwrap_or(&Zone::Frontmatter);
    let raw = match zone {
        Zone::Frontmatter => {
            // Use path navigation for dot/bracket names, flat lookup otherwise
            let val = if col.name.contains('.') || col.name.contains('[') {
                zdb_core::types::get_path_in_map(&z.meta.extra, &col.name).ok()
            } else {
                z.meta.extra.get(&col.name)
            };
            val.map(|v| match v {
                zdb_core::types::Value::String(s) => s.clone(),
                zdb_core::types::Value::Number(n) => n.to_string(),
                zdb_core::types::Value::Bool(b) => b.to_string(),
                _ => String::new(),
            })
        }
        Zone::Body => {
            // Extract section content under ## {column_name}
            extract_body_section(&z.body, &col.name)
        }
        Zone::Reference => {
            // Extract from inline_fields where zone=Reference and key=column_name
            z.inline_fields
                .iter()
                .find(|f| f.key == col.name && matches!(f.zone, Zone::Reference))
                .map(|f| f.value.clone())
        }
    };

    match raw {
        None => GqlValue::Null,
        Some(s) => match col.data_type.to_uppercase().as_str() {
            "BOOLEAN" => {
                let b = matches!(s.to_lowercase().as_str(), "true" | "1" | "yes");
                GqlValue::from(b)
            }
            "INTEGER" => s
                .parse::<i64>()
                .map(GqlValue::from)
                .unwrap_or(GqlValue::Null),
            "REAL" => s
                .parse::<f64>()
                .map(GqlValue::from)
                .unwrap_or(GqlValue::Null),
            _ => GqlValue::from(s),
        },
    }
}

/// Extract content under a ## heading in the body.
fn extract_body_section(body: &str, heading: &str) -> Option<String> {
    let target = format!("## {heading}");
    let mut lines = body.lines();
    // Find the heading
    let found = lines.by_ref().any(|l| l.trim() == target);
    if !found {
        return None;
    }
    // Collect lines until next heading or end
    let mut content = Vec::new();
    for line in lines {
        if line.starts_with("## ") {
            break;
        }
        content.push(line);
    }
    let text = content.join("\n").trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

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
        .field(simple_field("zettelId", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("zettelTitle", TypeRef::named(TypeRef::STRING)))
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

    let stale_zettel_type = Object::new("StaleZettel")
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("title", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field(
            "zettelType",
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

    let orphan_zettel_type = Object::new("OrphanZettel")
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("title", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field(
            "zettelType",
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

    // Base Zettel type
    let zettel_type = zettel_object("Zettel");

    // Input types
    let create_input = InputObject::new("CreateZettelInput")
        .field(InputValue::new("title", TypeRef::named_nn(TypeRef::STRING)))
        .field(InputValue::new("content", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new(
            "tags",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new("type", TypeRef::named(TypeRef::STRING)));

    let update_input = InputObject::new("UpdateZettelInput")
        .field(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)))
        .field(InputValue::new("title", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("content", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new(
            "tags",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new("type", TypeRef::named(TypeRef::STRING)));

    // -- Query fields --
    let mut query = Object::new("Query");

    // zettel(id)
    {
        query = query.field(
            Field::new("zettel", TypeRef::named("Zettel"), |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let id = ctx.args.try_get("id")?.string()?.to_string();
                    let z = pool.get_zettel(id).await.map_err(to_server_error)?;
                    Ok(Some(FieldValue::owned_any(zettel_to_value(&z))))
                })
            })
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID))),
        );
    }

    // zettels(type, tag, backlinksOf, limit, offset)
    {
        query = query.field(
            Field::new("zettels", TypeRef::named_nn_list_nn("Zettel"), |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let zettel_type = ctx
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
                    let zettels = pool
                        .list_zettels(zettel_type, tag, backlinks_of, vec![], limit, offset)
                        .await
                        .map_err(to_server_error)?;
                    Ok(Some(FieldValue::list(
                        zettels
                            .iter()
                            .map(|z| FieldValue::owned_any(zettel_to_value(z))),
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

    // search(query, limit?, offset?)
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
                    let result = pool
                        .search(q, limit, offset)
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

    // checkboxItems(state?, zettelId?, limit?, offset?)
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
                        let zettel_id = ctx
                            .args
                            .get("zettelId")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let limit = ctx.args.get("limit").and_then(|v| v.i64().ok());
                        let offset = ctx.args.get("offset").and_then(|v| v.i64().ok());
                        let rows = pool
                            .query_checkboxes(state, zettel_id, limit, offset)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(rows.iter().map(|row| {
                            FieldValue::owned_any(checkbox_row_to_value(row))
                        }))))
                    })
                },
            )
            .argument(InputValue::new("state", TypeRef::named(TypeRef::STRING)))
            .argument(InputValue::new("zettelId", TypeRef::named(TypeRef::ID)))
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

    // staleZettels(type: String): [StaleZettel!]!
    {
        query = query.field(
            Field::new(
                "staleZettels",
                TypeRef::named_nn_list_nn("StaleZettel"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let type_filter = ctx
                            .args
                            .get("type")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let stale = pool
                            .stale_zettels(type_filter)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(stale.iter().map(|s| {
                            let mut obj = IndexMap::new();
                            obj.insert(Name::new("id"), GqlValue::from(s.id.as_str()));
                            obj.insert(Name::new("title"), GqlValue::from(s.title.as_str()));
                            obj.insert(
                                Name::new("zettelType"),
                                GqlValue::from(s.zettel_type.as_str()),
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

    // orphanZettels(type: String): [OrphanZettel!]!
    {
        query = query.field(
            Field::new(
                "orphanZettels",
                TypeRef::named_nn_list_nn("OrphanZettel"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let type_filter = ctx
                            .args
                            .get("type")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let orphans = pool
                            .orphan_zettels(type_filter)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(orphans.iter().map(|o| {
                            let mut obj = IndexMap::new();
                            obj.insert(Name::new("id"), GqlValue::from(o.id.as_str()));
                            obj.insert(Name::new("title"), GqlValue::from(o.title.as_str()));
                            obj.insert(
                                Name::new("zettelType"),
                                GqlValue::from(o.zettel_type.as_str()),
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
        .field(simple_field("zettelId", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field(
            "brokenParentId",
            TypeRef::named_nn(TypeRef::ID),
        ));

    fn seq_node_to_gql(n: &zdb_core::types::SequenceNode) -> GqlValue {
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
                        obj.insert(Name::new("zettelId"), GqlValue::from(b.zettel_id.as_str()));
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
            log::warn!(
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
                            let zettels = pool
                                .filtered_list(crate::read_pool::FilteredListQuery {
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
                                    "id IN (SELECT zettel_id FROM _zdb_tags WHERE tag = '{}')",
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
                                zettels
                                    .iter()
                                    .map(|z| typed_zettel_to_value(z, &schema_clone))
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

    // -- Mutation fields --
    let mut mutation = Object::new("Mutation");

    // createZettel
    {
        mutation = mutation.field(
            Field::new("createZettel", TypeRef::named_nn("Zettel"), |ctx| {
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
                    let zettel_type = input
                        .get("type")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
                    let z = a
                        .create_zettel(title, content, tags, zettel_type)
                        .await
                        .map_err(to_server_error)?;
                    Ok(Some(FieldValue::owned_any(zettel_to_value(&z))))
                })
            })
            .argument(InputValue::new(
                "input",
                TypeRef::named_nn("CreateZettelInput"),
            )),
        );
    }

    // updateZettel
    {
        mutation = mutation.field(
            Field::new("updateZettel", TypeRef::named_nn("Zettel"), |ctx| {
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
                    let zettel_type = input
                        .get("type")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
                    let z = a
                        .update_zettel(id, title, content, tags, zettel_type)
                        .await
                        .map_err(to_server_error)?;
                    Ok(Some(FieldValue::owned_any(zettel_to_value(&z))))
                })
            })
            .argument(InputValue::new(
                "input",
                TypeRef::named_nn("UpdateZettelInput"),
            )),
        );
    }

    // deleteZettel
    {
        mutation = mutation.field(
            Field::new("deleteZettel", TypeRef::named_nn(TypeRef::BOOLEAN), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let id = ctx.args.try_get("id")?.string()?.to_string();
                    a.delete_zettel(id).await.map_err(to_server_error)?;
                    Ok(Some(FieldValue::value(GqlValue::from(true))))
                })
            })
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID))),
        );
    }

    // attachFile(zettelId, filename, dataBase64, mime?)
    {
        let attach_input = InputObject::new("AttachFileInput")
            .field(InputValue::new("zettelId", TypeRef::named_nn(TypeRef::ID)))
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
                    let zettel_id = input.try_get("zettelId")?.string()?.to_string();
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
                            zdb_core::types::AttachmentInfo::mime_from_filename(&filename)
                                .to_string()
                        });
                    let info = a
                        .attach_file(zettel_id, filename, bytes, mime)
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

    // detachFile(zettelId, filename)
    {
        mutation = mutation.field(
            Field::new("detachFile", TypeRef::named_nn(TypeRef::BOOLEAN), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let zettel_id = ctx.args.try_get("zettelId")?.string()?.to_string();
                    let filename = ctx.args.try_get("filename")?.string()?.to_string();
                    a.detach_file(zettel_id, filename)
                        .await
                        .map_err(to_server_error)?;
                    Ok(Some(FieldValue::value(GqlValue::from(true))))
                })
            })
            .argument(InputValue::new("zettelId", TypeRef::named_nn(TypeRef::ID)))
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
        .field(simple_field("resurrected", TypeRef::named_nn(TypeRef::INT)));

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

    // -- ZettelChangeEvent type --
    let change_event_type = Object::new("ZettelChangeEvent")
        .field(simple_field("action", TypeRef::named_nn(TypeRef::STRING)))
        .field(Field::new("zettel", TypeRef::named("Zettel"), |ctx| {
            FieldFuture::new(async move {
                let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                Ok(obj_field(obj, "zettel"))
            })
        }))
        .field(simple_field("zettelId", TypeRef::named_nn(TypeRef::ID)));

    // -- Subscription fields --
    let mut subscription = Subscription::new("Subscription");

    // zettelChanged: ZettelChangeEvent! — all events
    subscription = subscription.field(SubscriptionField::new(
        "zettelChanged",
        TypeRef::named_nn("ZettelChangeEvent"),
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
                        let zettel = if event.kind != EventKind::Deleted {
                            actor
                                .get_zettel(event.zettel_id.clone())
                                .await
                                .ok()
                                .map(|z| zettel_to_value(&z))
                        } else {
                            None
                        };
                        let mut map = IndexMap::new();
                        map.insert(Name::new("action"), GqlValue::from(action));
                        map.insert(
                            Name::new("zettelId"),
                            GqlValue::from(event.zettel_id.as_str()),
                        );
                        if let Some(z) = zettel {
                            map.insert(Name::new("zettel"), z);
                        }
                        Ok(FieldValue::owned_any(GqlValue::Object(map)))
                    }
                });
                Ok(stream)
            })
        },
    ));

    // zettelCreated: Zettel! — only Created events
    subscription = subscription.field(SubscriptionField::new(
        "zettelCreated",
        TypeRef::named_nn("Zettel"),
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
                        let z = actor.get_zettel(event.zettel_id).await.ok()?;
                        Some(Ok(FieldValue::owned_any(zettel_to_value(&z))))
                    }
                });
                Ok(stream)
            })
        },
    ));

    // zettelUpdated: Zettel! — only Updated events
    subscription = subscription.field(SubscriptionField::new(
        "zettelUpdated",
        TypeRef::named_nn("Zettel"),
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
                        let z = actor.get_zettel(event.zettel_id).await.ok()?;
                        Some(Ok(FieldValue::owned_any(zettel_to_value(&z))))
                    }
                });
                Ok(stream)
            })
        },
    ));

    // zettelDeleted: ID! — only Deleted events
    subscription = subscription.field(SubscriptionField::new(
        "zettelDeleted",
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
                        event.zettel_id.as_str(),
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
            TypeRef::named_nn("ZettelChangeEvent"),
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
                            if event.zettel_type.as_deref() != Some(&table_name) {
                                return None;
                            }
                            let action = match event.kind {
                                EventKind::Created => "created",
                                EventKind::Updated => "updated",
                                EventKind::Deleted => "deleted",
                            };
                            let zettel = if event.kind != EventKind::Deleted {
                                actor
                                    .get_zettel(event.zettel_id.clone())
                                    .await
                                    .ok()
                                    .map(|z| zettel_to_value(&z))
                            } else {
                                None
                            };
                            let mut map = IndexMap::new();
                            map.insert(Name::new("action"), GqlValue::from(action));
                            map.insert(
                                Name::new("zettelId"),
                                GqlValue::from(event.zettel_id.as_str()),
                            );
                            if let Some(z) = zettel {
                                map.insert(Name::new("zettel"), z);
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
    .register(zettel_type)
    .register(inline_field_type)
    .register(link_type)
    .register(search_hit_type)
    .register(search_connection_type)
    .register(column_info_type)
    .register(typedef_type)
    .register(sql_result_type)
    .register(create_input)
    .register(update_input)
    .register(attachment_type)
    .register(checkbox_item_type)
    .register(unlinked_mention_type)
    .register(suggestion_type)
    .register(stale_zettel_type)
    .register(orphan_zettel_type)
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

// -- Helper functions --

/// Check if a string is a valid GraphQL name (`/[_A-Za-z][_0-9A-Za-z]*/`).
pub fn is_valid_graphql_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn simple_field(name: &str, type_ref: TypeRef) -> Field {
    let name_owned = name.to_string();
    Field::new(name, type_ref, move |ctx| {
        let name = name_owned.clone();
        FieldFuture::new(async move {
            let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
            Ok(obj_field(obj, &name))
        })
    })
}

/// Convert a GqlValue into the correct FieldValue variant:
/// objects → owned_any, lists → recursive, scalars → value.
fn gql_to_field_value(val: GqlValue) -> FieldValue<'static> {
    match val {
        GqlValue::Object(_) => FieldValue::owned_any(val),
        GqlValue::List(items) => FieldValue::list(items.into_iter().map(gql_to_field_value)),
        other => FieldValue::value(other),
    }
}

fn obj_field(obj: &GqlValue, key: &str) -> Option<FieldValue<'static>> {
    match obj {
        GqlValue::Object(map) => Some(gql_to_field_value(map.get(key)?.clone())),
        _ => None,
    }
}

fn zettel_object(name: &str) -> Object {
    Object::new(name)
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("title", TypeRef::named(TypeRef::STRING)))
        .field(simple_field("date", TypeRef::named(TypeRef::STRING)))
        .field(simple_field("type", TypeRef::named(TypeRef::STRING)))
        .field(Field::new(
            "tags",
            TypeRef::named_nn_list_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "tags"))
                })
            },
        ))
        .field(simple_field("body", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("path", TypeRef::named_nn(TypeRef::STRING)))
        .field(Field::new(
            "fields",
            TypeRef::named_nn_list_nn("InlineField"),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "fields"))
                })
            },
        ))
        .field(Field::new(
            "links",
            TypeRef::named_nn_list_nn("Link"),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "links"))
                })
            },
        ))
        .field(Field::new(
            "attachments",
            TypeRef::named_nn_list_nn("Attachment"),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "attachments"))
                })
            },
        ))
}

/// Build a dynamic GraphQL object type for a _typedef schema.
fn build_typed_object(type_name: &str, schema: &TableSchema) -> Object {
    let mut obj = zettel_object(type_name);

    for col in &schema.columns {
        if !is_valid_graphql_name(&col.name) {
            log::warn!(
                "skipping column '{}' in type {type_name}: not a valid GraphQL identifier",
                col.name
            );
            continue;
        }
        let gql_type = column_to_gql_type(col);
        let col_name = col.name.clone();
        obj = obj.field(Field::new(&col.name, gql_type, move |ctx| {
            let col_name = col_name.clone();
            FieldFuture::new(async move {
                let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                Ok(obj_field(obj, &col_name))
            })
        }));
    }

    obj
}

fn column_to_gql_type(col: &ColumnDef) -> TypeRef {
    match col.data_type.to_uppercase().as_str() {
        "BOOLEAN" => TypeRef::named(TypeRef::BOOLEAN),
        "INTEGER" => TypeRef::named(TypeRef::INT),
        "REAL" => TypeRef::named(TypeRef::FLOAT),
        _ => TypeRef::named(TypeRef::STRING),
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn pluralize(s: &str) -> String {
    let s = s.to_lowercase();
    if s.ends_with('s') {
        format!("{s}es")
    } else if s.ends_with('y') {
        format!("{}ies", &s[..s.len() - 1])
    } else {
        format!("{s}s")
    }
}

/// Convert a broadcast::Receiver into a Stream that skips lag errors and ends on close.
fn event_stream(
    rx: broadcast::Receiver<crate::events::ZettelEvent>,
) -> impl futures_util::Stream<Item = async_graphql::Result<crate::events::ZettelEvent>> {
    futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(event) => return Some((Ok(event), rx)),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    log::warn!("subscription lagged, skipped {n} events");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
