mod event;
mod reload_signal;
#[cfg(test)]
mod tests;

pub use event::{EventIntent, IntentEmit};
pub use reload_signal::{ReloadIntent, ReloadSignal};

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use ddb_core::app_contract::{AppOutput, CreateCommand, UnregisteredTypePolicy, UpdateCommand};
use ddb_core::error::DoogatError;
use ddb_core::schema_diff::plan::SchemaApplyReport;
use ddb_core::service::DoogatService;
use ddb_core::sql_engine::{requires_schema_reload, SqlResult};
use ddb_core::types::{
    BatchCreateInput, BatchUpdateInput, BrokenSequence, CompactOptions, CompactionReport,
    ConflictAction, ListFilter, MaintenanceReport, OrphanDoogat, PaginatedSearchResult, ParsedDoogat, QueryValue,
    SearchFilters, SequenceInfo, SequenceNode, StaleDoogat, Suggestion, SyncReport, TableSchema,
    UnlinkedMention,
};
use tokio::sync::{mpsc, oneshot};

use ddb_core::service::UpsertOutcome;

use crate::events::EventBus;
use crate::reload::SchemaReloader;

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

/// The boxed, type-erased unit of work the closure transport ships to the actor
/// thread. It owns its caller's `f` and the per-call `oneshot::Sender<R>`, runs `f`
/// against the single `DoogatService`, emits at the closure boundary, and forwards
/// `R`.
type ActorRun = Box<dyn FnOnce(&mut DoogatService, &EventBus) + Send>;

/// Async handle to the repo actor.
#[derive(Clone)]
pub struct ActorHandle {
    tx: mpsc::Sender<ActorRun>,
    maintenance_tx: mpsc::Sender<ActorRun>,
    event_bus: EventBus,
    reloader: Arc<OnceLock<Arc<SchemaReloader>>>,
    remote_timeout: std::time::Duration,
}

impl ActorHandle {
    /// Spawn the actor on a std::thread. Returns the handle for async callers.
    ///
    /// Two threads run [`actor_loop`]: the write actor (`tx`) and a maintenance
    /// worker (`maintenance_tx`) for sync, compact and run_maintenance, so a
    /// slow maintenance verb never stalls ordinary writes. Each thread owns its
    /// own repository and SQLite index; both share ONE NoSQL mirror handle. The
    /// repo write lock serializes their git writes.
    ///
    /// `remote_timeout` bounds `sync`'s fetch/push legs (`ServerConfig`'s
    /// configured `[remote] timeout_secs`, or `git_ops::DEFAULT_REMOTE_TIMEOUT`
    /// for callers that have no server config to read, e.g. tests).
    pub fn spawn(
        repo_path: PathBuf,
        event_bus: EventBus,
        remote_timeout: std::time::Duration,
    ) -> ActorResult<Self> {
        let mirror = DoogatService::shared_mirror(&repo_path);
        let write_svc = DoogatService::open_shared_with_mirror(&repo_path, mirror.clone())?;
        let maintenance_svc = DoogatService::open_shared_with_mirror(&repo_path, mirror)?;

        let (tx, rx) = mpsc::channel::<ActorRun>(64);
        let (maintenance_tx, maintenance_rx) = mpsc::channel::<ActorRun>(8);
        let bus = event_bus.clone();
        // Only the write actor rebuilds on startup; two concurrent cold-start
        // rebuilds of one index would race.
        std::thread::spawn(move || {
            if let Err(e) = write_svc.rebuild_if_stale() {
                tracing::warn!(%e, "actor: index rebuild on startup failed");
            }
            actor_loop(write_svc, rx, bus)
        });
        let bus = event_bus.clone();
        std::thread::spawn(move || actor_loop(maintenance_svc, maintenance_rx, bus));
        Ok(Self {
            tx,
            maintenance_tx,
            event_bus,
            reloader: Arc::new(OnceLock::new()),
            remote_timeout,
        })
    }

    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// Register the schema reloader `call` triggers per its [`ReloadIntent`].
    pub fn set_reloader(&self, reloader: Arc<SchemaReloader>) {
        if self.reloader.set(reloader).is_err() {
            tracing::warn!(
                "actor: schema reloader already registered; second registration ignored"
            );
        }
    }

