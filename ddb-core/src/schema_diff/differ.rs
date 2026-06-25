//! Pure differ: turns a desired schema and the live schema into an ordered
//! [`SchemaPlan`]. No I/O, no `&self`; this is the unit-test core of
//! declarative schema apply (PRD 00161 task 3).

use crate::schema_diff::desired::SchemaDoc;
use crate::schema_diff::plan::{PlanOp, SchemaPlan};
use crate::types::{ColumnDef, TableSchema};

/// Diff `desired` against `live`, producing an ordered plan of DDL ops.
///
/// `live[i]` corresponds to `desired.types[i]`; `None` (or an index past the
/// end of `live`) means the type does not exist yet and must be created.
pub fn diff(desired: &SchemaDoc, live: &[Option<TableSchema>]) -> SchemaPlan {
    let mut plan = SchemaPlan { ops: Vec::new(), unsupported: Vec::new() };
    for (i, desired_type) in desired.types.iter().enumerate() {
        match live.get(i).and_then(Option::as_ref) {
            Some(current) => {
                let renamed_cols = apply_renames(desired_type, current, desired, &mut plan);
                diff_type(desired_type, current, &renamed_cols, &mut plan);
            }
            None => {
                plan.ops.push(PlanOp::CreateType(desired_type.clone()));
                if desired_type.search_key.is_some() {
                    plan.ops.push(PlanOp::SetSearchKey {
                        table: desired_type.table_name.clone(),
                        column: desired_type.search_key.clone(),
                    });
                }
            }
        }
    }

    // Destructive ops sort after every additive op (stable partition).
    let (additive, drops): (Vec<PlanOp>, Vec<PlanOp>) = std::mem::take(&mut plan.ops)
        .into_iter()
        .partition(|op| !matches!(op, PlanOp::DropColumn { .. }));
    plan.ops = additive;
    plan.ops.extend(drops);

    plan
}

/// Consume `desired.renames` for one existing type. Emit a `RenameColumn` (plus
/// an `AlterColumnType` when the renamed column also retypes) for each valid
/// directive, flag invalid ones as unsupported, and return the set of valid
/// `(from, to)` pairs so `diff_type` can suppress the matching Drop/Add.
fn apply_renames(
    desired_type: &TableSchema,
    current: &TableSchema,
    desired: &SchemaDoc,
    plan: &mut SchemaPlan,
) -> Vec<(String, String)> {
    let table = &desired_type.table_name;
    let mut valid = Vec::new();

    for r in desired.renames.iter().filter(|r| &r.table == table) {
        let from_col = current.columns.iter().find(|c| c.name == r.from);
        let to_in_current = current.columns.iter().any(|c| c.name == r.to);
        let to_in_desired = desired_type.columns.iter().find(|c| c.name == r.to);

        match (from_col, to_in_current, to_in_desired) {
            (Some(from_col), false, Some(to_col)) => {
                plan.ops.push(PlanOp::RenameColumn {
                    table: table.clone(),
                    from: r.from.clone(),
                    to: r.to.clone(),
                });
                if to_col.data_type != from_col.data_type {
                    plan.ops.push(PlanOp::AlterColumnType {
                        table: table.clone(),
                        column: r.to.clone(),
                        new_type: to_col.data_type.clone(),
                    });
                }
                valid.push((r.from.clone(), r.to.clone()));
            }
            _ => {
                let offending = if from_col.is_none() { &r.from } else { &r.to };
                plan.unsupported.push(format!(
                    "column {table}.{offending}: rename {} -> {} is unsupported",
                    r.from, r.to
                ));
            }
        }
    }

    valid
}

/// Append ops to converge `current` toward `desired` for one existing type.
/// `renames` is the set of valid `(from, to)` pairs already handled by
/// [`apply_renames`]; their Drop (`from`) and Add (`to`) are suppressed here.
fn diff_type(
    desired: &TableSchema,
    current: &TableSchema,
    renames: &[(String, String)],
    plan: &mut SchemaPlan,
) {
    diff_columns(desired, current, renames, plan);
    diff_dropped_columns(desired, current, renames, plan);
    diff_type_attributes(desired, current, plan);
}

