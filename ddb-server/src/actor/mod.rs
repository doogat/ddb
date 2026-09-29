mod handlers;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use chrono::Utc;
use ddb_core::app_contract::AppOutput;
use ddb_core::error::DoogatError;
use ddb_core::schema_diff::plan::SchemaApplyReport;
use ddb_core::service::DoogatService;
use ddb_core::sql_engine::SqlResult;
use ddb_core::types::{
    BatchCreateInput, BatchUpdateInput, BrokenSequence, CompactionReport, ConflictAction,
    MaintenanceReport, OrphanDoogat, PaginatedSearchResult, ParsedDoogat, QueryValue,
    SearchFilters, SequenceInfo, SequenceNode, StaleDoogat, Suggestion, SyncReport, TableSchema,
    UnlinkedMention,
};
use tokio::sync::{mpsc, oneshot};

use ddb_core::service::UpsertOutcome;

use crate::events::{DoogatEvent, EventBus, EventKind};

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
        params: Vec<QueryValue>,
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
    UpsertSingleton {
        type_name: String,
        fields: std::collections::BTreeMap<String, ddb_core::types::Value>,
    },
    ApplySchema {
        schema_doc: String,
        dry_run: bool,
        allow_destructive: bool,
    },
}

/// Replies from the actor.
pub enum ActorReply {
    Doogat(Box<ActorResult<ParsedDoogat>>),
    CreateOutput(Box<ActorResult<AppOutput<ParsedDoogat>>>),
    UpdateOutput(Box<ActorResult<AppOutput<ParsedDoogat>>>),
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
    Upsert(ActorResult<UpsertOutcome>),
    SchemaApply(Box<ActorResult<AppOutput<SchemaApplyReport>>>),
}

struct ActorMsg {
    cmd: ActorCommand,
    reply: oneshot::Sender<ActorReply>,
}

/// Explicit event intent carried alongside the closure for event-bearing verbs.
///
/// Non-mutating verbs use [`EventIntent::None`]. This replaces the `match &msg.cmd`
/// introspection the legacy [`actor_loop`] performs on the enum path: the intent
/// declares *what kind* of event a verb emits, while the closure's actual result
/// decides *whether* to emit (only `Ok` emits, matching the pre-refactor semantics).
pub enum EventIntent {
    /// Reads and non-mutating statements: emit nothing.
    None,
    /// `create`: one `Created` for the returned doogat.
    Created,
    /// `update`: one `Updated` for the returned doogat.
    Updated,
    /// `delete`: one `Deleted`. The id+type are resolved BEFORE the delete closure
    /// runs (the row is gone afterward), so the intent carries them.
    Deleted {
        id: String,
        doogat_type: Option<String>,
    },
    /// `batch_create`: one `Created` per returned doogat.
    CreateMany,
    /// `batch_update`: one `Updated` per returned doogat.
    BatchUpdate,
    /// `upsert_singleton`: `Created` if the outcome created a doogat, else `Updated`.
    Upsert { type_name: String },
}

/// Construct the actor-unavailable / dropped-reply structured error for a verb's
/// return type `R`.
///
/// Every verb routed through [`ActorHandle::call`] returns an
/// [`ActorResult<T>`] (`Result<T, DoogatError>`). The actor-gone and dropped-reply
/// fallbacks must construct the `Err` variant of that `R` without knowing `T`. This
/// trait provides exactly that: one impl over `Result<T, DoogatError>`, so the
/// fallbacks stay total and no verb can smuggle in a non-fallible return type — the
/// bound simply would not be satisfied.
pub trait FromActorError {
    /// Build the error value for an actor that has stopped (send failed).
    fn actor_stopped() -> Self;
    /// Build the error value for a reply channel dropped before a reply arrived.
    fn actor_dropped_reply() -> Self;
}

impl<T> FromActorError for Result<T, DoogatError> {
    fn actor_stopped() -> Self {
        Err(DoogatError::Validation("actor stopped".into()))
    }
    fn actor_dropped_reply() -> Self {
        Err(DoogatError::Validation("actor dropped reply".into()))
    }
}

