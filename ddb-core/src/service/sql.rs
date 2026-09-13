use crate::error::{DoogatError, Result};
use crate::sql_engine::{SqlEngine, SqlResult, TransactionBuffer};

use crate::traits::{GitBackend, IndexPort};

use super::{DoogatService, TransactionScope};

/// The first transaction verb anywhere in `sql`, if any.
///
/// A transaction verb only means something when the caller owns the service
/// across calls. A server caller does not: one `DoogatService` backs every
/// connection, so the buffer such a verb opens would park every other client's
/// writes instead of committing them to git, and a client that disappears
/// never closes it. Every statement is inspected, not just the first: the
/// engine executes a whole semicolon-joined batch before `execute` rejects it
/// for holding more than one statement, so a trailing `BEGIN` would otherwise
/// run.
fn transaction_verb(sql: &str) -> Option<&'static str> {
    use sqlparser::ast::Statement;
    use sqlparser::dialect::GenericDialect;
    use sqlparser::parser::Parser;

    let statements = Parser::parse_sql(&GenericDialect {}, sql).ok()?;
    statements.iter().find_map(|stmt| match stmt {
        Statement::StartTransaction { .. } => Some("BEGIN"),
        Statement::Commit { .. } => Some("COMMIT"),
        Statement::Rollback { .. } => Some("ROLLBACK"),
        _ => None,
    })
}