/// Add-or-compare pass over every desired column (renamed targets skipped):
/// a column missing from `current` is added, a present one is compared.
fn diff_columns(
    desired: &TableSchema,
    current: &TableSchema,
    renames: &[(String, String)],
    plan: &mut SchemaPlan,
) {
    let table = &desired.table_name;
    for desired_col in &desired.columns {
        if renames.iter().any(|(_, to)| to == &desired_col.name) {
            continue;
        }
        match current.columns.iter().find(|c| c.name == desired_col.name) {
            None => diff_added_column(table, desired_col, plan),
            Some(current_col) => diff_existing_column(table, desired_col, current_col, plan),
        }
    }
}

/// Plan the addition of a column absent from `current`.
///
/// `ADD COLUMN` carries no inline ZONE (the engine assigns a column added this
/// way to the zone its `data_type`/`references` infer to). So when the desired
/// column pins a zone the inference would NOT produce, emit a follow-up
/// `SetZone` right after the `AddColumn`; otherwise a single apply leaves the
/// column in the inferred zone and never converges. The check mirrors the
/// existing-column path: clone the desired column with `zone = None`, take its
/// `effective_zone()` (exactly what a ZONE-less ADD COLUMN yields), and emit
/// `SetZone` only when it differs from the desired effective zone.
fn diff_added_column(table: &str, desired_col: &ColumnDef, plan: &mut SchemaPlan) {
    plan.ops.push(PlanOp::AddColumn {
        table: table.to_string(),
        column: desired_col.clone(),
    });

    let mut zoneless = desired_col.clone();
    zoneless.zone = None;
    let desired_zone = desired_col.effective_zone();
    if zoneless.effective_zone() != desired_zone {
        plan.ops.push(PlanOp::SetZone {
            table: table.to_string(),
            column: desired_col.name.clone(),
            zone: desired_zone,
        });
    }
}

/// Compare a desired column against its live counterpart: emit the supported
/// ops (type alter, consequential zone set) and delegate unsupported
/// attribute changes to [`flag_unsupported_column_attrs`].
fn diff_existing_column(
    table: &str,
    desired_col: &ColumnDef,
    current_col: &ColumnDef,
    plan: &mut SchemaPlan,
) {
    if desired_col.data_type != current_col.data_type {
        plan.ops.push(PlanOp::AlterColumnType {
            table: table.to_string(),
            column: desired_col.name.clone(),
            new_type: desired_col.data_type.clone(),
        });
    }

    // Live zone as it will be after the (possible) type alter, so a pure
    // data_type change never emits a consequential SetZone.
    let mut post_alter = current_col.clone();
    post_alter.data_type = desired_col.data_type.clone();
    let desired_zone = desired_col.effective_zone();
    if post_alter.effective_zone() != desired_zone {
        plan.ops.push(PlanOp::SetZone {
            table: table.to_string(),
            column: desired_col.name.clone(),
            zone: desired_zone,
        });
    }

    flag_unsupported_column_attrs(table, desired_col, current_col, plan);
}

/// Flag every column attribute that has no DDL op to converge it
/// (required/default_value/references/on_delete/search_boost/allowed_values).
/// Each differing attribute records one unsupported-change warning.
fn flag_unsupported_column_attrs(
    table: &str,
    desired_col: &ColumnDef,
    current_col: &ColumnDef,
    plan: &mut SchemaPlan,
) {
    if desired_col.required != current_col.required {
        plan.unsupported.push(format!(
            "column {}.{}: required change is unsupported",
            table, desired_col.name
        ));
    }
    if desired_col.default_value != current_col.default_value {
        plan.unsupported.push(format!(
            "column {}.{}: default_value change is unsupported",
            table, desired_col.name
        ));
    }
    if desired_col.references != current_col.references {
        plan.unsupported.push(format!(
            "column {}.{}: references change is unsupported",
            table, desired_col.name
        ));
    }
    if desired_col.on_delete != current_col.on_delete {
        plan.unsupported.push(format!(
            "column {}.{}: on_delete change is unsupported",
            table, desired_col.name
        ));
    }
    if desired_col.search_boost != current_col.search_boost {
        plan.unsupported.push(format!(
            "column {}.{}: search_boost change is unsupported",
            table, desired_col.name
        ));
    }
    if desired_col.allowed_values != current_col.allowed_values {
        plan.unsupported.push(format!(
            "column {}.{}: allowed_values change is unsupported",
            table, desired_col.name
        ));
    }
}

