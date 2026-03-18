//! Read-only connection pool for concurrent query execution.
//!
//! Routes read operations to `spawn_blocking` tasks with fresh
//! `Index` and `GitRepo` handles, bypassing the single-writer actor.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use zdb_core::error::ZettelError;
use zdb_core::git_ops::GitRepo;
use zdb_core::indexer::Index;
use zdb_core::sql_engine::{SqlEngine, SqlResult};
use zdb_core::types::{
    BrokenSequence, OrphanZettel, PaginatedSearchResult, ParsedZettel, SequenceInfo, SequenceNode,
    StaleZettel, Suggestion, TableSchema, UnlinkedMention,
};

use crate::actor;

type Result<T> = std::result::Result<T, ZettelError>;

/// Query parameters for [`ReadPool::filtered_list`].
pub struct FilteredListQuery {
    pub table_name: String,
    pub where_sql: String,
    pub params: Vec<rusqlite::types::Value>,
    pub order_sql: Option<String>,
    pub tag: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Pool of read-only connections for concurrent query execution.
///
/// Each read acquires a semaphore permit and runs on `spawn_blocking`
/// with its own `Index` + `GitRepo` handles. Write operations must
/// still go through [`crate::actor::ActorHandle`].
#[derive(Clone)]
pub struct ReadPool {
    inner: Arc<Inner>,
}

struct Inner {
    repo_path: PathBuf,
    db_path: PathBuf,
    semaphore: Arc<Semaphore>,
}

impl ReadPool {
    pub fn new(repo_path: PathBuf, pool_size: usize) -> Result<Self> {
        let pool_size = pool_size.max(1);
        let db_path = repo_path.join(".zdb/index.db");

        Ok(Self {
            inner: Arc::new(Inner {
                repo_path,
                db_path,
                semaphore: Arc::new(Semaphore::new(pool_size)),
            }),
        })
    }

