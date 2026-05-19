use async_graphql::dynamic::*;
use async_graphql::{Name, Value as GqlValue};
use base64::engine::general_purpose as base64_engine;
use base64::Engine as _;
use ddb_core::error::DoogatError;
use ddb_core::types::{BatchCreateInput, BatchUpdateInput, ConflictAction, TableSchema};
use indexmap::IndexMap;

use std::sync::Arc;

use crate::actor::{ActorHandle, UpdateDoogatParams};
use crate::error::to_graphql_error;
use crate::read_pool::ReadPool;
use crate::reload::SchemaReloader;

use super::base_types::*;

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

    // createDoogat
    {
        mutation = mutation.field(
            Field::new("createDoogat", TypeRef::named_nn("Doogat"), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let input = ctx.args.try_get("input")?;
                    let input = input.object()?;
                    let title = input
                        .get("title")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
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
                    let doogat_type = input
                        .get("type")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
                    let fields = match input.get("fields").and_then(|v| v.string().ok()) {
                        Some(json_str) => parse_fields_json(json_str)
                            .map_err(|msg| async_graphql::ServerError::new(msg, None))?,
                        None => std::collections::BTreeMap::new(),
                    };
                    let on_conflict = match ctx
                        .args
                        .get("onConflict")
                        .map(|v| v.enum_name().ok().map(|s| s.to_string()))
                    {
                        Some(Some(ref s)) if s == "IGNORE" => ConflictAction::Ignore,
                        _ => ConflictAction::Error,
                    };
                    let z = a
                        .create_doogat(title, content, tags, doogat_type, fields, on_conflict)
                        .await
                        .map_err(to_graphql_error)?;
                    Ok(Some(FieldValue::owned_any(doogat_to_value(&z))))
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
            .description("Create a new doogat with a title, optional content, tags, and type."),
        );
    }

    // updateDoogat
    {
        mutation = mutation.field(
            Field::new("updateDoogat", TypeRef::named_nn("Doogat"), |ctx| {
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
                    let doogat_type = input
                        .get("type")
                        .and_then(|v| v.string().ok())
                        .map(|s| s.to_string());
                    let fields = match input.get("fields").and_then(|v| v.string().ok()) {
                        Some(json_str) => parse_fields_json(json_str)
                            .map_err(|msg| async_graphql::ServerError::new(msg, None))?,
                        None => std::collections::BTreeMap::new(),
                    };
                    let unset_fields: Vec<String> = input
                        .get("unsetFields")
                        .and_then(|v| v.list().ok())
                        .map(|l| {
                            l.iter()
                                .filter_map(|v| v.string().ok().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let z = a
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
                        .map_err(to_graphql_error)?;
                    Ok(Some(FieldValue::owned_any(doogat_to_value(&z))))
                })
            })
            .argument(
                InputValue::new("input", TypeRef::named_nn("UpdateDoogatInput"))
                    .description("Fields to update on the doogat."),
            )
            .description("Update an existing doogat. Omitted fields are left unchanged."),
        );
    }

    // batchUpdate
    {
        mutation = mutation.field(
            Field::new("batchUpdate", TypeRef::named_nn_list_nn("Doogat"), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let updates_val = ctx.args.try_get("updates")?.list()?;
                    let mut updates = Vec::with_capacity(updates_val.len());
                    for item in updates_val.iter() {
                        let obj = item.object()?;
                        let id = obj.try_get("id")?.string()?.to_string();
                        let title = obj
                            .get("title")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let body = obj
                            .get("content")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let tags = obj.get("tags").and_then(|v| v.list().ok()).map(|l| {
                            l.iter()
                                .filter_map(|v| v.string().ok().map(|s| s.to_string()))
                                .collect()
                        });
                        let doogat_type = obj
                            .get("type")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let fields = match obj.get("fields").and_then(|v| v.string().ok()) {
                            Some(json_str) => Some(
                                parse_fields_json(json_str)
                                    .map_err(|msg| async_graphql::ServerError::new(msg, None))?,
                            ),
                            None => None,
                        };
                        let unset_fields: Option<Vec<String>> =
                            obj.get("unsetFields").and_then(|v| v.list().ok()).map(|l| {
                                l.iter()
                                    .filter_map(|v| v.string().ok().map(|s| s.to_string()))
                                    .collect()
                            });
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
            .description(
                "Update multiple doogats atomically in a single git commit. All succeed or none.",
            )
            .argument(
                InputValue::new("updates", TypeRef::named_nn_list_nn("UpdateDoogatInput"))
                    .description("List of doogats to update atomically."),
            ),
        );
    }

    // createMany
    {
        mutation = mutation.field(
            Field::new("createMany", TypeRef::named_nn_list_nn("Doogat"), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let on_conflict = match ctx
                        .args
                        .get("onConflict")
                        .map(|v| v.enum_name().ok().map(|s| s.to_string()))
                    {
                        Some(Some(ref s)) if s == "IGNORE" => ConflictAction::Ignore,
                        _ => ConflictAction::Error,
                    };
                    let inputs_val = ctx.args.try_get("inputs")?.list()?;
                    let mut inputs = Vec::with_capacity(inputs_val.len());
                    for item in inputs_val.iter() {
                        let obj = item.object()?;
                        let title = obj
                            .get("title")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let body = obj
                            .get("content")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let tags = obj
                            .get("tags")
                            .and_then(|v| v.list().ok())
                            .map(|l| {
                                l.iter()
                                    .filter_map(|v| v.string().ok().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let doogat_type = obj
                            .get("type")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let fields = match obj.get("fields").and_then(|v| v.string().ok()) {
                            Some(json_str) => parse_fields_json(json_str)
                                .map_err(|msg| async_graphql::ServerError::new(msg, None))?,
                            None => std::collections::BTreeMap::new(),
                        };
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
            .description(
                "Create multiple doogats atomically in a single git commit. All succeed or none.",
            )
            .argument(
                InputValue::new("inputs", TypeRef::named_nn_list_nn("CreateManyItemInput"))
                    .description("List of doogats to create atomically."),
            )
            .argument(
                InputValue::new("onConflict", TypeRef::named("ConflictAction"))
                    .description("Action on unique constraint conflict. Defaults to ERROR."),
            ),
        );
    }

    // deleteDoogat
    {
        mutation = mutation.field(
            Field::new("deleteDoogat", TypeRef::named_nn(TypeRef::BOOLEAN), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let id = ctx.args.try_get("id")?.string()?.to_string();
                    a.delete_doogat(id).await.map_err(to_graphql_error)?;
                    Ok(Some(FieldValue::value(GqlValue::from(true))))
                })
            })
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)).description("The ID of the doogat to delete."))
            .description("Delete a doogat by ID. Cascades: removes junction table rows referencing this ID and cleans dangling wikilinks. All changes in a single atomic commit."),
        );
    }

    // attachFile(doogatId, filename, dataBase64, mime?)
    let attach_input = InputObject::new("AttachFileInput")
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
        .field(InputValue::new("mime", TypeRef::named(TypeRef::STRING)));

    {
        mutation = mutation.field(
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
            .description("Attach a file to a doogat. Provide base64-encoded data. MIME type is auto-detected from filename if omitted."),
        );
    }

    // detachFile(doogatId, filename)
    {
        mutation = mutation.field(
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
            .description("Remove an attached file from a doogat."),
        );
    }

    // executeSql
    {
        mutation = mutation.field(
            Field::new("executeSql", TypeRef::named_nn("SqlResult"), |ctx| {
                FieldFuture::new(async move {
                    let a = ctx.data::<ActorHandle>()?;
                    let sql = ctx.args.try_get("sql")?.string()?.to_string();
                    let fmt = validate_format(&ctx)?;
                    let result = a.execute_sql(sql.clone()).await.map_err(to_graphql_error)?;

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
            ),
        );
    }

    // executeBatch
    {
        mutation = mutation.field(
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

                        let has_ddl = statements.iter().any(|s| {
                            let upper = s.to_uppercase();
                            upper.contains("CREATE TABLE")
                                || upper.contains("DROP TABLE")
                                || upper.contains("ALTER TABLE")
                        });

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
            .argument(InputValue::new("format", TypeRef::named(TypeRef::STRING)).description("Response format: 'array' (default) or 'objects'.")),
        );
    }

    for schema in type_schemas {
        if !schema.singleton {
            continue;
        }

        let type_name = sanitize_type_name(&schema.table_name);
        let field_base = singleton_field_base(&schema.table_name);
        let update_field_name = format!("update_{field_base}");
        let upsert_field_name = format!("upsert_{field_base}");
        let table_name = schema.table_name.clone();
        let schema_clone = schema.clone();
        let update_desc = format!(
            "Update the {} singleton row. Rejects with SINGLETON_NOT_FOUND when the typedef is empty.",
            type_name
        );
        let upsert_desc = format!(
            "Upsert the {} singleton row. Returns id plus a created flag indicating whether the row was newly created.",
            type_name
        );

        mutation = mutation.field(
            Field::new(
                &update_field_name,
                TypeRef::named_nn(&type_name),
                move |ctx| {
                    let table_name = table_name.clone();
                    let schema = schema_clone.clone();
                    FieldFuture::new(async move {
                        let a = ctx.data::<ActorHandle>()?;
                        let pool = ctx.data::<ReadPool>()?;
                        // Reuse the existing JSON-string fields transport so singleton
                        // typedef mutations stay aligned with updateDoogat without
                        // generating per-typedef input objects.
                        let fields = parse_fields_json(ctx.args.try_get("input")?.string()?)
                            .map_err(|msg| async_graphql::ServerError::new(msg, None))?;
                        let rows = pool
                            .aggregate_query_rows(
                                format!(
                                    "SELECT id FROM \"{}\" LIMIT 1",
                                    table_name.replace('"', "\"\"")
                                ),
                                Vec::new(),
                            )
                            .await
                            .map_err(to_graphql_error)?;
                        let id = rows
                            .first()
                            .and_then(|row| row.first())
                            .cloned()
                            .ok_or_else(|| {
                                to_graphql_error(DoogatError::singleton_not_found(&table_name))
                            })?;
                        let z = a
                            .update_doogat(UpdateDoogatParams {
                                id,
                                title: None,
                                body: None,
                                tags: None,
                                doogat_type: None,
                                fields,
                                unset_fields: vec![],
                            })
                            .await
                            .map_err(to_graphql_error)?;
                        Ok(Some(FieldValue::owned_any(typed_doogat_to_value(
                            &z, &schema,
                        ))))
                    })
                },
            )
            .argument(
                InputValue::new("input", TypeRef::named_nn(TypeRef::STRING))
                    .description("JSON object of typed field values to update."),
            )
            .description(&update_desc),
        );

        let table_name = schema.table_name.clone();
        mutation = mutation.field(
            Field::new(
                &upsert_field_name,
                TypeRef::named_nn("UpsertResult"),
                move |ctx| {
                    let table_name = table_name.clone();
                    FieldFuture::new(async move {
                        let a = ctx.data::<ActorHandle>()?;
                        let fields = parse_fields_json(ctx.args.try_get("input")?.string()?)
                            .map_err(|msg| async_graphql::ServerError::new(msg, None))?;
                        let outcome = a
                            .upsert_singleton(table_name.clone(), fields)
                            .await
                            .map_err(to_graphql_error)?;
                        let mut obj = IndexMap::new();
                        obj.insert(Name::new("id"), GqlValue::String(outcome.id));
                        obj.insert(Name::new("created"), GqlValue::Boolean(outcome.created));
                        Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
                    })
                },
            )
            .argument(
                InputValue::new("input", TypeRef::named_nn(TypeRef::STRING))
                    .description("JSON object of typed field values to upsert."),
            )
            .description(&upsert_desc),
        );
    }

    // -- SingletonConflict output type (PRD 00139 cycle-3 #4) --
    let singleton_conflict_type = Object::new("SingletonConflictResolution")
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
        ));

    // -- SyncResult output type --
    let sync_result_type = Object::new("SyncResult")
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
        ));

    // -- CompactResult output type --
    let compact_result_type = Object::new("CompactResult")
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
            .description("Sync with a remote git repository. Pushes local commits, pulls remote changes, and resolves conflicts via CRDT."),
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
            .description("Run CRDT compaction and git garbage collection. Reduces repository size by merging CRDT temp files and pruning unreachable objects."),
        );
    }

    // -- GitMaintenanceResult output type --
    let git_maintenance_result_type = Object::new("GitMaintenanceResult")
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
            .description("Run git maintenance tasks (gc, repack, commit-graph). Optionally specify a single task to run."),
        );
    }

    let upsert_result_type = build_upsert_result_type();

    MutationOutput {
        mutation,
        sync_result_type,
        singleton_conflict_type,
        compact_result_type,
        git_maintenance_result_type,
        upsert_result_type,
        attach_input,
    }
}

fn build_upsert_result_type() -> Object {
    Object::new("UpsertResult")
        .description("Result of a singleton upsert operation.")
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("created", TypeRef::named_nn(TypeRef::BOOLEAN)))
}
