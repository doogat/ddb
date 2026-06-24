use async_graphql::dynamic::*;
use async_graphql::{Name, Value as GqlValue};
use base64::engine::general_purpose as base64_engine;
use base64::Engine as _;
use ddb_core::schema_diff::plan::SchemaApplyReport;
use ddb_core::sql_engine::requires_schema_reload;
use ddb_core::types::{BatchCreateInput, BatchUpdateInput};
use indexmap::IndexMap;

use std::sync::Arc;

use crate::actor::ActorHandle;
use crate::error::{to_graphql_error, to_graphql_error_from_app};
use crate::reload::SchemaReloader;
use crate::warning_extension::forward_warnings;

use crate::schema::base_types::*;
use crate::schema::input::{
    conflict_action, fields_map, opt_fields_map, opt_string, opt_string_list, string_list,
};

pub(super) fn build_create_doogat_field() -> Field {
    Field::new("createDoogat", TypeRef::named_nn("Doogat"), |ctx| {
        FieldFuture::new(async move {
            let a = ctx.data::<ActorHandle>()?;
            let input = ctx.args.try_get("input")?;
            let input = input.object()?;
            let map = input.as_index_map();
            let title = opt_string(map, "title");
            let content = opt_string(map, "content");
            let tags = string_list(map, "tags");
            let doogat_type = opt_string(map, "type");
            let fields =
                fields_map(map).map_err(|msg| async_graphql::ServerError::new(msg, None))?;
            let on_conflict = conflict_action(ctx.args.as_index_map());
            let output = a
                .create_doogat(title, content, tags, doogat_type, fields, on_conflict)
                .await
                .map_err(|e| to_graphql_error_from_app(e.into()))?;
            forward_warnings(&ctx, &output.warnings);
            Ok(Some(FieldValue::owned_any(doogat_to_value(&output.value))))
        })
    })
    .argument(
        InputValue::new("input", TypeRef::named_nn("CreateDoogatInput"))
            .description("The doogat to create."),
    )
    .argument(
        InputValue::new("onConflict", TypeRef::named("ConflictAction"))
            .description("Action on unique constraint conflict. Defaults to ERROR."),
    )
    .description("Create a new doogat with a title, optional content, tags, and type.")
}

pub(super) fn build_update_doogat_field() -> Field {
    Field::new("updateDoogat", TypeRef::named_nn("Doogat"), |ctx| {
        FieldFuture::new(async move {
            let a = ctx.data::<ActorHandle>()?;
            let input = ctx.args.try_get("input")?;
            let input = input.object()?;
            let id = input.try_get("id")?.string()?.to_string();
            let map = input.as_index_map();
            let title = opt_string(map, "title");
            let content = opt_string(map, "content");
            let tags = opt_string_list(map, "tags");
            let doogat_type = opt_string(map, "type");
            let fields =
                fields_map(map).map_err(|msg| async_graphql::ServerError::new(msg, None))?;
            let unset_fields = string_list(map, "unsetFields");
            let output = a
                .update_doogat(crate::actor::UpdateDoogatParams {
                    id,
                    title,
                    body: content,
                    tags,
                    doogat_type,
                    fields,
                    unset_fields,
                })
                .await
                .map_err(|e| to_graphql_error_from_app(e.into()))?;
            forward_warnings(&ctx, &output.warnings);
            Ok(Some(FieldValue::owned_any(doogat_to_value(&output.value))))
        })
    })
    .argument(
        InputValue::new("input", TypeRef::named_nn("UpdateDoogatInput"))
            .description("Fields to update on the doogat."),
    )
    .description("Update an existing doogat. Omitted fields are left unchanged.")
}

pub(super) fn build_batch_update_field() -> Field {
    Field::new("batchUpdate", TypeRef::named_nn_list_nn("Doogat"), |ctx| {
        FieldFuture::new(async move {
            let a = ctx.data::<ActorHandle>()?;
            let updates_val = ctx.args.try_get("updates")?.list()?;
            let mut updates = Vec::with_capacity(updates_val.len());
            for item in updates_val.iter() {
                let obj = item.object()?;
                let id = obj.try_get("id")?.string()?.to_string();
                let map = obj.as_index_map();
                let title = opt_string(map, "title");
                let body = opt_string(map, "content");
                let tags = opt_string_list(map, "tags");
                let doogat_type = opt_string(map, "type");
                let fields = opt_fields_map(map)
                    .map_err(|msg| async_graphql::ServerError::new(msg, None))?;
                let unset_fields = opt_string_list(map, "unsetFields");
                updates.push(BatchUpdateInput {
                    id,
                    title,
                    body,
                    tags,
                    doogat_type,
                    fields,
                    unset_fields,
                });
            }
            let results = a.batch_update(updates).await.map_err(to_graphql_error)?;
            Ok(Some(FieldValue::list(
                results
                    .iter()
                    .map(|z| FieldValue::owned_any(doogat_to_value(z))),
            )))
        })
    })
    .description("Update multiple doogats atomically in a single git commit. All succeed or none.")
    .argument(
        InputValue::new("updates", TypeRef::named_nn_list_nn("UpdateDoogatInput"))
            .description("List of doogats to update atomically."),
    )
}

