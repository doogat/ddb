//! Read-only connection pool for concurrent query execution.
//!
//! Routes read operations to `spawn_blocking` tasks with fresh
//! `DoogatService` handles, bypassing the single-writer actor.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use ddb_core::error::DoogatError;
use ddb_core::service::DoogatService;
use ddb_core::sql_engine::SqlResult;
use ddb_core::types::{
    BrokenSequence, ListFilter, OrphanDoogat, PaginatedSearchResult, ParsedDoogat, SearchFilters,
    SequenceInfo, SequenceNode, StaleDoogat, Suggestion, TableSchema, TypedListQuery,
    UnlinkedMention,
};

type Result<T> = std::result::Result<T, DoogatError>;

/// Pool of read-only connections for concurrent query execution.
///
/// Each read acquires a semaphore permit and runs on `spawn_blocking`
/// with its own `DoogatService` handle. Write operations must still
/// go through [`crate::actor::ActorHandle`].
#[derive(Clone)]
pub struct ReadPool {
    inner: Arc<Inner>,
}

struct Inner {
    repo_path: PathBuf,
    semaphore: Arc<Semaphore>,
}

impl ReadPool {
    pub fn new(repo_path: PathBuf, pool_size: usize) -> Result<Self> {
        let pool_size = pool_size.max(1);

        Ok(Self {
            inner: Arc::new(Inner {
                repo_path,
                semaphore: Arc::new(Semaphore::new(pool_size)),
            }),
        })
    }

