use crate::error::{DoogatError, Result};
use crate::sql_engine::{SqlEngine, SqlResult};

use crate::traits::GitBackend;

use super::DoogatService;

impl<G: GitBackend> DoogatService<G> {
    // ── SQL ─────────────────────────────────────────────────────────────

    pub fn execute_sql(&mut self, sql: &str) -> Result<SqlResult> {
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
        engine.execute("COMMIT").inspect_err(|_| {
            self.txn = engine.suspend_transaction();
        })?;
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