pub(super) fn build_create_many_field() -> Field {
    Field::new("createMany", TypeRef::named_nn_list_nn("Doogat"), |ctx| {
        FieldFuture::new(async move {
            let a = ctx.data::<ActorHandle>()?;
            let on_conflict = conflict_action(ctx.args.as_index_map());
            let inputs_val = ctx.args.try_get("inputs")?.list()?;
            let mut inputs = Vec::with_capacity(inputs_val.len());
            for item in inputs_val.iter() {
                let obj = item.object()?;
                let map = obj.as_index_map();
                let title = opt_string(map, "title");
                let body = opt_string(map, "content");
                let tags = string_list(map, "tags");
                let doogat_type = opt_string(map, "type");
                let fields =
                    fields_map(map).map_err(|msg| async_graphql::ServerError::new(msg, None))?;
                inputs.push(BatchCreateInput {
                    title,
                    body,
                    tags,
                    doogat_type,
                    fields,
                    on_conflict,
                });
            }
            let results = a.create_many(inputs).await.map_err(to_graphql_error)?;
            Ok(Some(FieldValue::list(
                results
                    .iter()
                    .map(|z| FieldValue::owned_any(doogat_to_value(z))),
            )))
        })
    })
    .description("Create multiple doogats atomically in a single git commit. All succeed or none.")
    .argument(
        InputValue::new("inputs", TypeRef::named_nn_list_nn("CreateManyItemInput"))
            .description("List of doogats to create atomically."),
    )
    .argument(
        InputValue::new("onConflict", TypeRef::named("ConflictAction"))
            .description("Action on unique constraint conflict. Defaults to ERROR."),
    )
}

pub(super) fn build_delete_doogat_field() -> Field {
    Field::new("deleteDoogat", TypeRef::named_nn(TypeRef::BOOLEAN), |ctx| {
        FieldFuture::new(async move {
            let a = ctx.data::<ActorHandle>()?;
            let id = ctx.args.try_get("id")?.string()?.to_string();
            a.delete_doogat(id).await.map_err(to_graphql_error)?;
            Ok(Some(FieldValue::value(GqlValue::from(true))))
        })
    })
    .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)).description("The ID of the doogat to delete."))
    .description("Delete a doogat by ID. Cascades: removes junction table rows referencing this ID and cleans dangling wikilinks. All changes in a single atomic commit.")
}

pub(super) fn build_attach_file_field() -> Field {
    Field::new("attachFile", TypeRef::named_nn("Attachment"), |ctx| {
        FieldFuture::new(async move {
            let a = ctx.data::<ActorHandle>()?;
            let input = ctx.args.try_get("input")?;
            let input = input.object()?;
            let doogat_id = input.try_get("doogatId")?.string()?.to_string();
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
                    ddb_core::types::AttachmentInfo::mime_from_filename(&filename)
                        .to_string()
                });
            let info = a
                .attach_file(doogat_id, filename, bytes, mime)
                .await
                .map_err(to_graphql_error)?;
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
    ).description("File attachment details."))
    .description("Attach a file to a doogat. Provide base64-encoded data. MIME type is auto-detected from filename if omitted.")
}

pub(super) fn build_detach_file_field() -> Field {
    Field::new("detachFile", TypeRef::named_nn(TypeRef::BOOLEAN), |ctx| {
        FieldFuture::new(async move {
            let a = ctx.data::<ActorHandle>()?;
            let doogat_id = ctx.args.try_get("doogatId")?.string()?.to_string();
            let filename = ctx.args.try_get("filename")?.string()?.to_string();
            a.detach_file(doogat_id, filename)
                .await
                .map_err(to_graphql_error)?;
            Ok(Some(FieldValue::value(GqlValue::from(true))))
        })
    })
    .argument(
        InputValue::new("doogatId", TypeRef::named_nn(TypeRef::ID))
            .description("The doogat to remove the file from."),
    )
    .argument(
        InputValue::new("filename", TypeRef::named_nn(TypeRef::STRING))
            .description("Name of the file to detach."),
    )
    .description("Remove an attached file from a doogat.")
}

