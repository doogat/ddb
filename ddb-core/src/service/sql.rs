use crate::error::{DoogatError, Result};
use crate::sql_engine::{SqlEngine, SqlResult};

use crate::traits::{GitBackend, IndexPort};

use super::DoogatService;

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    // ── SQL ─────────────────────────────────────────────────────────────

    pub fn execute_sql(&mut self, sql: &str) -> Result<SqlResult> {
        if self.txn.is_none() {
            self.ensure_fresh()?;
        }
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        if let Some(buf) = self.txn.take() {
            engine.resume_transaction(buf);
        }
        let result = engine.execute(sql).inspect_err(|_| {
            self.txn = engine.suspend_transaction();
        })?;
        self.txn = engine.suspend_transaction();
        Ok(result)
    }

    pub fn execute_batch(&mut self, sql: &str) -> Result<Vec<SqlResult>> {
        if self.txn.is_none() {
            self.ensure_fresh()?;
        }
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        if let Some(buf) = self.txn.take() {
            engine.resume_transaction(buf);
        }
        let results = engine.execute_batch(sql).inspect_err(|_| {
            self.txn = engine.suspend_transaction();
        })?;
        self.txn = engine.suspend_transaction();
        Ok(results)
    }

    pub fn begin_transaction(&mut self) -> Result<()> {
        if self.txn.is_some() {
            return Err(DoogatError::SqlEngine("transaction already active".into()));
        }
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        engine.execute("BEGIN")?;
        self.txn = engine.suspend_transaction();
        Ok(())
    }

    pub fn commit_transaction(&mut self) -> Result<()> {
        let buf = self
            .txn
            .take()
            .ok_or_else(|| DoogatError::SqlEngine("no active transaction".into()))?;
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        engine.resume_transaction(buf);
        // On COMMIT failure do NOT restore the buffer: a failed COMMIT cannot be
        // resumed, and leaving the SAVEPOINT open would poison the connection
        // (every later op would see "transaction already active"). Dropping the
        // engine with its buffer intact lets `Drop` roll back + RELEASE the
        // savepoint, and `self.txn` stays None (taken above). If the git commit
        // already landed before the failure, the index is simply left stale and
        // self-heals on the next read (git is source of truth). PRD 00161 task 10.
        engine.execute("COMMIT")?;
        Ok(())
    }

    pub fn rollback_transaction(&mut self) -> Result<()> {
        let buf = self
            .txn
            .take()
            .ok_or_else(|| DoogatError::SqlEngine("no active transaction".into()))?;
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        engine.resume_transaction(buf);
        engine.execute("ROLLBACK")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_svc() -> (TempDir, DoogatService) {
        let tmp = TempDir::new().unwrap();
        let svc = DoogatService::init(tmp.path()).unwrap();
        svc.reindex().unwrap();
        (tmp, svc)
    }

    #[test]
    fn execute_sql_inside_open_transaction_does_not_refresh_index() {
        let (_tmp, mut svc) = fresh_svc();

        // Build SQL state so a SELECT will succeed.
        svc.execute_sql("CREATE TABLE project (name TEXT, status TEXT)")
            .unwrap();
        svc.execute_sql("INSERT INTO project (name, status) VALUES ('alpha', 'active')")
            .unwrap();

        // Force index stale by writing a bogus HEAD.
        svc.index
            .store_head("0000000000000000000000000000000000000000")
            .unwrap();
        assert!(
            svc.index.is_stale(&svc.repo).unwrap(),
            "index must be stale before the test starts"
        );

        // Open a transaction then run a nested execute_sql.
        svc.begin_transaction().unwrap();
        svc.execute_sql("SELECT name, status FROM project").unwrap();

        // Index must still be stale: refresh must not fire inside an open transaction.
        assert!(
            svc.index.is_stale(&svc.repo).unwrap(),
            "index must remain stale: refresh must not run inside an open transaction"
        );

        svc.rollback_transaction().unwrap();
    }

    #[test]
    fn execute_sql_at_top_level_refreshes_stale_index() {
        let (_tmp, mut svc) = fresh_svc();

        // Build SQL state so a SELECT will succeed.
        svc.execute_sql("CREATE TABLE project (name TEXT, status TEXT)")
            .unwrap();
        svc.execute_sql("INSERT INTO project (name, status) VALUES ('alpha', 'active')")
            .unwrap();

        // Force index stale.
        svc.index
            .store_head("0000000000000000000000000000000000000000")
            .unwrap();
        assert!(
            svc.index.is_stale(&svc.repo).unwrap(),
            "index must be stale before the test starts"
        );

        // Top-level call — no open transaction.
        svc.execute_sql("SELECT name, status FROM project").unwrap();

        // Index must now be fresh: top-level execute_sql must refresh a stale index.
        assert!(
            !svc.index.is_stale(&svc.repo).unwrap(),
            "index must be fresh: top-level execute_sql must refresh a stale index"
        );
    }
}
