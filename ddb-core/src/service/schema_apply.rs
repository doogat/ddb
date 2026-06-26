//! `apply_schema` service verb + `describe_type` (PRD 00161 Phase 1).
//!
//! Declarative schema apply: parse a desired-schema YAML doc, diff it against
//! the live typedefs, and apply the resulting plan inside a single transaction.
//! The whole plan flushes as one git commit on success; a mid-plan failure rolls
//! the transaction back so nothing reaches git (PRD 00161 task 10 + task 11).

use crate::app_contract::{AppOutput, AppWarning, ApplySchemaCommand, SCHEMA_UNSUPPORTED_CHANGE};
use crate::error::{codes, DoogatError, ErrorValue, Result};
use crate::schema_diff::{self, plan::SchemaApplyReport};
use crate::sql_engine::SqlEngine;
use crate::traits::{GitBackend, IndexPort};
use crate::types::TableSchema;

use super::DoogatService;

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    /// Describe a type's live schema, or `None` if the type is not registered.
    ///
    /// Builds a fresh `SqlEngine` over the live index (refreshing it the same
    /// way `execute_sql` does when no transaction is open), then loads the
    /// typedef. Only the `table not found:` sentinel maps to `Ok(None)`; any
    /// other error propagates.
    pub fn describe_type(&mut self, type_name: &str) -> Result<Option<TableSchema>> {
        if self.txn.is_none() {
            self.ensure_fresh()?;
        }
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        match engine.load_schema(type_name) {
            Ok(schema) => Ok(Some(schema)),
            Err(DoogatError::SqlEngine(msg)) if msg.starts_with("table not found:") => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Apply a declarative desired-schema document.
    ///
    /// Atomic, not forward-recovery: the whole plan runs inside one transaction
    /// (task 10 + task 11). On success every op's git write — including a column
    /// rename's typedef + row moves — flushes as a single commit. On any op's
    /// failure the transaction rolls back, git HEAD is unchanged, and the error
    /// carries `SCHEMA_APPLY_PARTIAL` naming the ops that ran before the failure
    /// (now undone). Recovery is a clean re-apply.
    pub fn apply_schema(
        &mut self,
        cmd: ApplySchemaCommand,
    ) -> Result<AppOutput<SchemaApplyReport>> {
        let doc = schema_diff::desired::SchemaDoc::from_yaml(&cmd.schema_doc)?;
        let live: Vec<Option<TableSchema>> = doc
            .types
            .iter()
            .map(|t| self.describe_type(&t.table_name))
            .collect::<Result<Vec<_>>>()?;
        let plan = schema_diff::diff(&doc, &live);

        let warnings: Vec<AppWarning> = plan
            .unsupported
            .iter()
            .map(|m| AppWarning {
                code: SCHEMA_UNSUPPORTED_CHANGE,
                message: m.clone(),
            })
            .collect();

        if cmd.dry_run {
            return Ok(AppOutput {
                value: SchemaApplyReport::from_plan(&plan, true, false),
                warnings,
            });
        }

        if plan.is_empty() {
            return Ok(AppOutput {
                value: SchemaApplyReport::from_plan(&plan, false, false),
                warnings,
            });
        }

        if plan.has_destructive() && !cmd.allow_destructive {
            return Err(DoogatError::Structured {
                code: codes::SCHEMA_DESTRUCTIVE_BLOCKED,
                message:
                    "schema plan contains destructive operations (drop/rename); re-run with allow_destructive"
                        .to_string(),
                context: vec![],
            });
        }

        // Apply atomically (PRD 00161 task 10): buffer every DDL op's typedef
        // write in one transaction and flush them as a single git commit on
        // success. A mid-plan failure rolls the whole transaction back, so a
        // partially-applied plan never reaches git — superseding the Phase-1
        // per-op forward-recovery semantics.
        self.begin_transaction()?;
        let mut applied_kinds: Vec<String> = Vec::new();
        for op in &plan.ops {
            match self.execute_sql(&op.render_sql()) {
                Ok(_) => applied_kinds.push(op.kind().to_string()),
                Err(e) => {
                    // Discard the buffered writes and roll the SAVEPOINT back so
                    // nothing from this plan lands in git. The original op error
                    // is the meaningful one, but surface the rollback outcome too
                    // rather than asserting it succeeded: if the rollback itself
                    // fails the message must not claim a clean rollback (Drop is
                    // the backstop, reported here so the caller can react).
                    let rollback_outcome = match self.rollback_transaction() {
                        Ok(()) => "rolled back".to_string(),
                        Err(re) => {
                            format!("rollback ALSO failed ({re}); relying on Drop backstop")
                        }
                    };
                    return Err(DoogatError::Structured {
                        code: codes::SCHEMA_APPLY_PARTIAL,
                        message: format!(
                            "schema apply failed after {} of {} operations and {}: {}",
                            applied_kinds.len(),
                            plan.ops.len(),
                            rollback_outcome,
                            e
                        ),
                        context: vec![
                            (
                                "applied_ops".to_string(),
                                ErrorValue::List(applied_kinds.clone()),
                            ),
                            (
                                "failed_op".to_string(),
                                ErrorValue::String(op.kind().to_string()),
                            ),
                        ],
                    });
                }
            }
        }
        self.commit_transaction()?;

        Ok(AppOutput {
            value: SchemaApplyReport::from_plan(&plan, false, true),
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    fn fresh_svc() -> (TempDir, crate::service::DoogatService) {
        let tmp = TempDir::new().unwrap();
        let svc = crate::service::DoogatService::init(tmp.path()).unwrap();
        svc.reindex().unwrap();
        (tmp, svc)
    }

    /// Desired-schema YAML for type `widget` with a single frontmatter
    /// column `label`.
    const WIDGET_ONE_COLUMN: &str = "\
types:
  - name: widget
    columns:
      - name: label
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
";

    /// Desired-schema YAML for type `widget` with two columns: `label`
    /// (frontmatter) and `note` (body).
    const WIDGET_TWO_COLUMNS: &str = "\
types:
  - name: widget
    columns:
      - name: label
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
      - name: note
        data_type: TEXT
        zone: body
";

    /// Desired-schema YAML for type `widget` with THREE columns: `label`
    /// (frontmatter), `note` (body), and `extra` (body). Applying this over a
    /// live `widget` that has only `label` plans TWO `add_column` ops on the
    /// same existing type in one apply — the read-your-writes exerciser.
    const WIDGET_THREE_COLUMNS: &str = "\
types:
  - name: widget
    columns:
      - name: label
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
      - name: note
        data_type: TEXT
        zone: body
      - name: extra
        data_type: TEXT
        zone: body
";

    /// Desired-schema YAML for a DISTINCTIVE type `gadget` with a single
    /// body column `serial`. The distinctive names defeat a hardcoded
    /// `widget`/`label` literal: only a real parse of this doc yields them.
    const GADGET_ONE_COLUMN: &str = "\
types:
  - name: gadget
    columns:
      - name: serial
        data_type: TEXT
        zone: body
";

    /// Desired-schema YAML whose `types:` list contains the SAME type name
    /// (`widget`) twice — two CREATE entries for one type.
    const WIDGET_DUPLICATED: &str = "\
types:
  - name: widget
    columns:
      - name: label
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
  - name: widget
    columns:
      - name: label
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
";

    /// Desired-schema YAML for an existing `widget` (which starts with `label`)
    /// that renames `label` -> `caption` AND adds a `note` body column in ONE
    /// apply. The differ emits `RenameColumn` (additive partition, kept in doc
    /// order) followed by `AddColumn`, so this exercises a rename plus a second
    /// op flushing in a single transaction.
    const WIDGET_RENAME_AND_ADD: &str = "\
types:
  - name: widget
    columns:
      - name: caption
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
        rename_from: label
      - name: note
        data_type: TEXT
        zone: body
";

    /// Desired-schema YAML whose `types:` list FIRST renames `widget.label` ->
    /// `caption` (a valid `RenameColumn`) and THEN declares a DISTINCT type
    /// `gadget` TWICE. The differ emits, in order:
    ///   RenameColumn(widget) , CreateType(gadget) , CreateType(gadget)
    /// `RenameColumn` is not a DropColumn, so it stays in the additive partition
    /// at its doc position — BEFORE the duplicate `CreateType` that fails (the
    /// type already exists after the first create). The mid-plan failure lands
    /// AFTER the rename has run inside the transaction, so it proves the rename
    /// rolls back atomically with the rest of the plan.
    const WIDGET_RENAME_THEN_DUP_CREATE: &str = "\
types:
  - name: widget
    columns:
      - name: caption
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
        rename_from: label
  - name: gadget
    columns:
      - name: serial
        data_type: TEXT
        zone: body
  - name: gadget
    columns:
      - name: serial
        data_type: TEXT
        zone: body
";

    /// Desired-schema YAML for a HYPHENATED type `meeting-minutes` with a
    /// single HYPHENATED body column `long-desc`. Both names pass `SchemaDoc`
    /// validation (the validator deliberately accepts hyphens), so the apply
    /// must render quoted DDL the engine accepts and round-trips. With bare
    /// identifiers the SQL parser chokes on the `-`, the transactional apply
    /// rolls back, and the error is SCHEMA_APPLY_PARTIAL.
    const HYPHENATED_TYPE_AND_COLUMN: &str = "\
types:
  - name: meeting-minutes
    columns:
      - name: long-desc
        data_type: TEXT
        zone: body
";

    /// The same hyphenated `meeting-minutes` type plus a SECOND hyphenated body
    /// column `extra-notes`. Applied over the live one-column type it plans a
    /// single `add_column` op carrying hyphenated identifiers (table + column)
    /// into an `ALTER TABLE ... ADD COLUMN`.
    const HYPHENATED_TYPE_ADD_COLUMN: &str = "\
types:
  - name: meeting-minutes
    columns:
      - name: long-desc
        data_type: TEXT
        zone: body
      - name: extra-notes
        data_type: TEXT
        zone: body
";

    /// Renames `meeting-minutes.long-desc` -> `short-desc` via `rename_from`.
    /// Applied over the live hyphenated type it plans a destructive
    /// `rename_column` op whose table name AND both from/to column names are
    /// hyphenated identifiers.
    const HYPHENATED_RENAME: &str = "\
types:
  - name: meeting-minutes
    columns:
      - name: short-desc
        data_type: TEXT
        zone: body
        rename_from: long-desc
";

    /// A hyphenated type with two hyphenated columns and a `unique_together`
    /// over BOTH of them. The constraint renders inside the CreateType as
    /// `UNIQUE(\"long-desc\", \"extra-notes\")`; if those constraint columns are
    /// not quoted the parser rejects the `-` and the apply rolls back.
    const HYPHENATED_UNIQUE_TOGETHER: &str = "\
types:
  - name: meeting-minutes
    columns:
      - name: long-desc
        data_type: TEXT
        zone: body
      - name: extra-notes
        data_type: TEXT
        zone: body
    unique_together: [[long-desc, extra-notes]]
";

    fn cmd(
        doc: &str,
        dry_run: bool,
        allow_destructive: bool,
    ) -> crate::app_contract::ApplySchemaCommand {
        crate::app_contract::ApplySchemaCommand {
            schema_doc: doc.to_string(),
            dry_run,
            allow_destructive,
        }
    }

    /// True iff `schema` declares a column named `name`.
    fn has_column(schema: &crate::types::TableSchema, name: &str) -> bool {
        schema.columns.iter().any(|c| c.name == name)
    }

    #[test]
    fn describe_type_returns_none_for_absent_type() {
        // A type that was never declared must describe as None, not an
        // empty-but-Some schema. Pins the absence signal.
        let (_tmp, mut svc) = fresh_svc();
        assert!(svc.describe_type("nope").unwrap().is_none());
    }

    #[test]
    fn dry_run_returns_plan_without_mutating() {
        // dry_run=true returns the plan it WOULD apply (dry_run flag set,
        // applied=false, ops present because there is work) and creates
        // nothing. An impl that ignores dry_run and applies anyway would be
        // caught by the post-condition: describe_type must still be None.
        let (_tmp, mut svc) = fresh_svc();
        let report = svc
            .apply_schema(cmd(WIDGET_ONE_COLUMN, true, false))
            .unwrap();
        assert!(report.value.dry_run);
        assert!(!report.value.applied);
        // The plan must be the real diff: exactly one create_type op for
        // `widget`, parsed from the doc — not a fabricated garbage op.
        assert_eq!(
            report.value.ops.len(),
            1,
            "creating one new type must plan exactly one op, got: {:?}",
            report.value.ops
        );
        assert_eq!(report.value.ops[0].kind, "create_type");
        assert_eq!(report.value.ops[0].table, "widget");
        // Nothing was created.
        assert!(svc.describe_type("widget").unwrap().is_none());
    }

    #[test]
    fn apply_creates_declared_type() {
        // dry_run=false applies the plan: applied=true, the plan is the real
        // diff (one create_type op for `gadget`), and afterwards the type
        // exists with its declared `serial` column present. The distinctive
        // `gadget`/`serial` names force a real parse of the doc — a hardcoded
        // `widget`/`label` literal cannot satisfy this.
        let (_tmp, mut svc) = fresh_svc();
        let report = svc
            .apply_schema(cmd(GADGET_ONE_COLUMN, false, false))
            .unwrap();
        assert!(report.value.applied);
        assert_eq!(
            report.value.ops.len(),
            1,
            "creating one new type must apply exactly one op, got: {:?}",
            report.value.ops
        );
        assert_eq!(report.value.ops[0].kind, "create_type");
        assert_eq!(report.value.ops[0].table, "gadget");

        let schema = svc
            .describe_type("gadget")
            .unwrap()
            .expect("gadget must exist after a non-dry-run apply");
        assert!(
            has_column(&schema, "serial"),
            "applied schema is missing declared column `serial`: {:?}",
            schema.columns
        );

        // Persistence cross-check, independent of describe_type (which is part
        // of the unit under test): a really-created type is a queryable table
        // in the live index. An impl that fakes apply/describe in process
        // memory without persisting the typedef cannot answer this — a SELECT
        // against an unregistered type errors.
        assert!(
            svc.execute_sql("SELECT serial FROM gadget").is_ok(),
            "an applied type must be a real, queryable table (persisted, not faked in memory)"
        );
    }

    #[test]
    fn add_only_drift_applies_without_allow_destructive() {
        // Adding a column to an existing type is NON-destructive: it must
        // apply even with allow_destructive=false. This defeats a blanket
        // "schema changed -> block" heuristic AND a blanket "existing type ->
        // empty no-op" heuristic, and proves convergence WITH pending work.
        let (_tmp, mut svc) = fresh_svc();
        svc.apply_schema(cmd(WIDGET_ONE_COLUMN, false, false))
            .unwrap();

        // Second apply: same type, one extra column. Non-destructive add.
        let report = svc
            .apply_schema(cmd(WIDGET_TWO_COLUMNS, false, false))
            .unwrap();
        assert!(
            report.value.applied,
            "adding a column must apply without allow_destructive"
        );
        assert!(
            report
                .value
                .ops
                .iter()
                .any(|op| op.kind == "add_column" && op.table == "widget"),
            "adding a column must plan an add_column op on widget, got: {:?}",
            report.value.ops
        );

        // The live schema now carries BOTH the original and the added column.
        let schema = svc
            .describe_type("widget")
            .unwrap()
            .expect("widget must exist after the add-column apply");
        assert!(
            has_column(&schema, "label"),
            "add-column apply must keep `label`: {:?}",
            schema.columns
        );
        assert!(
            has_column(&schema, "note"),
            "add-column apply must add `note`: {:?}",
            schema.columns
        );

        // Re-applying the same 2-column doc converges: nothing left to do.
        let report = svc
            .apply_schema(cmd(WIDGET_TWO_COLUMNS, false, false))
            .unwrap();
        assert!(
            !report.value.applied,
            "re-applying the converged 2-column doc must not report applied"
        );
        assert!(
            report.value.ops.is_empty(),
            "re-applying the converged 2-column doc must produce no ops, got: {:?}",
            report.value.ops
        );
    }

    #[test]
    fn reapply_same_doc_is_idempotent_noop() {
        // Re-applying a doc that already matches the live schema is a no-op:
        // applied=false and an empty ops list (nothing left to do). Guards an
        // impl that recreates the type or reports phantom ops on convergence.
        let (_tmp, mut svc) = fresh_svc();
        svc.apply_schema(cmd(WIDGET_ONE_COLUMN, false, false))
            .unwrap();

        let report = svc
            .apply_schema(cmd(WIDGET_ONE_COLUMN, false, false))
            .unwrap();
        assert!(
            !report.value.applied,
            "re-applying an already-matching doc must not report applied"
        );
        assert!(
            report.value.ops.is_empty(),
            "re-applying an already-matching doc must produce no ops, got: {:?}",
            report.value.ops
        );
    }

    #[test]
    fn destructive_drop_without_allow_destructive_is_blocked_and_mutates_nothing() {
        // Live `widget` has `label` + `note`. A desired doc with only `label`
        // implies dropping `note` (destructive). Without allow_destructive,
        // the apply must fail with SCHEMA_DESTRUCTIVE_BLOCKED and leave the
        // live schema untouched (note still present).
        let (_tmp, mut svc) = fresh_svc();
        svc.apply_schema(cmd(WIDGET_TWO_COLUMNS, false, false))
            .unwrap();

        let err = svc
            .apply_schema(cmd(WIDGET_ONE_COLUMN, false, false))
            .expect_err("dropping a column without allow_destructive must error");
        match err {
            crate::error::DoogatError::Structured { code, .. } => {
                assert_eq!(code, crate::error::codes::SCHEMA_DESTRUCTIVE_BLOCKED);
            }
            other => panic!("expected Structured SCHEMA_DESTRUCTIVE_BLOCKED, got: {other:?}"),
        }

        // No mutation: `note` must still be there.
        let schema = svc
            .describe_type("widget")
            .unwrap()
            .expect("widget must still exist after a blocked destructive apply");
        assert!(
            has_column(&schema, "note"),
            "blocked destructive apply must not drop `note`: {:?}",
            schema.columns
        );
    }

    #[test]
    fn partial_apply_rolls_back_atomically_then_reapply_creates() {
        // PRD 00161 Phase 2 (task 10): apply is now ATOMIC. A doc that declares
        // `widget` twice creates it on the first op and fails on the second
        // (the type now exists). Under transactional apply the WHOLE plan rolls
        // back: the error is still SCHEMA_APPLY_PARTIAL, but the first op's
        // create is UNDONE — `widget` must be ABSENT afterwards (git unchanged),
        // superseding the Phase-1 forward-recovery behavior where the first op
        // persisted. A fresh single-`widget` apply then creates it cleanly.
        let (_tmp, mut svc) = fresh_svc();

        let err = svc
            .apply_schema(cmd(WIDGET_DUPLICATED, false, false))
            .expect_err("a doc declaring the same type twice must fail mid-plan");
        match err {
            crate::error::DoogatError::Structured { code, context, .. } => {
                assert_eq!(code, crate::error::codes::SCHEMA_APPLY_PARTIAL);
                // The error still reports which ops ran before the failure
                // (now rolled back), keyed `applied_ops`. A fabricated
                // empty-context error fails this.
                let applied_ops = context
                    .iter()
                    .find(|(k, _)| k == "applied_ops")
                    .map(|(_, v)| v)
                    .expect("SCHEMA_APPLY_PARTIAL must carry an `applied_ops` context entry");
                match applied_ops {
                    crate::error::ErrorValue::List(list) => assert!(
                        !list.is_empty(),
                        "`applied_ops` must list the ops that ran before the failure"
                    ),
                    other => {
                        panic!("`applied_ops` must be an ErrorValue::List, got: {other:?}")
                    }
                }
            }
            other => panic!("expected Structured SCHEMA_APPLY_PARTIAL, got: {other:?}"),
        }

        // ATOMIC ROLLBACK: the first op's create is undone — `widget` is ABSENT.
        // The Phase-1 forward-recovery code left it persisted; this assertion is
        // the behavioral pin that the apply is now transactional.
        assert!(
            svc.describe_type("widget").unwrap().is_none(),
            "atomic apply must roll the partial create back — widget must be absent"
        );

        // A fresh single-widget apply now creates it from scratch.
        let report = svc
            .apply_schema(cmd(WIDGET_ONE_COLUMN, false, false))
            .unwrap();
        assert!(report.value.applied, "fresh re-apply must create widget");
        assert!(
            svc.describe_type("widget").unwrap().is_some(),
            "widget must exist after the clean re-apply"
        );
    }

    #[test]
    fn apply_failure_leaves_git_head_unchanged() {
        // The atomicity contract at the git layer: a mid-plan failure must
        // leave the repository's HEAD commit exactly where it was — no typedef
        // from the partially-applied plan may survive in git. Captures the
        // HEAD oid before/after the failing apply and asserts equality.
        let (_tmp, mut svc) = fresh_svc();
        let head_before = svc.repo.head_oid().unwrap().0;

        let err = svc
            .apply_schema(cmd(WIDGET_DUPLICATED, false, false))
            .expect_err("the duplicated-type doc must fail mid-plan");
        assert!(
            matches!(err, crate::error::DoogatError::Structured { .. }),
            "expected a Structured error, got: {err:?}"
        );

        let head_after = svc.repo.head_oid().unwrap().0;
        assert_eq!(
            head_before, head_after,
            "atomic apply must leave git HEAD unchanged on a mid-plan failure"
        );
    }

    #[test]
    fn multi_add_column_one_apply_preserves_every_column() {
        // Two `add_column` ops on the SAME existing type in ONE apply must BOTH
        // survive. Under transactional apply each op's typedef write is buffered
        // (not committed), so the second op's schema read must see the first
        // op's buffered column (read-your-writes) — otherwise the second op
        // reads the stale pre-buffer typedef and overwrites the first, silently
        // losing a column. Live `widget` starts with `label`; the 3-column doc
        // adds `note` and `extra` in one apply.
        let (_tmp, mut svc) = fresh_svc();
        svc.apply_schema(cmd(WIDGET_ONE_COLUMN, false, false))
            .unwrap();

        let report = svc
            .apply_schema(cmd(WIDGET_THREE_COLUMNS, false, false))
            .unwrap();
        assert!(report.value.applied, "adding two columns must apply");
        let add_ops = report
            .value
            .ops
            .iter()
            .filter(|op| op.kind == "add_column" && op.table == "widget")
            .count();
        assert_eq!(
            add_ops, 2,
            "adding two new columns must plan two add_column ops, got: {:?}",
            report.value.ops
        );

        // Every column survives in the persisted typedef — the read-your-writes
        // guarantee. A buffer-blind second op would drop `note`.
        let schema = svc
            .describe_type("widget")
            .unwrap()
            .expect("widget must exist after the multi-add apply");
        for col in ["label", "note", "extra"] {
            assert!(
                has_column(&schema, col),
                "multi-add apply must preserve `{col}`: {:?}",
                schema.columns
            );
        }

        // The materialized table is consistent too: both added columns are
        // queryable (proves the deferred rematerialize ran against the
        // committed typedef, not a stale one).
        assert!(
            svc.execute_sql("SELECT note, extra FROM widget").is_ok(),
            "both added columns must be queryable on the materialized table"
        );
    }

    #[test]
    fn apply_with_rename_then_failure_rolls_back_atomically() {
        // The bug this PRD 00161 cycle-1 rework fixes: a column rename used to
        // commit IMMEDIATELY (its own git commit), ignoring the open apply
        // transaction, so a mid-plan failure AFTER the rename stranded a
        // half-applied migration in git. With the rename routed through the
        // transaction buffer, a plan that renames `widget.label` -> `caption`
        // and THEN fails (duplicate `gadget` create) must leave git HEAD exactly
        // where it was AND leave the rename un-applied: `label` still present,
        // `caption` absent.
        let (_tmp, mut svc) = fresh_svc();
        svc.apply_schema(cmd(WIDGET_ONE_COLUMN, false, false))
            .unwrap();
        // Sanity: pre-state has `label`, not `caption`.
        let before = svc.describe_type("widget").unwrap().unwrap();
        assert!(has_column(&before, "label"));
        assert!(!has_column(&before, "caption"));

        let head_before = svc.repo.head_oid().unwrap().0;

        let err = svc
            // allow_destructive=true: a rename is destructive, so the plan would
            // otherwise be blocked before any op runs.
            .apply_schema(cmd(WIDGET_RENAME_THEN_DUP_CREATE, false, true))
            .expect_err("rename + duplicate-create must fail mid-plan");
        assert!(
            matches!(err, crate::error::DoogatError::Structured { .. }),
            "expected a Structured error, got: {err:?}"
        );
        // The partial-apply error surfaces the rollback outcome rather than
        // swallowing it (cycle-1 finding C): on the success path the message
        // reports a clean rollback.
        if let crate::error::DoogatError::Structured { message, .. } = &err {
            assert!(
                message.contains("rolled back"),
                "partial-apply error must surface the rollback outcome: {message}"
            );
        }

        // GIT ATOMICITY: HEAD is unchanged — no commit from the rolled-back plan
        // (the pre-fix immediate rename commit would have advanced HEAD here).
        let head_after = svc.repo.head_oid().unwrap().0;
        assert_eq!(
            head_before, head_after,
            "a rename inside a failed apply must leave git HEAD unchanged"
        );

        // THE RENAME DID NOT LAND: `label` is still there, `caption` is absent.
        let after = svc
            .describe_type("widget")
            .unwrap()
            .expect("widget must still exist after the rolled-back apply");
        assert!(
            has_column(&after, "label"),
            "rolled-back rename must keep the old column `label`: {:?}",
            after.columns
        );
        assert!(
            !has_column(&after, "caption"),
            "rolled-back rename must NOT have applied `caption`: {:?}",
            after.columns
        );
    }

    #[test]
    fn apply_renaming_and_adding_produces_single_commit() {
        // A success plan that renames a column AND adds another in one apply must
        // produce EXACTLY ONE new git commit, not two. Before the fix the rename
        // committed immediately and the buffered transaction committed the add
        // separately -> two commits. We assert HEAD advanced by exactly one
        // commit: the new HEAD's first parent is the pre-apply HEAD.
        use crate::traits::GitHistory;

        let (_tmp, mut svc) = fresh_svc();
        svc.apply_schema(cmd(WIDGET_ONE_COLUMN, false, false))
            .unwrap();

        let head_before = svc.repo.head_oid().unwrap().0;

        let report = svc
            // allow_destructive=true because a rename is destructive.
            .apply_schema(cmd(WIDGET_RENAME_AND_ADD, false, true))
            .unwrap();
        assert!(report.value.applied, "the rename+add plan must apply");
        assert!(
            report
                .value
                .ops
                .iter()
                .any(|op| op.kind == "rename_column" && op.table == "widget"),
            "the plan must contain a rename_column op, got: {:?}",
            report.value.ops
        );
        assert!(
            report
                .value
                .ops
                .iter()
                .any(|op| op.kind == "add_column" && op.table == "widget"),
            "the plan must contain an add_column op, got: {:?}",
            report.value.ops
        );

        let head_after = svc.repo.head_oid().unwrap().0;
        assert_ne!(
            head_before, head_after,
            "a non-empty apply must advance HEAD"
        );
        // Exactly one commit added: the new HEAD's first parent is the old HEAD.
        let parent = svc.repo.commit_parent_oid(&head_after, 0).unwrap();
        assert_eq!(
            parent, head_before,
            "rename+add must flush as ONE commit (new HEAD parent == pre-apply HEAD), not two"
        );

        // Both the rename and the add landed: `caption` (renamed) and `note`
        // (added) are present, `label` (old name) is gone.
        let schema = svc.describe_type("widget").unwrap().unwrap();
        assert!(has_column(&schema, "caption"), "{:?}", schema.columns);
        assert!(has_column(&schema, "note"), "{:?}", schema.columns);
        assert!(!has_column(&schema, "label"), "{:?}", schema.columns);
        // Materialized table is consistent: the renamed + added columns query.
        assert!(
            svc.execute_sql("SELECT caption, note FROM widget").is_ok(),
            "renamed and added columns must be queryable on the materialized table"
        );
    }

    #[test]
    fn apply_hyphenated_type_and_column_succeeds_and_is_idempotent() {
        // Regression for the schema-apply DDL quoting bug. A type and a column
        // whose names contain hyphens pass SchemaDoc validation, so the apply
        // MUST render quoted DDL the engine accepts and round-trips. Before the
        // fix the rendered DDL is bare (`CREATE TABLE meeting-minutes (...)`),
        // the SQL parser rejects the `-`, the transactional apply rolls back,
        // and the error is SCHEMA_APPLY_PARTIAL. dry_run=false drives the real
        // DDL through the engine, so a bare-identifier impl fails here.
        let (_tmp, mut svc) = fresh_svc();

        let report = svc
            .apply_schema(cmd(HYPHENATED_TYPE_AND_COLUMN, false, false))
            .expect("applying a hyphenated type+column must succeed, not fail mid-plan");
        assert!(
            report.value.applied,
            "a hyphenated type+column apply must report applied=true (no SCHEMA_APPLY_PARTIAL)"
        );
        assert_eq!(
            report.value.ops.len(),
            1,
            "creating one hyphenated type must plan exactly one op, got: {:?}",
            report.value.ops
        );
        assert_eq!(report.value.ops[0].kind, "create_type");
        assert_eq!(report.value.ops[0].table, "meeting-minutes");

        // The type really exists with its hyphenated column: proof the quoted
        // DDL round-tripped and the engine stored the unquoted inner names.
        let schema = svc
            .describe_type("meeting-minutes")
            .unwrap()
            .expect("meeting-minutes must exist after a non-dry-run apply");
        assert!(
            has_column(&schema, "long-desc"),
            "applied schema is missing the declared hyphenated column `long-desc`: {:?}",
            schema.columns
        );

        // Re-applying the identical doc converges to a no-op: empty plan,
        // applied=false. Guards against a non-idempotent re-render.
        let report = svc
            .apply_schema(cmd(HYPHENATED_TYPE_AND_COLUMN, false, false))
            .unwrap();
        assert!(
            !report.value.applied,
            "re-applying the converged hyphenated doc must not report applied"
        );
        assert!(
            report.value.ops.is_empty(),
            "re-applying the converged hyphenated doc must produce no ops, got: {:?}",
            report.value.ops
        );
    }

    #[test]
    fn apply_adds_hyphenated_column_to_existing_hyphenated_type() {
        // ALTER path: a hyphenated column added to an existing hyphenated type
        // renders `ALTER TABLE "meeting-minutes" ADD COLUMN "extra-notes" TEXT`.
        // Both identifiers must be quoted or the parser rejects the `-` and the
        // apply rolls back with SCHEMA_APPLY_PARTIAL. Non-destructive, so it
        // must apply without allow_destructive.
        let (_tmp, mut svc) = fresh_svc();
        svc.apply_schema(cmd(HYPHENATED_TYPE_AND_COLUMN, false, false))
            .unwrap();

        let report = svc
            .apply_schema(cmd(HYPHENATED_TYPE_ADD_COLUMN, false, false))
            .expect("adding a hyphenated column must succeed, not fail mid-plan");
        assert!(report.value.applied, "adding a hyphenated column must apply");
        assert!(
            report
                .value
                .ops
                .iter()
                .any(|op| op.kind == "add_column" && op.table == "meeting-minutes"),
            "must plan an add_column op on the hyphenated type, got: {:?}",
            report.value.ops
        );

        let schema = svc
            .describe_type("meeting-minutes")
            .unwrap()
            .expect("meeting-minutes must exist after the add-column apply");
        for col in ["long-desc", "extra-notes"] {
            assert!(
                has_column(&schema, col),
                "add-column apply must keep/add hyphenated column `{col}`: {:?}",
                schema.columns
            );
        }
    }

    #[test]
    fn apply_renames_hyphenated_column_on_hyphenated_type() {
        // Destructive rename path: `meeting-minutes.long-desc` -> `short-desc`
        // renders `ALTER TABLE "meeting-minutes" RENAME COLUMN "long-desc" TO
        // "short-desc"`. The table name and BOTH column names are hyphenated, so
        // all three must be quoted for the rename to apply. allow_destructive
        // because a rename is destructive.
        let (_tmp, mut svc) = fresh_svc();
        svc.apply_schema(cmd(HYPHENATED_TYPE_AND_COLUMN, false, false))
            .unwrap();

        let report = svc
            .apply_schema(cmd(HYPHENATED_RENAME, false, true))
            .expect("renaming a hyphenated column must succeed, not fail mid-plan");
        assert!(report.value.applied, "the hyphenated rename must apply");
        assert!(
            report
                .value
                .ops
                .iter()
                .any(|op| op.kind == "rename_column" && op.table == "meeting-minutes"),
            "must plan a rename_column op on the hyphenated type, got: {:?}",
            report.value.ops
        );

        let schema = svc
            .describe_type("meeting-minutes")
            .unwrap()
            .expect("meeting-minutes must exist after the rename apply");
        assert!(
            has_column(&schema, "short-desc"),
            "rename must produce the new hyphenated column `short-desc`: {:?}",
            schema.columns
        );
        assert!(
            !has_column(&schema, "long-desc"),
            "rename must remove the old hyphenated column `long-desc`: {:?}",
            schema.columns
        );
    }

    #[test]
    fn apply_hyphenated_unique_together_succeeds() {
        // unique_together over hyphenated columns renders inside the CreateType
        // as `UNIQUE("long-desc", "extra-notes")`. Those constraint column names
        // are identifiers and must be quoted independently of the column
        // definitions; an unquoted `UNIQUE(long-desc, ...)` makes the parser
        // reject the `-` and the apply rolls back with SCHEMA_APPLY_PARTIAL.
        // Apply success is the signal that the UNIQUE() identifiers rendered
        // quoted.
        let (_tmp, mut svc) = fresh_svc();

        let report = svc
            .apply_schema(cmd(HYPHENATED_UNIQUE_TOGETHER, false, false))
            .expect("a hyphenated unique_together create must succeed, not fail mid-plan");
        assert!(
            report.value.applied,
            "a hyphenated unique_together create must apply"
        );

        let schema = svc
            .describe_type("meeting-minutes")
            .unwrap()
            .expect("meeting-minutes must exist after the unique_together apply");
        for col in ["long-desc", "extra-notes"] {
            assert!(
                has_column(&schema, col),
                "unique_together create must declare hyphenated column `{col}`: {:?}",
                schema.columns
            );
        }
    }
}