pub(super) fn build_execute_sql_field() -> Field {
    Field::new("executeSql", TypeRef::named_nn("SqlResult"), |ctx| {
        FieldFuture::new(async move {
            let a = ctx.data::<ActorHandle>()?;
            let sql = ctx.args.try_get("sql")?.string()?.to_string();
            let fmt = validate_format(&ctx)?;
            let result = a.execute_sql(sql.clone()).await.map_err(to_graphql_error)?;

            // Await schema reload if this was a typedef-mutating statement
            if requires_schema_reload(&sql) {
                if let Ok(reloader) = ctx.data::<Arc<SchemaReloader>>() {
                    reloader.trigger_reload_and_wait().await;
                }
            }

            Ok(Some(FieldValue::owned_any(sql_result_to_value(
                &result, fmt,
            ))))
        })
    })
    .description("Execute a single SQL statement (DDL or DML). DDL triggers schema reload.")
    .argument(
        InputValue::new("sql", TypeRef::named_nn(TypeRef::STRING))
            .description("SQL statement to execute."),
    )
    .argument(
        InputValue::new("format", TypeRef::named(TypeRef::STRING))
            .description("Response format: 'array' (default) or 'objects'."),
    )
}

pub(super) fn build_execute_batch_field() -> Field {
    Field::new(
        "executeBatch",
        TypeRef::named_nn_list_nn("SqlResult"),
        |ctx| {
            FieldFuture::new(async move {
                let a = ctx.data::<ActorHandle>()?;
                let stmts_val = ctx.args.try_get("statements")?.list()?;
                let fmt = validate_format(&ctx)?;
                let statements: Vec<String> = stmts_val
                    .iter()
                    .map(|v| v.string().unwrap_or_default().to_string())
                    .collect();

                let has_ddl = statements.iter().any(|s| requires_schema_reload(s));

                let results =
                    a.execute_batch(statements).await.map_err(to_graphql_error)?;

                if has_ddl {
                    if let Ok(reloader) = ctx.data::<Arc<SchemaReloader>>() {
                        reloader.trigger_reload_and_wait().await;
                    }
                }

                Ok(Some(FieldValue::list(
                    results.iter().map(|r| FieldValue::owned_any(sql_result_to_value(r, fmt))),
                )))
            })
        },
    )
    .description("Execute multiple SQL statements atomically. DML statements run in an implicit transaction: if any fails, all are rolled back. DDL commits immediately and triggers schema reload.")
    .argument(InputValue::new(
        "statements",
        TypeRef::named_nn_list_nn(TypeRef::STRING),
    ).description("SQL statements to execute atomically."))
    .argument(InputValue::new("format", TypeRef::named(TypeRef::STRING)).description("Response format: 'array' (default) or 'objects'."))
}

pub(super) fn build_sync_field() -> Field {
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
            let report = a.sync(remote, branch).await.map_err(to_graphql_error)?;
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
            obj.insert(
                Name::new("collisionsReassigned"),
                GqlValue::from(report.collisions_reassigned as i64),
            );
            // PRD 00139 cycle-3 #4: expose SINGLETON sweep detail
            // (count + per-conflict triples) alongside the existing
            // count fields so GraphQL clients can audit which
            // (table, winner, losers) triples landed during this
            // sync, not just the aggregate count.
            obj.insert(
                Name::new("singletonConflictsResolved"),
                GqlValue::from(report.singleton_conflicts_resolved as i64),
            );
            let singleton_conflicts: Vec<GqlValue> = report
                .singleton_conflicts
                .iter()
                .map(|resolution| {
                    let mut detail = IndexMap::new();
                    detail.insert(
                        Name::new("table"),
                        GqlValue::from(resolution.table.as_str()),
                    );
                    detail.insert(
                        Name::new("winner"),
                        GqlValue::from(resolution.winner.as_str()),
                    );
                    detail.insert(
                        Name::new("losers"),
                        GqlValue::List(
                            resolution
                                .losers
                                .iter()
                                .map(|id| GqlValue::from(id.as_str()))
                                .collect(),
                        ),
                    );
                    GqlValue::Object(detail)
                })
                .collect();
            obj.insert(
                Name::new("singletonConflicts"),
                GqlValue::List(singleton_conflicts),
            );
            Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
        })
    })
    .argument(InputValue::new("remote", TypeRef::named(TypeRef::STRING)).description("Git remote name. Default 'origin'."))
    .argument(InputValue::new("branch", TypeRef::named(TypeRef::STRING)).description("Branch name. Default 'master'."))
    .description("Sync with a remote git repository. Pushes local commits, pulls remote changes, and resolves conflicts via CRDT.")
}

