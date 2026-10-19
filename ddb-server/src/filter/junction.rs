use async_graphql::Value as GqlValue;
use ddb_core::types::QueryValue;

use super::where_build::WhereCtx;

/// PRD 00129 §5 + issue #11: build the SQL fragment for a `TagsFilter`
/// value, validating that the caller supplied at least one operator and
/// no empty lists.
///
/// Returns the EXISTS clause that joins the materialized type table
/// against the doogat-level `_ddb_tags` index. All values are bound
/// parameters (never interpolated).
///
/// Errors:
/// - `tags filter requires at least one of: contains, containsAll, containsAny`
///   when no operator is supplied (or none with a usable value).
/// - `containsAll cannot be empty` / `containsAny cannot be empty` when
///   the array is `[]` (silently matching nothing was the trap).
pub(super) fn build_tags_filter_condition(
    value: &GqlValue,
    ctx: &mut WhereCtx,
) -> Result<Option<String>, String> {
    let GqlValue::Object(filter) = value else {
        return Ok(None);
    };

    let table_name = ctx.row_ref().to_string();
    let mut clauses = Vec::new();
    let mut any_operator_supplied = false;
    for (op, val) in filter {
        let op_str = op.as_str();
        if matches!(val, GqlValue::Null) {
            continue;
        }
        match op_str {
            "contains" => {
                let GqlValue::String(tag) = val else { continue };
                any_operator_supplied = true;
                ctx.params.push(QueryValue::Text(tag.clone()));
                clauses.push(format!(
                    "EXISTS (SELECT 1 FROM _ddb_tags _t WHERE _t.doogat_id = \"{table}\".id AND _t.tag = ?)",
                    table = table_name,
                ));
            }
            "containsAll" => {
                let GqlValue::List(items) = val else { continue };
                any_operator_supplied = true;
                if items.is_empty() {
                    return Err("containsAll cannot be empty".to_string());
                }
                for item in items {
                    let GqlValue::String(tag) = item else {
                        continue;
                    };
                    ctx.params.push(QueryValue::Text(tag.clone()));
                    clauses.push(format!(
                        "EXISTS (SELECT 1 FROM _ddb_tags _t WHERE _t.doogat_id = \"{table}\".id AND _t.tag = ?)",
                        table = table_name,
                    ));
                }
            }
            "containsAny" => {
                let GqlValue::List(items) = val else { continue };
                any_operator_supplied = true;
                if items.is_empty() {
                    return Err("containsAny cannot be empty".to_string());
                }
                let mut placeholders = Vec::with_capacity(items.len());
                for item in items {
                    let GqlValue::String(tag) = item else {
                        continue;
                    };
                    ctx.params.push(QueryValue::Text(tag.clone()));
                    placeholders.push("?".to_string());
                }
                if placeholders.is_empty() {
                    continue;
                }
                clauses.push(format!(
                    "EXISTS (SELECT 1 FROM _ddb_tags _t WHERE _t.doogat_id = \"{table}\".id AND _t.tag IN ({tags}))",
                    table = table_name,
                    tags = placeholders.join(", "),
                ));
            }
            _ => {}
        }
    }

    if !any_operator_supplied {
        return Err(
            "tags filter requires at least one of: contains, containsAll, containsAny".to_string(),
        );
    }

    if clauses.is_empty() {
        Ok(None)
    } else {
        Ok(Some(format!("({})", clauses.join(" AND "))))
    }
}