/// Drop every live column absent from `desired` (renamed sources skipped,
/// since [`apply_renames`] already moved their data).
fn diff_dropped_columns(
    desired: &TableSchema,
    current: &TableSchema,
    renames: &[(String, String)],
    plan: &mut SchemaPlan,
) {
    let table = &desired.table_name;
    for current_col in &current.columns {
        if renames.iter().any(|(from, _)| from == &current_col.name) {
            continue;
        }
        if !desired.columns.iter().any(|c| c.name == current_col.name) {
            plan.ops.push(PlanOp::DropColumn {
                table: table.clone(),
                column: current_col.name.clone(),
            });
        }
    }
}

/// Diff the per-type attributes: emit the supported ops (search_key, singleton)
/// and flag unsupported type-attribute changes. `origin` is never diffed.
fn diff_type_attributes(desired: &TableSchema, current: &TableSchema, plan: &mut SchemaPlan) {
    let table = &desired.table_name;

    if desired.search_key != current.search_key {
        plan.ops.push(PlanOp::SetSearchKey {
            table: table.clone(),
            column: desired.search_key.clone(),
        });
    }
    if desired.singleton != current.singleton {
        plan.ops.push(PlanOp::SetSingleton {
            table: table.clone(),
            on: desired.singleton,
        });
    }

    // origin is never diffed.
    if desired.title_template != current.title_template {
        plan.unsupported
            .push(format!("type {table}: title_template change is unsupported"));
    }
    if desired.crdt_strategy != current.crdt_strategy {
        plan.unsupported
            .push(format!("type {table}: crdt_strategy change is unsupported"));
    }
    if desired.template_sections != current.template_sections {
        plan.unsupported
            .push(format!("type {table}: template_sections change is unsupported"));
    }
    if desired.folder != current.folder {
        plan.unsupported
            .push(format!("type {table}: folder change is unsupported"));
    }
    if desired.stale_after_days != current.stale_after_days {
        plan.unsupported
            .push(format!("type {table}: stale_after_days change is unsupported"));
    }
    if desired.unique_together != current.unique_together {
        plan.unsupported
            .push(format!("type {table}: unique_together change is unsupported"));
    }
}

#[cfg(test)]
mod tests {
    use super::diff;
    use crate::schema_diff::desired::{ColumnRename, SchemaDoc};
    use crate::schema_diff::plan::PlanOp;
    use crate::types::{ColumnDef, OnDeleteAction, TableSchema, Zone};

    /// Minimal column builder; everything not named is the inert default.
    fn col(name: &str, data_type: &str) -> ColumnDef {
        ColumnDef {
            name: name.into(),
            data_type: data_type.into(),
            references: None,
            zone: None,
            required: false,
            search_boost: None,
            allowed_values: None,
            default_value: None,
            on_delete: OnDeleteAction::Restrict,
        }
    }

    /// Minimal table builder; everything not named is the inert default.
    fn tbl(name: &str, cols: Vec<ColumnDef>) -> TableSchema {
        TableSchema {
            table_name: name.into(),
            columns: cols,
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
            unique_together: None,
            search_key: None,
            singleton: false,
        }
    }

    fn doc(types: Vec<TableSchema>) -> SchemaDoc {
        SchemaDoc {
            types,
            renames: vec![],
        }
    }

    fn doc_with_renames(types: Vec<TableSchema>, renames: Vec<ColumnRename>) -> SchemaDoc {
        SchemaDoc { types, renames }
    }

