use crate::error::{DoogatError, Result};

use super::{SqlEngine, SqlResult, TransactionBuffer};

impl Drop for SqlEngine<'_> {
    fn drop(&mut self) {
        if self.txn.take().is_some() {
            if let Err(e) = self.index.sql_conn().execute("ROLLBACK TO ddb_txn", []) {
                tracing::warn!(error = %e, "sql_engine drop: rollback failed");
            }
            if let Err(e) = self.index.sql_conn().execute("RELEASE ddb_txn", []) {
                tracing::warn!(error = %e, "sql_engine drop: release failed");
            }
        }
    }
}

impl SqlEngine<'_> {
    pub(super) fn handle_begin(&mut self) -> Result<SqlResult> {
        if self.txn.is_some() {
            return Err(DoogatError::SqlEngine("transaction already active".into()));
        }
        self.index
            .sql_conn()
            .execute("SAVEPOINT ddb_txn", [])
            .map_err(|e| DoogatError::SqlEngine(format!("savepoint: {e}")))?;
        self.txn = Some(TransactionBuffer::default());
        Ok(SqlResult::Ok("BEGIN".into()))
    }

    pub(super) fn handle_commit(&mut self) -> Result<SqlResult> {
        let buf = self
            .txn
            .as_ref()
            .ok_or_else(|| DoogatError::SqlEngine("no active transaction".into()))?;

        // Flush buffered writes/deletes to git in a single commit.
        // Cancelled operations: if a path was written then deleted, skip both
        // (the file may not exist in git if it was created within the txn).
        let delete_paths: std::collections::HashSet<&str> =
            buf.deletes.iter().map(|d| d.path.as_str()).collect();

        let writes: Vec<(&str, &str)> = buf
            .writes
            .iter()
            .filter(|w| !delete_paths.contains(w.path.as_str()))
            .map(|w| (w.path.as_str(), w.content.as_str()))
            .collect();
        // Only delete files that exist in git (not buffer-only creations)
        let deletes: Vec<&str> = buf
            .deletes
            .iter()
            .filter(|d| self.repo.read_file(&d.path).is_ok())
            .map(|d| d.path.as_str())
            .collect();

        // Types whose materialized table must be rebuilt after the flush
        // (PRD 00161 task 10). Cloned out so the `buf` borrow can be dropped
        // before the index reads below.
        let rematerialize: Vec<String> = buf.rematerialize.clone();

        if !writes.is_empty() || !deletes.is_empty() {
            self.repo.commit_batch(&writes, &deletes, "transaction")?;
        }

        // The buffered typedef writes are now committed to git, so
        // `rematerialize_type` (which reads typedefs + rows from git) sees the
        // final schema. Runs under the still-open SAVEPOINT so the index
        // rebuild is atomic with the rest of the transaction.
        for table_name in &rematerialize {
            self.index.rematerialize_type(table_name, self.repo)?;
        }

        self.index
            .sql_conn()
            .execute("RELEASE ddb_txn", [])
            .map_err(|e| DoogatError::SqlEngine(format!("release: {e}")))?;
        // Clear txn only after both git commit and RELEASE succeed
        self.txn.take();
        Ok(SqlResult::Ok("COMMIT".into()))
    }

    pub(super) fn handle_rollback(&mut self) -> Result<SqlResult> {
        if self.txn.is_none() {
            return Err(DoogatError::SqlEngine("no active transaction".into()));
        }
        self.index
            .sql_conn()
            .execute("ROLLBACK TO ddb_txn", [])
            .map_err(|e| DoogatError::SqlEngine(format!("rollback: {e}")))?;
        self.index
            .sql_conn()
            .execute("RELEASE ddb_txn", [])
            .map_err(|e| DoogatError::SqlEngine(format!("release: {e}")))?;
        // Only clear txn after SQLite ops succeed — Drop still cleans up on failure
        self.txn.take();
        Ok(SqlResult::Ok("ROLLBACK".into()))
    }
}
