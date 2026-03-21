use std::path::PathBuf;

use chrono::Utc;
use tokio::sync::{mpsc, oneshot};
use zdb_core::error::ZettelError;
use zdb_core::service::ZettelService;
use zdb_core::sql_engine::SqlResult;
use zdb_core::types::{
    BrokenSequence, CompactOptions, CompactionReport, ListFilter, MaintenanceReport, OrphanZettel,
    PaginatedSearchResult, ParsedZettel, SequenceInfo, SequenceNode, StaleZettel, Suggestion,
    SyncReport, TableSchema, UnlinkedMention,
};

use crate::events::{EventBus, EventKind, ZettelEvent};

/// Serializable result from the actor.
pub type ActorResult<T> = Result<T, ZettelError>;

/// Commands the actor understands.
pub enum ActorCommand {
    GetZettel {
        id: String,
    },
    ListZettels {
        zettel_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
        field_filters: Vec<(String, String)>,
        limit: Option<i64>,
        offset: Option<i64>,
    },
    Search {
        query: String,
        limit: usize,
        offset: usize,
    },
    CreateZettel {
        title: String,
        body: Option<String>,
        tags: Vec<String>,
        zettel_type: Option<String>,
    },
    UpdateZettel {
        id: String,
        title: Option<String>,
        body: Option<String>,
        tags: Option<Vec<String>>,
        zettel_type: Option<String>,
    },
    DeleteZettel {
        id: String,
    },
    ExecuteSql {
        sql: String,
    },
    GetTypeSchemas,
    GetBacklinks {
        id: String,
    },
    CountZettels {
        zettel_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
        field_filters: Vec<(String, String)>,
    },
    FilteredList(zdb_core::types::TypedListQuery),
    AggregateQuery {
        sql: String,
        params: Vec<rusqlite::types::Value>,
    },
    AttachFile {
        zettel_id: String,
        filename: String,
        bytes: Vec<u8>,
        mime: String,
    },
    DetachFile {
        zettel_id: String,
        filename: String,
    },
    ListAttachments {
        zettel_id: String,
    },
    Compact {
        force: bool,
        no_backup: bool,
        backup_path: Option<String>,
    },
    GitMaintenance {
        task: Option<String>,
    },
    Sync {
        remote: String,
        branch: String,
    },
    NoSqlGet {
        id: String,
    },
    NoSqlScanType {
        type_name: String,
    },
    NoSqlScanTag {
        tag: String,
    },
    NoSqlBacklinks {
        id: String,
    },
    UnlinkedMentions {
        id: String,
    },
    SuggestLinks {
        id: String,
        limit: usize,
    },
    StaleZettels {
        type_filter: Option<String>,
    },
    OrphanZettels {
        type_filter: Option<String>,
    },
    SequenceInfo {
        id: String,
    },
    SequenceChildren {
        id: String,
    },
    SequenceBreadcrumb {
        id: String,
    },
    BrokenSequences,
    HealthCheck,
}

/// Replies from the actor.
pub enum ActorReply {
    Zettel(Box<ActorResult<ParsedZettel>>),
    ZettelList(ActorResult<Vec<ParsedZettel>>),
    SearchResults(ActorResult<PaginatedSearchResult>),
    SqlResult(ActorResult<SqlResult>),
    TypeSchemas(ActorResult<Vec<TableSchema>>),
    Backlinks(ActorResult<Vec<String>>),
    Deleted(ActorResult<()>),
    Count(ActorResult<i64>),
    /// Single row of string values from an aggregate query.
    AggregateRow(ActorResult<Vec<String>>),
    Attachment(ActorResult<zdb_core::types::AttachmentInfo>),
    AttachmentList(ActorResult<Vec<zdb_core::types::AttachmentInfo>>),
    Maintenance(ActorResult<CompactionReport>),
    GitMaintenance(ActorResult<MaintenanceReport>),
    SyncResult(ActorResult<SyncReport>),
    NoSqlZettel(Box<ActorResult<Option<ParsedZettel>>>),
    NoSqlIds(ActorResult<Vec<String>>),
    UnlinkedMentions(ActorResult<Vec<UnlinkedMention>>),
    Suggestions(ActorResult<Vec<Suggestion>>),
    StaleZettels(ActorResult<Vec<StaleZettel>>),
    OrphanZettels(ActorResult<Vec<OrphanZettel>>),
    SequenceInfoResult(ActorResult<SequenceInfo>),
    SequenceNodes(ActorResult<Vec<SequenceNode>>),
    BrokenSequences(ActorResult<Vec<BrokenSequence>>),
    HealthStatus(ActorResult<bool>),
}