/// Emit the `DoogatEvent`s a just-run verb produced, derived from its explicit
/// [`EventIntent`] and its actual result.
///
/// This is the closure-boundary emission the design's Option A calls for: the
/// **intent** decides the event *kind*, the **result** (only `Ok` emits) supplies the
/// id/type. Because `call` is generic over the verb's return type `R`, the per-type
/// knowledge of how to read the doogat(s) out of an `Ok` result lives in the
/// [`IntentEmit`] trait. Verb return types that never emit (`Deleted` aside) can rely
/// on the [`no_emit_intent!`] macro; the mutation payloads carry a hand-written impl.
///
/// This is the closure-side twin of the pre-refactor `emit_mutation_events` and must
/// stay parity-preserving with it.
pub trait IntentEmit {
    /// Emit any events this result warrants under `intent`. `Err` results emit
    /// nothing (matching the pre-refactor Ok-gating).
    fn emit(&self, bus: &EventBus, intent: &EventIntent);
}

/// Emit a `Deleted` event when the intent is `Deleted` and the result is `Ok`.
///
/// `Deleted` carries its id/type in the intent (resolved before the delete ran), so
/// it is the one intent whose emission does not read the Ok payload. Every
/// [`IntentEmit`] impl calls this first, then handles its own payload-bearing kinds.
fn emit_deleted_if<T>(bus: &EventBus, intent: &EventIntent, result: &Result<T, DoogatError>) {
    if let EventIntent::Deleted { id, doogat_type } = intent {
        if result.is_ok() {
            bus.send(DoogatEvent {
                kind: EventKind::Deleted,
                doogat_id: id.clone(),
                doogat_type: doogat_type.clone(),
                timestamp: Utc::now(),
            });
        }
    }
}

/// Implement [`IntentEmit`] for verb return types that emit only under `Deleted`
/// (reads, counts, SQL results, attachment info, maintenance reports, ...). A
/// mutating verb that returns one of these payload types (e.g. `delete` returns
/// `ActorResult<()>`) still emits via the `Deleted` intent.
macro_rules! no_emit_intent {
    ($($ty:ty),* $(,)?) => {
        $(
            impl IntentEmit for Result<$ty, DoogatError> {
                fn emit(&self, bus: &EventBus, intent: &EventIntent) {
                    emit_deleted_if(bus, intent, self);
                }
            }
        )*
    };
}

no_emit_intent!(
    (),
    bool,
    i64,
    ParsedDoogat,
    Option<ParsedDoogat>,
    PaginatedSearchResult,
    SqlResult,
    Vec<SqlResult>,
    Vec<TableSchema>,
    Vec<String>,
    ddb_core::types::AttachmentInfo,
    Vec<ddb_core::types::AttachmentInfo>,
    CompactionReport,
    MaintenanceReport,
    SyncReport,
    Vec<UnlinkedMention>,
    Vec<Suggestion>,
    Vec<StaleDoogat>,
    Vec<OrphanDoogat>,
    SequenceInfo,
    Vec<SequenceNode>,
    Vec<BrokenSequence>,
    AppOutput<SchemaApplyReport>,
);

/// `create`/`update`: emit one `Created`/`Updated` for the returned doogat.
impl IntentEmit for Result<AppOutput<ParsedDoogat>, DoogatError> {
    fn emit(&self, bus: &EventBus, intent: &EventIntent) {
        emit_deleted_if(bus, intent, self);
        if let Ok(output) = self {
            match intent {
                EventIntent::Created => {
                    bus.send(doogat_event(&EventKind::Created, &output.value, Utc::now()))
                }
                EventIntent::Updated => {
                    bus.send(doogat_event(&EventKind::Updated, &output.value, Utc::now()))
                }
                _ => {}
            }
        }
    }
}

