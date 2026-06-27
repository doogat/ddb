use crate::types::{ColumnDef, OnDeleteAction, TableSchema, Zone};

fn zone_token(zone: &Zone) -> &'static str {
    match zone {
        Zone::Frontmatter => "frontmatter",
        Zone::Body => "body",
        Zone::Reference => "reference",
    }
}

/// Double-quote a SQL identifier, doubling any embedded `"`. This is the
/// SQL-correct quoting primitive at the DDL trust boundary; the escape stays
/// even though `validate_identifier` already rejects `"` (defense-in-depth).
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Render a single column. `include_zone` is `true` for `CREATE TABLE`
/// columns and `false` for `ADD COLUMN` (which does not accept inline ZONE).
fn render_column(column: &ColumnDef, include_zone: bool) -> String {
    // The engine normalizes ENUM(...)/SET(...) to data_type="TEXT" plus a
    // separate allowed_values list, so render the enum form back from
    // allowed_values; otherwise the constraint is lost on apply.
    let type_token = match &column.allowed_values {
        Some(values) if !values.is_empty() => {
            let quoted: Vec<String> = values
                .iter()
                .map(|v| format!("'{}'", v.replace('\'', "''")))
                .collect();
            format!("ENUM({})", quoted.join(", "))
        }
        _ => column.data_type.clone(),
    };
    let mut s = format!("{} {}", quote_ident(&column.name), type_token);
    if column.required {
        s.push_str(" NOT NULL");
    }
    if let Some(default) = &column.default_value {
        s.push_str(&format!(" DEFAULT {default}"));
    }
    if let Some(references) = &column.references {
        s.push_str(&format!(" REFERENCES {}", quote_ident(references)));
        if column.on_delete == OnDeleteAction::Cascade {
            s.push_str(" ON DELETE CASCADE");
        }
    }
    if include_zone {
        if let Some(zone) = &column.zone {
            s.push_str(&format!(" ZONE {}", zone_token(zone)));
        }
    }
    s
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlanOp {
    CreateType(TableSchema),
    AddColumn { table: String, column: ColumnDef },
    AlterColumnType { table: String, column: String, new_type: String },
    SetZone { table: String, column: String, zone: Zone },
    SetSearchKey { table: String, column: Option<String> },
    SetSingleton { table: String, on: bool },
    RenameColumn { table: String, from: String, to: String },
    DropColumn { table: String, column: String },
}

impl PlanOp {
    /// Render this op to DDL. Never ends with a `;`.
    pub fn render_sql(&self) -> String {
        match self {
            PlanOp::CreateType(schema) => {
                let mut parts: Vec<String> = schema
                    .columns
                    .iter()
                    .map(|c| render_column(c, true))
                    .collect();
                if let Some(constraints) = &schema.unique_together {
                    for cols in constraints {
                        if !cols.is_empty() {
                            let quoted: Vec<String> =
                                cols.iter().map(|c| quote_ident(c)).collect();
                            parts.push(format!("UNIQUE({})", quoted.join(", ")));
                        }
                    }
                }
                let mut sql = format!(
                    "CREATE TABLE {} ({})",
                    quote_ident(&schema.table_name),
                    parts.join(", ")
                );
                if schema.singleton {
                    sql.push_str(" SINGLETON");
                }
                sql
            }
            PlanOp::AddColumn { table, column } => {
                format!(
                    "ALTER TABLE {} ADD COLUMN {}",
                    quote_ident(table),
                    render_column(column, false)
                )
            }
            PlanOp::AlterColumnType { table, column, new_type } => {
                format!(
                    "ALTER TABLE {} ALTER COLUMN {} TYPE {new_type}",
                    quote_ident(table),
                    quote_ident(column)
                )
            }
            PlanOp::SetZone { table, column, zone } => {
                format!(
                    "ALTER TABLE {} SET ZONE {} FOR {}",
                    quote_ident(table),
                    zone_token(zone),
                    quote_ident(column)
                )
            }
            PlanOp::SetSearchKey { table, column } => match column {
                Some(col) => {
                    format!("ALTER TABLE {} SET SEARCH KEY {}", quote_ident(table), quote_ident(col))
                }
                None => format!("ALTER TABLE {} DROP SEARCH KEY", quote_ident(table)),
            },
            PlanOp::SetSingleton { table, on } => {
                if *on {
                    format!("ALTER TABLE {} SET SINGLETON", quote_ident(table))
                } else {
                    format!("ALTER TABLE {} DROP SINGLETON", quote_ident(table))
                }
            }
            PlanOp::RenameColumn { table, from, to } => {
                format!(
                    "ALTER TABLE {} RENAME COLUMN {} TO {}",
                    quote_ident(table),
                    quote_ident(from),
                    quote_ident(to)
                )
            }
            PlanOp::DropColumn { table, column } => {
                format!("ALTER TABLE {} DROP COLUMN {}", quote_ident(table), quote_ident(column))
            }
        }
    }

    /// Stable kind identifier for reporting.
    pub fn kind(&self) -> &'static str {
        match self {
            PlanOp::CreateType(_) => "create_type",
            PlanOp::AddColumn { .. } => "add_column",
            PlanOp::AlterColumnType { .. } => "alter_column_type",
            PlanOp::SetZone { .. } => "set_zone",
            PlanOp::SetSearchKey { .. } => "set_search_key",
            PlanOp::SetSingleton { .. } => "set_singleton",
            PlanOp::RenameColumn { .. } => "rename_column",
            PlanOp::DropColumn { .. } => "drop_column",
        }
    }

    pub fn is_destructive(&self) -> bool {
        matches!(self, PlanOp::DropColumn { .. } | PlanOp::RenameColumn { .. })
    }

    /// User-facing dry-run description.
    pub fn describe(&self) -> String {
        match self {
            PlanOp::CreateType(s) => format!("create type {}", s.table_name),
            PlanOp::AddColumn { table, column } => {
                format!("add column {} to {table}", column.name)
            }
            PlanOp::AlterColumnType { table, column, new_type } => {
                format!("alter column {column} on {table} to type {new_type}")
            }
            PlanOp::SetZone { table, column, zone } => {
                format!("set zone of {column} on {table} to {}", zone_token(zone))
            }
            PlanOp::SetSearchKey { table, column } => match column {
                Some(col) => format!("set search key of {table} to {col}"),
                None => format!("reset search key of {table} to title"),
            },
            PlanOp::SetSingleton { table, on } => {
                if *on {
                    format!("set singleton on {table}")
                } else {
                    format!("clear singleton on {table}")
                }
            }
            PlanOp::RenameColumn { table, from, to } => {
                format!("rename column {from} to {to} on {table}")
            }
            PlanOp::DropColumn { table, column } => {
                format!("drop column {column} from {table}")
            }
        }
    }

    /// Affected table, read from the structured field (never parsed from SQL).
    fn affected_table(&self) -> &str {
        match self {
            PlanOp::CreateType(s) => &s.table_name,
            PlanOp::AddColumn { table, .. }
            | PlanOp::AlterColumnType { table, .. }
            | PlanOp::SetZone { table, .. }
            | PlanOp::SetSearchKey { table, .. }
            | PlanOp::SetSingleton { table, .. }
            | PlanOp::RenameColumn { table, .. }
            | PlanOp::DropColumn { table, .. } => table,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaPlan {
    pub ops: Vec<PlanOp>,
    pub unsupported: Vec<String>,
}

impl SchemaPlan {
    /// `true` when there are no ops (ignores `unsupported`).
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn has_destructive(&self) -> bool {
        self.ops.iter().any(|op| op.is_destructive())
    }

    /// Render all ops as `;`-terminated statements joined by newlines.
    pub fn to_sql(&self) -> String {
        self.ops
            .iter()
            .map(|op| format!("{};", op.render_sql()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SchemaApplyReport {
    pub dry_run: bool,
    pub applied: bool,
    pub ops: Vec<PlanOpReport>,
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PlanOpReport {
    pub kind: String,
    pub table: String,
    pub detail: String,
    pub destructive: bool,
    pub sql: String,
}

impl SchemaApplyReport {
    pub fn from_plan(plan: &SchemaPlan, dry_run: bool, applied: bool) -> Self {
        let ops = plan
            .ops
            .iter()
            .map(|op| PlanOpReport {
                kind: op.kind().to_string(),
                table: op.affected_table().to_string(),
                detail: op.describe(),
                destructive: op.is_destructive(),
                sql: op.render_sql(),
            })
            .collect();
        SchemaApplyReport {
            dry_run,
            applied,
            ops,
            unsupported: plan.unsupported.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ColumnDef, OnDeleteAction, TableSchema, Zone};

    /// Build a `ColumnDef` with all fields explicit. `zone` and the
    /// reference/on_delete behavior are the parts the rendering contract
    /// keys off, so they are parameterized; everything else is fixed.
    fn col(
        name: &str,
        data_type: &str,
        required: bool,
        zone: Option<Zone>,
        default_value: Option<&str>,
        references: Option<&str>,
        on_delete: OnDeleteAction,
    ) -> ColumnDef {
        ColumnDef {
            name: name.into(),
            data_type: data_type.into(),
            references: references.map(Into::into),
            zone,
            required,
            search_boost: None,
            allowed_values: None,
            default_value: default_value.map(Into::into),
            on_delete,
        }
    }

    /// A plain non-required column with no zone/default/reference.
    fn simple_col(name: &str, data_type: &str) -> ColumnDef {
        col(
            name,
            data_type,
            false,
            None,
            None,
            None,
            OnDeleteAction::Restrict,
        )
    }

    /// Build a `TableSchema` with all fields explicit. Only the fields the
    /// rendering contract reads (`table_name`, `columns`, `unique_together`,
    /// `singleton`) vary; the rest are fixed.
    fn schema(
        name: &str,
        columns: Vec<ColumnDef>,
        singleton: bool,
        unique_together: Option<Vec<Vec<String>>>,
    ) -> TableSchema {
        TableSchema {
            table_name: name.into(),
            columns,
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
            unique_together,
            search_key: None,
            singleton,
        }
    }

    // ── render_sql + kind per PlanOp variant ────────────────────────────

    #[test]
    fn render_create_type_single_required_column_with_zone() {
        // Contract example: required column with frontmatter zone, no
        // singleton. Pins NOT NULL ordering before ZONE and the lowercase
        // zone token.
        let op = PlanOp::CreateType(schema(
            "contact",
            vec![col(
                "email",
                "VARCHAR(255)",
                true,
                Some(Zone::Frontmatter),
                None,
                None,
                OnDeleteAction::Restrict,
            )],
            false,
            None,
        ));
        assert_eq!(
            op.render_sql(),
            r#"CREATE TABLE "contact" ("email" VARCHAR(255) NOT NULL ZONE frontmatter)"#
        );
        assert_eq!(op.kind(), "create_type");
    }

    #[test]
    fn render_create_type_multiple_columns_comma_separated() {
        // Two columns must be joined by ", " inside the paren list, in
        // declaration order. A naive impl that renders only the first
        // column, or drops the separator, fails here.
        let op = PlanOp::CreateType(schema(
            "project",
            vec![
                simple_col("status", "VARCHAR(50)"),
                simple_col("owner", "VARCHAR(100)"),
            ],
            false,
            None,
        ));
        assert_eq!(
            op.render_sql(),
            r#"CREATE TABLE "project" ("status" VARCHAR(50), "owner" VARCHAR(100))"#
        );
    }

    #[test]
    fn render_create_type_column_with_zone_none_omits_zone_clause() {
        // zone: None must produce NO ` ZONE` token. Guards against an impl
        // that always appends a zone (e.g. via effective_zone inference).
        let op = PlanOp::CreateType(schema(
            "project",
            vec![simple_col("status", "VARCHAR(50)")],
            false,
            None,
        ));
        assert_eq!(op.render_sql(), r#"CREATE TABLE "project" ("status" VARCHAR(50))"#);
        assert!(!op.render_sql().contains("ZONE"));
    }

    #[test]
    fn render_create_type_zone_body_and_reference_tokens_are_lowercase() {
        // Each non-frontmatter zone variant maps to its lowercase token.
        let body = PlanOp::CreateType(schema(
            "note",
            vec![col(
                "content",
                "TEXT",
                false,
                Some(Zone::Body),
                None,
                None,
                OnDeleteAction::Restrict,
            )],
            false,
            None,
        ));
        assert_eq!(body.render_sql(), r#"CREATE TABLE "note" ("content" TEXT ZONE body)"#);

        let reference = PlanOp::CreateType(schema(
            "task",
            vec![col(
                "project",
                "VARCHAR(100)",
                false,
                Some(Zone::Reference),
                None,
                None,
                OnDeleteAction::Restrict,
            )],
            false,
            None,
        ));
        assert_eq!(
            reference.render_sql(),
            r#"CREATE TABLE "task" ("project" VARCHAR(100) ZONE reference)"#
        );
    }

    #[test]
    fn render_create_type_default_value_clause() {
        // DEFAULT clause appears iff default_value.is_some(), after the
        // NOT NULL slot (absent here) and verbatim value.
        let op = PlanOp::CreateType(schema(
            "project",
            vec![col(
                "status",
                "VARCHAR(50)",
                false,
                None,
                Some("active"),
                None,
                OnDeleteAction::Restrict,
            )],
            false,
            None,
        ));
        assert_eq!(
            op.render_sql(),
            r#"CREATE TABLE "project" ("status" VARCHAR(50) DEFAULT active)"#
        );
    }

    #[test]
    fn render_create_type_references_with_on_delete_restrict_omits_cascade() {
        // RESTRICT is the default and is OMITTED: only ` REFERENCES r`
        // renders, no ` ON DELETE` clause.
        let op = PlanOp::CreateType(schema(
            "task",
            vec![col(
                "owner",
                "VARCHAR(100)",
                false,
                None,
                None,
                Some("contact"),
                OnDeleteAction::Restrict,
            )],
            false,
            None,
        ));
        assert_eq!(
            op.render_sql(),
            r#"CREATE TABLE "task" ("owner" VARCHAR(100) REFERENCES "contact")"#
        );
        assert!(!op.render_sql().contains("ON DELETE"));
    }

    #[test]
    fn render_create_type_references_with_on_delete_cascade_renders_clause() {
        // Cascade renders ` ON DELETE CASCADE` immediately after REFERENCES.
        let op = PlanOp::CreateType(schema(
            "task",
            vec![col(
                "owner",
                "VARCHAR(100)",
                false,
                None,
                None,
                Some("contact"),
                OnDeleteAction::Cascade,
            )],
            false,
            None,
        ));
        assert_eq!(
            op.render_sql(),
            r#"CREATE TABLE "task" ("owner" VARCHAR(100) REFERENCES "contact" ON DELETE CASCADE)"#
        );
    }

    #[test]
    fn render_create_type_full_clause_ordering() {
        // All optional clauses present on one column to pin the order:
        // NOT NULL, DEFAULT, REFERENCES (+ON DELETE CASCADE), ZONE.
        let op = PlanOp::CreateType(schema(
            "task",
            vec![col(
                "owner",
                "VARCHAR(100)",
                true,
                Some(Zone::Reference),
                Some("nobody"),
                Some("contact"),
                OnDeleteAction::Cascade,
            )],
            false,
            None,
        ));
        assert_eq!(
            op.render_sql(),
            r#"CREATE TABLE "task" ("owner" VARCHAR(100) NOT NULL DEFAULT nobody REFERENCES "contact" ON DELETE CASCADE ZONE reference)"#
        );
    }

    #[test]
    fn render_create_type_singleton_appends_trailing_singleton() {
        // singleton:true appends a trailing ` SINGLETON` after the paren list.
        let op = PlanOp::CreateType(schema(
            "settings",
            vec![simple_col("theme", "VARCHAR(50)")],
            true,
            None,
        ));
        assert_eq!(
            op.render_sql(),
            r#"CREATE TABLE "settings" ("theme" VARCHAR(50)) SINGLETON"#
        );
    }

    #[test]
    fn render_create_type_unique_together_appends_table_level_constraints() {
        // Each unique_together constraint renders as a table-level
        // UNIQUE(...) appended inside the paren list after the columns,
        // comma-separated. Two constraints prove both the inner comma list
        // and the multi-constraint separation.
        let op = PlanOp::CreateType(schema(
            "membership",
            vec![
                simple_col("team", "VARCHAR(50)"),
                simple_col("person", "VARCHAR(50)"),
            ],
            false,
            Some(vec![
                vec!["team".into(), "person".into()],
                vec!["person".into()],
            ]),
        ));
        assert_eq!(
            op.render_sql(),
            r#"CREATE TABLE "membership" ("team" VARCHAR(50), "person" VARCHAR(50), UNIQUE("team", "person"), UNIQUE("person"))"#
        );
    }

    #[test]
    fn render_create_type_empty_unique_together_adds_no_constraint() {
        // Some(empty) is non-Some-and-non-empty -> no UNIQUE constraint.
        // Guards an impl that emits a dangling ", UNIQUE()" for an empty vec.
        let op = PlanOp::CreateType(schema(
            "project",
            vec![simple_col("status", "VARCHAR(50)")],
            false,
            Some(vec![]),
        ));
        assert_eq!(op.render_sql(), r#"CREATE TABLE "project" ("status" VARCHAR(50))"#);
        assert!(!op.render_sql().contains("UNIQUE"));
    }

    #[test]
    fn render_create_type_does_not_emit_search_key() {
        // search_key has no inline CREATE form: even when set on the schema
        // it must never appear in CreateType render.
        let mut s = schema(
            "contact",
            vec![simple_col("email", "VARCHAR(255)")],
            false,
            None,
        );
        s.search_key = Some("email".into());
        let op = PlanOp::CreateType(s);
        assert_eq!(op.render_sql(), r#"CREATE TABLE "contact" ("email" VARCHAR(255))"#);
        assert!(!op.render_sql().contains("SEARCH KEY"));
    }

    #[test]
    fn render_create_type_enum_column_emits_enum_constraint() {
        // allowed_values has a real CREATE form (ENUM(...)). The engine
        // normalizes ENUM(...) to data_type="TEXT" + separate allowed_values,
        // so rendering the bare data_type ("TEXT") would silently drop the
        // constraint on apply. The render must reconstruct ENUM(...).
        let status = ColumnDef {
            name: "status".into(),
            data_type: "TEXT".into(),
            references: None,
            zone: None,
            required: false,
            search_boost: None,
            allowed_values: Some(vec!["todo".into(), "doing".into(), "done".into()]),
            default_value: None,
            on_delete: OnDeleteAction::Restrict,
        };
        let op = PlanOp::CreateType(schema("task", vec![status], false, None));
        assert_eq!(
            op.render_sql(),
            r#"CREATE TABLE "task" ("status" ENUM('todo', 'doing', 'done'))"#
        );
    }

    #[test]
    fn render_add_column_enum_emits_enum_constraint() {
        // ADD COLUMN accepts ENUM(...) too, so an added enum column must
        // carry its constraint rather than render as a bare TEXT column.
        let status = ColumnDef {
            name: "status".into(),
            data_type: "TEXT".into(),
            references: None,
            zone: None,
            required: false,
            search_boost: None,
            allowed_values: Some(vec!["todo".into(), "done".into()]),
            default_value: None,
            on_delete: OnDeleteAction::Restrict,
        };
        let op = PlanOp::AddColumn {
            table: "task".into(),
            column: status,
        };
        assert_eq!(
            op.render_sql(),
            r#"ALTER TABLE "task" ADD COLUMN "status" ENUM('todo', 'done')"#
        );
    }

    #[test]
    fn render_add_column_basic() {
        let op = PlanOp::AddColumn {
            table: "contact".into(),
            column: simple_col("phone", "VARCHAR(30)"),
        };
        assert_eq!(
            op.render_sql(),
            r#"ALTER TABLE "contact" ADD COLUMN "phone" VARCHAR(30)"#
        );
        assert_eq!(op.kind(), "add_column");
    }

    #[test]
    fn render_add_column_required_default_references_cascade() {
        // ADD COLUMN renders NOT NULL / DEFAULT / REFERENCES (+ON DELETE
        // CASCADE) clauses in the same order as CreateType columns.
        let op = PlanOp::AddColumn {
            table: "task".into(),
            column: col(
                "owner",
                "VARCHAR(100)",
                true,
                Some(Zone::Reference),
                Some("nobody"),
                Some("contact"),
                OnDeleteAction::Cascade,
            ),
        };
        assert_eq!(
            op.render_sql(),
            r#"ALTER TABLE "task" ADD COLUMN "owner" VARCHAR(100) NOT NULL DEFAULT nobody REFERENCES "contact" ON DELETE CASCADE"#
        );
    }

    #[test]
    fn render_add_column_never_emits_zone_even_when_set() {
        // ADD COLUMN does not accept inline ZONE: a column carrying a zone
        // must still render WITHOUT a ZONE clause.
        let op = PlanOp::AddColumn {
            table: "contact".into(),
            column: col(
                "email",
                "VARCHAR(255)",
                false,
                Some(Zone::Frontmatter),
                None,
                None,
                OnDeleteAction::Restrict,
            ),
        };
        assert_eq!(
            op.render_sql(),
            r#"ALTER TABLE "contact" ADD COLUMN "email" VARCHAR(255)"#
        );
        assert!(!op.render_sql().contains("ZONE"));
    }

    #[test]
    fn render_alter_column_type() {
        let op = PlanOp::AlterColumnType {
            table: "contact".into(),
            column: "email".into(),
            new_type: "TEXT".into(),
        };
        assert_eq!(
            op.render_sql(),
            r#"ALTER TABLE "contact" ALTER COLUMN "email" TYPE TEXT"#
        );
        assert_eq!(op.kind(), "alter_column_type");
    }

    #[test]
    fn render_set_zone_uses_zone_then_for_column_order() {
        // Contract: `SET ZONE <zone> FOR <column>` — zone token precedes the
        // column, and the zone token is lowercase.
        let op = PlanOp::SetZone {
            table: "contact".into(),
            column: "email".into(),
            zone: Zone::Body,
        };
        assert_eq!(
            op.render_sql(),
            r#"ALTER TABLE "contact" SET ZONE body FOR "email""#
        );
        assert_eq!(op.kind(), "set_zone");
    }

    #[test]
    fn render_set_search_key_some_names_the_column() {
        let op = PlanOp::SetSearchKey {
            table: "contact".into(),
            column: Some("fqn".into()),
        };
        assert_eq!(op.render_sql(), r#"ALTER TABLE "contact" SET SEARCH KEY "fqn""#);
        assert_eq!(op.kind(), "set_search_key");
    }

    #[test]
    fn render_set_search_key_none_renders_drop() {
        // None means reset-to-title -> DROP SEARCH KEY, NOT a SET with an
        // empty/placeholder column.
        let op = PlanOp::SetSearchKey {
            table: "contact".into(),
            column: None,
        };
        assert_eq!(op.render_sql(), r#"ALTER TABLE "contact" DROP SEARCH KEY"#);
        assert_eq!(op.kind(), "set_search_key");
    }

    #[test]
    fn render_set_singleton_on_renders_set() {
        let op = PlanOp::SetSingleton {
            table: "settings".into(),
            on: true,
        };
        assert_eq!(op.render_sql(), r#"ALTER TABLE "settings" SET SINGLETON"#);
        assert_eq!(op.kind(), "set_singleton");
    }

    #[test]
    fn render_set_singleton_off_renders_drop() {
        // on:false must render DROP SINGLETON, not SET.
        let op = PlanOp::SetSingleton {
            table: "settings".into(),
            on: false,
        };
        assert_eq!(op.render_sql(), r#"ALTER TABLE "settings" DROP SINGLETON"#);
        assert_eq!(op.kind(), "set_singleton");
    }

    #[test]
    fn render_rename_column() {
        let op = PlanOp::RenameColumn {
            table: "contact".into(),
            from: "mail".into(),
            to: "email".into(),
        };
        assert_eq!(
            op.render_sql(),
            r#"ALTER TABLE "contact" RENAME COLUMN "mail" TO "email""#
        );
        assert_eq!(op.kind(), "rename_column");
    }

    #[test]
    fn render_drop_column() {
        let op = PlanOp::DropColumn {
            table: "contact".into(),
            column: "fax".into(),
        };
        assert_eq!(op.render_sql(), r#"ALTER TABLE "contact" DROP COLUMN "fax""#);
        assert_eq!(op.kind(), "drop_column");
    }

    #[test]
    fn render_hyphenated_identifiers_are_quoted_in_create_and_add() {
        // The bug this contract fixes: hyphenated names pass SchemaDoc
        // validation (e.g. type `meeting-minutes`, column `long-desc`), but a
        // bare-identifier render emits DDL the SQL parser rejects on the `-`.
        // Every identifier the render emits must be double-quoted so a
        // hyphenated name round-trips — the table name AND the column name, in
        // both CreateType and AddColumn. Non-identifiers (the TEXT data type)
        // stay unquoted.
        let create = PlanOp::CreateType(schema(
            "meeting-minutes",
            vec![simple_col("long-desc", "TEXT")],
            false,
            None,
        ));
        assert_eq!(
            create.render_sql(),
            r#"CREATE TABLE "meeting-minutes" ("long-desc" TEXT)"#
        );

        let add = PlanOp::AddColumn {
            table: "meeting-minutes".into(),
            column: simple_col("long-desc", "TEXT"),
        };
        assert_eq!(
            add.render_sql(),
            r#"ALTER TABLE "meeting-minutes" ADD COLUMN "long-desc" TEXT"#
        );
    }

    #[test]
    fn render_create_type_quotes_reserved_word_and_novel_identifiers() {
        // The render-level pin for NOVEL names: a column named with the SQL
        // reserved word `order` on a non-allowlisted type `ledger` must still be
        // double-quoted. Neither name is hyphenated, and neither is in the
        // finite golden set the other render_* tests use, so this kills a
        // quote-only-on-hyphen impl AND a quote-only-for-a-hardcoded-name
        // -allowlist impl: every identifier the render emits is quoted, not just
        // the ones prior tests happen to exercise. (Non-identifiers — the TEXT
        // data type — stay unquoted.)
        let op = PlanOp::CreateType(schema(
            "ledger",
            vec![simple_col("order", "TEXT")],
            false,
            None,
        ));
        assert_eq!(op.render_sql(), r#"CREATE TABLE "ledger" ("order" TEXT)"#);
    }

    #[test]
    fn render_sql_has_no_trailing_semicolon() {
        // Cross-variant invariant: render_sql never ends with ';'.
        let ops = vec![
            PlanOp::CreateType(schema("c", vec![simple_col("x", "TEXT")], false, None)),
            PlanOp::AddColumn {
                table: "c".into(),
                column: simple_col("y", "TEXT"),
            },
            PlanOp::AlterColumnType {
                table: "c".into(),
                column: "x".into(),
                new_type: "TEXT".into(),
            },
            PlanOp::SetZone {
                table: "c".into(),
                column: "x".into(),
                zone: Zone::Body,
            },
            PlanOp::SetSearchKey {
                table: "c".into(),
                column: Some("x".into()),
            },
            PlanOp::SetSearchKey {
                table: "c".into(),
                column: None,
            },
            PlanOp::SetSingleton {
                table: "c".into(),
                on: true,
            },
            PlanOp::SetSingleton {
                table: "c".into(),
                on: false,
            },
            PlanOp::RenameColumn {
                table: "c".into(),
                from: "x".into(),
                to: "z".into(),
            },
            PlanOp::DropColumn {
                table: "c".into(),
                column: "x".into(),
            },
        ];
        for op in &ops {
            assert!(
                !op.render_sql().ends_with(';'),
                "render_sql must not end with ';': {:?}",
                op
            );
        }
    }

    // ── is_destructive ──────────────────────────────────────────────────

    #[test]
    fn is_destructive_true_for_drop_and_rename_only() {
        assert!(PlanOp::DropColumn {
            table: "t".into(),
            column: "c".into(),
        }
        .is_destructive());
        assert!(PlanOp::RenameColumn {
            table: "t".into(),
            from: "a".into(),
            to: "b".into(),
        }
        .is_destructive());
    }

    #[test]
    fn is_destructive_false_for_all_other_variants() {
        let non_destructive = vec![
            PlanOp::CreateType(schema("t", vec![simple_col("c", "TEXT")], false, None)),
            PlanOp::AddColumn {
                table: "t".into(),
                column: simple_col("c", "TEXT"),
            },
            PlanOp::AlterColumnType {
                table: "t".into(),
                column: "c".into(),
                new_type: "TEXT".into(),
            },
            PlanOp::SetZone {
                table: "t".into(),
                column: "c".into(),
                zone: Zone::Body,
            },
            PlanOp::SetSearchKey {
                table: "t".into(),
                column: Some("c".into()),
            },
            PlanOp::SetSearchKey {
                table: "t".into(),
                column: None,
            },
            PlanOp::SetSingleton {
                table: "t".into(),
                on: true,
            },
            PlanOp::SetSingleton {
                table: "t".into(),
                on: false,
            },
        ];
        for op in &non_destructive {
            assert!(!op.is_destructive(), "expected non-destructive: {:?}", op);
        }
    }

    // ── describe (substring asserts only) ───────────────────────────────

    #[test]
    fn describe_mentions_table_and_target_for_drop_column() {
        let op = PlanOp::DropColumn {
            table: "contact".into(),
            column: "fax".into(),
        };
        let d = op.describe();
        assert!(d.contains("contact"), "describe missing table: {d}");
        assert!(d.contains("fax"), "describe missing column: {d}");
    }

    #[test]
    fn describe_mentions_both_names_for_rename_column() {
        let op = PlanOp::RenameColumn {
            table: "contact".into(),
            from: "mail".into(),
            to: "email".into(),
        };
        let d = op.describe();
        assert!(d.contains("mail"), "describe missing from: {d}");
        assert!(d.contains("email"), "describe missing to: {d}");
    }

    #[test]
    fn describe_mentions_table_for_create_type() {
        let op = PlanOp::CreateType(schema(
            "contact",
            vec![simple_col("email", "VARCHAR(255)")],
            false,
            None,
        ));
        assert!(
            op.describe().contains("contact"),
            "describe missing table name: {}",
            op.describe()
        );
    }

    // ── SchemaPlan: is_empty / has_destructive / to_sql ─────────────────

    #[test]
    fn is_empty_true_when_no_ops_even_with_unsupported() {
        // is_empty IGNORES unsupported: a plan with no ops but a non-empty
        // unsupported list is still empty.
        let plan = SchemaPlan {
            ops: vec![],
            unsupported: vec!["drop type contact".into()],
        };
        assert!(plan.is_empty());
    }

    #[test]
    fn is_empty_false_when_ops_present() {
        let plan = SchemaPlan {
            ops: vec![PlanOp::SetSingleton {
                table: "t".into(),
                on: true,
            }],
            unsupported: vec![],
        };
        assert!(!plan.is_empty());
    }

    #[test]
    fn has_destructive_true_when_any_op_destructive() {
        let plan = SchemaPlan {
            ops: vec![
                PlanOp::AddColumn {
                    table: "t".into(),
                    column: simple_col("c", "TEXT"),
                },
                PlanOp::DropColumn {
                    table: "t".into(),
                    column: "old".into(),
                },
            ],
            unsupported: vec![],
        };
        assert!(plan.has_destructive());
    }

    #[test]
    fn has_destructive_false_when_no_op_destructive() {
        let plan = SchemaPlan {
            ops: vec![
                PlanOp::AddColumn {
                    table: "t".into(),
                    column: simple_col("c", "TEXT"),
                },
                PlanOp::SetSingleton {
                    table: "t".into(),
                    on: false,
                },
            ],
            unsupported: vec![],
        };
        assert!(!plan.has_destructive());
    }

    #[test]
    fn to_sql_empty_plan_is_blank() {
        let plan = SchemaPlan {
            ops: vec![],
            unsupported: vec![],
        };
        assert_eq!(plan.to_sql(), "");
    }

    #[test]
    fn to_sql_empty_ops_blank_even_with_unsupported() {
        // to_sql renders ops only; an unsupported-only plan still yields "".
        let plan = SchemaPlan {
            ops: vec![],
            unsupported: vec!["something".into()],
        };
        assert_eq!(plan.to_sql(), "");
    }

    #[test]
    fn to_sql_appends_semicolon_per_op_and_joins_with_newline() {
        // [A, B] -> "<A>;\n<B>;": each op's render_sql gets a ';', joined by
        // '\n'. A trailing ';' on the final op is required.
        let plan = SchemaPlan {
            ops: vec![
                PlanOp::DropColumn {
                    table: "contact".into(),
                    column: "fax".into(),
                },
                PlanOp::SetSingleton {
                    table: "contact".into(),
                    on: true,
                },
            ],
            unsupported: vec![],
        };
        assert_eq!(
            plan.to_sql(),
            "ALTER TABLE \"contact\" DROP COLUMN \"fax\";\nALTER TABLE \"contact\" SET SINGLETON;"
        );
    }

    #[test]
    fn to_sql_single_op_has_trailing_semicolon_and_no_newline() {
        let plan = SchemaPlan {
            ops: vec![PlanOp::SetSingleton {
                table: "contact".into(),
                on: false,
            }],
            unsupported: vec![],
        };
        assert_eq!(plan.to_sql(), "ALTER TABLE \"contact\" DROP SINGLETON;");
    }

    // ── SchemaApplyReport::from_plan ────────────────────────────────────

    #[test]
    fn from_plan_copies_dry_run_and_applied_flags() {
        let plan = SchemaPlan {
            ops: vec![],
            unsupported: vec![],
        };
        let r1 = SchemaApplyReport::from_plan(&plan, true, false);
        assert!(r1.dry_run);
        assert!(!r1.applied);

        let r2 = SchemaApplyReport::from_plan(&plan, false, true);
        assert!(!r2.dry_run);
        assert!(r2.applied);
    }

    #[test]
    fn from_plan_clones_unsupported() {
        let plan = SchemaPlan {
            ops: vec![],
            unsupported: vec!["drop type a".into(), "rename type b".into()],
        };
        let report = SchemaApplyReport::from_plan(&plan, true, false);
        assert_eq!(
            report.unsupported,
            vec!["drop type a".to_string(), "rename type b".to_string()]
        );
    }

    #[test]
    fn from_plan_emits_one_report_per_op_in_order() {
        let plan = SchemaPlan {
            ops: vec![
                PlanOp::AddColumn {
                    table: "contact".into(),
                    column: simple_col("phone", "VARCHAR(30)"),
                },
                PlanOp::DropColumn {
                    table: "contact".into(),
                    column: "fax".into(),
                },
            ],
            unsupported: vec![],
        };
        let report = SchemaApplyReport::from_plan(&plan, false, true);
        assert_eq!(report.ops.len(), 2);
        assert_eq!(report.ops[0].kind, "add_column");
        assert_eq!(report.ops[1].kind, "drop_column");
    }

    #[test]
    fn from_plan_maps_each_field_from_matching_plan_op() {
        // The op-report fields must come from the matching PlanOp's own
        // methods: kind/detail/destructive/sql, plus the affected table.
        let drop = PlanOp::DropColumn {
            table: "contact".into(),
            column: "fax".into(),
        };
        let plan = SchemaPlan {
            ops: vec![drop.clone()],
            unsupported: vec![],
        };
        let report = SchemaApplyReport::from_plan(&plan, true, false);
        let r = &report.ops[0];
        assert_eq!(r.kind, drop.kind());
        assert_eq!(r.table, "contact");
        assert_eq!(r.detail, drop.describe());
        assert_eq!(r.destructive, drop.is_destructive());
        assert!(r.destructive); // DropColumn is destructive — pins the bool, not just equality
        assert_eq!(r.sql, drop.render_sql());
        assert_eq!(r.sql, r#"ALTER TABLE "contact" DROP COLUMN "fax""#);
    }

    #[test]
    fn from_plan_create_type_table_is_schema_table_name() {
        // For CreateType the report's `table` is schema.table_name.
        let create = PlanOp::CreateType(schema(
            "contact",
            vec![simple_col("email", "VARCHAR(255)")],
            false,
            None,
        ));
        let plan = SchemaPlan {
            ops: vec![create.clone()],
            unsupported: vec![],
        };
        let report = SchemaApplyReport::from_plan(&plan, true, false);
        let r = &report.ops[0];
        assert_eq!(r.table, "contact");
        assert_eq!(r.kind, "create_type");
        assert!(!r.destructive);
        assert_eq!(r.sql, create.render_sql());
    }

    #[test]
    fn from_plan_non_destructive_op_reports_destructive_false() {
        let add = PlanOp::AddColumn {
            table: "contact".into(),
            column: simple_col("phone", "VARCHAR(30)"),
        };
        let plan = SchemaPlan {
            ops: vec![add],
            unsupported: vec![],
        };
        let report = SchemaApplyReport::from_plan(&plan, false, true);
        assert!(!report.ops[0].destructive);
    }

    // ── describe (exact, unambiguous) ───────────────────────────────────

    #[test]
    fn describe_is_exact_and_unambiguous_per_variant() {
        // describe() is the user-facing dry-run line. Pin it exactly so a
        // CreateType can never read as a "drop" and a rename's direction is
        // explicit. (Substring-only asserts let garbage describe() text
        // through — a wrong impl described every op as "drop".)
        assert_eq!(
            PlanOp::CreateType(schema(
                "contact",
                vec![simple_col("email", "VARCHAR(255)")],
                false,
                None,
            ))
            .describe(),
            "create type contact"
        );
        assert_eq!(
            PlanOp::AddColumn {
                table: "contact".into(),
                column: simple_col("phone", "VARCHAR(30)"),
            }
            .describe(),
            "add column phone to contact"
        );
        assert_eq!(
            PlanOp::AlterColumnType {
                table: "contact".into(),
                column: "email".into(),
                new_type: "TEXT".into(),
            }
            .describe(),
            "alter column email on contact to type TEXT"
        );
        assert_eq!(
            PlanOp::SetZone {
                table: "contact".into(),
                column: "email".into(),
                zone: Zone::Body,
            }
            .describe(),
            "set zone of email on contact to body"
        );
        assert_eq!(
            PlanOp::SetSearchKey {
                table: "contact".into(),
                column: Some("fqn".into()),
            }
            .describe(),
            "set search key of contact to fqn"
        );
        assert_eq!(
            PlanOp::SetSearchKey {
                table: "contact".into(),
                column: None,
            }
            .describe(),
            "reset search key of contact to title"
        );
        assert_eq!(
            PlanOp::SetSingleton {
                table: "settings".into(),
                on: true,
            }
            .describe(),
            "set singleton on settings"
        );
        assert_eq!(
            PlanOp::SetSingleton {
                table: "settings".into(),
                on: false,
            }
            .describe(),
            "clear singleton on settings"
        );
        assert_eq!(
            PlanOp::RenameColumn {
                table: "contact".into(),
                from: "mail".into(),
                to: "email".into(),
            }
            .describe(),
            "rename column mail to email on contact"
        );
        assert_eq!(
            PlanOp::DropColumn {
                table: "contact".into(),
                column: "fax".into(),
            }
            .describe(),
            "drop column fax from contact"
        );
    }

    // ── PartialEq derive (value comparison) ─────────────────────────────

    #[test]
    fn plan_op_and_schema_plan_are_comparable_by_value() {
        // The contract derives PartialEq on PlanOp and SchemaPlan. Pin it so
        // the derives cannot be silently dropped — no other test compares a
        // whole op or plan by value, so without this a non-PartialEq impl
        // (or one over-/under-comparing fields) passes.
        let a = PlanOp::AddColumn {
            table: "contact".into(),
            column: simple_col("phone", "VARCHAR(30)"),
        };
        let b = PlanOp::AddColumn {
            table: "contact".into(),
            column: simple_col("phone", "VARCHAR(30)"),
        };
        let c = PlanOp::AddColumn {
            table: "contact".into(),
            column: simple_col("fax", "VARCHAR(30)"),
        };
        assert_eq!(a, b);
        assert_ne!(a, c);

        // CreateType equality exercises TableSchema/ColumnDef PartialEq.
        let create_a = PlanOp::CreateType(schema(
            "contact",
            vec![simple_col("email", "VARCHAR(255)")],
            false,
            None,
        ));
        let create_b = PlanOp::CreateType(schema(
            "contact",
            vec![simple_col("email", "VARCHAR(255)")],
            false,
            None,
        ));
        let create_c = PlanOp::CreateType(schema(
            "contact",
            vec![simple_col("email", "VARCHAR(255)")],
            true, // singleton differs
            None,
        ));
        assert_eq!(create_a, create_b);
        assert_ne!(create_a, create_c);

        let plan_a = SchemaPlan {
            ops: vec![a.clone()],
            unsupported: vec!["x".into()],
        };
        let plan_b = SchemaPlan {
            ops: vec![b.clone()],
            unsupported: vec!["x".into()],
        };
        let plan_c = SchemaPlan {
            ops: vec![c.clone()],
            unsupported: vec!["x".into()],
        };
        assert_eq!(plan_a, plan_b);
        assert_ne!(plan_a, plan_c);
    }
}