    /// Route ONE verb through the actor via a boxed closure.
    ///
    /// Allocates a `oneshot::channel::<R>()`, boxes a closure that runs `f`, emits
    /// events from `(event, &result)` at the closure boundary (only `Ok` emits), then
    /// sends `R` (ignoring a dropped receiver). An actor-gone or dropped-reply
    /// condition yields a structured `Err` of `R` via [`FromActorError`].
    ///
    /// `f` takes `&mut DoogatService` as a parameter (no borrow crosses the thread
    /// boundary) and must be `Send + 'static`; the compiler rejects any closure that
    /// captures adapter-local borrowed state.
    ///
    /// After the reply arrives, `reload` decides (from the result, via
    /// [`ReloadSignal`]) whether to trigger and await a schema reload on the
    /// registered reloader. No registered reloader means no reload.
    pub async fn call<R, F>(&self, event: EventIntent, reload: ReloadIntent, f: F) -> R
    where
        F: FnOnce(&mut DoogatService) -> R + Send + 'static,
        R: FromActorError + IntentEmit + ReloadSignal + Send + 'static,
    {
        self.dispatch(&self.tx, event, reload, f).await
    }

    /// Like [`Self::call`], but on the maintenance worker thread.
    async fn call_maintenance<R, F>(&self, event: EventIntent, reload: ReloadIntent, f: F) -> R
    where
        F: FnOnce(&mut DoogatService) -> R + Send + 'static,
        R: FromActorError + IntentEmit + ReloadSignal + Send + 'static,
    {
        self.dispatch(&self.maintenance_tx, event, reload, f).await
    }

    async fn dispatch<R, F>(
        &self,
        tx: &mpsc::Sender<ActorRun>,
        event: EventIntent,
        reload: ReloadIntent,
        f: F,
    ) -> R
    where
        F: FnOnce(&mut DoogatService) -> R + Send + 'static,
        R: FromActorError + IntentEmit + ReloadSignal + Send + 'static,
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

        // Actor gone: return a structured error, never panic.
        if tx.send(run).await.is_err() {
            return R::actor_stopped();
        }

        // Reply dropped (thread died mid-verb): structured error, never panic.
        let result = reply_rx.await.unwrap_or_else(|_| R::actor_dropped_reply());