    /// Convenience for building a ColumnRename in tests.
    fn rename(table: &str, from: &str, to: &str) -> ColumnRename {
        ColumnRename {
            table: table.into(),
            from: from.into(),
            to: to.into(),
        }
    }

    // ---- 1 & 2: creation of new types ----

    #[test]
    fn empty_live_yields_create_type_per_declared_type() {
        let a = tbl("a", vec![col("x", "TEXT")]);
        let b = tbl("b", vec![col("y", "INTEGER")]);
        let plan = diff(&doc(vec![a.clone(), b.clone()]), &[None, None]);

        assert_eq!(
            plan.ops,
            vec![PlanOp::CreateType(a), PlanOp::CreateType(b)],
            "exactly one CreateType per declared type, in declared order"
        );
        assert!(plan.unsupported.is_empty(), "create path adds nothing to unsupported");
    }

    #[test]
    fn shorter_live_slice_still_creates_trailing_declared_types() {
        let a = tbl("a", vec![]);
        let b = tbl("b", vec![]);
        // live is shorter than declared types: index 1 has no entry.
        let plan = diff(&doc(vec![a.clone(), b.clone()]), &[None]);

        assert_eq!(plan.ops, vec![PlanOp::CreateType(a), PlanOp::CreateType(b)]);
        assert!(plan.unsupported.is_empty());
    }

    #[test]
    fn new_type_with_search_key_emits_set_search_key_right_after_create() {
        let mut t = tbl("c", vec![col("name", "TEXT")]);
        t.search_key = Some("name".into());
        let plan = diff(&doc(vec![t.clone()]), &[None]);

        assert_eq!(
            plan.ops,
            vec![
                PlanOp::CreateType(t),
                PlanOp::SetSearchKey { table: "c".into(), column: Some("name".into()) },
            ],
            "SetSearchKey must immediately follow the CreateType"
        );
        assert!(plan.unsupported.is_empty());
    }

    #[test]
    fn new_type_without_search_key_emits_only_create() {
        let t = tbl("c", vec![col("name", "TEXT")]);
        let plan = diff(&doc(vec![t.clone()]), &[None]);

        assert_eq!(plan.ops, vec![PlanOp::CreateType(t)]);
        assert!(plan.unsupported.is_empty());
    }

    // ---- 3: no-op / idempotent ----

    #[test]
    fn fully_matching_desired_yields_empty_plan() {
        let live = tbl("a", vec![col("x", "TEXT"), col("y", "INTEGER")]);
        let plan = diff(&doc(vec![live.clone()]), &[Some(live)]);

        assert!(plan.is_empty(), "identical desired/live contributes zero ops");
        assert!(plan.unsupported.is_empty(), "identical desired/live contributes zero unsupported");
    }

    // ---- 4: column added ----

    #[test]
    fn column_present_in_desired_only_yields_add_column() {
        let desired = tbl("a", vec![col("x", "TEXT"), col("y", "INTEGER")]);
        let live = tbl("a", vec![col("x", "TEXT")]);
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert_eq!(
            plan.ops,
            vec![PlanOp::AddColumn {
                table: "a".into(),
                column: col("y", "INTEGER"),
            }],
            "added column must surface as exactly one AddColumn carrying the desired ColumnDef"
        );
    }

    /// Adding a column whose explicitly-desired zone differs from what a
    /// ZONE-less `ADD COLUMN` would infer must converge in ONE apply: the plan
    /// emits `AddColumn` immediately followed by `SetZone` carrying the desired
    /// effective zone. Without the follow-up SetZone the engine inserts the
    /// column into its inferred zone (here Frontmatter for INTEGER), so a single
    /// apply does not converge even though it reports applied=true.
    #[test]
    fn added_column_with_non_default_zone_yields_add_then_set_zone() {
        // INTEGER infers to Frontmatter via a ZONE-less ADD COLUMN; desired
        // pins Body, so the inferred zone is wrong and needs a SetZone.
        let mut desired_col = col("count", "INTEGER");
        desired_col.zone = Some(Zone::Body);
        let desired = tbl("a", vec![col("x", "TEXT"), desired_col.clone()]);
        let live = tbl("a", vec![col("x", "TEXT")]);
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert_eq!(
            plan.ops,
            vec![
                PlanOp::AddColumn {
                    table: "a".into(),
                    column: desired_col,
                },
                PlanOp::SetZone {
                    table: "a".into(),
                    column: "count".into(),
                    zone: Zone::Body,
                },
            ],
            "an added column with a non-inferable desired zone must emit AddColumn then SetZone, in that order"
        );
    }

