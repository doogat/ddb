use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_graphql::dynamic::*;
use async_graphql::{Name, Value as GqlValue};
use indexmap::IndexMap;
use tokio::sync::broadcast;
use ddb_core::sql_engine::SqlResult;
use ddb_core::types::{ColumnDef, ParsedDoogat, SearchResult, TableSchema, Zone};

use crate::events;

// -- Shared schema data --

/// Map of type name → TableSchema, accessible to resolvers via `ctx.data()`.
#[derive(Clone)]
pub(crate) struct TypeSchemaMap(pub Arc<HashMap<String, TableSchema>>);

// -- Value converters --

pub(crate) fn doogat_to_value(z: &ParsedDoogat) -> GqlValue {
    let id = z.meta.id.as_ref().map(|i| i.0.as_str()).unwrap_or("");
    let title = z.meta.title.as_deref().unwrap_or("");
    let date = z.meta.date.as_deref().unwrap_or("");
    let ztype = z.meta.doogat_type.as_deref().unwrap_or("");
    let mut seen = std::collections::HashSet::new();
    let tags: Vec<GqlValue> = z
        .meta
        .tags
        .iter()
        .chain(z.body_tags.iter())
        .filter(|t| seen.insert(t.as_str()))
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
        use ddb_core::types::Value;
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

pub(crate) fn checkbox_row_to_value(row: &[String]) -> GqlValue {
    let doogat_id = row.first().map(|s| s.as_str()).unwrap_or("");
    let doogat_title = row.get(1).map(|s| s.as_str()).unwrap_or("");
    let state = row.get(2).map(|s| s.as_str()).unwrap_or("");
    let content = row.get(3).map(|s| s.as_str()).unwrap_or("");
    let date = row.get(4).filter(|s| !s.is_empty());
    let due_date = row.get(5).filter(|s| !s.is_empty());
    let line_number = row.get(6).and_then(|s| s.parse::<i64>().ok());
    let indent_level = row.get(7).and_then(|s| s.parse::<i64>().ok());

    let mut obj = IndexMap::new();
    obj.insert(Name::new("doogatId"), GqlValue::from(doogat_id));
    obj.insert(Name::new("doogatTitle"), GqlValue::from(doogat_title));
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

pub(crate) fn search_hit_to_value(r: &SearchResult) -> GqlValue {
    let mut obj = IndexMap::new();
    obj.insert(Name::new("id"), GqlValue::from(r.id.as_str()));
    obj.insert(Name::new("title"), GqlValue::from(r.title.as_str()));
    obj.insert(Name::new("path"), GqlValue::from(r.path.as_str()));
    obj.insert(Name::new("snippet"), GqlValue::from(r.snippet.as_str()));
    obj.insert(Name::new("rank"), GqlValue::from(r.rank));
    GqlValue::Object(obj)
}

pub(crate) fn tag_info_to_value(name: &str, count: i64) -> GqlValue {
    let mut obj = IndexMap::new();
    obj.insert(Name::new("name"), GqlValue::from(name));
    obj.insert(Name::new("count"), GqlValue::from(count));
    GqlValue::Object(obj)
}

pub(crate) fn sql_result_to_value(r: &SqlResult) -> GqlValue {
    let mut obj = IndexMap::new();
    match r {
        SqlResult::Rows { columns, rows } => {
            let gql_cols: Vec<GqlValue> =
                columns.iter().map(|c| GqlValue::from(c.as_str())).collect();
            obj.insert(Name::new("columns"), GqlValue::List(gql_cols));
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
            obj.insert(Name::new("columns"), GqlValue::Null);
            obj.insert(Name::new("rows"), GqlValue::Null);
            obj.insert(Name::new("affected"), GqlValue::from(*n as i64));
            obj.insert(Name::new("message"), GqlValue::Null);
        }
        SqlResult::Ok(msg) => {
            obj.insert(Name::new("columns"), GqlValue::Null);
            obj.insert(Name::new("rows"), GqlValue::Null);
            obj.insert(Name::new("affected"), GqlValue::Null);
            obj.insert(Name::new("message"), GqlValue::from(msg.as_str()));
        }
    }
    GqlValue::Object(obj)
}

pub(crate) fn typedef_to_value(s: &TableSchema) -> GqlValue {
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

/// Convert a ParsedDoogat into a typed GraphQL value with native typed fields from its schema.
pub(crate) fn typed_doogat_to_value(z: &ParsedDoogat, schema: &TableSchema) -> GqlValue {
    // Start with base doogat fields
    let base = doogat_to_value(z);
    let mut obj = match base {
        GqlValue::Object(o) => o,
        _ => return base,
    };

    // Add typed columns
    for col in &schema.columns {
        let val = extract_typed_field(z, col);
        obj.insert(Name::new(&col.name), val);

        // Add pluralized list for REFERENCES columns
        if col.references.is_some() {
            let ref_values: Vec<GqlValue> = z
                .inline_fields
                .iter()
                .filter(|f| f.key == col.name && matches!(f.zone, Zone::Reference))
                .map(|f| GqlValue::from(f.value.clone()))
                .collect();
            obj.insert(
                Name::new(pluralize(&col.name)),
                GqlValue::List(ref_values),
            );
        }
    }

    GqlValue::Object(obj)
}

/// Extract a typed field value from a ParsedDoogat based on the column definition.
pub(crate) fn extract_typed_field(z: &ParsedDoogat, col: &ColumnDef) -> GqlValue {
    let zone = col.zone.as_ref().unwrap_or(&Zone::Frontmatter);
    let raw = match zone {
        Zone::Frontmatter => {
            // Use path navigation for dot/bracket names, flat lookup otherwise
            let val = if col.name.contains('.') || col.name.contains('[') {
                ddb_core::types::get_path_in_map(&z.meta.extra, &col.name).ok()
            } else {
                z.meta.extra.get(&col.name)
            };
            val.map(|v| match v {
                ddb_core::types::Value::String(s) => s.clone(),
                ddb_core::types::Value::Number(n) => n.to_string(),
                ddb_core::types::Value::Bool(b) => b.to_string(),
                _ => String::new(),
            })
        }
        Zone::Body => {
            // Extract section content under ## {column_name}
            extract_body_section(&z.body, &col.name)
        }
        Zone::Reference => {
            // Return first matching reference value (list field has all)
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

pub(crate) use ddb_core::consistency::extract_body_section;

// -- Type builders & helpers --

/// Check if a string is a valid GraphQL name (`/[_A-Za-z][_0-9A-Za-z]*/`).
pub fn is_valid_graphql_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

pub(crate) fn simple_field(name: &str, type_ref: TypeRef) -> Field {
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
pub(crate) fn gql_to_field_value(val: GqlValue) -> FieldValue<'static> {
    match val {
        GqlValue::Object(_) => FieldValue::owned_any(val),
        GqlValue::List(items) => FieldValue::list(items.into_iter().map(gql_to_field_value)),
        other => FieldValue::value(other),
    }
}

pub(crate) fn obj_field(obj: &GqlValue, key: &str) -> Option<FieldValue<'static>> {
    match obj {
        GqlValue::Object(map) => Some(gql_to_field_value(map.get(key)?.clone())),
        _ => None,
    }
}

pub(crate) fn doogat_object(name: &str) -> Object {
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

/// Determine the GQL type name for a REFERENCES column's target.
/// Falls back to "Doogat" if the target type isn't in the known set.
fn ref_target_gql_type(col: &ColumnDef, known_types: &HashSet<String>) -> String {
    let target = col.references.as_deref().unwrap_or("");
    if known_types.contains(target) {
        capitalize(target)
    } else {
        "Doogat".to_string()
    }
}

/// Build a dynamic GraphQL object type for a _typedef schema.
pub(crate) fn build_typed_object(
    type_name: &str,
    schema: &TableSchema,
    known_types: &HashSet<String>,
) -> Object {
    let mut obj = doogat_object(type_name);

    for col in &schema.columns {
        if !is_valid_graphql_name(&col.name) {
            tracing::warn!(
                "skipping column '{}' in type {type_name}: not a valid GraphQL identifier",
                col.name
            );
            continue;
        }

        if col.references.is_some() {
            // Singular: resolves as the referenced typed object (nullable)
            let target_type = ref_target_gql_type(col, known_types);
            let target_ref_name = col.references.clone().unwrap_or_default();
            let col_name = col.name.clone();
            obj = obj.field(Field::new(
                &col.name,
                TypeRef::named(&target_type),
                move |ctx| {
                    let col_name = col_name.clone();
                    let target_ref_name = target_ref_name.clone();
                    FieldFuture::new(async move {
                        let parent = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                        let id = match parent {
                            GqlValue::Object(map) => match map.get(col_name.as_str()) {
                                Some(GqlValue::String(s)) if !s.is_empty() => s.to_string(),
                                _ => return Ok(None),
                            },
                            _ => return Ok(None),
                        };
                        let pool = ctx.data::<crate::read_pool::ReadPool>()?;
                        let schemas = ctx.data::<TypeSchemaMap>()?;
                        let doogat = match pool.get_doogat(id).await {
                            Ok(z) => z,
                            Err(_) => return Ok(None),
                        };
                        let val = match schemas.0.get(&target_ref_name) {
                            Some(ts) => typed_doogat_to_value(&doogat, ts),
                            None => doogat_to_value(&doogat),
                        };
                        Ok(Some(FieldValue::owned_any(val)))
                    })
                },
            ));

            // Plural: resolves as list of referenced typed objects
            let list_name = pluralize(&col.name);
            if is_valid_graphql_name(&list_name) {
                let target_type = ref_target_gql_type(col, known_types);
                let target_ref_name = col.references.clone().unwrap_or_default();
                let list_col_name = list_name.clone();
                obj = obj.field(Field::new(
                    &list_name,
                    TypeRef::named_nn_list_nn(&target_type),
                    move |ctx| {
                        let list_col_name = list_col_name.clone();
                        let target_ref_name = target_ref_name.clone();
                        FieldFuture::new(async move {
                            let parent = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                            let ids: Vec<String> = match parent {
                                GqlValue::Object(map) => match map.get(list_col_name.as_str()) {
                                    Some(GqlValue::List(items)) => items
                                        .iter()
                                        .filter_map(|v| match v {
                                            GqlValue::String(s) if !s.is_empty() => {
                                                Some(s.to_string())
                                            }
                                            _ => None,
                                        })
                                        .collect(),
                                    _ => return Ok(Some(FieldValue::list(
                                        std::iter::empty::<FieldValue>(),
                                    ))),
                                },
                                _ => {
                                    return Ok(Some(FieldValue::list(
                                        std::iter::empty::<FieldValue>(),
                                    )))
                                }
                            };
                            if ids.is_empty() {
                                return Ok(Some(FieldValue::list(
                                    std::iter::empty::<FieldValue>(),
                                )));
                            }
                            let pool = ctx.data::<crate::read_pool::ReadPool>()?;
                            let schemas = ctx.data::<TypeSchemaMap>()?;
                            let target_schema = schemas.0.get(&target_ref_name);
                            let mut resolved = Vec::with_capacity(ids.len());
                            for id in ids {
                                if let Ok(z) = pool.get_doogat(id).await {
                                    let val = match target_schema {
                                        Some(ts) => typed_doogat_to_value(&z, ts),
                                        None => doogat_to_value(&z),
                                    };
                                    resolved.push(FieldValue::owned_any(val));
                                }
                            }
                            Ok(Some(FieldValue::list(resolved)))
                        })
                    },
                ));
            }
        } else {
            // Non-REFERENCES scalar field
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
    }

    obj
}

pub(crate) fn column_to_gql_type(col: &ColumnDef) -> TypeRef {
    match col.data_type.to_uppercase().as_str() {
        "BOOLEAN" => TypeRef::named(TypeRef::BOOLEAN),
        "INTEGER" => TypeRef::named(TypeRef::INT),
        "REAL" => TypeRef::named(TypeRef::FLOAT),
        _ => TypeRef::named(TypeRef::STRING),
    }
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

pub(crate) fn pluralize(s: &str) -> String {
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
pub(crate) fn event_stream(
    rx: broadcast::Receiver<events::DoogatEvent>,
) -> impl futures_util::Stream<Item = async_graphql::Result<events::DoogatEvent>> {
    futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(event) => return Some((Ok(event), rx)),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("subscription lagged, skipped {n} events");
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

    #[test]
    fn typed_doogat_multi_ref_list_field() {
        use ddb_core::types::{InlineField, Zone, TableSchema, ColumnDef, DoogatMeta};
        use ddb_core::types::ParsedDoogat;

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
            }],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
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
        };
        let val = sql_result_to_value(&result);
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
    fn sql_result_affected_has_null_columns() {
        let result = SqlResult::Affected(3);
        let val = sql_result_to_value(&result);
        let obj = match &val {
            GqlValue::Object(o) => o,
            _ => panic!("expected object"),
        };
        assert_eq!(obj.get("columns").unwrap(), &GqlValue::Null);
        assert_eq!(obj.get("rows").unwrap(), &GqlValue::Null);
        assert_eq!(obj.get("affected").unwrap(), &GqlValue::from(3i64));
    }

    #[test]
    fn sql_result_ok_has_null_columns() {
        let result = SqlResult::Ok("done".into());
        let val = sql_result_to_value(&result);
        let obj = match &val {
            GqlValue::Object(o) => o,
            _ => panic!("expected object"),
        };
        assert_eq!(obj.get("columns").unwrap(), &GqlValue::Null);
        assert_eq!(obj.get("rows").unwrap(), &GqlValue::Null);
        assert_eq!(
            obj.get("message").unwrap(),
            &GqlValue::from("done")
        );
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
}