    pub fn default_pool_size() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get().min(4))
            .unwrap_or(2)
    }

    // --- DoogatService reads ---

    pub async fn get_doogat(&self, id: String) -> Result<ParsedDoogat> {
        self.with_service(move |svc| svc.get_doogat_parsed(&id))
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
    ) -> Result<Vec<ParsedDoogat>> {
        self.with_service(move |svc| {
            svc.list_doogats_filtered(&ListFilter {
                doogat_type,
                tag,
                backlinks_of,
                field_filters,
                limit,
                offset,
            })
        })
        .await
    }

    pub async fn filtered_list(&self, q: TypedListQuery) -> Result<Vec<ParsedDoogat>> {
        self.with_service(move |svc| svc.typed_filtered_list(&q))
            .await
    }

    pub async fn get_type_schemas(&self) -> Result<Vec<TableSchema>> {
        self.with_service(move |svc| svc.list_type_schemas())
            .await
    }

    pub async fn execute_select(&self, sql: String) -> Result<SqlResult> {
        self.with_service_mut(move |svc| svc.execute_sql(&sql))
            .await
    }

    pub async fn search(
        &self,
        query: String,
        limit: usize,
        offset: usize,
        filters: SearchFilters,
    ) -> Result<PaginatedSearchResult> {
        self.with_service(move |svc| {
            svc.search_paginated_filtered(&query, limit, offset, &filters)
        })
        .await
    }

    pub async fn count_doogats(
        &self,
        doogat_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
        field_filters: Vec<(String, String)>,
    ) -> Result<i64> {
        self.with_service(move |svc| {
            svc.count_doogats_filtered(&ListFilter {
                doogat_type,
                tag,
                backlinks_of,
                field_filters,
                limit: None,
                offset: None,
            })
        })
        .await
    }

    pub async fn get_backlinks(&self, id: String) -> Result<Vec<String>> {
        self.with_service(move |svc| svc.backlink_ids(&id)).await
    }

    pub async fn query_checkboxes(
        &self,
        state: Option<String>,
        doogat_id: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Vec<Vec<String>>> {
        self.with_service(move |svc| {
            let mut conditions = Vec::new();
            let mut params: Vec<rusqlite::types::Value> = Vec::new();

            if let Some(s) = state {
                conditions.push(format!("c.state = ?{}", params.len() + 1));
                params.push(rusqlite::types::Value::Text(s));
            }
            if let Some(id) = doogat_id {
                conditions.push(format!("c.doogat_id = ?{}", params.len() + 1));
                params.push(rusqlite::types::Value::Text(id));
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

            let sql = format!(
                "SELECT c.doogat_id, z.title, c.state, c.content, c.date, c.due_date, c.line_number, c.indent_level \
                 FROM _ddb_checkboxes c \
                 JOIN doogats z ON c.doogat_id = z.id{where_clause} \
                 ORDER BY c.doogat_id DESC, c.line_number ASC{limit_clause}"
            );

            svc.query_raw_with_params(&sql, &params)
        })
        .await
    }

    pub async fn aggregate_query(
        &self,
        sql: String,
        params: Vec<rusqlite::types::Value>,
    ) -> Result<Vec<String>> {
        self.with_service(move |svc| svc.aggregate_query(&sql, &params))
            .await
    }

    // --- Discovery reads ---

    pub async fn unlinked_mentions(&self, id: String) -> Result<Vec<UnlinkedMention>> {
        self.with_service(move |svc| svc.unlinked_mentions(&id))
            .await
    }

    pub async fn suggest_links(&self, id: String, limit: usize) -> Result<Vec<Suggestion>> {
        self.with_service(move |svc| svc.suggest_links(&id, limit))
            .await
    }

    pub async fn stale_doogats(&self, type_filter: Option<String>) -> Result<Vec<StaleDoogat>> {
        self.with_service(move |svc| svc.stale_doogats(type_filter.as_deref()))
            .await
    }

    pub async fn orphan_doogats(&self, type_filter: Option<String>) -> Result<Vec<OrphanDoogat>> {
        self.with_service(move |svc| svc.orphan_doogats(type_filter.as_deref()))
            .await
    }

    pub async fn sequence_info(&self, id: String) -> Result<SequenceInfo> {
        self.with_service(move |svc| svc.sequence_info(&id)).await
    }

    pub async fn sequence_children(&self, id: String) -> Result<Vec<SequenceNode>> {
        self.with_service(move |svc| svc.sequence_children(&id))
            .await
    }

    pub async fn sequence_breadcrumb(&self, id: String) -> Result<Vec<SequenceNode>> {
        self.with_service(move |svc| svc.sequence_breadcrumb(&id))
            .await
    }

    pub async fn broken_sequences(&self) -> Result<Vec<BrokenSequence>> {
        self.with_service(move |svc| svc.broken_sequences()).await
    }

    pub async fn list_tags(&self) -> Result<Vec<(String, i64)>> {
        self.with_service(move |svc| svc.list_tags()).await
    }

    // --- Dispatch helpers ---

    async fn acquire(&self) -> Result<OwnedSemaphorePermit> {
        Arc::clone(&self.inner.semaphore)
            .acquire_owned()
            .await
            .map_err(|_| DoogatError::Validation("read pool closed".into()))
    }

    async fn with_service<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&DoogatService) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self.acquire().await?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut svc = DoogatService::open(&inner.repo_path)?;
            // Actor keeps index fresh; skip integrity/staleness checks
            // to avoid PRAGMA integrity_check lock contention on Windows.
            svc.set_skip_stale_check(true);
            f(&svc)
        })
        .await
        .map_err(|e| DoogatError::Validation(format!("read task panicked: {e}")))?
    }

    async fn with_service_mut<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut DoogatService) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self.acquire().await?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut svc = DoogatService::open(&inner.repo_path)?;
            svc.set_skip_stale_check(true);
            f(&mut svc)
        })
        .await
        .map_err(|e| DoogatError::Validation(format!("read task panicked: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pool_size_is_bounded() {
        let size = ReadPool::default_pool_size();
        assert!(
            (1..=4).contains(&size),
            "pool size {size} out of range 1..=4"
        );
    }

    fn setup_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let svc = DoogatService::init(dir.path()).unwrap();
        // Ensure index is built
        svc.reindex().unwrap();
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    #[tokio::test]
    async fn read_fails_on_invalid_path() {
        let pool = ReadPool::new(PathBuf::from("/nonexistent/repo"), 2).unwrap();
        let result = pool.search("anything".to_string(), 10, 0, SearchFilters::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_doogat_not_found() {
        let (_dir, path) = setup_repo();
        let pool = ReadPool::new(path, 2).unwrap();
        let result = pool.get_doogat("99999999999999".to_string()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn search_empty_repo() {
        let (_dir, path) = setup_repo();
        let pool = ReadPool::new(path, 2).unwrap();
        let result = pool.search("anything".to_string(), 10, 0).await.unwrap();
        assert!(result.hits.is_empty());
        assert_eq!(result.total_count, 0);
    }

    #[tokio::test]
    async fn count_doogats_empty_repo() {
        let (_dir, path) = setup_repo();
        let pool = ReadPool::new(path, 2).unwrap();
        let count = pool.count_doogats(None, None, None, vec![]).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn concurrent_reads_succeed() {
        let (_dir, path) = setup_repo();
        let pool = ReadPool::new(path, 4).unwrap();

        // Fire 8 concurrent searches — all should succeed
        let mut handles = Vec::new();
        for i in 0..8 {
            let p = pool.clone();
            handles.push(tokio::spawn(async move {
                p.search(format!("query{i}"), 10, 0, SearchFilters::default()).await
            }));
        }
        for h in handles {
            let result = h.await.unwrap();
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn execute_select_works() {
        let (_dir, path) = setup_repo();
        let pool = ReadPool::new(path, 2).unwrap();
        let result = pool
            .execute_select("SELECT 1 AS n".to_string())
            .await
            .unwrap();
        match result {
            SqlResult::Rows { rows, columns } => {
                assert_eq!(columns, vec!["n"]);
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], "1");
            }
            _ => panic!("expected Rows result"),
        }
    }

    #[tokio::test]
    async fn backlinks_empty() {
        let (_dir, path) = setup_repo();
        let pool = ReadPool::new(path, 2).unwrap();
        let links = pool
            .get_backlinks("20260101000000".to_string())
            .await
            .unwrap();
        assert!(links.is_empty());
    }

    #[tokio::test]
    async fn read_after_write_visible() {
        let (_dir, path) = setup_repo();

        // Write a doogat via DoogatService (the writer path)
        let svc = DoogatService::open(&path).unwrap();
        let id = svc
            .create_doogat("ReadAfterWrite", &[], Some("note"), "Body text.")
            .unwrap();

        // Read via ReadPool — should see the write immediately (WAL)
        let pool = ReadPool::new(path, 2).unwrap();
        let result = pool.get_doogat(id.clone()).await.unwrap();
        assert_eq!(result.meta.id.as_ref().map(|z| z.0.as_str()), Some(id.as_str()));
        assert_eq!(result.meta.title.as_deref(), Some("ReadAfterWrite"));
    }

    #[tokio::test]
    async fn pool_exhaustion_queues_reads() {
        let (_dir, path) = setup_repo();
        // Pool of 1 — second read must queue behind the first
        let pool = ReadPool::new(path, 1).unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let p1 = pool.clone();
        // Occupy the single slot with a blocking service call
        let slow = tokio::spawn(async move {
            p1.with_service(move |svc| {
                // Hold the permit until signaled
                rx.blocking_recv().ok();
                svc.search("hold").map(|_| ())
            })
            .await
        });

        // Give the slow task time to acquire the permit
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // This read should queue (not error) and complete once the slot frees
        let p2 = pool.clone();
        let queued = tokio::spawn(async move { p2.search("queued".into(), 10, 0, SearchFilters::default()).await });

        // Release the slow task
        tx.send(()).unwrap();
        slow.await.unwrap().unwrap();

        // Queued read should now complete
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), queued)
            .await
            .expect("queued read timed out")
            .unwrap();
        assert!(result.is_ok());
    }
}