    /// The negative: when the desired column's effective zone EQUALS what a
    /// ZONE-less `ADD COLUMN` would infer, no spurious SetZone is emitted. Here
    /// INTEGER infers to Frontmatter and the desired zone is also Frontmatter,
    /// so AddColumn alone converges.
    #[test]
    fn added_column_with_default_matching_zone_yields_only_add() {
        let mut desired_col = col("count", "INTEGER");
        desired_col.zone = Some(Zone::Frontmatter); // matches INTEGER inference
        let desired = tbl("a", vec![col("x", "TEXT"), desired_col.clone()]);
        let live = tbl("a", vec![col("x", "TEXT")]);
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert_eq!(
            plan.ops,
            vec![PlanOp::AddColumn {
                table: "a".into(),
                column: desired_col,
            }],
            "an added column whose desired zone equals the inferred zone must not emit a SetZone"
        );
    }

    // ---- 5: column removed ----

    #[test]
    fn column_present_in_live_only_yields_drop_column() {
        let desired = tbl("a", vec![col("x", "TEXT")]);
        let live = tbl("a", vec![col("x", "TEXT"), col("gone", "INTEGER")]);
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert_eq!(
            plan.ops,
            vec![PlanOp::DropColumn {
                table: "a".into(),
                column: "gone".into(),
            }],
            "removed column must surface as exactly one DropColumn"
        );
    }

    // ---- 6: data_type changed ----

    #[test]
    fn changed_data_type_yields_alter_column_type_to_desired() {
        let desired = tbl("a", vec![col("x", "VARCHAR(50)")]);
        let live = tbl("a", vec![col("x", "TEXT")]);
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert_eq!(
            plan.ops,
            vec![PlanOp::AlterColumnType {
                table: "a".into(),
                column: "x".into(),
                new_type: "VARCHAR(50)".into(),
            }],
            "AlterColumnType.new_type must be the DESIRED column's data_type, as the sole op"
        );
    }

    // ---- 7: effective-zone changes ----

    #[test]
    fn same_effective_zone_despite_differing_zone_field_yields_no_set_zone() {
        // Desired pins zone explicitly to Body; live leaves it None but its
        // data_type "TEXT" infers to Body. effective_zone() matches -> no op.
        let mut desired_col = col("x", "TEXT");
        desired_col.zone = Some(Zone::Body);
        let desired = tbl("a", vec![desired_col]);
        let live = tbl("a", vec![col("x", "TEXT")]);
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert!(
            !plan.ops.iter().any(|op| matches!(op, PlanOp::SetZone { .. })),
            "equal effective zones must not produce a SetZone, even if raw zone fields differ"
        );
    }

    #[test]
    fn changed_effective_zone_yields_set_zone_to_desired_zone() {
        // Desired forces Frontmatter; live "TEXT"/None infers to Body.
        let mut desired_col = col("x", "TEXT");
        desired_col.zone = Some(Zone::Frontmatter);
        let desired = tbl("a", vec![desired_col]);
        let live = tbl("a", vec![col("x", "TEXT")]);
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert_eq!(
            plan.ops,
            vec![PlanOp::SetZone {
                table: "a".into(),
                column: "x".into(),
                zone: Zone::Frontmatter,
            }],
            "SetZone.zone must be the desired column's effective_zone(), as the sole op"
        );
    }

    // ---- 8: search_key changed on existing type ----