    pub fn default_pool_size() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get().min(4))
            .unwrap_or(2)
    }

    // --- Index + GitRepo reads ---

    pub async fn get_zettel(&self, id: String) -> Result<ParsedZettel> {
        self.with_index_repo(move |index, repo| actor::get_zettel(repo, index, &id))
            .await
    }

    pub async fn list_zettels(
        &self,
        zettel_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
        field_filters: Vec<(String, String)>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Vec<ParsedZettel>> {
        self.with_index_repo(move |index, repo| {
            actor::list_zettels(
                repo,
                index,
                actor::ListQuery {
                    zettel_type,
                    tag,
                    backlinks_of,
                    field_filters,
                    limit,
                    offset,
                },
            )
        })
        .await
    }

    pub async fn filtered_list(&self, q: FilteredListQuery) -> Result<Vec<ParsedZettel>> {
        self.with_index_repo(move |index, repo| actor::filtered_list(repo, index, &q))
            .await
    }

    pub async fn get_type_schemas(&self) -> Result<Vec<TableSchema>> {
        self.with_index_repo(move |index, repo| actor::get_type_schemas(repo, index))
            .await
    }

    pub async fn execute_select(&self, sql: String) -> Result<SqlResult> {
        self.with_index_repo(move |index, repo| {
            let mut engine = SqlEngine::new(index, repo);
            engine.execute(&sql)
        })
        .await
    }

    // --- Index-only reads ---

    pub async fn search(
        &self,
        query: String,
        limit: usize,
        offset: usize,
    ) -> Result<PaginatedSearchResult> {
        self.with_index(move |index| index.search_paginated(&query, limit, offset))
            .await
    }

    pub async fn count_zettels(
        &self,
        zettel_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
        field_filters: Vec<(String, String)>,
    ) -> Result<i64> {
        self.with_index(move |index| {
            actor::count_zettels(index, zettel_type, tag, backlinks_of, &field_filters)
        })
        .await
    }

    pub async fn get_backlinks(&self, id: String) -> Result<Vec<String>> {
        self.with_index(move |index| index.backlinks(&id)).await
    }

    pub async fn query_checkboxes(
        &self,
        state: Option<String>,
        zettel_id: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Vec<Vec<String>>> {
        self.with_index(move |index| {
            let mut conditions = Vec::new();
            let mut params: Vec<rusqlite::types::Value> = Vec::new();

            if let Some(s) = state {
                conditions.push(format!("c.state = ?{}", params.len() + 1));
                params.push(rusqlite::types::Value::Text(s));
            }
            if let Some(id) = zettel_id {
                conditions.push(format!("c.zettel_id = ?{}", params.len() + 1));
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
                "SELECT c.zettel_id, z.title, c.state, c.content, c.date, c.due_date, c.line_number, c.indent_level \
                 FROM _zdb_checkboxes c \
                 JOIN zettels z ON c.zettel_id = z.id{where_clause} \
                 ORDER BY c.zettel_id DESC, c.line_number ASC{limit_clause}"
            );

            index.query_raw_with_params(&sql, &params)
        })
        .await
    }

    pub async fn aggregate_query(
        &self,
        sql: String,
        params: Vec<rusqlite::types::Value>,
    ) -> Result<Vec<String>> {
        self.with_index(move |index| {
            index
                .query_raw_with_params(&sql, &params)
                .map(|rows| rows.into_iter().next().unwrap_or_default())
        })
        .await
    }

    // --- Discovery reads ---

    pub async fn unlinked_mentions(&self, id: String) -> Result<Vec<UnlinkedMention>> {
        self.with_index(move |index| index.unlinked_mentions(&id))
            .await
    }

    pub async fn suggest_links(&self, id: String, limit: usize) -> Result<Vec<Suggestion>> {
        self.with_index(move |index| index.suggest_links(&id, limit))
            .await
    }

    pub async fn stale_zettels(&self, type_filter: Option<String>) -> Result<Vec<StaleZettel>> {
        self.with_index_repo(move |index, repo| index.stale_zettels(repo, type_filter.as_deref()))
            .await
    }

    pub async fn orphan_zettels(&self, type_filter: Option<String>) -> Result<Vec<OrphanZettel>> {
        self.with_index(move |index| index.orphan_zettels(type_filter.as_deref()))
            .await
    }

    pub async fn sequence_info(&self, id: String) -> Result<SequenceInfo> {
        self.with_index(move |index| index.sequence_info(&id)).await
    }

    pub async fn sequence_children(&self, id: String) -> Result<Vec<SequenceNode>> {
        self.with_index(move |index| index.sequence_children(&id))
            .await
    }

    pub async fn sequence_breadcrumb(&self, id: String) -> Result<Vec<SequenceNode>> {
        self.with_index(move |index| index.sequence_breadcrumb(&id))
            .await
    }

    pub async fn broken_sequences(&self) -> Result<Vec<BrokenSequence>> {
        self.with_index(move |index| index.broken_sequences()).await
    }

    // --- Dispatch helpers ---

    async fn acquire(&self) -> Result<OwnedSemaphorePermit> {
        Arc::clone(&self.inner.semaphore)
            .acquire_owned()
            .await
            .map_err(|_| ZettelError::Validation("read pool closed".into()))
    }

    async fn with_index<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Index) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self.acquire().await?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let index = Index::open(&inner.db_path)?;
            f(&index)
        })
        .await
        .map_err(|e| ZettelError::Validation(format!("read task panicked: {e}")))?
    }

    async fn with_index_repo<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Index, &GitRepo) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self.acquire().await?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let index = Index::open(&inner.db_path)?;
            let repo = GitRepo::open(&inner.repo_path)?;
            f(&index, &repo)
        })
        .await
        .map_err(|e| ZettelError::Validation(format!("read task panicked: {e}")))?
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
        let repo = GitRepo::init(dir.path()).unwrap();
        // Create index so ReadPool can validate
        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let index = Index::open(&db_path).unwrap();
        index.rebuild(&repo).unwrap();
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    #[tokio::test]
    async fn read_fails_on_invalid_path() {
        let pool = ReadPool::new(PathBuf::from("/nonexistent/repo"), 2).unwrap();
        let result = pool.search("anything".to_string(), 10, 0).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_zettel_not_found() {
        let (_dir, path) = setup_repo();
        let pool = ReadPool::new(path, 2).unwrap();
        let result = pool.get_zettel("99999999999999".to_string()).await;
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
    async fn count_zettels_empty_repo() {
        let (_dir, path) = setup_repo();
        let pool = ReadPool::new(path, 2).unwrap();
        let count = pool.count_zettels(None, None, None, vec![]).await.unwrap();
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
                p.search(format!("query{i}"), 10, 0).await
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

        // Write a zettel via git + index (the actor/writer path)
        let repo = GitRepo::open(&path).unwrap();
        let id = "20260314120000";
        let rel_path = format!("zettelkasten/{id}.md");
        let content =
            format!("---\ntitle: ReadAfterWrite\ntype: note\ncreated: {id}\n---\nBody text.\n");
        repo.commit_file(&rel_path, &content, "add test zettel")
            .unwrap();
        let db_path = path.join(".zdb/index.db");
        let index = Index::open(&db_path).unwrap();
        let parsed = zdb_core::parser::parse(&content, &rel_path).unwrap();
        index.index_zettel(&parsed).unwrap();

        // Read via ReadPool — should see the write immediately (WAL)
        let pool = ReadPool::new(path, 2).unwrap();
        let result = pool.get_zettel(id.to_string()).await.unwrap();
        assert_eq!(result.meta.id.as_ref().map(|z| z.0.as_str()), Some(id));
        assert_eq!(result.meta.title.as_deref(), Some("ReadAfterWrite"));
    }

    #[tokio::test]
    async fn pool_exhaustion_queues_reads() {
        let (_dir, path) = setup_repo();
        // Pool of 1 — second read must queue behind the first
        let pool = ReadPool::new(path, 1).unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let p1 = pool.clone();
        // Occupy the single slot with a blocking search
        let slow = tokio::spawn(async move {
            p1.with_index(move |index| {
                // Hold the permit until signaled
                rx.blocking_recv().ok();
                index.search("hold").map(|_| ())
            })
            .await
        });

        // Give the slow task time to acquire the permit
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // This read should queue (not error) and complete once the slot frees
        let p2 = pool.clone();
        let queued = tokio::spawn(async move { p2.search("queued".into(), 10, 0).await });

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