/// Take the buffer the engine still holds, for the service to keep.
///
/// A shared client cannot open a buffer for a later request. Leaving it on
/// the engine lets `Drop` roll the savepoint back, including when pre-parse
/// rewrites hid a verb from `transaction_verb`. An already-owned buffer comes
/// from the explicit transaction API (e.g. schema apply owns one transaction
/// across its helper calls), and must survive until that operation closes it.
fn detach_transaction(
    engine: &mut SqlEngine<'_>,
    scope: TransactionScope,
    caller_owned: bool,
) -> Result<Option<TransactionBuffer>> {
    match engine.suspend_transaction() {
        Some(buf) if scope == TransactionScope::Shared && !caller_owned => {
            engine.resume_transaction(buf);
            Err(DoogatError::transaction_not_supported("BEGIN"))
        }
        other => Ok(other),
    }
}

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    // ── SQL ─────────────────────────────────────────────────────────────

    pub fn execute_sql(&mut self, sql: &str) -> Result<SqlResult> {
        if self.transaction_scope == TransactionScope::Shared {
            if let Some(verb) = transaction_verb(sql) {
                return Err(DoogatError::transaction_not_supported(verb));
            }
        }
        if self.txn.is_none() {
            self.ensure_fresh()?;
        }
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        let caller_owned = self.txn.is_some();
        if let Some(buf) = self.txn.take() {
            engine.resume_transaction(buf);
        }
        let result = engine.execute(sql);
        self.txn = detach_transaction(&mut engine, self.transaction_scope, caller_owned)?;
        result
    }

    pub fn execute_batch(&mut self, sql: &str) -> Result<Vec<SqlResult>> {
        if self.txn.is_none() {
            self.ensure_fresh()?;
        }
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        let caller_owned = self.txn.is_some();
        if let Some(buf) = self.txn.take() {
            engine.resume_transaction(buf);
        }
        let results = engine.execute_batch(sql);
        // A shared caller may open and close a transaction within this batch,
        // but may not leave it for the next client.
        self.txn = detach_transaction(&mut engine, self.transaction_scope, caller_owned)?;
        results
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
    use crate::traits::SqlBackend;
    use tempfile::TempDir;

    fn fresh_svc() -> (TempDir, DoogatService) {
        let tmp = TempDir::new().unwrap();
        let svc = DoogatService::init(tmp.path()).unwrap();
        svc.reindex().unwrap();
        (tmp, svc)
    }

    fn fresh_shared_svc() -> (TempDir, DoogatService) {
        let (tmp, svc) = fresh_svc();
        assert!(svc.transaction_scope == TransactionScope::Exclusive);
        drop(svc);
        let svc = DoogatService::open_shared(tmp.path()).unwrap();
        assert!(svc.transaction_scope == TransactionScope::Shared);
        (tmp, svc)
    }

    fn assert_rejected(err: DoogatError) {
        match err {
            DoogatError::Structured { code, .. } => {
                assert_eq!(code, crate::error::codes::TRANSACTION_NOT_SUPPORTED);
            }
            other => panic!("expected TRANSACTION_NOT_SUPPORTED, got {other:?}"),
        }
    }

    /// A write reaching git is what "success" must mean: a buffered write
    /// reports an id but leaves HEAD where it was.
    fn assert_insert_reaches_git(svc: &mut DoogatService) {
        let before = svc.repo.head_oid().unwrap();
        svc.execute_sql("INSERT INTO project (name, status) VALUES ('probe', 'active')")
            .unwrap();
        assert_ne!(
            before,
            svc.repo.head_oid().unwrap(),
            "INSERT reported success without committing to git: a transaction buffer is still open"
        );
    }

    #[test]
    fn rejects_a_standalone_transaction_verb() {
        let (_tmp, mut svc) = fresh_shared_svc();

        for verb in ["BEGIN", "COMMIT", "ROLLBACK"] {
            assert_rejected(svc.execute_sql(verb).unwrap_err());
            assert!(svc.txn.is_none(), "{verb} left a buffer on the service");
        }
    }

    #[test]
    fn rejects_a_transaction_verb_in_any_position() {
        let (_tmp, mut svc) = fresh_shared_svc();
        svc.execute_sql("CREATE TABLE project (name TEXT, status TEXT)")
            .unwrap();

        assert_rejected(
            svc.execute_sql("SELECT name FROM project; BEGIN")
                .unwrap_err(),
        );
        assert!(
            svc.txn.is_none(),
            "a trailing BEGIN left a buffer on the service"
        );
        assert_insert_reaches_git(&mut svc);
    }

    #[test]
    fn rejects_a_batch_that_leaves_its_transaction_open() {
        let (_tmp, mut svc) = fresh_shared_svc();
        svc.execute_sql("CREATE TABLE project (name TEXT, status TEXT)")
            .unwrap();

        assert_rejected(
            svc.execute_batch("BEGIN; INSERT INTO project (name, status) VALUES ('a', 'active')")
                .unwrap_err(),
        );
        assert!(
            svc.txn.is_none(),
            "an unbalanced batch left a buffer on the service"
        );
        let count: i64 = svc
            .index
            .sql_conn()
            .query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "rejected batch must roll back its index writes");
        assert_insert_reaches_git(&mut svc);
    }

    #[test]
    fn exclusive_batch_can_leave_a_transaction_for_the_next_call() {
        let (_tmp, mut svc) = fresh_svc();
        svc.execute_sql("CREATE TABLE project (name TEXT, status TEXT)")
            .unwrap();
        let before = svc.repo.head_oid().unwrap();
        svc.execute_batch("BEGIN; INSERT INTO project (name, status) VALUES ('kept', 'active')")
            .unwrap();
        assert!(svc.txn.is_some());
        assert_eq!(svc.repo.head_oid().unwrap(), before);
        svc.execute_sql("COMMIT").unwrap();
        assert_ne!(svc.repo.head_oid().unwrap(), before);
    }

    #[test]
    fn shared_batch_failure_rolls_back_before_the_next_client() {
        let (_tmp, mut svc) = fresh_shared_svc();
        svc.execute_sql("CREATE TABLE project (name TEXT, status TEXT)")
            .unwrap();
        let before = svc.repo.head_oid().unwrap();
        assert_rejected(svc.execute_batch(
            "BEGIN; INSERT INTO project (name, status) VALUES ('discarded', 'active'); SELECT missing FROM project"
        ).unwrap_err());
        assert!(svc.txn.is_none());
        assert_eq!(svc.repo.head_oid().unwrap(), before);
        let count: i64 = svc
            .index
            .sql_conn()
            .query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert_insert_reaches_git(&mut svc);
    }

    #[test]
    fn accepts_a_batch_that_closes_its_own_transaction() {
        let (_tmp, mut svc) = fresh_shared_svc();
        svc.execute_sql("CREATE TABLE project (name TEXT, status TEXT)")
            .unwrap();

        svc.execute_batch(
            "BEGIN; INSERT INTO project (name, status) VALUES ('a', 'active'); COMMIT",
        )
        .unwrap();
        assert!(svc.txn.is_none(), "a balanced batch left a buffer behind");
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