    #[test]
    fn changed_search_key_on_existing_type_yields_set_search_key() {
        let mut desired = tbl("a", vec![col("name", "TEXT")]);
        desired.search_key = Some("name".into());
        let live = tbl("a", vec![col("name", "TEXT")]); // search_key None
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert_eq!(
            plan.ops,
            vec![PlanOp::SetSearchKey {
                table: "a".into(),
                column: Some("name".into()),
            }],
            "differing search_key on an existing type must yield exactly one SetSearchKey with the desired value"
        );
    }

    #[test]
    fn cleared_search_key_on_existing_type_yields_set_search_key_none() {
        let desired = tbl("a", vec![col("name", "TEXT")]); // search_key None
        let mut live = tbl("a", vec![col("name", "TEXT")]);
        live.search_key = Some("name".into());
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert_eq!(
            plan.ops,
            vec![PlanOp::SetSearchKey {
                table: "a".into(),
                column: None,
            }],
            "clearing search_key must yield exactly one SetSearchKey with column None"
        );
    }

    // ---- 9: singleton changed on existing type ----

    #[test]
    fn changed_singleton_on_existing_type_yields_set_singleton() {
        let mut desired = tbl("a", vec![]);
        desired.singleton = true;
        let live = tbl("a", vec![]); // singleton false
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert_eq!(
            plan.ops,
            vec![PlanOp::SetSingleton { table: "a".into(), on: true }],
            "differing singleton must yield exactly one SetSingleton with the desired flag"
        );
    }

    // ---- 10: non-alterable column attribute ----

    #[test]
    fn changed_required_attribute_records_unsupported_and_emits_no_op() {
        let mut desired_col = col("x", "TEXT");
        desired_col.required = true;
        let desired = tbl("a", vec![desired_col]);
        let live = tbl("a", vec![col("x", "TEXT")]); // required false
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert!(
            plan.ops.is_empty(),
            "no schema-change op exists for a required-flag change"
        );
        assert_eq!(
            plan.unsupported.len(),
            1,
            "exactly one warning for one unsupported change"
        );
        assert!(
            plan.unsupported[0].contains("required"),
            "warning must name the unsupported attribute: {}",
            plan.unsupported[0]
        );
    }

    // ---- 11: non-alterable type attribute ----

    #[test]
    fn changed_title_template_records_unsupported_and_emits_no_op() {
        let mut desired = tbl("a", vec![]);
        desired.title_template = Some("{{name}}".into());
        let live = tbl("a", vec![]); // title_template None
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert!(plan.ops.is_empty(), "no op exists for a title_template change");
        assert_eq!(
            plan.unsupported.len(),
            1,
            "exactly one warning for one unsupported change"
        );
        assert!(
            plan.unsupported[0].contains("title_template"),
            "warning must name the unsupported attribute: {}",
            plan.unsupported[0]
        );
    }

    // ---- 12: origin is not diffed ----

    #[test]
    fn origin_only_difference_yields_empty_plan() {
        let desired = tbl("a", vec![col("x", "TEXT")]);
        let mut live = tbl("a", vec![col("x", "TEXT")]);
        live.origin = Some("bundled".into());
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert!(plan.is_empty(), "origin differences must not generate ops");
        assert!(plan.unsupported.is_empty(), "origin differences must not be flagged unsupported");
    }

    // ---- 13: renames are never inferred ----

    #[test]
    fn removed_plus_added_column_never_infers_rename() {
        let desired = tbl("a", vec![col("new_name", "TEXT")]);
        let live = tbl("a", vec![col("old_name", "TEXT")]);
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        assert!(
            !plan.ops.iter().any(|op| matches!(op, PlanOp::RenameColumn { .. })),
            "a remove+add must never be collapsed into a RenameColumn"
        );
        assert!(
            plan.ops.contains(&PlanOp::DropColumn {
                table: "a".into(),
                column: "old_name".into(),
            }) && plan.ops.contains(&PlanOp::AddColumn {
                table: "a".into(),
                column: col("new_name", "TEXT"),
            }),
            "the result must be a DropColumn plus an AddColumn"
        );
        assert_eq!(
            plan.ops.len(),
            2,
            "exactly a DropColumn and an AddColumn, no phantom ops"
        );
    }

    // ---- 14: destructive ordering ----

