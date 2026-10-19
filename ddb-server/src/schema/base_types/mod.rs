use std::collections::HashMap;
use std::sync::Arc;

use ddb_core::types::TableSchema;

mod naming;
mod schema_object;
mod to_gql;
mod validation;

#[cfg(test)]
mod tests;

pub(crate) use naming::{
    pluralize_preserving_case, resolve_column, sanitize_field_name, sanitize_type_name,
    singleton_field_base,
};
pub(crate) use schema_object::{
    add_typed_doogat_accessors, build_typed_object, doogat_object, obj_field, simple_field,
};
pub(crate) use to_gql::{
    checkbox_row_to_value, decode_json_fields_object, doogat_to_value, search_hit_to_value,
    sql_result_to_value, tag_entry_to_value, tag_info_to_value, typed_doogat_to_value,
    typedef_to_value,
};
pub(crate) use validation::{event_stream, validate_format, validate_limit, validate_offset};

// -- Shared schema data --

/// Map of type name → TableSchema, accessible to resolvers via `ctx.data()`.
#[derive(Clone)]
pub(crate) struct TypeSchemaMap(pub Arc<HashMap<String, TableSchema>>);