/// `create_many`/`batch_update`: emit one `Created`/`Updated` per returned doogat.
impl IntentEmit for Result<Vec<ParsedDoogat>, DoogatError> {
    fn emit(&self, bus: &EventBus, intent: &EventIntent) {
        emit_deleted_if(bus, intent, self);
        if let Ok(doogats) = self {
            let kind = match intent {
                EventIntent::CreateMany => EventKind::Created,
                EventIntent::BatchUpdate => EventKind::Updated,
                _ => return,
            };
            let now = Utc::now();
            for z in doogats {
                bus.send(doogat_event(&kind, z, now));
            }
        }
    }
}

/// `upsert_singleton`: emit `Created` iff the outcome created a doogat, else `Updated`.
impl IntentEmit for Result<UpsertOutcome, DoogatError> {
    fn emit(&self, bus: &EventBus, intent: &EventIntent) {
        emit_deleted_if(bus, intent, self);
        if let (EventIntent::Upsert { type_name }, Ok(outcome)) = (intent, self) {
            bus.send(DoogatEvent {
                kind: if outcome.created {
                    EventKind::Created
                } else {
                    EventKind::Updated
                },
                doogat_id: outcome.id.clone(),
                doogat_type: Some(type_name.clone()),
                timestamp: Utc::now(),
            });
        }
    }
}

/// The boxed, type-erased unit of work the closure transport ships to the actor
/// thread. It owns its caller's `f` and the per-call `oneshot::Sender<R>`, runs `f`
/// against the single `DoogatService`, emits at the closure boundary, and forwards
/// `R`.
type ActorRun = Box<dyn FnOnce(&mut DoogatService, &EventBus) + Send>;

/// A closure-carrying message: the closure IS the dispatch. Introduced beside the
/// legacy [`ActorMsg`] during Phase 0 so both transports compile; the enum path is
/// deleted in Phase 2.
struct ClosureMsg {
    run: ActorRun,
    #[allow(dead_code)]
    event: EventIntent,
}

/// One envelope the actor channel carries, tagging which transport a message uses.
/// Phase 0 additive shim: the [`Envelope::Enum`] arm drives the legacy
/// `ActorCommand`/`ActorReply` path, the [`Envelope::Closure`] arm drives the new
/// closure path. Collapses to just the closure form in Phase 2.
enum Envelope {
    Enum(ActorMsg),
    Closure(ClosureMsg),
}

/// Async handle to the repo actor.
#[derive(Clone)]
pub struct ActorHandle {
    tx: mpsc::Sender<Envelope>,
    event_bus: EventBus,
}

impl ActorHandle {
    /// Spawn the actor on a std::thread. Returns the handle for async callers.
    pub fn spawn(repo_path: PathBuf, event_bus: EventBus) -> ActorResult<Self> {
        // Validate repo opens before spawning
        let _ = DoogatService::open(&repo_path)?;

        let (tx, rx) = mpsc::channel::<Envelope>(64);
        let bus = event_bus.clone();
        std::thread::spawn(move || {
            actor_loop(repo_path, rx, bus);
        });
        Ok(Self { tx, event_bus })
    }

    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// Route ONE verb through the actor via a boxed closure.
    ///
    /// Allocates a `oneshot::channel::<R>()`, boxes a closure that runs `f`, emits
    /// events from `(event, &result)` at the closure boundary (Ok-gated, matching
    /// the pre-refactor `emit_mutation_events` semantics), then sends `R` (ignoring a
    /// dropped receiver). No `ActorCommand`/`ActorReply` value is allocated and there
    /// is no "unexpected reply" fallback: an actor-gone or dropped-reply condition
    /// yields a structured `Err` of `R` via [`FromActorError`].
    ///
    /// `f` takes `&mut DoogatService` as a parameter (no borrow crosses the thread
    /// boundary) and must be `Send + 'static`; the compiler rejects any closure that
    /// captures adapter-local borrowed state.
    pub async fn call<R, F>(&self, event: EventIntent, f: F) -> R
    where
        F: FnOnce(&mut DoogatService) -> R + Send + 'static,
        R: FromActorError + IntentEmit + Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel::<R>();

        // Box a type-erased closure that OWNS reply_tx and runs the caller's `f`.
        // Event emission happens inside this boundary (Option A) so it derives from
        // the actual result while the intent decides the event kind.
        let run: ActorRun = Box::new(move |svc, bus| {
            let result = f(svc);
            result.emit(bus, &event);
            // Receiver may have dropped; that is not an error for the actor.
            let _ = reply_tx.send(result);
        });

