mod handlers;

use std::path::PathBuf;

use chrono::Utc;
use tokio::sync::{mpsc, oneshot};
use ddb_core::error::DoogatError;
use ddb_core::service::DoogatService;
use ddb_core::sql_engine::SqlResult;
use ddb_core::types::{
    BatchCreateInput, BatchUpdateInput, BrokenSequence, CompactionReport,
    ConflictAction, MaintenanceReport, OrphanDoogat, PaginatedSearchResult,
    ParsedDoogat, SearchFilters, SequenceInfo, SequenceNode, StaleDoogat, Suggestion, SyncReport,
    TableSchema, UnlinkedMention,
};

use crate::events::{EventBus, EventKind, DoogatEvent};

/// Serializable result from the actor.
pub type ActorResult<T> = Result<T, DoogatError>;

/// Parameters for updating a single doogat through the actor.
pub struct UpdateDoogatParams {
    pub id: String,
    pub title: Option<String>,
    pub body: Option<String>,
    pub tags: Option<Vec<String>>,
    pub doogat_type: Option<String>,
    pub fields: std::collections::BTreeMap<String, ddb_core::types::Value>,
    pub unset_fields: Vec<String>,
}

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
        title: Option<String>,
        body: Option<String>,
        tags: Vec<String>,
        doogat_type: Option<String>,
        fields: std::collections::BTreeMap<String, ddb_core::types::Value>,
        on_conflict: ConflictAction,
    },
    UpdateDoogat {
        id: String,
        title: Option<String>,
        body: Option<String>,
        tags: Option<Vec<String>>,
        doogat_type: Option<String>,
        fields: std::collections::BTreeMap<String, ddb_core::types::Value>,
        unset_fields: Vec<String>,
    },
    DeleteDoogat {
        id: String,
    },
    ExecuteSql {
        sql: String,
    },
    BatchUpdate {
        updates: Vec<BatchUpdateInput>,
    },
    CreateMany {
        inputs: Vec<BatchCreateInput>,
    },
    ExecuteBatch {
        statements: Vec<String>,
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
    SqlResults(ActorResult<Vec<SqlResult>>),
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
        title: Option<String>,
        body: Option<String>,
        tags: Vec<String>,
        doogat_type: Option<String>,
        fields: std::collections::BTreeMap<String, ddb_core::types::Value>,
        on_conflict: ConflictAction,
    ) -> ActorResult<ParsedDoogat> {
        match self
            .send(ActorCommand::CreateDoogat {
                title,
                body,
                tags,
                doogat_type,
                fields,
                on_conflict,
            })
            .await
        {
            ActorReply::Doogat(r) => *r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn update_doogat(
        &self,
        params: UpdateDoogatParams,
    ) -> ActorResult<ParsedDoogat> {
        match self
            .send(ActorCommand::UpdateDoogat {
                id: params.id,
                title: params.title,
                body: params.body,
                tags: params.tags,
                doogat_type: params.doogat_type,
                fields: params.fields,
                unset_fields: params.unset_fields,
            })
            .await
        {
            ActorReply::Doogat(r) => *r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn batch_update(
        &self,
        updates: Vec<BatchUpdateInput>,
    ) -> ActorResult<Vec<ParsedDoogat>> {
        match self.send(ActorCommand::BatchUpdate { updates }).await {
            ActorReply::DoogatList(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn create_many(
        &self,
        inputs: Vec<BatchCreateInput>,
    ) -> ActorResult<Vec<ParsedDoogat>> {
        match self.send(ActorCommand::CreateMany { inputs }).await {
            ActorReply::DoogatList(r) => r,
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

    pub async fn execute_batch(&self, statements: Vec<String>) -> ActorResult<Vec<SqlResult>> {
        match self
            .send(ActorCommand::ExecuteBatch { statements })
            .await
        {
            ActorReply::SqlResults(r) => r,
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

    if let Err(e) = svc.rebuild_if_stale() {
        tracing::warn!(%e, "actor: index rebuild on startup failed");
    }

    while let Some(msg) = rx.blocking_recv() {
        let (delete_id, delete_type) = match &msg.cmd {
            ActorCommand::DeleteDoogat { id } => (
                Some(id.clone()),
                svc.get_doogat_parsed(id)
                    .ok()
                    .and_then(|z| z.meta.doogat_type),
            ),
            _ => (None, None),
        };
        let is_batch_update = matches!(&msg.cmd, ActorCommand::BatchUpdate { .. });
        let is_create_many = matches!(&msg.cmd, ActorCommand::CreateMany { .. });
        let mutation_kind = match &msg.cmd {
            ActorCommand::CreateDoogat { .. } => Some(EventKind::Created),
            ActorCommand::UpdateDoogat { .. } => Some(EventKind::Updated),
            ActorCommand::DeleteDoogat { .. } => Some(EventKind::Deleted),
            _ => None,
        };

        let reply = handlers::handle_command(&mut svc, msg.cmd);
        emit_mutation_events(
            &event_bus,
            &reply,
            mutation_kind.as_ref(),
            &delete_id,
            &delete_type,
            is_batch_update,
            is_create_many,
        );
        let _ = msg.reply.send(reply);
    }
}

fn doogat_event(
    kind: &EventKind,
    z: &ParsedDoogat,
    timestamp: chrono::DateTime<Utc>,
) -> DoogatEvent {
    DoogatEvent {
        kind: kind.clone(),
        doogat_id: z.meta.id.as_ref().map(ToString::to_string).unwrap_or_default(),
        doogat_type: z.meta.doogat_type.clone(),
        timestamp,
    }
}

/// Emit events for successful singular and batch mutations.
fn emit_mutation_events(
    event_bus: &EventBus,
    reply: &ActorReply,
    mutation_kind: Option<&EventKind>,
    delete_id: &Option<String>,
    delete_type: &Option<String>,
    is_batch_update: bool,
    is_create_many: bool,
) {
    if let Some(kind) = mutation_kind {
        match (kind, reply) {
            (EventKind::Created | EventKind::Updated, ActorReply::Doogat(r)) => {
                if let Ok(z) = r.as_ref() {
                    event_bus.send(doogat_event(kind, z, Utc::now()));
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
            _ => {}
        }
    }

    if is_batch_update || is_create_many {
        if let ActorReply::DoogatList(Ok(ref doogats)) = reply {
            let kind = if is_create_many { EventKind::Created } else { EventKind::Updated };
            let now = Utc::now();
            for z in doogats {
                event_bus.send(doogat_event(&kind, z, now));
            }
        }
    }
}
