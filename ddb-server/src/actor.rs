use std::path::PathBuf;

use chrono::Utc;
use tokio::sync::{mpsc, oneshot};
use ddb_core::error::DoogatError;
use ddb_core::service::DoogatService;
use ddb_core::sql_engine::SqlResult;
use ddb_core::types::{
    BrokenSequence, CompactOptions, CompactionReport, ListFilter, MaintenanceReport, OrphanDoogat,
    PaginatedSearchResult, ParsedDoogat, SearchFilters, SequenceInfo, SequenceNode, StaleDoogat,
    Suggestion, SyncReport, TableSchema, UnlinkedMention,
};

use crate::events::{EventBus, EventKind, DoogatEvent};

/// Serializable result from the actor.
pub type ActorResult<T> = Result<T, DoogatError>;

/// Commands the actor understands.
pub enum ActorCommand {
    GetDoogat {
        id: String,
    },
    ListDoogats {
        doogat_type: Option<String>,
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
        filters: SearchFilters,
    },
    CreateDoogat {
        title: String,
        body: Option<String>,
        tags: Vec<String>,
        doogat_type: Option<String>,
    },
    UpdateDoogat {
        id: String,
        title: Option<String>,
        body: Option<String>,
        tags: Option<Vec<String>>,
        doogat_type: Option<String>,
    },
    DeleteDoogat {
        id: String,
    },
    ExecuteSql {
        sql: String,
    },
    GetTypeSchemas,
    GetBacklinks {
        id: String,
    },
    CountDoogats {
        doogat_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
        field_filters: Vec<(String, String)>,
    },
    FilteredList(ddb_core::types::TypedListQuery),
    AggregateQuery {
        sql: String,
        params: Vec<rusqlite::types::Value>,
    },
    AttachFile {
        doogat_id: String,
        filename: String,
        bytes: Vec<u8>,
        mime: String,
    },
    DetachFile {
        doogat_id: String,
        filename: String,
    },
    ListAttachments {
        doogat_id: String,
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
    StaleDoogats {
        type_filter: Option<String>,
    },
    OrphanDoogats {
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
    Doogat(Box<ActorResult<ParsedDoogat>>),
    DoogatList(ActorResult<Vec<ParsedDoogat>>),
    SearchResults(ActorResult<PaginatedSearchResult>),
    SqlResult(ActorResult<SqlResult>),
    TypeSchemas(ActorResult<Vec<TableSchema>>),
    Backlinks(ActorResult<Vec<String>>),
    Deleted(ActorResult<()>),
    Count(ActorResult<i64>),
    /// Single row of string values from an aggregate query.
    AggregateRow(ActorResult<Vec<String>>),
    Attachment(ActorResult<ddb_core::types::AttachmentInfo>),
    AttachmentList(ActorResult<Vec<ddb_core::types::AttachmentInfo>>),
    Maintenance(ActorResult<CompactionReport>),
    GitMaintenance(ActorResult<MaintenanceReport>),
    SyncResult(ActorResult<SyncReport>),
    NoSqlDoogat(Box<ActorResult<Option<ParsedDoogat>>>),
    NoSqlIds(ActorResult<Vec<String>>),
    UnlinkedMentions(ActorResult<Vec<UnlinkedMention>>),
    Suggestions(ActorResult<Vec<Suggestion>>),
    StaleDoogats(ActorResult<Vec<StaleDoogat>>),
    OrphanDoogats(ActorResult<Vec<OrphanDoogat>>),
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
        let _ = DoogatService::open(&repo_path)?;

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

    pub async fn get_doogat(&self, id: String) -> ActorResult<ParsedDoogat> {
        match self.send(ActorCommand::GetDoogat { id }).await {
            ActorReply::Doogat(r) => *r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn list_doogats(
        &self,
        doogat_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
        field_filters: Vec<(String, String)>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> ActorResult<Vec<ParsedDoogat>> {
        match self
            .send(ActorCommand::ListDoogats {
                doogat_type,
                tag,
                backlinks_of,
                field_filters,
                limit,
                offset,
            })
            .await
        {
            ActorReply::DoogatList(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn filtered_list(
        &self,
        q: ddb_core::types::TypedListQuery,
    ) -> ActorResult<Vec<ParsedDoogat>> {
        match self.send(ActorCommand::FilteredList(q)).await {
            ActorReply::DoogatList(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
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
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn search(
        &self,
        query: String,
        limit: usize,
        offset: usize,
        filters: SearchFilters,
    ) -> ActorResult<PaginatedSearchResult> {
        match self
            .send(ActorCommand::Search {
                query,
                limit,
                offset,
                filters,
            })
            .await
        {
            ActorReply::SearchResults(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn create_doogat(
        &self,
        title: String,
        body: Option<String>,
        tags: Vec<String>,
        doogat_type: Option<String>,
    ) -> ActorResult<ParsedDoogat> {
        match self
            .send(ActorCommand::CreateDoogat {
                title,
                body,
                tags,
                doogat_type,
            })
            .await
        {
            ActorReply::Doogat(r) => *r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn update_doogat(
        &self,
        id: String,
        title: Option<String>,
        body: Option<String>,
        tags: Option<Vec<String>>,
        doogat_type: Option<String>,
    ) -> ActorResult<ParsedDoogat> {
        match self
            .send(ActorCommand::UpdateDoogat {
                id,
                title,
                body,
                tags,
                doogat_type,
            })
            .await
        {
            ActorReply::Doogat(r) => *r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn delete_doogat(&self, id: String) -> ActorResult<()> {
        match self.send(ActorCommand::DeleteDoogat { id }).await {
            ActorReply::Deleted(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn execute_sql(&self, sql: String) -> ActorResult<SqlResult> {
        match self.send(ActorCommand::ExecuteSql { sql }).await {
            ActorReply::SqlResult(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn get_type_schemas(&self) -> ActorResult<Vec<TableSchema>> {
        match self.send(ActorCommand::GetTypeSchemas).await {
            ActorReply::TypeSchemas(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn get_backlinks(&self, id: String) -> ActorResult<Vec<String>> {
        match self.send(ActorCommand::GetBacklinks { id }).await {
            ActorReply::Backlinks(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn count_doogats(
        &self,
        doogat_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
        field_filters: Vec<(String, String)>,
    ) -> ActorResult<i64> {
        match self
            .send(ActorCommand::CountDoogats {
                doogat_type,
                tag,
                backlinks_of,
                field_filters,
            })
            .await
        {
            ActorReply::Count(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn attach_file(
        &self,
        doogat_id: String,
        filename: String,
        bytes: Vec<u8>,
        mime: String,
    ) -> ActorResult<ddb_core::types::AttachmentInfo> {
        match self
            .send(ActorCommand::AttachFile {
                doogat_id,
                filename,
                bytes,
                mime,
            })
            .await
        {
            ActorReply::Attachment(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn detach_file(&self, doogat_id: String, filename: String) -> ActorResult<()> {
        match self
            .send(ActorCommand::DetachFile {
                doogat_id,
                filename,
            })
            .await
        {
            ActorReply::Deleted(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn list_attachments(
        &self,
        doogat_id: String,
    ) -> ActorResult<Vec<ddb_core::types::AttachmentInfo>> {
        match self.send(ActorCommand::ListAttachments { doogat_id }).await {
            ActorReply::AttachmentList(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
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
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn run_maintenance(&self, task: Option<String>) -> ActorResult<MaintenanceReport> {
        match self.send(ActorCommand::GitMaintenance { task }).await {
            ActorReply::GitMaintenance(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn sync(&self, remote: String, branch: String) -> ActorResult<SyncReport> {
        match self.send(ActorCommand::Sync { remote, branch }).await {
            ActorReply::SyncResult(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn nosql_get(&self, id: String) -> ActorResult<Option<ParsedDoogat>> {
        match self.send(ActorCommand::NoSqlGet { id }).await {
            ActorReply::NoSqlDoogat(r) => *r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn nosql_scan_type(&self, type_name: String) -> ActorResult<Vec<String>> {
        match self.send(ActorCommand::NoSqlScanType { type_name }).await {
            ActorReply::NoSqlIds(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn nosql_scan_tag(&self, tag: String) -> ActorResult<Vec<String>> {
        match self.send(ActorCommand::NoSqlScanTag { tag }).await {
            ActorReply::NoSqlIds(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn nosql_backlinks(&self, id: String) -> ActorResult<Vec<String>> {
        match self.send(ActorCommand::NoSqlBacklinks { id }).await {
            ActorReply::NoSqlIds(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn unlinked_mentions(&self, id: String) -> ActorResult<Vec<UnlinkedMention>> {
        match self.send(ActorCommand::UnlinkedMentions { id }).await {
            ActorReply::UnlinkedMentions(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn suggest_links(&self, id: String, limit: usize) -> ActorResult<Vec<Suggestion>> {
        match self.send(ActorCommand::SuggestLinks { id, limit }).await {
            ActorReply::Suggestions(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn stale_doogats(
        &self,
        type_filter: Option<String>,
    ) -> ActorResult<Vec<StaleDoogat>> {
        match self.send(ActorCommand::StaleDoogats { type_filter }).await {
            ActorReply::StaleDoogats(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn orphan_doogats(
        &self,
        type_filter: Option<String>,
    ) -> ActorResult<Vec<OrphanDoogat>> {
        match self.send(ActorCommand::OrphanDoogats { type_filter }).await {
            ActorReply::OrphanDoogats(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn sequence_info(&self, id: String) -> ActorResult<SequenceInfo> {
        match self.send(ActorCommand::SequenceInfo { id }).await {
            ActorReply::SequenceInfoResult(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn sequence_children(&self, id: String) -> ActorResult<Vec<SequenceNode>> {
        match self.send(ActorCommand::SequenceChildren { id }).await {
            ActorReply::SequenceNodes(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn sequence_breadcrumb(&self, id: String) -> ActorResult<Vec<SequenceNode>> {
        match self.send(ActorCommand::SequenceBreadcrumb { id }).await {
            ActorReply::SequenceNodes(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn broken_sequences(&self) -> ActorResult<Vec<BrokenSequence>> {
        match self.send(ActorCommand::BrokenSequences).await {
            ActorReply::BrokenSequences(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn health_check(&self) -> ActorResult<bool> {
        match self.send(ActorCommand::HealthCheck).await {
            ActorReply::HealthStatus(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
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
            return ActorReply::Deleted(Err(DoogatError::Validation("actor stopped".into())));
        }
        reply_rx
            .await
            .unwrap_or(ActorReply::Deleted(Err(DoogatError::Validation(
                "actor dropped reply".into(),
            ))))
    }
}

/// The blocking actor loop, runs on its own OS thread.
fn actor_loop(repo_path: PathBuf, mut rx: mpsc::Receiver<ActorMsg>, event_bus: EventBus) {
    let mut svc = match DoogatService::open(&repo_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(%e, "actor: failed to open DoogatService");
            return;
        }
    };

    // Ensure index is up to date on startup (catches external changes)
    if let Err(e) = svc.rebuild_if_stale() {
        tracing::warn!(%e, "actor: index rebuild on startup failed");
    }

    while let Some(msg) = rx.blocking_recv() {
        // Capture delete ID and type before cmd is moved (doogat won't exist after delete)
        let (delete_id, delete_type) = match &msg.cmd {
            ActorCommand::DeleteDoogat { id } => (
                Some(id.clone()),
                svc.get_doogat_parsed(id)
                    .ok()
                    .and_then(|z| z.meta.doogat_type),
            ),
            _ => (None, None),
        };
        let mutation_kind = match &msg.cmd {
            ActorCommand::CreateDoogat { .. } => Some(EventKind::Created),
            ActorCommand::UpdateDoogat { .. } => Some(EventKind::Updated),
            ActorCommand::DeleteDoogat { .. } => Some(EventKind::Deleted),
            _ => None,
        };

        let reply = handle_command(&mut svc, msg.cmd);

        // Emit event for successful mutations
        if let Some(ref kind) = mutation_kind {
            match (&kind, &reply) {
                (EventKind::Created | EventKind::Updated, ActorReply::Doogat(r)) => {
                    if let Ok(z) = r.as_ref() {
                        event_bus.send(DoogatEvent {
                            kind: kind.clone(),
                            doogat_id: z
                                .meta
                                .id
                                .as_ref()
                                .map(ToString::to_string)
                                .unwrap_or_default(),
                            doogat_type: z.meta.doogat_type.clone(),
                            timestamp: Utc::now(),
                        });
                    }
                }
                (EventKind::Deleted, ActorReply::Deleted(Ok(()))) => {
                    event_bus.send(DoogatEvent {
                        kind: kind.clone(),
                        doogat_id: delete_id.clone().unwrap_or_default(),
                        doogat_type: delete_type.clone(),
                        timestamp: Utc::now(),
                    });
                }
                _ => {} // mutation failed, no event
            }
        }

        let _ = msg.reply.send(reply);
    }
}

fn handle_command(svc: &mut DoogatService, cmd: ActorCommand) -> ActorReply {
    match cmd {
        ActorCommand::NoSqlGet { id } => {
            ActorReply::NoSqlDoogat(Box::new(svc.nosql_get(&id)))
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
        ActorCommand::GetDoogat { id } => {
            ActorReply::Doogat(Box::new(svc.get_doogat_parsed(&id)))
        }
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
        } => ActorReply::SearchResults(svc.search_paginated_filtered(&query, limit, offset, &filters)),
        ActorCommand::CreateDoogat {
            title,
            body,
            tags,
            doogat_type,
        } => {
            let result = svc.create_doogat_parsed(
                &title,
                &tags,
                doogat_type.as_deref(),
                &body.unwrap_or_default(),
            );
            ActorReply::Doogat(Box::new(result))
        }
        ActorCommand::UpdateDoogat {
            id,
            title,
            body,
            tags,
            doogat_type,
        } => {
            let result = svc.update_doogat_parsed(
                &id,
                title.as_deref(),
                tags.as_deref(),
                doogat_type.as_deref(),
                body.as_deref(),
            );
            ActorReply::Doogat(Box::new(result))
        }
        ActorCommand::DeleteDoogat { id } => {
            ActorReply::Deleted(
                svc.delete_doogat(&id, &format!("delete doogat {id}"))
                    .map(|_broken| ()),
            )
        }
        ActorCommand::ExecuteSql { sql } => ActorReply::SqlResult(svc.execute_sql(&sql)),
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