        let msg = Envelope::Closure(ClosureMsg {
            run,
            event: EventIntent::None,
        });

        // Actor gone: return a structured error, never panic.
        if self.tx.send(msg).await.is_err() {
            return R::actor_stopped();
        }

        // Reply dropped (thread died mid-verb): structured error, never panic.
        reply_rx.await.unwrap_or_else(|_| R::actor_dropped_reply())
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
        params: Vec<QueryValue>,
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
    ) -> ActorResult<AppOutput<ParsedDoogat>> {
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
            ActorReply::CreateOutput(r) => *r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn update_doogat(
        &self,
        params: UpdateDoogatParams,
    ) -> ActorResult<AppOutput<ParsedDoogat>> {
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
            ActorReply::UpdateOutput(r) => *r,
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
        match self.send(ActorCommand::ExecuteBatch { statements }).await {
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

    pub async fn upsert_singleton(
        &self,
        type_name: String,
        fields: std::collections::BTreeMap<String, ddb_core::types::Value>,
    ) -> ActorResult<UpsertOutcome> {
        match self
            .send(ActorCommand::UpsertSingleton { type_name, fields })
            .await
        {
            ActorReply::Upsert(r) => r,
            _ => Err(DoogatError::Validation("unexpected reply".into())),
        }
    }

    pub async fn apply_schema(
        &self,
        schema_doc: String,
        dry_run: bool,
        allow_destructive: bool,
    ) -> ActorResult<AppOutput<SchemaApplyReport>> {
        match self
            .send(ActorCommand::ApplySchema {
                schema_doc,
                dry_run,
                allow_destructive,
            })
            .await
        {
            ActorReply::SchemaApply(r) => *r,
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
        if self.tx.send(Envelope::Enum(msg)).await.is_err() {
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
fn actor_loop(repo_path: PathBuf, mut rx: mpsc::Receiver<Envelope>, event_bus: EventBus) {
    let mut svc = match DoogatService::open_shared(&repo_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(%e, "actor: failed to open DoogatService");
            return;
        }
    };

    if let Err(e) = svc.rebuild_if_stale() {
        tracing::warn!(%e, "actor: index rebuild on startup failed");
    }

    while let Some(envelope) = rx.blocking_recv() {
        // Closure path (new): the closure IS the dispatch and emits its own events
        // at the boundary (Option A). The legacy enum path stays below, unchanged,
        // until Phase 2 deletes it.
        let msg = match envelope {
            Envelope::Closure(closure_msg) => {
                (closure_msg.run)(&mut svc, &event_bus);
                continue;
            }
            Envelope::Enum(msg) => msg,
        };

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
        let upsert_type = match &msg.cmd {
            ActorCommand::UpsertSingleton { type_name, .. } => Some(type_name.clone()),
            _ => None,
        };
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
        if let (Some(t), ActorReply::Upsert(Ok(outcome))) = (&upsert_type, &reply) {
            event_bus.send(DoogatEvent {
                kind: if outcome.created {
                    EventKind::Created
                } else {
                    EventKind::Updated
                },
                doogat_id: outcome.id.clone(),
                doogat_type: Some(t.clone()),
                timestamp: Utc::now(),
            });
        }
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
        doogat_id: z
            .meta
            .id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
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
            (EventKind::Created, ActorReply::CreateOutput(r)) => {
                if let Ok(output) = r.as_ref() {
                    event_bus.send(doogat_event(kind, &output.value, Utc::now()));
                }
            }
            (EventKind::Updated, ActorReply::UpdateOutput(r)) => {
                if let Ok(output) = r.as_ref() {
                    event_bus.send(doogat_event(kind, &output.value, Utc::now()));
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
            let kind = if is_create_many {
                EventKind::Created
            } else {
                EventKind::Updated
            };
            let now = Utc::now();
            for z in doogats {
                event_bus.send(doogat_event(&kind, z, now));
            }
        }
    }
}
