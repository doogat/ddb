//! `apply_schema` service verb + `describe_type` (PRD 00161 Phase 1).
//!
//! Declarative schema apply: parse a desired-schema YAML doc, diff it against
//! the live typedefs, and apply the resulting plan one op at a time with
//! forward-recovery semantics (no rollback — see PRD 00161 §3.5).

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
    /// Forward-recovery, not rollback: ops apply in order; on failure the
    /// already-applied ops are reported via `SCHEMA_APPLY_PARTIAL` and left in
    /// place so a re-apply can converge.
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
                    // is the meaningful one; a rollback failure is swallowed
                    // (Drop also rolls back as a backstop).
                    let _ = self.rollback_transaction();
                    return Err(DoogatError::Structured {
                        code: codes::SCHEMA_APPLY_PARTIAL,
                        message: format!(
                            "schema apply failed after {} of {} operations and rolled back: {}",
                            applied_kinds.len(),
                            plan.ops.len(),
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
}
