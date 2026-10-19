use std::collections::BTreeMap;

use crate::types::{ColumnDef, DoogatId, DoogatMeta, ParsedDoogat, TableSchema, Value, Zone};

/// Convert a single `ColumnDef` into a YAML-style `Value::Map`.
fn build_column_yaml(col: &ColumnDef) -> Value {
    let mut map = BTreeMap::new();
    map.insert("name".to_string(), Value::String(col.name.clone()));
    map.insert(
        "data_type".to_string(),
        Value::String(col.data_type.clone()),
    );
    if let Some(ref zone) = col.zone {
        let zone_str = match zone {
            Zone::Frontmatter => "frontmatter",
            Zone::Body => "body",
            Zone::Reference => "reference",
        };
        map.insert("zone".to_string(), Value::String(zone_str.into()));
    }
    if col.required {
        map.insert("required".to_string(), Value::Bool(true));
    }
    if let Some(boost) = col.search_boost {
        map.insert("search_boost".to_string(), Value::Number(boost));
    }
    if let Some(ref r) = col.references {
        map.insert("references".to_string(), Value::String(r.clone()));
        // PRD 00129 §2: only emit `on_delete` when it diverges from the
        // RESTRICT default. Keeps existing typedefs unchanged on rewrite
        // and avoids polluting the YAML for the common case.
        if col.on_delete != crate::types::OnDeleteAction::Restrict {
            map.insert(
                "on_delete".to_string(),
                Value::String(col.on_delete.as_typedef_str().into()),
            );
        }
    }
    if let Some(ref vals) = col.allowed_values {
        map.insert(
            "allowed_values".to_string(),
            Value::List(vals.iter().map(|v| Value::String(v.clone())).collect()),
        );
    }
    if let Some(ref default) = col.default_value {
        map.insert("default_value".to_string(), Value::String(default.clone()));
    }
    Value::Map(map)
}

/// Build the `columns` YAML list from a schema's column definitions.
fn build_columns_yaml(schema: &TableSchema) -> Value {
    Value::List(schema.columns.iter().map(build_column_yaml).collect())
}

/// Insert optional typedef extra fields (crdt_strategy, template_sections, etc.) into `extra`.
fn build_typedef_extra_fields(schema: &TableSchema, extra: &mut BTreeMap<String, Value>) {
    if let Some(ref strategy) = schema.crdt_strategy {
        extra.insert("crdt_strategy".to_string(), Value::String(strategy.clone()));
    }
    if !schema.template_sections.is_empty() {
        extra.insert(
            "template_sections".to_string(),
            Value::List(
                schema
                    .template_sections
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if schema.folder {
        extra.insert("folder".to_string(), Value::Bool(true));
    }
    if let Some(ref tt) = schema.title_template {
        extra.insert("title_template".to_string(), Value::String(tt.clone()));
    }
    if let Some(ref o) = schema.origin {
        extra.insert("origin".to_string(), Value::String(o.clone()));
    }
    if let Some(ref sk) = schema.search_key {
        extra.insert("search_key".to_string(), Value::String(sk.clone()));
    }
    if let Some(ref constraints) = schema.unique_together {
        if !constraints.is_empty() {
            let outer = Value::List(
                constraints
                    .iter()
                    .map(|cols| {
                        Value::List(cols.iter().map(|c| Value::String(c.clone())).collect())
                    })
                    .collect(),
            );
            extra.insert("unique_together".to_string(), outer);
        }
    }
    // PRD 00139 §1: emit `singleton: true` only when set, so non-singleton
    // typedef YAMLs remain byte-identical to their pre-PRD shape.
    if schema.singleton {
        extra.insert("singleton".to_string(), Value::Bool(true));
    }
}

/// Build a _typedef doogat from a TableSchema.
pub fn build_typedef_doogat(id: &DoogatId, schema: &TableSchema) -> ParsedDoogat {
    let mut extra = BTreeMap::new();
    extra.insert("columns".to_string(), build_columns_yaml(schema));
    build_typedef_extra_fields(schema, &mut extra);

    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(id.clone()),
            title: Some(schema.table_name.clone()),
            date: None,
            doogat_type: Some("_typedef".into()),
            tags: vec![],
            extra,
        },
        body: String::new(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: format!("ddb/_typedef/{}.md", id.0),
        updated_at: None,
    }
}
