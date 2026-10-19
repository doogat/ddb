use async_graphql::{Name, Value as GqlValue};
use ddb_core::sql_engine::SqlResult;
use ddb_core::types::{ColumnDef, ParsedDoogat, SearchResult, TableSchema, Value, Zone};
use indexmap::IndexMap;

use super::naming::pluralize;
use super::schema_object::BASE_DOOGAT_FIELDS;

pub(crate) use ddb_core::consistency::extract_body_section;

/// Decode a `JSON`-scalar object into `RawFieldsInput`, keeping native value
/// variants. A top-level `Null` becomes `None` (a per-field null signal); a
/// null nested inside a list or object is rejected with an error naming the
/// offending key path.
pub(crate) fn decode_json_fields_object(
    obj: &IndexMap<Name, GqlValue>,
) -> Result<ddb_core::app_contract::RawFieldsInput, String> {
    obj.iter()
        .map(|(k, v)| {
            let value = match v {
                GqlValue::Null => None,
                other => Some(gql_to_value_strict(other, k.as_str())?),
            };
            Ok((k.to_string(), value))
        })
        .collect()
}

/// Convert a non-top-level JSON value; `path` names the location for errors.
fn gql_to_value_strict(v: &GqlValue, path: &str) -> Result<Value, String> {
    Ok(match v {
        GqlValue::Null => {
            return Err(format!(
                "field '{path}': null is not allowed inside a nested list or object"
            ))
        }
        GqlValue::String(s) => Value::String(s.clone()),
        GqlValue::Number(n) => Value::Number(
            n.as_f64()
                .ok_or_else(|| format!("field '{path}': non-finite number"))?,
        ),
        GqlValue::Boolean(b) => Value::Bool(*b),
        GqlValue::List(items) => Value::List(
            items
                .iter()
                .enumerate()
                .map(|(i, item)| gql_to_value_strict(item, &format!("{path}[{i}]")))
                .collect::<Result<_, _>>()?,
        ),
        GqlValue::Object(map) => Value::Map(
            map.iter()
                .map(|(k, item)| {
                    Ok((k.to_string(), gql_to_value_strict(item, &format!("{path}.{k}"))?))
                })
                .collect::<Result<_, String>>()?,
        ),
        other => return Err(format!("field '{path}': unsupported value {other:?}")),
    })
}

// -- Value converters --

fn zone_str(zone: &Zone) -> &'static str {
    match zone {
        Zone::Frontmatter => "frontmatter",
        Zone::Body => "body",
        Zone::Reference => "reference",
    }
}

fn build_tags_list(z: &ParsedDoogat) -> Vec<GqlValue> {
    let mut seen = std::collections::HashSet::new();
    z.meta
        .tags
        .iter()
        .chain(z.body_tags.iter())
        .filter(|t| seen.insert(t.as_str()))
        .map(|t| GqlValue::from(t.as_str()))
        .collect()
}

fn build_inline_fields_list(z: &ParsedDoogat) -> Vec<GqlValue> {
    z.inline_fields
        .iter()
        .map(|f| {
            GqlValue::Object(
                [
                    (Name::new("key"), GqlValue::from(f.key.as_str())),
                    (Name::new("value"), GqlValue::from(f.value.as_str())),
                    (Name::new("zone"), GqlValue::from(zone_str(&f.zone))),
                ]
                .into_iter()
                .collect(),
            )
        })
        .collect()
}

fn build_links_list(z: &ParsedDoogat) -> Vec<GqlValue> {
    z.links
        .iter()
        .map(|l| {
            let mut obj = IndexMap::new();
            obj.insert(Name::new("target"), GqlValue::from(l.target.as_str()));
            obj.insert(
                Name::new("display"),
                l.display
                    .as_deref()
                    .map(GqlValue::from)
                    .unwrap_or(GqlValue::Null),
            );
            obj.insert(Name::new("zone"), GqlValue::from(zone_str(&l.zone)));
            obj.insert(Name::new("kind"), GqlValue::from(l.kind.as_str()));
            obj.insert(
                Name::new("section"),
                l.section
                    .as_deref()
                    .map(GqlValue::from)
                    .unwrap_or(GqlValue::Null),
            );
            GqlValue::Object(obj)
        })
        .collect()
}

