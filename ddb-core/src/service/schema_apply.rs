//! `apply_schema` service verb + `describe_type` (PRD 00161 Phase 1).
//!
//! Non-test implementation is added by the implementer; this file currently
//! holds only the unit tests for the declarative schema-apply contract.

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
    fn partial_apply_reports_then_reapply_converges() {
        // A doc that declares `widget` twice creates it on the first op and
        // fails on the second (the type now exists), yielding a partial
        // failure with code SCHEMA_APPLY_PARTIAL. A subsequent apply of a
        // single-`widget` doc must then converge to a no-op (the type already
        // exists from the first, applied, op).
        let (_tmp, mut svc) = fresh_svc();

        let err = svc
            .apply_schema(cmd(WIDGET_DUPLICATED, false, false))
            .expect_err("a doc declaring the same type twice must fail mid-plan");
        match err {
            crate::error::DoogatError::Structured { code, context, .. } => {
                assert_eq!(code, crate::error::codes::SCHEMA_APPLY_PARTIAL);
                // The partial-failure error must carry the ops that DID land
                // (the first, applied, create_type), keyed `applied_ops`. A
                // fabricated empty-context error fails this.
                let applied_ops = context
                    .iter()
                    .find(|(k, _)| k == "applied_ops")
                    .map(|(_, v)| v)
                    .expect("SCHEMA_APPLY_PARTIAL must carry an `applied_ops` context entry");
                match applied_ops {
                    crate::error::ErrorValue::List(list) => assert!(
                        !list.is_empty(),
                        "`applied_ops` must list the ops that landed before the failure"
                    ),
                    other => {
                        panic!("`applied_ops` must be an ErrorValue::List, got: {other:?}")
                    }
                }
            }
            other => panic!("expected Structured SCHEMA_APPLY_PARTIAL, got: {other:?}"),
        }

        // Re-apply the single-widget form: widget already exists from the
        // first op of the partial run, so this converges.
        let report = svc
            .apply_schema(cmd(WIDGET_ONE_COLUMN, false, false))
            .unwrap();
        assert!(
            !report.value.applied,
            "convergence re-apply must not report applied"
        );
        assert!(
            report.value.ops.is_empty(),
            "convergence re-apply must produce no ops, got: {:?}",
            report.value.ops
        );
    }
}