pub(super) fn build_compact_field() -> Field {
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
                .map_err(to_graphql_error)?;
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
    .argument(InputValue::new("force", TypeRef::named(TypeRef::BOOLEAN)).description("Force compaction even if below thresholds."))
    .argument(InputValue::new(
        "noBackup",
        TypeRef::named(TypeRef::BOOLEAN),
    ).description("Skip creating a backup before compaction."))
    .argument(InputValue::new(
        "backupPath",
        TypeRef::named(TypeRef::STRING),
    ).description("Custom backup directory path."))
    .description("Run CRDT compaction and git garbage collection. Reduces repository size by merging CRDT temp files and pruning unreachable objects.")
}

pub(super) fn build_maintenance_field() -> Field {
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
                let report = a.run_maintenance(task).await.map_err(to_graphql_error)?;
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
    .argument(InputValue::new("task", TypeRef::named(TypeRef::STRING)).description("Specific maintenance task to run. Omit to run all."))
    .description("Run git maintenance tasks (gc, repack, commit-graph). Optionally specify a single task to run.")
}

fn schema_apply_report_to_value(report: &SchemaApplyReport) -> GqlValue {
    let ops: Vec<GqlValue> = report
        .ops
        .iter()
        .map(|op| {
            let mut o = IndexMap::new();
            o.insert(Name::new("kind"), GqlValue::from(op.kind.as_str()));
            o.insert(Name::new("table"), GqlValue::from(op.table.as_str()));
            o.insert(Name::new("detail"), GqlValue::from(op.detail.as_str()));
            o.insert(Name::new("destructive"), GqlValue::from(op.destructive));
            o.insert(Name::new("sql"), GqlValue::from(op.sql.as_str()));
            GqlValue::Object(o)
        })
        .collect();
    let unsupported: Vec<GqlValue> = report
        .unsupported
        .iter()
        .map(|s| GqlValue::from(s.as_str()))
        .collect();
    let mut obj = IndexMap::new();
    obj.insert(Name::new("dryRun"), GqlValue::from(report.dry_run));
    obj.insert(Name::new("applied"), GqlValue::from(report.applied));
    obj.insert(Name::new("ops"), GqlValue::List(ops));
    obj.insert(Name::new("unsupported"), GqlValue::List(unsupported));
    GqlValue::Object(obj)
}

pub(super) fn build_apply_schema_field() -> Field {
    Field::new("applySchema", TypeRef::named_nn("SchemaApplyReport"), |ctx| {
        FieldFuture::new(async move {
            let a = ctx.data::<ActorHandle>()?;
            let schema_doc = ctx.args.try_get("schema")?.string()?.to_string();
            let dry_run = ctx
                .args
                .get("dryRun")
                .and_then(|v| v.boolean().ok())
                .unwrap_or(false);
            let allow_destructive = ctx
                .args
                .get("allowDestructive")
                .and_then(|v| v.boolean().ok())
                .unwrap_or(false);
            let output = a
                .apply_schema(schema_doc, dry_run, allow_destructive)
                .await
                .map_err(|e| to_graphql_error_from_app(e.into()))?;
            forward_warnings(&ctx, &output.warnings);
            Ok(Some(FieldValue::owned_any(schema_apply_report_to_value(
                &output.value,
            ))))
        })
    })
    .argument(
        InputValue::new("schema", TypeRef::named_nn(TypeRef::STRING))
            .description("Declarative desired-schema document (YAML)."),
    )
    .argument(
        InputValue::new("dryRun", TypeRef::named(TypeRef::BOOLEAN))
            .description("Plan only without mutating. Defaults to false."),
    )
    .argument(
        InputValue::new("allowDestructive", TypeRef::named(TypeRef::BOOLEAN))
            .description("Permit destructive ops (drop/rename column). Defaults to false."),
    )
    .description("Apply a declarative desired-schema document, diffing it against the live typedefs and applying the resulting plan.")
}