fn build_attachments_list(z: &ParsedDoogat) -> Vec<GqlValue> {
    let Some(Value::List(items)) = z.meta.extra.get("attachments") else {
        return Vec::new();
    };
    let zid = z.meta.id.as_ref().map(|i| i.0.as_str()).unwrap_or("");
    items
        .iter()
        .filter_map(|item| {
            let Value::Map(map) = item else { return None };
            let name = map.get("name")?.as_str()?;
            let mime = map
                .get("mime")
                .and_then(|v| v.as_str())
                .unwrap_or("application/octet-stream");
            let size = map.get("size").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
            let url = format!("/attachments/{zid}/{name}");
            let mut a = IndexMap::new();
            a.insert(Name::new("name"), GqlValue::from(name));
            a.insert(Name::new("mime"), GqlValue::from(mime));
            a.insert(Name::new("size"), GqlValue::from(size));
            a.insert(Name::new("url"), GqlValue::from(url.as_str()));
            Some(GqlValue::Object(a))
        })
        .collect()
}

pub(crate) fn doogat_to_value(z: &ParsedDoogat) -> GqlValue {
    let id = z.meta.id.as_ref().map(|i| i.0.as_str()).unwrap_or("");
    let title = z.meta.title.as_deref().unwrap_or("");
    let date = z.meta.date.as_deref().unwrap_or("");
    let ztype = z.meta.doogat_type.as_deref().unwrap_or("");

    let mut obj = IndexMap::new();
    obj.insert(Name::new("id"), GqlValue::from(id));
    obj.insert(Name::new("title"), GqlValue::from(title));
    obj.insert(Name::new("date"), GqlValue::from(date));
    obj.insert(Name::new("type"), GqlValue::from(ztype));
    obj.insert(Name::new("tags"), GqlValue::List(build_tags_list(z)));
    obj.insert(Name::new("body"), GqlValue::from(z.body.as_str()));
    obj.insert(Name::new("path"), GqlValue::from(z.path.as_str()));
    obj.insert(
        Name::new("fields"),
        GqlValue::List(build_inline_fields_list(z)),
    );
    obj.insert(Name::new("links"), GqlValue::List(build_links_list(z)));
    obj.insert(
        Name::new("attachments"),
        GqlValue::List(build_attachments_list(z)),
    );
    obj.insert(
        Name::new("updated_at"),
        z.updated_at
            .as_deref()
            .map(GqlValue::from)
            .unwrap_or(GqlValue::Null),
    );
    obj.insert(
        Name::new("created_at"),
        if date.is_empty() {
            GqlValue::Null
        } else {
            GqlValue::from(date)
        },
    );

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
    obj.insert(
        Name::new("updated_at"),
        if r.updated_at.is_empty() {
            GqlValue::Null
        } else {
            GqlValue::from(r.updated_at.as_str())
        },
    );
    obj.insert(
        Name::new("tags"),
        GqlValue::List(r.tags.iter().map(|t| GqlValue::from(t.as_str())).collect()),
    );
    obj.insert(
        Name::new("type"),
        match &r.doogat_type {
            Some(t) => GqlValue::from(t.as_str()),
            None => GqlValue::Null,
        },
    );
    obj.insert(
        Name::new("fields"),
        match &r.fields {
            Some(f) => {
                let field_map: IndexMap<Name, GqlValue> = f
                    .iter()
                    .map(|(k, v)| (Name::new(k), GqlValue::from(v.as_str())))
                    .collect();
                GqlValue::Object(field_map)
            }
            None => GqlValue::Null,
        },
    );
    obj.insert(
        Name::new("created_at"),
        match &r.created_at {
            Some(d) => GqlValue::from(d.as_str()),
            None => GqlValue::Null,
        },
    );
    GqlValue::Object(obj)
}

pub(crate) fn tag_info_to_value(name: &str, count: i64) -> GqlValue {
    let mut obj = IndexMap::new();
    obj.insert(Name::new("name"), GqlValue::from(name));
    obj.insert(Name::new("count"), GqlValue::from(count));
    GqlValue::Object(obj)
}

