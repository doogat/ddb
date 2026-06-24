use async_graphql::dynamic::*;
use async_graphql::Value as GqlValue;

use crate::schema::base_types::*;

/// Input object for `attachFile`. Registered on the schema separately from the
/// Mutation object (pushed onto `dynamic_inputs` in `schema/mod.rs`).
pub(super) fn build_attach_input() -> InputObject {
    InputObject::new("AttachFileInput")
        .description("Input for attaching a file to a doogat.")
        .field(InputValue::new("doogatId", TypeRef::named_nn(TypeRef::ID)))
        .field(InputValue::new(
            "filename",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "dataBase64",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .field(InputValue::new("mime", TypeRef::named(TypeRef::STRING)))
}

// -- SingletonConflict output type (PRD 00139 cycle-3 #4) --
pub(super) fn build_singleton_conflict_type() -> Object {
    Object::new("SingletonConflictResolution")
        .description(
            "One SINGLETON conflict resolved by the post-merge sweep. \
             `winner` materializes in the typed table; each `losers` id \
             is moved to ddb/_conflicts/<id>.md.",
        )
        .field(simple_field("table", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("winner", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field(
            "losers",
            TypeRef::List(Box::new(TypeRef::named_nn(TypeRef::STRING))),
        ))
}

// -- SyncResult output type --
pub(super) fn build_sync_result_type() -> Object {
    Object::new("SyncResult")
        .description("Result of a sync operation with a remote repository.")
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
        ))
        // PRD 00139 cycle-3 #4: surface SINGLETON post-sync sweep detail.
        .field(simple_field(
            "singletonConflictsResolved",
            TypeRef::named_nn(TypeRef::INT),
        ))
        .field(simple_field(
            "singletonConflicts",
            TypeRef::List(Box::new(TypeRef::named_nn("SingletonConflictResolution"))),
        ))
}

// -- CompactResult output type --
pub(super) fn build_compact_result_type() -> Object {
    Object::new("CompactResult")
        .description("Result of CRDT compaction and git garbage collection.")
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
        .field(simple_field("backupPath", TypeRef::named(TypeRef::STRING)))
}

// -- GitMaintenanceResult output type --
pub(super) fn build_git_maintenance_result_type() -> Object {
    Object::new("GitMaintenanceResult")
        .description("Result of git maintenance tasks (gc, repack, commit-graph).")
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
        ))
}

pub(super) fn build_upsert_result_type() -> Object {
    Object::new("UpsertResult")
        .description("Result of a singleton upsert operation.")
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("created", TypeRef::named_nn(TypeRef::BOOLEAN)))
}

// -- SchemaApplyReport + PlanOpReport output types (PRD 00161 T6) --
pub(super) fn build_plan_op_report_type() -> Object {
    Object::new("PlanOpReport")
        .description("A single planned schema-diff operation with its rendered DDL.")
        .field(simple_field("kind", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("table", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("detail", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field(
            "destructive",
            TypeRef::named_nn(TypeRef::BOOLEAN),
        ))
        .field(simple_field("sql", TypeRef::named_nn(TypeRef::STRING)))
}

pub(super) fn build_schema_apply_report_type() -> Object {
    Object::new("SchemaApplyReport")
        .description("Result of applying a declarative desired-schema document.")
        .field(simple_field("dryRun", TypeRef::named_nn(TypeRef::BOOLEAN)))
        .field(simple_field("applied", TypeRef::named_nn(TypeRef::BOOLEAN)))
        .field(simple_field(
            "ops",
            TypeRef::named_nn_list_nn("PlanOpReport"),
        ))
        .field(simple_field(
            "unsupported",
            TypeRef::named_nn_list_nn(TypeRef::STRING),
        ))
}
