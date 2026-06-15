use async_graphql::dynamic::{InputObject, Object};
use ddb_core::types::TableSchema;

mod operations;
mod singleton;
mod types;

/// Auxiliary types and inputs produced by the mutation builder that must be
/// registered on the schema alongside the Mutation object itself.
pub(crate) struct MutationOutput {
    pub mutation: Object,
    pub sync_result_type: Object,
    /// PRD 00139 cycle-3 #4: nested type held by SyncResult.singletonConflicts.
    pub singleton_conflict_type: Object,
    pub compact_result_type: Object,
    pub git_maintenance_result_type: Object,
    pub upsert_result_type: Object,
    pub attach_input: InputObject,
}

pub(crate) fn build_mutation_fields(type_schemas: &[TableSchema]) -> MutationOutput {
    let mut mutation = Object::new("Mutation");

    mutation = mutation.field(operations::build_create_doogat_field());
    mutation = mutation.field(operations::build_update_doogat_field());
    mutation = mutation.field(operations::build_batch_update_field());
    mutation = mutation.field(operations::build_create_many_field());
    mutation = mutation.field(operations::build_delete_doogat_field());
    mutation = mutation.field(operations::build_attach_file_field());
    mutation = mutation.field(operations::build_detach_file_field());
    mutation = mutation.field(operations::build_execute_sql_field());
    mutation = mutation.field(operations::build_execute_batch_field());

    for schema in type_schemas {
        if !schema.singleton {
            continue;
        }
        let (update_field, upsert_field) = singleton::build_singleton_fields(schema);
        mutation = mutation.field(update_field);
        mutation = mutation.field(upsert_field);
    }

    mutation = mutation.field(operations::build_sync_field());
    mutation = mutation.field(operations::build_compact_field());
    mutation = mutation.field(operations::build_maintenance_field());

    MutationOutput {
        mutation,
        sync_result_type: types::build_sync_result_type(),
        singleton_conflict_type: types::build_singleton_conflict_type(),
        compact_result_type: types::build_compact_result_type(),
        git_maintenance_result_type: types::build_git_maintenance_result_type(),
        upsert_result_type: types::build_upsert_result_type(),
        attach_input: types::build_attach_input(),
    }
}