    #[test]
    fn drop_column_sorts_after_additive_ops() {
        // One type that both gains "added" and loses "gone".
        let desired = tbl("a", vec![col("kept", "TEXT"), col("added", "INTEGER")]);
        let live = tbl("a", vec![col("kept", "TEXT"), col("gone", "INTEGER")]);
        let plan = diff(&doc(vec![desired]), &[Some(live)]);

        let add_idx = plan
            .ops
            .iter()
            .position(|op| matches!(op, PlanOp::AddColumn { .. }))
            .expect("an AddColumn op is expected");
        let drop_idx = plan
            .ops
            .iter()
            .position(|op| matches!(op, PlanOp::DropColumn { .. }))
            .expect("a DropColumn op is expected");

        assert!(
            drop_idx > add_idx,
            "DropColumn must appear after every additive op"
        );
        assert_eq!(
            plan.ops.len(),
            2,
            "exactly one AddColumn and one DropColumn, no phantom ops"
        );
    }

    #[test]
    fn all_drops_sort_after_all_adds_across_types() {
        let desired_a = tbl("a", vec![col("kept", "TEXT"), col("added_a", "INTEGER")]);
        let live_a = tbl("a", vec![col("kept", "TEXT"), col("gone_a", "INTEGER")]);
        let desired_b = tbl("b", vec![col("kept", "TEXT"), col("added_b", "INTEGER")]);
        let live_b = tbl("b", vec![col("kept", "TEXT"), col("gone_b", "INTEGER")]);
        let plan = diff(
            &doc(vec![desired_a, desired_b]),
            &[Some(live_a), Some(live_b)],
        );

        let last_add = plan
            .ops
            .iter()
            .rposition(|op| matches!(op, PlanOp::AddColumn { .. }))
            .expect("AddColumn ops are expected");
        let first_drop = plan
            .ops
            .iter()
            .position(|op| matches!(op, PlanOp::DropColumn { .. }))
            .expect("DropColumn ops are expected");

        assert!(
            first_drop > last_add,
            "every DropColumn must come after every AddColumn, even across types"
        );
        assert_eq!(
            plan.ops.len(),
            4,
            "exactly two AddColumns and two DropColumns across the two types, no phantom ops"
        );
    }

    // ---- 15: explicit column-rename directive ----

    /// A valid rename directive (from IS in live, to is NOT in live, to IS in
    /// desired) emits exactly one RenameColumn and suppresses the DropColumn
    /// of `from` and the AddColumn of `to`. Without the directive this same
    /// shape is a Drop+Add (see removed_plus_added_column_never_infers_rename);
    /// the directive is what turns it into a data-preserving rename.
    #[test]
    fn rename_directive_emits_rename_not_drop_add() {
        let desired = tbl("a", vec![col("owner", "TEXT")]);
        let live = tbl("a", vec![col("assignee", "TEXT")]);
        let plan = diff(
            &doc_with_renames(vec![desired], vec![rename("a", "assignee", "owner")]),
            &[Some(live)],
        );

        assert_eq!(
            plan.ops,
            vec![PlanOp::RenameColumn {
                table: "a".into(),
                from: "assignee".into(),
                to: "owner".into(),
            }],
            "a valid rename directive must yield exactly one RenameColumn"
        );
        assert!(
            !plan
                .ops
                .iter()
                .any(|op| matches!(op, PlanOp::DropColumn { .. } | PlanOp::AddColumn { .. })),
            "a valid rename must not also Drop `from` or Add `to`"
        );
        assert!(plan.unsupported.is_empty());
    }