struct ActorMsg {
    cmd: ActorCommand,
    reply: oneshot::Sender<ActorReply>,
}

/// Async handle to the repo actor.
#[derive(Clone)]
pub struct ActorHandle {
    tx: mpsc::Sender<ActorMsg>,
    event_bus: EventBus,
}

impl ActorHandle {
    /// Spawn the actor on a std::thread. Returns the handle for async callers.
    pub fn spawn(repo_path: PathBuf, event_bus: EventBus) -> ActorResult<Self> {
        // Validate repo opens before spawning
        let _ = ZettelService::open(&repo_path)?;

        let (tx, rx) = mpsc::channel::<ActorMsg>(64);
        let bus = event_bus.clone();
        std::thread::spawn(move || {
            actor_loop(repo_path, rx, bus);
        });
        Ok(Self { tx, event_bus })
    }

    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    pub async fn get_zettel(&self, id: String) -> ActorResult<ParsedZettel> {
        match self.send(ActorCommand::GetZettel { id }).await {
            ActorReply::Zettel(r) => *r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn list_zettels(
        &self,
        zettel_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
        field_filters: Vec<(String, String)>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> ActorResult<Vec<ParsedZettel>> {
        match self
            .send(ActorCommand::ListZettels {
                zettel_type,
                tag,
                backlinks_of,
                field_filters,
                limit,
                offset,
            })
            .await
        {
            ActorReply::ZettelList(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn filtered_list(
        &self,
        q: zdb_core::types::TypedListQuery,
    ) -> ActorResult<Vec<ParsedZettel>> {
        match self.send(ActorCommand::FilteredList(q)).await {
            ActorReply::ZettelList(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn aggregate_query(
        &self,
        sql: String,
        params: Vec<rusqlite::types::Value>,
    ) -> ActorResult<Vec<String>> {
        match self
            .send(ActorCommand::AggregateQuery { sql, params })
            .await
        {
            ActorReply::AggregateRow(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn search(
        &self,
        query: String,
        limit: usize,
        offset: usize,
    ) -> ActorResult<PaginatedSearchResult> {
        match self
            .send(ActorCommand::Search {
                query,
                limit,
                offset,
            })
            .await
        {
            ActorReply::SearchResults(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn create_zettel(
        &self,
        title: String,
        body: Option<String>,
        tags: Vec<String>,
        zettel_type: Option<String>,
    ) -> ActorResult<ParsedZettel> {
        match self
            .send(ActorCommand::CreateZettel {
                title,
                body,
                tags,
                zettel_type,
            })
            .await
        {
            ActorReply::Zettel(r) => *r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn update_zettel(
        &self,
        id: String,
        title: Option<String>,
        body: Option<String>,
        tags: Option<Vec<String>>,
        zettel_type: Option<String>,
    ) -> ActorResult<ParsedZettel> {
        match self
            .send(ActorCommand::UpdateZettel {
                id,
                title,
                body,
                tags,
                zettel_type,
            })
            .await
        {
            ActorReply::Zettel(r) => *r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn delete_zettel(&self, id: String) -> ActorResult<()> {
        match self.send(ActorCommand::DeleteZettel { id }).await {
            ActorReply::Deleted(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn execute_sql(&self, sql: String) -> ActorResult<SqlResult> {
        match self.send(ActorCommand::ExecuteSql { sql }).await {
            ActorReply::SqlResult(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn get_type_schemas(&self) -> ActorResult<Vec<TableSchema>> {
        match self.send(ActorCommand::GetTypeSchemas).await {
            ActorReply::TypeSchemas(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn get_backlinks(&self, id: String) -> ActorResult<Vec<String>> {
        match self.send(ActorCommand::GetBacklinks { id }).await {
            ActorReply::Backlinks(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn count_zettels(
        &self,
        zettel_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
        field_filters: Vec<(String, String)>,
    ) -> ActorResult<i64> {
        match self
            .send(ActorCommand::CountZettels {
                zettel_type,
                tag,
                backlinks_of,
                field_filters,
            })
            .await
        {
            ActorReply::Count(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn attach_file(
        &self,
        zettel_id: String,
        filename: String,
        bytes: Vec<u8>,
        mime: String,
    ) -> ActorResult<zdb_core::types::AttachmentInfo> {
        match self
            .send(ActorCommand::AttachFile {
                zettel_id,
                filename,
                bytes,
                mime,
            })
            .await
        {
            ActorReply::Attachment(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn detach_file(&self, zettel_id: String, filename: String) -> ActorResult<()> {
        match self
            .send(ActorCommand::DetachFile {
                zettel_id,
                filename,
            })
            .await
        {
            ActorReply::Deleted(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn list_attachments(
        &self,
        zettel_id: String,
    ) -> ActorResult<Vec<zdb_core::types::AttachmentInfo>> {
        match self.send(ActorCommand::ListAttachments { zettel_id }).await {
            ActorReply::AttachmentList(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn compact(
        &self,
        force: bool,
        no_backup: bool,
        backup_path: Option<String>,
    ) -> ActorResult<CompactionReport> {
        match self
            .send(ActorCommand::Compact {
                force,
                no_backup,
                backup_path,
            })
            .await
        {
            ActorReply::Maintenance(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn run_maintenance(&self, task: Option<String>) -> ActorResult<MaintenanceReport> {
        match self.send(ActorCommand::GitMaintenance { task }).await {
            ActorReply::GitMaintenance(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn sync(&self, remote: String, branch: String) -> ActorResult<SyncReport> {
        match self.send(ActorCommand::Sync { remote, branch }).await {
            ActorReply::SyncResult(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn nosql_get(&self, id: String) -> ActorResult<Option<ParsedZettel>> {
        match self.send(ActorCommand::NoSqlGet { id }).await {
            ActorReply::NoSqlZettel(r) => *r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn nosql_scan_type(&self, type_name: String) -> ActorResult<Vec<String>> {
        match self.send(ActorCommand::NoSqlScanType { type_name }).await {
            ActorReply::NoSqlIds(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn nosql_scan_tag(&self, tag: String) -> ActorResult<Vec<String>> {
        match self.send(ActorCommand::NoSqlScanTag { tag }).await {
            ActorReply::NoSqlIds(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn nosql_backlinks(&self, id: String) -> ActorResult<Vec<String>> {
        match self.send(ActorCommand::NoSqlBacklinks { id }).await {
            ActorReply::NoSqlIds(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn unlinked_mentions(&self, id: String) -> ActorResult<Vec<UnlinkedMention>> {
        match self.send(ActorCommand::UnlinkedMentions { id }).await {
            ActorReply::UnlinkedMentions(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn suggest_links(&self, id: String, limit: usize) -> ActorResult<Vec<Suggestion>> {
        match self.send(ActorCommand::SuggestLinks { id, limit }).await {
            ActorReply::Suggestions(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn stale_zettels(
        &self,
        type_filter: Option<String>,
    ) -> ActorResult<Vec<StaleZettel>> {
        match self.send(ActorCommand::StaleZettels { type_filter }).await {
            ActorReply::StaleZettels(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn orphan_zettels(
        &self,
        type_filter: Option<String>,
    ) -> ActorResult<Vec<OrphanZettel>> {
        match self.send(ActorCommand::OrphanZettels { type_filter }).await {
            ActorReply::OrphanZettels(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn sequence_info(&self, id: String) -> ActorResult<SequenceInfo> {
        match self.send(ActorCommand::SequenceInfo { id }).await {
            ActorReply::SequenceInfoResult(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn sequence_children(&self, id: String) -> ActorResult<Vec<SequenceNode>> {
        match self.send(ActorCommand::SequenceChildren { id }).await {
            ActorReply::SequenceNodes(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn sequence_breadcrumb(&self, id: String) -> ActorResult<Vec<SequenceNode>> {
        match self.send(ActorCommand::SequenceBreadcrumb { id }).await {
            ActorReply::SequenceNodes(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn broken_sequences(&self) -> ActorResult<Vec<BrokenSequence>> {
        match self.send(ActorCommand::BrokenSequences).await {
            ActorReply::BrokenSequences(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    pub async fn health_check(&self) -> ActorResult<bool> {
        match self.send(ActorCommand::HealthCheck).await {
            ActorReply::HealthStatus(r) => r,
            _ => Err(ZettelError::Validation("unexpected reply".into())),
        }
    }

    async fn send(&self, cmd: ActorCommand) -> ActorReply {
        let (reply_tx, reply_rx) = oneshot::channel();
        let msg = ActorMsg {
            cmd,
            reply: reply_tx,
        };
        // If send fails, the actor is gone
        if self.tx.send(msg).await.is_err() {
            return ActorReply::Deleted(Err(ZettelError::Validation("actor stopped".into())));
        }
        reply_rx
            .await
            .unwrap_or(ActorReply::Deleted(Err(ZettelError::Validation(
                "actor dropped reply".into(),
            ))))
    }
}

/// The blocking actor loop, runs on its own OS thread.
fn actor_loop(repo_path: PathBuf, mut rx: mpsc::Receiver<ActorMsg>, event_bus: EventBus) {
    let mut svc = match ZettelService::open(&repo_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(%e, "actor: failed to open ZettelService");
            return;
        }
    };

    while let Some(msg) = rx.blocking_recv() {
        // Capture delete ID and type before cmd is moved (zettel won't exist after delete)
        let (delete_id, delete_type) = match &msg.cmd {
            ActorCommand::DeleteZettel { id } => (
                Some(id.clone()),
                svc.get_zettel_parsed(id)
                    .ok()
                    .and_then(|z| z.meta.zettel_type),
            ),
            _ => (None, None),
        };
        let mutation_kind = match &msg.cmd {
            ActorCommand::CreateZettel { .. } => Some(EventKind::Created),
            ActorCommand::UpdateZettel { .. } => Some(EventKind::Updated),
            ActorCommand::DeleteZettel { .. } => Some(EventKind::Deleted),
            _ => None,
        };

        let reply = handle_command(&mut svc, msg.cmd);

        // Emit event for successful mutations
        if let Some(ref kind) = mutation_kind {
            match (&kind, &reply) {
                (EventKind::Created | EventKind::Updated, ActorReply::Zettel(r)) => {
                    if let Ok(z) = r.as_ref() {
                        event_bus.send(ZettelEvent {
                            kind: kind.clone(),
                            zettel_id: z
                                .meta
                                .id
                                .as_ref()
                                .map(ToString::to_string)
                                .unwrap_or_default(),
                            zettel_type: z.meta.zettel_type.clone(),
                            timestamp: Utc::now(),
                        });
                    }
                }
                (EventKind::Deleted, ActorReply::Deleted(Ok(()))) => {
                    event_bus.send(ZettelEvent {
                        kind: kind.clone(),
                        zettel_id: delete_id.clone().unwrap_or_default(),
                        zettel_type: delete_type.clone(),
                        timestamp: Utc::now(),
                    });
                }
                _ => {} // mutation failed, no event
            }
        }

        let _ = msg.reply.send(reply);
    }
}

fn handle_command(svc: &mut ZettelService, cmd: ActorCommand) -> ActorReply {
    match cmd {
        ActorCommand::NoSqlGet { id } => {
            ActorReply::NoSqlZettel(Box::new(svc.nosql_get(&id)))
        }
        ActorCommand::NoSqlScanType { type_name } => {
            ActorReply::NoSqlIds(svc.nosql_scan_type(&type_name))
        }
        ActorCommand::NoSqlScanTag { tag } => {
            ActorReply::NoSqlIds(svc.nosql_scan_tag(&tag))
        }
        ActorCommand::NoSqlBacklinks { id } => {
            ActorReply::NoSqlIds(svc.nosql_backlinks(&id))
        }
        ActorCommand::GetZettel { id } => {
            ActorReply::Zettel(Box::new(svc.get_zettel_parsed(&id)))
        }
        ActorCommand::ListZettels {
            zettel_type,
            tag,
            backlinks_of,
            field_filters,
            limit,
            offset,
        } => ActorReply::ZettelList(svc.list_zettels_filtered(&ListFilter {
            zettel_type,
            tag,
            backlinks_of,
            field_filters,
            limit,
            offset,
        })),
        ActorCommand::Search {
            query,
            limit,
            offset,
        } => ActorReply::SearchResults(svc.search_paginated(&query, limit, offset)),
        ActorCommand::CreateZettel {
            title,
            body,
            tags,
            zettel_type,
        } => {
            let result = svc.create_zettel_parsed(
                &title,
                &tags,
                zettel_type.as_deref(),
                &body.unwrap_or_default(),
            );
            ActorReply::Zettel(Box::new(result))
        }
        ActorCommand::UpdateZettel {
            id,
            title,
            body,
            tags,
            zettel_type,
        } => {
            let result = svc.update_zettel_parsed(
                &id,
                title.as_deref(),
                tags.as_deref(),
                zettel_type.as_deref(),
                body.as_deref(),
            );
            ActorReply::Zettel(Box::new(result))
        }
        ActorCommand::DeleteZettel { id } => {
            ActorReply::Deleted(
                svc.delete_zettel(&id, &format!("delete zettel {id}"))
                    .map(|_broken| ()),
            )
        }
        ActorCommand::ExecuteSql { sql } => ActorReply::SqlResult(svc.execute_sql(&sql)),
        ActorCommand::GetTypeSchemas => ActorReply::TypeSchemas(svc.list_type_schemas()),
        ActorCommand::GetBacklinks { id } => ActorReply::Backlinks(svc.backlink_ids(&id)),
        ActorCommand::CountZettels {
            zettel_type,
            tag,
            backlinks_of,
            field_filters,
        } => ActorReply::Count(svc.count_zettels_filtered(&ListFilter {
            zettel_type,
            tag,
            backlinks_of,
            field_filters,
            limit: None,
            offset: None,
        })),
        ActorCommand::FilteredList(q) => ActorReply::ZettelList(svc.typed_filtered_list(&q)),
        ActorCommand::AggregateQuery { sql, params } => {
            ActorReply::AggregateRow(svc.aggregate_query(&sql, &params))
        }
        ActorCommand::AttachFile {
            zettel_id,
            filename,
            bytes,
            mime,
        } => ActorReply::Attachment(svc.attach_file(&zettel_id, &filename, &bytes, &mime)),
        ActorCommand::DetachFile {
            zettel_id,
            filename,
        } => ActorReply::Deleted(svc.detach_file(&zettel_id, &filename)),
        ActorCommand::ListAttachments { zettel_id } => {
            ActorReply::AttachmentList(svc.list_attachments(&zettel_id))
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
        ActorCommand::StaleZettels { type_filter } => {
            ActorReply::StaleZettels(svc.stale_zettels(type_filter.as_deref()))
        }
        ActorCommand::OrphanZettels { type_filter } => {
            ActorReply::OrphanZettels(svc.orphan_zettels(type_filter.as_deref()))
        }
        ActorCommand::SequenceInfo { id } => {
            ActorReply::SequenceInfoResult(svc.sequence_info(&id))
        }
        ActorCommand::SequenceChildren { id } => {
            ActorReply::SequenceNodes(svc.sequence_children(&id))
        }
        ActorCommand::SequenceBreadcrumb { id } => {
            ActorReply::SequenceNodes(svc.sequence_breadcrumb(&id))
        }
        ActorCommand::BrokenSequences => ActorReply::BrokenSequences(svc.broken_sequences()),
        ActorCommand::HealthCheck => ActorReply::HealthStatus(svc.health_check()),
    }
}
