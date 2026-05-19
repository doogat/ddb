use std::path::PathBuf;

use ddb_core::service::DoogatService;
use ddb_core::types::{BatchCreateInput, CompactOptions, ListFilter};

use super::{ActorCommand, ActorReply};

pub(crate) fn handle_command(svc: &mut DoogatService, cmd: ActorCommand) -> ActorReply {
    match cmd {
        ActorCommand::NoSqlGet { id } => ActorReply::NoSqlDoogat(Box::new(svc.nosql_get(&id))),
        ActorCommand::NoSqlScanType { type_name } => {
            ActorReply::NoSqlIds(svc.nosql_scan_type(&type_name))
        }
        ActorCommand::NoSqlScanTag { tag } => ActorReply::NoSqlIds(svc.nosql_scan_tag(&tag)),
        ActorCommand::NoSqlBacklinks { id } => ActorReply::NoSqlIds(svc.nosql_backlinks(&id)),
        ActorCommand::GetDoogat { id } => ActorReply::Doogat(Box::new(svc.get_doogat_parsed(&id))),
        ActorCommand::ListDoogats {
            doogat_type,
            tag,
            backlinks_of,
            field_filters,
            limit,
            offset,
        } => ActorReply::DoogatList(svc.list_doogats_filtered(&ListFilter {
            doogat_type,
            tag,
            backlinks_of,
            field_filters,
            limit,
            offset,
            ..Default::default()
        })),
        ActorCommand::Search {
            query,
            limit,
            offset,
            filters,
        } => ActorReply::SearchResults(
            svc.search_paginated_filtered(&query, limit, offset, &filters),
        ),
        ActorCommand::CreateDoogat {
            title,
            body,
            tags,
            doogat_type,
            fields,
            on_conflict,
        } => {
            let input = BatchCreateInput {
                title,
                body,
                tags,
                doogat_type,
                fields,
                on_conflict,
            };
            let result = svc.batch_create(&[input]).and_then(|mut v| {
                v.pop().ok_or_else(|| {
                    ddb_core::error::DoogatError::Validation("no doogat created".into())
                })
            });
            ActorReply::Doogat(Box::new(result))
        }
        ActorCommand::UpdateDoogat {
            id,
            title,
            body,
            tags,
            doogat_type,
            fields,
            unset_fields,
        } => {
            let extra = ddb_core::service::ExtraFieldUpdates {
                set: &fields,
                unset: &unset_fields,
            };
            let result = svc.update_doogat_parsed(
                &id,
                title.as_deref(),
                tags.as_deref(),
                doogat_type.as_deref(),
                body.as_deref(),
                &extra,
            );
            ActorReply::Doogat(Box::new(result))
        }
        ActorCommand::BatchUpdate { updates } => ActorReply::DoogatList(svc.batch_update(&updates)),
        ActorCommand::CreateMany { inputs } => ActorReply::DoogatList(svc.batch_create(&inputs)),
        ActorCommand::DeleteDoogat { id } => ActorReply::Deleted(
            svc.delete_doogat(&id, &format!("delete doogat {id}"))
                .map(|_broken| ()),
        ),
        ActorCommand::ExecuteSql { sql } => ActorReply::SqlResult(svc.execute_sql(&sql)),
        ActorCommand::ExecuteBatch { statements } => {
            let combined = statements.join(";\n");
            ActorReply::SqlResults(svc.execute_batch(&combined))
        }
        ActorCommand::GetTypeSchemas => ActorReply::TypeSchemas(svc.list_type_schemas()),
        ActorCommand::GetBacklinks { id } => ActorReply::Backlinks(svc.backlink_ids(&id)),
        ActorCommand::CountDoogats {
            doogat_type,
            tag,
            backlinks_of,
            field_filters,
        } => ActorReply::Count(svc.count_doogats_filtered(&ListFilter {
            doogat_type,
            tag,
            backlinks_of,
            field_filters,
            limit: None,
            offset: None,
            ..Default::default()
        })),
        ActorCommand::FilteredList(q) => ActorReply::DoogatList(svc.typed_filtered_list(&q)),
        ActorCommand::AggregateQuery { sql, params } => {
            ActorReply::AggregateRow(svc.aggregate_query(&sql, &params))
        }
        ActorCommand::AttachFile {
            doogat_id,
            filename,
            bytes,
            mime,
        } => ActorReply::Attachment(svc.attach_file(&doogat_id, &filename, &bytes, &mime)),
        ActorCommand::DetachFile {
            doogat_id,
            filename,
        } => ActorReply::Deleted(svc.detach_file(&doogat_id, &filename)),
        ActorCommand::ListAttachments { doogat_id } => {
            ActorReply::AttachmentList(svc.list_attachments(&doogat_id))
        }
        ActorCommand::Compact {
            force,
            no_backup,
            backup_path,
        } => {
            let result = svc.compact(&CompactOptions {
                force,
                skip_backup: no_backup,
                backup_path: backup_path.map(PathBuf::from),
            });
            if result.is_ok() {
                if let Err(e) = svc.rebuild_if_stale() {
                    tracing::warn!(%e, "actor: index rebuild after compact failed");
                }
            }
            ActorReply::Maintenance(result)
        }
        ActorCommand::GitMaintenance { task } => {
            let tasks_vec: Vec<&str>;
            let tasks_opt = match &task {
                Some(t) => {
                    tasks_vec = vec![t.as_str()];
                    Some(tasks_vec.as_slice())
                }
                None => None,
            };
            ActorReply::GitMaintenance(svc.run_maintenance(tasks_opt))
        }
        ActorCommand::Sync { remote, branch } => {
            let result = svc.sync(&remote, &branch);
            if result.is_ok() {
                if let Err(e) = svc.rebuild_if_stale() {
                    tracing::warn!(%e, "actor: index rebuild after sync failed");
                }
            }
            ActorReply::SyncResult(result)
        }
        ActorCommand::UnlinkedMentions { id } => {
            ActorReply::UnlinkedMentions(svc.unlinked_mentions(&id))
        }
        ActorCommand::SuggestLinks { id, limit } => {
            ActorReply::Suggestions(svc.suggest_links(&id, limit))
        }
        ActorCommand::StaleDoogats { type_filter } => {
            ActorReply::StaleDoogats(svc.stale_doogats(type_filter.as_deref()))
        }
        ActorCommand::OrphanDoogats { type_filter } => {
            ActorReply::OrphanDoogats(svc.orphan_doogats(type_filter.as_deref()))
        }
        ActorCommand::SequenceInfo { id } => ActorReply::SequenceInfoResult(svc.sequence_info(&id)),
        ActorCommand::SequenceChildren { id } => {
            ActorReply::SequenceNodes(svc.sequence_children(&id))
        }
        ActorCommand::SequenceBreadcrumb { id } => {
            ActorReply::SequenceNodes(svc.sequence_breadcrumb(&id))
        }
        ActorCommand::BrokenSequences => ActorReply::BrokenSequences(svc.broken_sequences()),
        ActorCommand::HealthCheck => ActorReply::HealthStatus(svc.health_check()),
        ActorCommand::UpsertSingleton { type_name, fields } => {
            ActorReply::Upsert(svc.upsert_singleton(&type_name, fields))
        }
    }
}