        if result.schema_changed(&reload) {
            if let Some(reloader) = self.reloader.get() {
                reloader.trigger_reload_and_wait().await;
            }
        }
        result
    }

    pub async fn get_doogat(&self, id: String) -> ActorResult<ParsedDoogat> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.get_doogat_parsed(&id)
        })
        .await
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
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.list_doogats_filtered(&ListFilter {
                doogat_type,
                tag,
                backlinks_of,
                field_filters,
                limit,
                offset,
                ..Default::default()
            })
        })
        .await
    }

    pub async fn filtered_list(
        &self,
        q: ddb_core::types::TypedListQuery,
    ) -> ActorResult<Vec<ParsedDoogat>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.typed_filtered_list(&q)
        })
        .await
    }

    pub async fn aggregate_query(
        &self,
        sql: String,
        params: Vec<QueryValue>,
    ) -> ActorResult<Vec<String>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.aggregate_query(&sql, &params)
        })
        .await
    }

    pub async fn search(
        &self,
        query: String,
        limit: usize,
        offset: usize,
        filters: SearchFilters,
    ) -> ActorResult<PaginatedSearchResult> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.search_paginated_filtered(&query, limit, offset, &filters)
        })
        .await
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
        self.call(EventIntent::Created, ReloadIntent::None, move |svc| {
            let cmd = CreateCommand {
                title,
                body,
                tags,
                doogat_type,
                fields,
                on_conflict,
                // PRD 00155: GraphQL + REST reject unregistered types
                // (TYPE_NOT_REGISTERED). Only the CLI opts into BaseOnly.
                unregistered_type_policy: UnregisteredTypePolicy::Strict,
            };
            svc.create(cmd)
        })
        .await
    }

    pub async fn update_doogat(
        &self,
        params: UpdateDoogatParams,
    ) -> ActorResult<AppOutput<ParsedDoogat>> {
        self.call(EventIntent::Updated, ReloadIntent::None, move |svc| {
            let cmd = UpdateCommand {
                id: params.id,
                title: params.title,
                tags: params.tags,
                doogat_type: params.doogat_type,
                body: params.body,
                fields: params.fields,
                unset_fields: params.unset_fields,
            };
            svc.update(cmd)
        })
        .await
    }

    pub async fn batch_update(
        &self,
        updates: Vec<BatchUpdateInput>,
    ) -> ActorResult<Vec<ParsedDoogat>> {
        self.call(EventIntent::BatchUpdate, ReloadIntent::None, move |svc| {
            svc.batch_update(&updates)
        })
        .await
    }

    pub async fn create_many(
        &self,
        inputs: Vec<BatchCreateInput>,
    ) -> ActorResult<Vec<ParsedDoogat>> {
        self.call(EventIntent::CreateMany, ReloadIntent::None, move |svc| {
            svc.batch_create(&inputs)
        })
        .await
    }

    pub async fn delete_doogat(&self, id: String) -> ActorResult<()> {
        let doogat_type: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let doogat_type_for_closure = doogat_type.clone();
        self.call(
            EventIntent::Deleted {
                id: id.clone(),
                doogat_type,
            },
            ReloadIntent::None,
            move |svc| {
                let resolved = svc
                    .get_doogat_parsed(&id)
                    .ok()
                    .and_then(|z| z.meta.doogat_type);
                *doogat_type_for_closure
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = resolved;
                svc.delete_doogat(&id, &format!("delete doogat {id}"))
                    .map(|_broken| ())
            },
        )
        .await
    }

    pub async fn execute_sql(&self, sql: String) -> ActorResult<SqlResult> {
        let reload = ReloadIntent::Explicit(requires_schema_reload(&sql));
        self.call(EventIntent::None, reload, move |svc| svc.execute_sql(&sql))
            .await
    }

    pub async fn execute_batch(&self, statements: Vec<String>) -> ActorResult<Vec<SqlResult>> {
        let reload =
            ReloadIntent::Explicit(statements.iter().any(|s| requires_schema_reload(s)));
        let combined = statements.join(";\n");
        self.call(EventIntent::None, reload, move |svc| {
            svc.execute_batch(&combined)
        })
        .await
    }

    pub async fn get_type_schemas(&self) -> ActorResult<Vec<TableSchema>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.list_type_schemas()
        })
        .await
    }

    pub async fn get_backlinks(&self, id: String) -> ActorResult<Vec<String>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.backlink_ids(&id)
        })
        .await
    }

    pub async fn count_doogats(
        &self,
        doogat_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
        field_filters: Vec<(String, String)>,
    ) -> ActorResult<i64> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.count_doogats_filtered(&ListFilter {
                doogat_type,
                tag,
                backlinks_of,
                field_filters,
                limit: None,
                offset: None,
                ..Default::default()
            })
        })
        .await
    }

    pub async fn attach_file(
        &self,
        doogat_id: String,
        filename: String,
        bytes: Vec<u8>,
        mime: String,
    ) -> ActorResult<ddb_core::types::AttachmentInfo> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.attach_file(&doogat_id, &filename, &bytes, &mime)
        })
        .await
    }

    pub async fn detach_file(&self, doogat_id: String, filename: String) -> ActorResult<()> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.detach_file(&doogat_id, &filename)
        })
        .await
    }

    pub async fn list_attachments(
        &self,
        doogat_id: String,
    ) -> ActorResult<Vec<ddb_core::types::AttachmentInfo>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.list_attachments(&doogat_id)
        })
        .await
    }

    pub async fn compact(
        &self,
        force: bool,
        no_backup: bool,
        backup_path: Option<String>,
    ) -> ActorResult<CompactionReport> {
        self.call_maintenance(EventIntent::None, ReloadIntent::None, move |svc| {
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
            result
        })
        .await
    }

    pub async fn run_maintenance(&self, task: Option<String>) -> ActorResult<MaintenanceReport> {
        self.call_maintenance(EventIntent::None, ReloadIntent::None, move |svc| {
            let tasks_vec: Vec<&str>;
            let tasks_opt = match &task {
                Some(t) => {
                    tasks_vec = vec![t.as_str()];
                    Some(tasks_vec.as_slice())
                }
                None => None,
            };
            svc.run_maintenance(tasks_opt)
        })
        .await
    }

    pub async fn sync(&self, remote: String, branch: String) -> ActorResult<SyncReport> {
        let remote_timeout = self.remote_timeout;
        self.call_maintenance(EventIntent::None, ReloadIntent::None, move |svc| {
            let result = svc.sync_with_timeout(&remote, &branch, remote_timeout);
            if result.is_ok() {
                if let Err(e) = svc.rebuild_if_stale() {
                    tracing::warn!(%e, "actor: index rebuild after sync failed");
                }
            }
            result
        })
        .await
    }

    pub async fn nosql_get(&self, id: String) -> ActorResult<Option<ParsedDoogat>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.nosql_get(&id)
        })
        .await
    }

    pub async fn nosql_scan_type(&self, type_name: String) -> ActorResult<Vec<String>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.nosql_scan_type(&type_name)
        })
        .await
    }

    pub async fn nosql_scan_tag(&self, tag: String) -> ActorResult<Vec<String>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.nosql_scan_tag(&tag)
        })
        .await
    }

    pub async fn nosql_backlinks(&self, id: String) -> ActorResult<Vec<String>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.nosql_backlinks(&id)
        })
        .await
    }

    pub async fn unlinked_mentions(&self, id: String) -> ActorResult<Vec<UnlinkedMention>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.unlinked_mentions(&id)
        })
        .await
    }

    pub async fn suggest_links(&self, id: String, limit: usize) -> ActorResult<Vec<Suggestion>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.suggest_links(&id, limit)
        })
        .await
    }

    pub async fn stale_doogats(
        &self,
        type_filter: Option<String>,
    ) -> ActorResult<Vec<StaleDoogat>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.stale_doogats(type_filter.as_deref())
        })
        .await
    }

    pub async fn orphan_doogats(
        &self,
        type_filter: Option<String>,
    ) -> ActorResult<Vec<OrphanDoogat>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.orphan_doogats(type_filter.as_deref())
        })
        .await
    }

    pub async fn sequence_info(&self, id: String) -> ActorResult<SequenceInfo> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.sequence_info(&id)
        })
        .await
    }

    pub async fn sequence_children(&self, id: String) -> ActorResult<Vec<SequenceNode>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.sequence_children(&id)
        })
        .await
    }

    pub async fn sequence_breadcrumb(&self, id: String) -> ActorResult<Vec<SequenceNode>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.sequence_breadcrumb(&id)
        })
        .await
    }

    pub async fn broken_sequences(&self) -> ActorResult<Vec<BrokenSequence>> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.broken_sequences()
        })
        .await
    }

    pub async fn health_check(&self) -> ActorResult<bool> {
        self.call(EventIntent::None, ReloadIntent::None, move |svc| {
            svc.health_check()
        })
        .await
    }

    pub async fn upsert_singleton(
        &self,
        type_name: String,
        fields: std::collections::BTreeMap<String, ddb_core::types::Value>,
    ) -> ActorResult<UpsertOutcome> {
        self.call(
            EventIntent::Upsert {
                type_name: type_name.clone(),
            },
            ReloadIntent::None,
            move |svc| svc.upsert_singleton(&type_name, fields),
        )
        .await
    }

    pub async fn apply_schema(
        &self,
        schema_doc: String,
        dry_run: bool,
        allow_destructive: bool,
    ) -> ActorResult<AppOutput<SchemaApplyReport>> {
        self.call(
            EventIntent::None,
            ReloadIntent::SchemaApply { dry_run },
            move |svc| {
                let cmd = ddb_core::app_contract::ApplySchemaCommand {
                    schema_doc,
                    dry_run,
                    allow_destructive,
                };
                svc.apply_schema(cmd)
            },
        )
        .await
    }
}

/// The blocking actor loop, runs on its own OS thread.
fn actor_loop(mut svc: DoogatService, mut rx: mpsc::Receiver<ActorRun>, event_bus: EventBus) {
    while let Some(run) = rx.blocking_recv() {
        // The closure IS the dispatch and emits its own events at the boundary.
        run(&mut svc, &event_bus);
    }
}
