use std::path::PathBuf;

use chrono::Utc;
use tokio::sync::{mpsc, oneshot};
use zdb_core::error::ZettelError;
use zdb_core::git_ops::GitRepo;
use zdb_core::indexer::Index;
use zdb_core::parser;
use zdb_core::sql_engine::{schema_from_parsed, SqlEngine, SqlResult};
use zdb_core::types::{
    CompactionReport, MaintenanceReport, PaginatedSearchResult, ParsedZettel, SyncReport,
    TableSchema, ZettelMeta,
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
    FilteredList(crate::read_pool::FilteredListQuery),
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
        let _ = GitRepo::open(&repo_path)?;

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
        q: crate::read_pool::FilteredListQuery,
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
    let repo = match GitRepo::open(&repo_path) {
        Ok(r) => r,
        Err(e) => {
            log::error!("actor: failed to open repo: {e}");
            return;
        }
    };

    let db_path = repo_path.join(".zdb/index.db");
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let index = match Index::open(&db_path) {
        Ok(i) => i,
        Err(e) => {
            log::error!("actor: failed to open index: {e}");
            return;
        }
    };

    // Ensure index is up to date
    if let Err(e) = index.rebuild_if_stale(&repo) {
        log::warn!("actor: index rebuild on startup failed: {e}");
    }

    // Open redb NoSQL index
    let redb_index = {
        let redb_path = repo_path.join(".zdb/nosql.redb");
        match zdb_core::nosql::RedbIndex::open(&redb_path) {
            Ok(ri) => {
                // Rebuild redb on startup to ensure it's in sync
                if let Err(e) = ri.rebuild(&repo) {
                    log::warn!("actor: redb rebuild failed: {e}");
                }
                Some(ri)
            }
            Err(e) => {
                log::warn!("actor: failed to open redb index: {e}");
                None
            }
        }
    };

    while let Some(msg) = rx.blocking_recv() {
        // Capture delete ID and type before cmd is moved (zettel won't exist after delete)
        let (delete_id, delete_type) = match &msg.cmd {
            ActorCommand::DeleteZettel { id } => (
                Some(id.clone()),
                get_zettel(&repo, &index, id)
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

        let reply = handle_command(&repo, &index, &repo_path, redb_index.as_ref(), msg.cmd);

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

        // Dual-write to redb for successful mutations
        if let Some(ref ri) = redb_index {
            match &reply {
                ActorReply::Zettel(r) if mutation_kind.is_some() => {
                    if let Ok(z) = r.as_ref() {
                        if let Err(e) = ri.index_zettel(z) {
                            log::warn!("redb dual-write failed: {e}");
                        }
                    }
                }
                ActorReply::Deleted(Ok(())) => {
                    if let Some(ref did) = delete_id {
                        if let Err(e) = ri.remove_zettel(did) {
                            log::warn!("redb dual-write (delete) failed: {e}");
                        }
                    }
                }
                _ => {}
            }
        }

        let _ = msg.reply.send(reply);
    }
}

fn handle_command(
    repo: &GitRepo,
    index: &Index,
    repo_path: &std::path::Path,
    redb: Option<&zdb_core::nosql::RedbIndex>,
    cmd: ActorCommand,
) -> ActorReply {
    match cmd {
        ActorCommand::NoSqlGet { id } => ActorReply::NoSqlZettel(Box::new(
            redb.map(|r| r.get(&id))
                .unwrap_or(Err(ZettelError::Validation("nosql not available".into()))),
        )),
        ActorCommand::NoSqlScanType { type_name } => ActorReply::NoSqlIds(
            redb.map(|r| r.scan_by_type(&type_name))
                .unwrap_or(Err(ZettelError::Validation("nosql not available".into()))),
        ),
        ActorCommand::NoSqlScanTag { tag } => ActorReply::NoSqlIds(
            redb.map(|r| r.scan_by_tag(&tag))
                .unwrap_or(Err(ZettelError::Validation("nosql not available".into()))),
        ),
        ActorCommand::NoSqlBacklinks { id } => ActorReply::NoSqlIds(
            redb.map(|r| r.backlinks(&id))
                .unwrap_or(Err(ZettelError::Validation("nosql not available".into()))),
        ),
        _ => handle_command_shared(repo, index, repo_path, cmd),
    }
}

fn handle_command_shared(
    repo: &GitRepo,
    index: &Index,
    repo_path: &std::path::Path,
    cmd: ActorCommand,
) -> ActorReply {
    match cmd {
        ActorCommand::GetZettel { id } => {
            ActorReply::Zettel(Box::new(get_zettel(repo, index, &id)))
        }
        ActorCommand::ListZettels {
            zettel_type,
            tag,
            backlinks_of,
            field_filters,
            limit,
            offset,
        } => ActorReply::ZettelList(list_zettels(
            repo,
            index,
            ListQuery {
                zettel_type,
                tag,
                backlinks_of,
                field_filters,
                limit,
                offset,
            },
        )),
        ActorCommand::Search {
            query,
            limit,
            offset,
        } => ActorReply::SearchResults(index.search_paginated(&query, limit, offset)),
        ActorCommand::CreateZettel {
            title,
            body,
            tags,
            zettel_type,
        } => ActorReply::Zettel(Box::new(create_zettel(
            repo,
            index,
            repo_path,
            title,
            body,
            tags,
            zettel_type,
        ))),
        ActorCommand::UpdateZettel {
            id,
            title,
            body,
            tags,
            zettel_type,
        } => ActorReply::Zettel(Box::new(update_zettel(
            repo,
            index,
            &id,
            title,
            body,
            tags,
            zettel_type,
        ))),
        ActorCommand::DeleteZettel { id } => ActorReply::Deleted(delete_zettel(repo, index, &id)),
        ActorCommand::ExecuteSql { sql } => {
            let mut engine = SqlEngine::new(index, repo);
            ActorReply::SqlResult(engine.execute(&sql))
        }
        ActorCommand::GetTypeSchemas => ActorReply::TypeSchemas(get_type_schemas(repo, index)),
        ActorCommand::GetBacklinks { id } => ActorReply::Backlinks(index.backlinks(&id)),
        ActorCommand::CountZettels {
            zettel_type,
            tag,
            backlinks_of,
            field_filters,
        } => ActorReply::Count(count_zettels(
            index,
            zettel_type,
            tag,
            backlinks_of,
            &field_filters,
        )),
        ActorCommand::FilteredList(q) => ActorReply::ZettelList(filtered_list(repo, index, &q)),
        ActorCommand::AggregateQuery { sql, params } => ActorReply::AggregateRow(
            index
                .query_raw_with_params(&sql, &params)
                .map(|rows| rows.into_iter().next().unwrap_or_default()),
        ),
        ActorCommand::AttachFile {
            zettel_id,
            filename,
            bytes,
            mime,
        } => {
            let id = zdb_core::types::ZettelId(zettel_id);
            ActorReply::Attachment(zdb_core::attachments::attach_file(
                repo, index, &id, &filename, &bytes, &mime,
            ))
        }
        ActorCommand::DetachFile {
            zettel_id,
            filename,
        } => {
            let id = zdb_core::types::ZettelId(zettel_id);
            ActorReply::Deleted(zdb_core::attachments::detach_file(
                repo, index, &id, &filename,
            ))
        }
        ActorCommand::ListAttachments { zettel_id } => {
            let id = zdb_core::types::ZettelId(zettel_id);
            ActorReply::AttachmentList(zdb_core::attachments::list_attachments(repo, &id))
        }
        ActorCommand::Compact {
            force,
            no_backup,
            backup_path,
        } => ActorReply::Maintenance(handle_compact(
            repo,
            index,
            repo_path,
            force,
            no_backup,
            backup_path,
        )),
        ActorCommand::GitMaintenance { task } => {
            let tasks_vec: Vec<&str>;
            let tasks_opt = match &task {
                Some(t) => {
                    tasks_vec = vec![t.as_str()];
                    Some(tasks_vec.as_slice())
                }
                None => None,
            };
            ActorReply::GitMaintenance(zdb_core::maintenance::run(&repo.path, tasks_opt))
        }
        ActorCommand::Sync { remote, branch } => {
            ActorReply::SyncResult(run_sync(repo, index, &remote, &branch))
        }
        // NoSQL variants are handled in handle_command before delegation
        ActorCommand::NoSqlGet { .. }
        | ActorCommand::NoSqlScanType { .. }
        | ActorCommand::NoSqlScanTag { .. }
        | ActorCommand::NoSqlBacklinks { .. } => {
            unreachable!("NoSQL commands handled in handle_command")
        }
    }
}

pub(crate) fn get_zettel(repo: &GitRepo, index: &Index, id: &str) -> ActorResult<ParsedZettel> {
    let path = index.resolve_path(id)?;
    let content = repo.read_file(&path)?;
    parser::parse(&content, &path)
}

pub(crate) struct ListQuery {
    pub zettel_type: Option<String>,
    pub tag: Option<String>,
    pub backlinks_of: Option<String>,
    pub field_filters: Vec<(String, String)>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub(crate) fn list_zettels(
    repo: &GitRepo,
    index: &Index,
    q: ListQuery,
) -> ActorResult<Vec<ParsedZettel>> {
    let sql = build_filtered_sql(
        q.zettel_type.as_deref(),
        q.tag.as_deref(),
        q.backlinks_of.as_deref(),
        &q.field_filters,
        q.limit,
        q.offset,
    );
    let rows = index.query_raw(&sql)?;

    let mut zettels = Vec::new();
    for row in rows {
        if row.len() >= 2 {
            let path = &row[1];
            if let Ok(content) = repo.read_file(path) {
                if let Ok(parsed) = parser::parse(&content, path) {
                    zettels.push(parsed);
                }
            }
        }
    }
    Ok(zettels)
}

/// Build SQL with quoted string literals (safe: values are internal filter strings).
fn build_filtered_sql(
    zettel_type: Option<&str>,
    tag: Option<&str>,
    backlinks_of: Option<&str>,
    field_filters: &[(String, String)],
    limit: Option<i64>,
    offset: Option<i64>,
) -> String {
    let mut conditions = Vec::new();

    if let Some(t) = zettel_type {
        conditions.push(format!("z.type = '{}'", t.replace('\'', "''")));
    }
    if let Some(t) = tag {
        conditions.push(format!(
            "z.id IN (SELECT zettel_id FROM _zdb_tags WHERE tag = '{}')",
            t.replace('\'', "''")
        ));
    }
    if let Some(bl) = backlinks_of {
        conditions.push(format!(
            "z.id IN (SELECT source_id FROM _zdb_links WHERE target_path = '{}')",
            bl.replace('\'', "''")
        ));
    }
    for (key, value) in field_filters {
        conditions.push(format!(
            "z.id IN (SELECT zettel_id FROM _zdb_fields WHERE key = '{}' AND value = '{}')",
            key.replace('\'', "''"),
            value.replace('\'', "''")
        ));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let limit_clause = match (limit, offset) {
        (Some(l), Some(o)) => format!(" LIMIT {l} OFFSET {o}"),
        (Some(l), None) => format!(" LIMIT {l}"),
        (None, Some(o)) => format!(" LIMIT -1 OFFSET {o}"),
        (None, None) => String::new(),
    };

    format!("SELECT z.id, z.path FROM zettels z{where_clause} ORDER BY z.id DESC{limit_clause}")
}

pub(crate) fn filtered_list(
    repo: &GitRepo,
    index: &Index,
    q: &crate::read_pool::FilteredListQuery,
) -> ActorResult<Vec<ParsedZettel>> {
    // Combine where_sql and tag filter
    let mut conditions = Vec::new();
    if !q.where_sql.is_empty() {
        conditions.push(q.where_sql.to_string());
    }
    if let Some(t) = &q.tag {
        conditions.push(format!(
            "id IN (SELECT zettel_id FROM _zdb_tags WHERE tag = '{}')",
            t.replace('\'', "''")
        ));
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let order = q.order_sql.as_deref().unwrap_or("id DESC");
    let limit_clause = match (q.limit, q.offset) {
        (Some(l), Some(o)) => format!(" LIMIT {l} OFFSET {o}"),
        (Some(l), None) => format!(" LIMIT {l}"),
        (None, Some(o)) => format!(" LIMIT -1 OFFSET {o}"),
        (None, None) => String::new(),
    };

    let sql = format!(
        "SELECT id FROM \"{}\"{where_clause} ORDER BY {order}{limit_clause}",
        q.table_name
    );

    let rows = index.query_raw_with_params(&sql, &q.params)?;

    let mut zettels = Vec::new();
    for row in rows {
        if let Some(id) = row.first() {
            if let Ok(path) = index.resolve_path(id) {
                if let Ok(content) = repo.read_file(&path) {
                    if let Ok(parsed) = parser::parse(&content, &path) {
                        zettels.push(parsed);
                    }
                }
            }
        }
    }
    Ok(zettels)
}

fn create_zettel(
    repo: &GitRepo,
    index: &Index,
    repo_path: &std::path::Path,
    title: String,
    body: Option<String>,
    tags: Vec<String>,
    zettel_type: Option<String>,
) -> ActorResult<ParsedZettel> {
    let id = unique_id(repo_path);
    let id_str = id.to_string();
    let folder = zettel_type
        .as_deref()
        .map(|t| index.type_uses_folder(t, repo))
        .unwrap_or(false);
    let path = zdb_core::git_ops::zettel_path(&id_str, zettel_type.as_deref(), folder);

    let meta = ZettelMeta {
        id: Some(id),
        title: Some(title),
        date: Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
        zettel_type,
        tags,
        extra: Default::default(),
    };

    let parsed = ParsedZettel {
        meta,
        body: body.unwrap_or_default(),
        reference_section: String::new(),
        inline_fields: vec![],
        wikilinks: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: path.clone(),
    };

    let content = parser::serialize(&parsed);
    repo.commit_file(&path, &content, &format!("create zettel {id_str}"))?;
    index.index_zettel(&parsed)?;

    Ok(parsed)
}

fn update_zettel(
    repo: &GitRepo,
    index: &Index,
    id: &str,
    title: Option<String>,
    body: Option<String>,
    tags: Option<Vec<String>>,
    zettel_type: Option<String>,
) -> ActorResult<ParsedZettel> {
    let path = index.resolve_path(id)?;
    let content = repo.read_file(&path)?;
    let mut parsed = parser::parse(&content, &path)?;

    if let Some(t) = title {
        parsed.meta.title = Some(t);
    }
    if let Some(t) = tags {
        parsed.meta.tags = t;
    }
    if let Some(t) = zettel_type {
        parsed.meta.zettel_type = Some(t);
    }
    if let Some(b) = body {
        parsed.body = b;
    }

    let new_content = parser::serialize(&parsed);
    repo.commit_file(&path, &new_content, &format!("update zettel {id}"))?;
    // Re-parse to get updated inline fields/wikilinks
    let parsed = parser::parse(&new_content, &path)?;
    index.index_zettel(&parsed)?;

    Ok(parsed)
}

fn delete_zettel(repo: &GitRepo, index: &Index, id: &str) -> ActorResult<()> {
    let path = index.resolve_path(id)?;
    repo.delete_file(&path, &format!("delete zettel {id}"))?;
    index.remove_zettel(id)?;
    Ok(())
}

pub(crate) fn get_type_schemas(repo: &GitRepo, index: &Index) -> ActorResult<Vec<TableSchema>> {
    // Find all _typedef zettels
    let rows = index.query_raw("SELECT path FROM zettels WHERE type = '_typedef'")?;

    let mut schemas = Vec::new();
    for row in rows {
        if let Some(path) = row.first() {
            match repo.read_file(path) {
                Ok(content) => match parser::parse(&content, path) {
                    Ok(parsed) => match schema_from_parsed(&parsed) {
                        Ok(schema) => schemas.push(schema),
                        Err(e) => log::warn!("typedef {path}: schema extraction failed: {e}"),
                    },
                    Err(e) => log::warn!("typedef {path}: parse failed: {e}"),
                },
                Err(e) => log::warn!("typedef {path}: read failed: {e}"),
            }
        }
    }
    Ok(schemas)
}

pub(crate) fn count_zettels(
    index: &Index,
    zettel_type: Option<String>,
    tag: Option<String>,
    backlinks_of: Option<String>,
    field_filters: &[(String, String)],
) -> ActorResult<i64> {
    let select_sql = build_filtered_sql(
        zettel_type.as_deref(),
        tag.as_deref(),
        backlinks_of.as_deref(),
        field_filters,
        None,
        None,
    );
    // Wrap SELECT as COUNT subquery
    let count_sql = format!("SELECT COUNT(*) FROM ({select_sql})");
    let rows = index.query_raw(&count_sql)?;
    let count = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    Ok(count)
}

/// Generate a unique ID, spin-waiting if a zettel with that ID already exists on disk.
fn unique_id(repo_path: &std::path::Path) -> zdb_core::types::ZettelId {
    let zk = repo_path.join("zettelkasten");
    parser::generate_unique_id(|candidate| {
        let filename = format!("{candidate}.md");
        if zk.join(&filename).exists() {
            return true;
        }
        if let Ok(entries) = std::fs::read_dir(&zk) {
            for entry in entries.flatten() {
                if entry.path().is_dir() && entry.path().join(&filename).exists() {
                    return true;
                }
            }
        }
        false
    })
}

/// Run compaction + stale node detection.
fn handle_compact(
    repo: &GitRepo,
    index: &Index,
    _repo_path: &std::path::Path,
    force: bool,
    no_backup: bool,
    backup_path: Option<String>,
) -> ActorResult<CompactionReport> {
    let mgr = match zdb_core::sync_manager::SyncManager::open(repo) {
        Ok(m) => m,
        Err(ZettelError::NotFound(_)) => {
            log::info!("maintenance: no node registered, skipping compaction");
            return Ok(CompactionReport {
                files_removed: 0,
                crdt_docs_compacted: 0,
                gc_success: false,
                crdt_temp_bytes_before: 0,
                crdt_temp_bytes_after: 0,
                crdt_temp_files_before: 0,
                crdt_temp_files_after: 0,
                repo_bytes_before: 0,
                repo_bytes_after: 0,
                backup_path: None,
            });
        }
        Err(e) => return Err(e),
    };

    let opts = zdb_core::types::CompactOptions {
        force,
        skip_backup: no_backup,
        backup_path: backup_path.map(std::path::PathBuf::from),
    };
    let report = zdb_core::compaction::compact(repo, &mgr, &opts)?;
    log::info!(
        "maintenance: compacted — files_removed={} crdt_compacted={} gc={} backup={}",
        report.files_removed,
        report.crdt_docs_compacted,
        if report.gc_success { "ok" } else { "failed" },
        report
            .backup_path
            .as_ref()
            .map_or("skipped".to_string(), |p| p.display().to_string()),
    );

    // Rebuild index after compaction
    index.rebuild_if_stale(repo)?;

    let config = repo.load_config().unwrap_or_default();
    match mgr.detect_stale_nodes(config.compaction.stale_ttl_days) {
        Ok(stale) => {
            if !stale.is_empty() {
                log::info!("maintenance: {} stale node(s) detected", stale.len());
            }
        }
        Err(e) => {
            log::warn!("maintenance: stale node detection failed: {e}");
        }
    }

    Ok(report)
}

/// Run sync via SyncManager.
fn run_sync(repo: &GitRepo, index: &Index, remote: &str, branch: &str) -> ActorResult<SyncReport> {
    let mut mgr = zdb_core::sync_manager::SyncManager::open(repo)?;
    let report = mgr.sync(remote, branch, index)?;
    // Rebuild index after sync
    index.rebuild_if_stale(repo)?;
    Ok(report)
}