    /// A rename directive whose `from` is absent in live has nothing to
    /// rename. It must NOT silently no-op: record an unsupported message
    /// naming the offending column and emit no RenameColumn.
    #[test]
    fn rename_directive_with_absent_from_records_unsupported_no_rename() {
        let desired = tbl("a", vec![col("owner", "TEXT")]);
        // live has no `assignee` column.
        let live = tbl("a", vec![col("owner", "TEXT")]);
        let plan = diff(
            &doc_with_renames(vec![desired], vec![rename("a", "assignee", "owner")]),
            &[Some(live)],
        );

        assert!(
            !plan.ops.iter().any(|op| matches!(op, PlanOp::RenameColumn { .. })),
            "a rename whose `from` is absent must not emit a RenameColumn"
        );
        assert_eq!(
            plan.unsupported.len(),
            1,
            "exactly one warning for the invalid rename"
        );
        assert!(
            plan.unsupported[0].contains("assignee"),
            "warning must name the offending column: {}",
            plan.unsupported[0]
        );
    }

    /// A rename directive whose `to` already exists in live is a target-name
    /// collision. It must NOT silently no-op: record an unsupported message
    /// naming the offending column and emit no RenameColumn.
    #[test]
    fn rename_directive_with_colliding_to_records_unsupported_no_rename() {
        // desired keeps both `owner` and `assignee` so the differ does not try
        // to drop the existing target; the directive itself is the conflict.
        let desired = tbl("a", vec![col("owner", "TEXT"), col("assignee", "TEXT")]);
        // live already has both names: renaming assignee -> owner collides.
        let live = tbl("a", vec![col("owner", "TEXT"), col("assignee", "TEXT")]);
        let plan = diff(
            &doc_with_renames(vec![desired], vec![rename("a", "assignee", "owner")]),
            &[Some(live)],
        );

        assert!(
            !plan.ops.iter().any(|op| matches!(op, PlanOp::RenameColumn { .. })),
            "a rename whose target already exists must not emit a RenameColumn"
        );
        assert_eq!(
            plan.unsupported.len(),
            1,
            "exactly one warning for the colliding rename"
        );
        assert!(
            plan.unsupported[0].contains("owner"),
            "warning must name the offending target column: {}",
            plan.unsupported[0]
        );
    }

    /// Any RenameColumn op must come before any DropColumn op so the live
    /// data is moved to the new name before unrelated columns are removed.
    #[test]
    fn rename_column_sorts_before_drop_column() {
        // `assignee` -> `owner` rename, plus an unrelated `gone` column drop.
        let desired = tbl("a", vec![col("owner", "TEXT")]);
        let live = tbl("a", vec![col("assignee", "TEXT"), col("gone", "INTEGER")]);
        let plan = diff(
            &doc_with_renames(vec![desired], vec![rename("a", "assignee", "owner")]),
            &[Some(live)],
        );

        let rename_idx = plan
            .ops
            .iter()
            .position(|op| matches!(op, PlanOp::RenameColumn { .. }))
            .expect("a RenameColumn op is expected");
        let drop_idx = plan
            .ops
            .iter()
            .position(|op| matches!(op, PlanOp::DropColumn { .. }))
            .expect("a DropColumn op is expected");

        assert!(
            rename_idx < drop_idx,
            "RenameColumn must appear before DropColumn"
        );
    }

    /// A valid rename that ALSO changes the column's data_type must surface
    /// the type change, not swallow it. The plan emits the RenameColumn AND
    /// an AlterColumnType targeting the NEW column name with the desired type.
    /// This guards against an impl that suppresses the whole column on rename.
    #[test]
    fn rename_with_retype_also_emits_alter_column_type() {
        // live `assignee` is TEXT; desired `owner` is VARCHAR(100).
        let desired = tbl("a", vec![col("owner", "VARCHAR(100)")]);
        let live = tbl("a", vec![col("assignee", "TEXT")]);
        let plan = diff(
            &doc_with_renames(vec![desired], vec![rename("a", "assignee", "owner")]),
            &[Some(live)],
        );

        assert!(
            plan.ops.contains(&PlanOp::RenameColumn {
                table: "a".into(),
                from: "assignee".into(),
                to: "owner".into(),
            }),
            "the rename itself must be emitted"
        );
        assert!(
            plan.ops.contains(&PlanOp::AlterColumnType {
                table: "a".into(),
                column: "owner".into(),
                new_type: "VARCHAR(100)".into(),
            }),
            "the type change must be surfaced against the NEW column name, not dropped: {:?}",
            plan.ops
        );
    }
}