pub(crate) fn tag_entry_to_value(entry: &ddb_core::types::TagEntry) -> GqlValue {
    let mut obj = IndexMap::new();
    obj.insert(
        Name::new("doogatId"),
        GqlValue::from(entry.doogat_id.as_str()),
    );
    obj.insert(Name::new("tag"), GqlValue::from(entry.tag.as_str()));
    obj.insert(Name::new("source"), GqlValue::from(entry.source.as_str()));
    GqlValue::Object(obj)
}

pub(crate) fn sql_result_to_value(r: &SqlResult, format: &str) -> GqlValue {
    let mut obj = IndexMap::new();
    match r {
        SqlResult::Rows { columns, rows, .. } => {
            let gql_cols: Vec<GqlValue> =
                columns.iter().map(|c| GqlValue::from(c.as_str())).collect();
            obj.insert(Name::new("columns"), GqlValue::List(gql_cols));
            let gql_rows: Vec<GqlValue> = if format == "objects" {
                rows.iter()
                    .map(|row| {
                        let obj: serde_json::Map<String, serde_json::Value> = columns
                            .iter()
                            .zip(row.iter())
                            .map(|(col, val)| (col.clone(), serde_json::Value::String(val.clone())))
                            .collect();
                        let json = serde_json::to_string(&obj).unwrap_or_default();
                        GqlValue::from(json)
                    })
                    .collect()
            } else {
                rows.iter()
                    .map(|row| {
                        let json = serde_json::to_string(row).unwrap_or_default();
                        GqlValue::from(json)
                    })
                    .collect()
            };
            obj.insert(Name::new("rows"), GqlValue::List(gql_rows));
            obj.insert(Name::new("affected"), GqlValue::Null);
            obj.insert(Name::new("message"), GqlValue::Null);
        }
        SqlResult::Affected(n) => {
            obj.insert(Name::new("columns"), GqlValue::List(vec![]));
            obj.insert(Name::new("rows"), GqlValue::List(vec![]));
            obj.insert(Name::new("affected"), GqlValue::from(*n as i64));
            obj.insert(Name::new("message"), GqlValue::Null);
        }
        SqlResult::Ok(msg) => {
            obj.insert(Name::new("columns"), GqlValue::List(vec![]));
            obj.insert(Name::new("rows"), GqlValue::List(vec![]));
            obj.insert(Name::new("affected"), GqlValue::Null);
            obj.insert(Name::new("message"), GqlValue::from(msg.as_str()));
        }
    }
    GqlValue::Object(obj)
}

fn column_def_to_value(c: &ColumnDef) -> GqlValue {
    let mut obj = IndexMap::new();
    obj.insert(Name::new("name"), GqlValue::from(c.name.as_str()));
    obj.insert(Name::new("dataType"), GqlValue::from(c.data_type.as_str()));
    obj.insert(
        Name::new("zone"),
        c.zone
            .as_ref()
            .map(|z| GqlValue::from(zone_str(z)))
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
            .map(|vals| GqlValue::List(vals.iter().map(|v| GqlValue::from(v.as_str())).collect()))
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
}

pub(crate) fn typedef_to_value(s: &TableSchema) -> GqlValue {
    let columns: Vec<GqlValue> = s.columns.iter().map(column_def_to_value).collect();
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

    // Add typed columns. Skip any column whose name collides with a base
    // doogat field (id, title, date, type, tags, body, path, fields, links,
    // attachments, updated_at, created_at): those are populated by
    // `doogat_to_value` above from `meta.*` / structural fields, and
    // `extract_typed_field` would look them up in `meta.extra`, find
    // nothing, and clobber the base value with NULL.
    //
    // After PRD 00122, `title` (and conceivably others) can appear in
    // `schema.columns` so the SQL validator enforces declared constraints
    // (e.g. `title VARCHAR(255) NOT NULL`). The base field still resolves
    // via the parent value, so users can query `title` without losing the
    // typed-column constraint enforcement on the write path.
    for col in &schema.columns {
        if BASE_DOOGAT_FIELDS.contains(&col.name.as_str()) {
            continue;
        }
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
            obj.insert(Name::new(pluralize(&col.name)), GqlValue::List(ref_values));
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
