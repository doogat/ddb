mod eval;
mod identifiers;
mod inline_zone;
mod regexes;
mod template;
mod types;

#[cfg(test)]
mod tests;

pub(super) use eval::{
    eval_values, eval_values_nullable, expr_to_string, is_literal_expr,
    sqlite_value_to_string_nullable, value_to_sql,
};
// `eval_expr` is used only by sql_engine's #[cfg(test)] module.
#[cfg(test)]
pub(super) use eval::eval_expr;
pub(super) use identifiers::{
    extract_from_table, is_reserved_table, unquote_identifier, validate_rename_target_name,
};
pub(super) use inline_zone::{strip_inline_zones, TableColumnZones};
pub(super) use regexes::{
    extract_junction_where, extract_where_id, re_create_table_singleton, re_drop_search_key,
    re_drop_singleton, re_drop_title_template, re_set_search_key, re_set_singleton,
    re_set_title_template, re_set_zone, re_unfilled_placeholder,
};
pub(super) use template::{is_safe_sql_identifier, parse_title_template};
// `TemplatePlaceholder` is constructed directly only by sql_engine's #[cfg(test)] module.
#[cfg(test)]
pub(super) use template::TemplatePlaceholder;
pub(super) use types::{
    data_type_to_string, extract_allowed_values, extract_default, extract_on_delete,
    extract_references, is_not_null, is_numeric_type, is_short_string_type,
    normalize_alter_column_type, to_yaml_value,
};
