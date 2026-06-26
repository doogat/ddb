use std::collections::BTreeMap;

use sqlparser::ast::{ColumnOption, Statement};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

use crate::error::{DoogatError, Result};
use crate::parser::from_serde_yaml;
use crate::sql_engine::schema_from_parsed;
use crate::types::{DoogatMeta, ParsedDoogat, TableSchema, Value};

/// A declarative description of the desired typedefs.
pub struct SchemaDoc {
    /// One [`TableSchema`](crate::types::TableSchema) per declared type.
    pub types: Vec<crate::types::TableSchema>,
    /// Explicit per-column rename directives, one per `rename_from:` key.
    pub renames: Vec<ColumnRename>,
}

/// An explicit column-rename directive: rename `from` to `to` on `table`.
#[derive(Debug, Clone)]
pub struct ColumnRename {
    pub table: String,
    pub from: String,
    pub to: String,
}

impl SchemaDoc {
    /// Parse a desired-schema YAML document into one `TableSchema` per type.
    pub fn from_yaml(doc: &str) -> Result<SchemaDoc> {
        let root: serde_yaml::Value = serde_yaml::from_str(doc)?;

        let types_seq = root
            .get("types")
            .and_then(|v| v.as_sequence())
            .ok_or_else(|| DoogatError::SqlEngine("desired schema missing `types:` sequence".into()))?;

        let mut types = Vec::with_capacity(types_seq.len());
        let mut renames = Vec::new();
        for entry in types_seq {
            let map = entry
                .as_mapping()
                .ok_or_else(|| DoogatError::SqlEngine("type entry must be a mapping".into()))?;

            let name = map
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| DoogatError::SqlEngine("type entry missing `name`".into()))?
                .to_string();

            // Trust-boundary check: the type name becomes `CREATE TABLE {name}`
            // verbatim in PlanOp::render_sql, so reject any name that is not a
            // safe SQL identifier before it can smuggle DDL.
            validate_identifier("table name", &name)?;

            // Capture and strip per-column `rename_from:` directives, then hand
            // the cleaned columns to schema_from_parsed via `extra`.
            let columns = strip_renames(map.get("columns"), &name, &mut renames);

            let extra: BTreeMap<String, Value> = map
                .iter()
                .filter_map(|(k, v)| {
                    let key = k.as_str()?;
                    if key == "name" {
                        return None;
                    }
                    if key == "columns" {
                        return Some(("columns".to_string(), from_serde_yaml(columns.clone())));
                    }
                    Some((key.to_string(), from_serde_yaml(v.clone())))
                })
                .collect();

            let meta = DoogatMeta {
                title: Some(name),
                extra,
                ..Default::default()
            };
            let parsed = ParsedDoogat {
                meta,
                body: String::new(),
                sections: Vec::new(),
                reference_section: String::new(),
                inline_fields: Vec::new(),
                links: Vec::new(),
                body_tags: Vec::new(),
                checkboxes: Vec::new(),
                path: String::new(),
                updated_at: None,
            };
            let schema = schema_from_parsed(&parsed)?;
            validate_schema(&schema)?;
            types.push(schema);
        }

        // Rename directives render into `ALTER TABLE {table} RENAME COLUMN
        // {from} TO {to}` verbatim. `table`/`to` are already validated above
        // (table name + the column's own `name`), but `from` (the old column
        // name) comes straight from the directive value, so guard it here.
        for r in &renames {
            validate_identifier("rename_from", &r.from)?;
            validate_identifier("rename target", &r.to)?;
        }

        Ok(SchemaDoc { types, renames })
    }
}

/// A safe SQL identifier for the declarative-schema trust boundary:
/// starts with an ASCII letter or `_`, followed by ASCII letters, digits,
/// `_`, or `-`. This mirrors the engine's own `is_safe_sql_identifier`
/// (`sql_engine::helpers`) so a name accepted here is exactly one the engine
/// already treats as injection-safe; hyphens are allowed because typedef
/// names like `meeting-minutes` are legitimate. The helper is `pub(super)`
/// and unreachable from this module, so the rule is re-stated here rather
/// than relaxing its visibility in a file this task may not touch.
fn validate_identifier(role: &str, ident: &str) -> Result<()> {
    let mut chars = ident.chars();
    let ok = match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(DoogatError::SqlEngine(format!(
            "invalid {role}: {ident:?} is not a safe SQL identifier"
        )))
    }
}

/// Validate every interpolation site of an assembled `TableSchema` that
/// `PlanOp::render_sql` emits without escaping. The schema arrives as untrusted
/// client input, and the `allow_destructive` gate keys off PlanOp *variants*,
/// not the rendered SQL text, so any unescaped value here can smuggle DDL past
/// the gate. The sites covered:
///
/// - each column's `name` (`render_column`: `{name} {type}` in CreateType /
///   AddColumn);
/// - each column's `references` target (`render_column`: `REFERENCES {target}`);
/// - each column's raw `data_type` (`render_column`'s type token, and the
///   `AlterColumnType.new_type` the differ copies from a desired column's
///   `data_type`) — see [`validate_data_type`];
/// - each column's `default_value` (`render_column`: `DEFAULT {default}`) — see
///   [`validate_default_value`];
/// - every `unique_together` column name (CreateType: `UNIQUE({cols.join})`);
/// - the type's `search_key`, when set (the differ's `SetSearchKey`:
///   `SET SEARCH KEY {col}`).
///
/// Sites NOT checked here because they are inherently safe or guarded
/// elsewhere: the table name (validated in [`SchemaDoc::from_yaml`] before
/// assembly), rename `from`/`to` (validated there too), ENUM allowed-values
/// (single-quote-escaped in `render_column`), the closed zone-token enum, and
/// `DropColumn` whose column comes from the live schema, not this doc.
fn validate_schema(schema: &TableSchema) -> Result<()> {
    for col in &schema.columns {
        validate_identifier("column name", &col.name)?;
        if let Some(target) = &col.references {
            validate_identifier("references target", target)?;
        }
        validate_data_type(&col.data_type)?;
        if let Some(default) = &col.default_value {
            validate_default_value(default)?;
        }
    }
    if let Some(constraints) = &schema.unique_together {
        for cols in constraints {
            for col in cols {
                validate_identifier("unique_together column", col)?;
            }
        }
    }
    if let Some(key) = &schema.search_key {
        validate_identifier("search key", key)?;
    }
    Ok(())
}

/// Validate a raw `data_type` string against the engine's own type grammar.
///
/// The string is rendered verbatim into DDL by `PlanOp::render_sql`, so it
/// must be a single, well-formed column type and nothing more. Rather than
/// inventing a character allow/deny list (which cannot distinguish
/// `VARCHAR(255)` / `ENUM('a','b')` from smuggled SQL), this parses
/// `CREATE TABLE _v ("c" <data_type>)` with the same `GenericDialect` the
/// apply path uses and accepts only when the parse yields exactly one
/// `CREATE TABLE` statement with exactly one column and no extra constraints.
/// A `data_type` carrying a statement terminator, an extra column, or any
/// trailing tokens therefore fails to round-trip and is rejected.
fn validate_data_type(data_type: &str) -> Result<()> {
    let probe = format!(r#"CREATE TABLE _v ("c" {data_type})"#);
    let reject = || {
        DoogatError::SqlEngine(format!(
            "invalid data_type: {data_type:?} is not a single well-formed column type"
        ))
    };
    let statements = Parser::parse_sql(&GenericDialect {}, &probe).map_err(|_| reject())?;
    let [Statement::CreateTable(ct)] = statements.as_slice() else {
        return Err(reject());
    };
    if ct.columns.len() != 1 || !ct.constraints.is_empty() {
        return Err(reject());
    }
    if !ct.columns[0].options.is_empty() {
        return Err(reject());
    }
    Ok(())
}

/// Validate a raw `default_value` string against the engine's own default-
/// expression grammar.
///
/// The string is rendered verbatim into DDL by `render_column` as
/// ` DEFAULT {default_value}`, so it must be a single, well-formed default
/// expression and nothing more. Mirroring [`validate_data_type`], this parses
/// `CREATE TABLE _v ("c" TEXT DEFAULT {default_value})` with the same
/// `GenericDialect` the apply path uses and accepts only when the parse yields
/// exactly one `CREATE TABLE` statement with exactly one column and no extra
/// constraints. A `default_value` carrying a statement terminator, an extra
/// column, or any trailing tokens therefore fails to round-trip and is
/// rejected, while legitimate defaults (numbers, single-quoted strings,
/// `CURRENT_TIMESTAMP`, `TRUE`/`FALSE`) parse cleanly.
fn validate_default_value(default_value: &str) -> Result<()> {
    let probe = format!(r#"CREATE TABLE _v ("c" TEXT DEFAULT {default_value})"#);
    let reject = || {
        DoogatError::SqlEngine(format!(
            "invalid default_value: {default_value:?} is not a single well-formed default expression"
        ))
    };
    let statements = Parser::parse_sql(&GenericDialect {}, &probe).map_err(|_| reject())?;
    let [Statement::CreateTable(ct)] = statements.as_slice() else {
        return Err(reject());
    };
    if ct.columns.len() != 1 || !ct.constraints.is_empty() {
        return Err(reject());
    }
    let [option] = ct.columns[0].options.as_slice() else {
        return Err(reject());
    };
    if !matches!(option.option, ColumnOption::Default(_)) {
        return Err(reject());
    }
    Ok(())
}

/// Walk a type's `columns` sequence structurally. For each column mapping that
/// carries a `rename_from:` key, push a [`ColumnRename`] (tagged with `table`,
/// `from` = the directive value, `to` = the column's own `name:`) and return a
/// copy of the columns with the `rename_from` key removed so it never reaches
/// the column assembler. Non-sequence/absent `columns` are passed through
/// unchanged for the existing assembler to validate.
fn strip_renames(
    columns: Option<&serde_yaml::Value>,
    table: &str,
    renames: &mut Vec<ColumnRename>,
) -> serde_yaml::Value {
    let Some(seq) = columns.and_then(|c| c.as_sequence()) else {
        return columns.cloned().unwrap_or(serde_yaml::Value::Null);
    };

    let rename_key = serde_yaml::Value::String("rename_from".to_string());
    let cleaned: Vec<serde_yaml::Value> = seq
        .iter()
        .map(|col| {
            let Some(map) = col.as_mapping() else {
                return col.clone();
            };
            let Some(from) = map.get(&rename_key).and_then(|v| v.as_str()) else {
                return col.clone();
            };
            let to = map
                .get(serde_yaml::Value::String("name".to_string()))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            renames.push(ColumnRename {
                table: table.to_string(),
                from: from.to_string(),
                to: to.to_string(),
            });
            let mut cleaned = map.clone();
            cleaned.remove(&rename_key);
            serde_yaml::Value::Mapping(cleaned)
        })
        .collect();
    serde_yaml::Value::Sequence(cleaned)
}

#[cfg(test)]
mod tests {
    use crate::schema_diff::desired::SchemaDoc;
    use crate::types::Zone;

    /// A multi-type desired doc parses into one TableSchema per declared
    /// `name:`, preserving each column's name/data_type and the type order.
    /// A parser that drops field data or collapses types would fail the
    /// per-column assertions, not just the count.
    #[test]
    fn parses_multiple_types() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: VARCHAR(50)
      - name: owner
        data_type: VARCHAR(100)
        references: contact
  - name: contact
    columns:
      - name: email
        data_type: VARCHAR(255)
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("valid multi-type doc parses");
        assert_eq!(doc.types.len(), 2);

        let project = &doc.types[0];
        assert_eq!(project.table_name, "project");
        assert_eq!(project.columns.len(), 2);
        assert_eq!(project.columns[0].name, "status");
        assert_eq!(project.columns[0].data_type, "VARCHAR(50)");
        assert_eq!(project.columns[0].references, None);
        assert_eq!(project.columns[1].name, "owner");
        assert_eq!(project.columns[1].data_type, "VARCHAR(100)");
        assert_eq!(project.columns[1].references, Some("contact".to_string()));

        let contact = &doc.types[1];
        assert_eq!(contact.table_name, "contact");
        assert_eq!(contact.columns.len(), 1);
        assert_eq!(contact.columns[0].name, "email");
        assert_eq!(contact.columns[0].data_type, "VARCHAR(255)");
    }

    /// `references: <target>` round-trips onto ColumnDef.references and,
    /// per effective_zone() inference, a REFERENCES column with no explicit
    /// zone resolves to Zone::Reference. Asserting the resolved zone guards
    /// against a parser that records the reference string but loses it from
    /// the column's zone semantics.
    #[test]
    fn reference_column_round_trips() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: owner
        data_type: VARCHAR(100)
        references: contact
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("reference column parses");
        let col = &doc.types[0].columns[0];
        assert_eq!(col.references, Some("contact".to_string()));
        assert_eq!(col.zone, None);
        assert_eq!(col.effective_zone(), Zone::Reference);
    }

    /// An explicit `zone:` value is parsed onto ColumnDef.zone verbatim and
    /// wins over type-based inference. `status: VARCHAR(50)` would infer
    /// Frontmatter; declaring `zone: body` must override that, proving the
    /// explicit zone is read, not ignored.
    #[test]
    fn explicit_zone_round_trips() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: VARCHAR(50)
        zone: body
      - name: note
        data_type: TEXT
        zone: frontmatter
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("explicit zones parse");
        let cols = &doc.types[0].columns;
        assert_eq!(cols[0].zone, Some(Zone::Body));
        assert_eq!(cols[0].effective_zone(), Zone::Body);
        assert_eq!(cols[1].zone, Some(Zone::Frontmatter));
        assert_eq!(cols[1].effective_zone(), Zone::Frontmatter);
    }

    /// `required: true` is parsed onto ColumnDef.required; a column without
    /// the key defaults to false. Asserting both values rules out a parser
    /// that hardcodes the flag either way.
    #[test]
    fn required_flag_round_trips() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: VARCHAR(50)
        required: true
      - name: owner
        data_type: VARCHAR(100)
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("required flag parses");
        let cols = &doc.types[0].columns;
        assert!(cols[0].required);
        assert!(!cols[1].required);
    }

    /// Type-level optional fields (search_key, singleton, unique_together)
    /// are assembled onto TableSchema. Asserting their concrete values
    /// catches a parser that handles columns but drops type-level metadata.
    #[test]
    fn type_level_optional_fields_round_trip() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: VARCHAR(50)
    search_key: status
    singleton: true
    unique_together: [["status"]]
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("type-level fields parse");
        let project = &doc.types[0];
        assert_eq!(project.search_key, Some("status".to_string()));
        assert!(project.singleton);
        assert_eq!(
            project.unique_together,
            Some(vec![vec!["status".to_string()]])
        );
    }

    /// An empty `types:` sequence is valid and yields zero types (algorithm
    /// step 2: a present sequence is read; an empty one produces no entries).
    #[test]
    fn empty_types_list_yields_zero_types() {
        let yaml = "types: []\n";
        let doc = SchemaDoc::from_yaml(yaml).expect("empty types list is valid");
        assert!(doc.types.is_empty());
    }

    /// Several distinct malformed YAML inputs must each surface an error
    /// rather than silently yielding an empty or partial doc (algorithm
    /// step 1). A fake parser cannot special-case every encoding-level
    /// defect: an unclosed quote, a tab-indented line, a stray document
    /// directive, and an unterminated flow sequence all require a real
    /// YAML lexer to reject.
    #[test]
    fn rejects_various_malformed_inputs() {
        let malformed = [
            // Unterminated flow sequence.
            "types:\n  - name: project\n    columns: [unterminated",
            // Unclosed double-quote on a scalar.
            "types:\n  - name: \"project\n    columns: []",
            // Tab character used for indentation (illegal in YAML).
            "types:\n\t- name: project",
            // Stray/duplicate YAML directive without a document.
            "%YAML 1.2\n%YAML 1.1\ntypes: []",
        ];
        for (i, yaml) in malformed.iter().enumerate() {
            assert!(
                SchemaDoc::from_yaml(yaml).is_err(),
                "malformed input #{i} must be rejected: {yaml:?}"
            );
        }
    }

    /// A type entry without a `name:` key is an error: every declared type
    /// must name itself so it can become TableSchema.table_name
    /// (algorithm step 3).
    #[test]
    fn rejects_type_missing_name() {
        let yaml = r#"
types:
  - columns:
      - name: status
        data_type: VARCHAR(50)
"#;
        assert!(SchemaDoc::from_yaml(yaml).is_err());
    }

    /// A column missing its required `data_type:` key is an error, matching
    /// schema_from_parsed's column-assembly contract (algorithm step 3).
    #[test]
    fn rejects_column_missing_data_type() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
"#;
        assert!(SchemaDoc::from_yaml(yaml).is_err());
    }

    /// A top-level document with no `types:` key is an error (algorithm
    /// step 2: absent sequence => Err).
    #[test]
    fn rejects_missing_types_key() {
        let yaml = "name: project\n";
        assert!(SchemaDoc::from_yaml(yaml).is_err());
    }

    // --- Identifier / data_type injection guards (PRD 00161 cycle 1 rework) ---
    //
    // `from_yaml` is the trust boundary for the schema doc, which arrives as
    // UNTRUSTED client input via `applySchema` (GraphQL) and `POST
    // /schema/apply` (REST). Names and `data_type` strings flow verbatim into
    // `PlanOp::render_sql`'s raw `format!` DDL with no escaping, and the
    // `allow_destructive` gate keys off PlanOp VARIANTS, not the rendered SQL
    // text. So a benign-looking AddColumn whose name embeds `DROP TABLE`
    // smuggles destructive DDL past the gate. These tests pin that `from_yaml`
    // rejects such inputs at the boundary while still accepting every
    // legitimate identifier and type the engine supports.

    /// A column name that is not a safe SQL identifier (here it embeds `)`,
    /// `;`, whitespace, and `--`) must be rejected: rendered verbatim it would
    /// close the column list early and smuggle a `DROP TABLE`.
    #[test]
    fn rejects_column_name_with_injection_payload() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: "x); DROP TABLE users; --"
        data_type: TEXT
"#;
        assert!(
            SchemaDoc::from_yaml(yaml).is_err(),
            "an injection payload column name must be rejected at the trust boundary"
        );
    }

    /// A table name that is not a safe identifier is rejected. The name becomes
    /// `CREATE TABLE {name} (...)` verbatim, so a space/paren/semicolon in it
    /// smuggles arbitrary DDL.
    #[test]
    fn rejects_table_name_not_an_identifier() {
        let yaml = r#"
types:
  - name: "project; DROP TABLE users; --"
    columns:
      - name: status
        data_type: TEXT
"#;
        assert!(
            SchemaDoc::from_yaml(yaml).is_err(),
            "a non-identifier table name must be rejected"
        );
    }

    /// A `rename_from:` value that is not a safe identifier is rejected. It is
    /// rendered into `ALTER TABLE {table} RENAME COLUMN {from} TO {to}`
    /// verbatim, so an injection payload in `from` smuggles DDL.
    #[test]
    fn rejects_rename_from_not_an_identifier() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: owner
        data_type: VARCHAR(100)
        rename_from: "assignee); DROP TABLE users; --"
"#;
        assert!(
            SchemaDoc::from_yaml(yaml).is_err(),
            "a non-identifier rename_from value must be rejected"
        );
    }

    /// A `data_type` that smuggles statement-terminating SQL is rejected. The
    /// raw string is rendered verbatim into the column DDL, so
    /// `TEXT); DROP TABLE users; --` would close the column list and inject a
    /// destructive statement that the variant-keyed gate never flags.
    #[test]
    fn rejects_data_type_smuggling_sql() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: "TEXT); DROP TABLE users; --"
"#;
        assert!(
            SchemaDoc::from_yaml(yaml).is_err(),
            "a data_type that smuggles SQL must be rejected"
        );
    }

    /// A `data_type` that parses as two columns (`TEXT, evil TEXT`) is rejected:
    /// rendered verbatim it would add an undeclared column. Only a single,
    /// well-formed type token is accepted.
    #[test]
    fn rejects_data_type_with_extra_column() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: "TEXT, evil TEXT"
"#;
        assert!(
            SchemaDoc::from_yaml(yaml).is_err(),
            "a data_type that introduces an extra column must be rejected"
        );
    }

    /// A `data_type` carrying any extra column option is rejected. A
    /// `data_type` must be a bare, well-formed type and NOTHING more; rendered
    /// verbatim into DDL, a trailing `REFERENCES`/`NOT NULL`/`UNIQUE`/`CHECK`
    /// attaches a column-level constraint the structured schema fields never
    /// declare, smuggling it past the variant-keyed destructive-operation gate.
    #[test]
    fn rejects_data_type_with_extra_column_options() {
        let references = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: "INTEGER REFERENCES contact"
"#;
        assert!(
            SchemaDoc::from_yaml(references).is_err(),
            "a data_type smuggling a REFERENCES foreign key must be rejected"
        );

        let not_null = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: "TEXT NOT NULL"
"#;
        assert!(
            SchemaDoc::from_yaml(not_null).is_err(),
            "a data_type smuggling a NOT NULL constraint must be rejected"
        );

        let unique = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: "TEXT UNIQUE"
"#;
        assert!(
            SchemaDoc::from_yaml(unique).is_err(),
            "a data_type smuggling a UNIQUE constraint must be rejected"
        );

        let check = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: "INTEGER CHECK(priority > 0)"
"#;
        assert!(
            SchemaDoc::from_yaml(check).is_err(),
            "a data_type smuggling a CHECK constraint must be rejected"
        );
    }

    /// A `default_value` that smuggles statement-terminating SQL is rejected.
    /// It is rendered verbatim by `render_column` as ` DEFAULT {default}`, so
    /// `0); DROP TABLE users; --` would close the column list and inject a
    /// destructive statement while the plan still reports a non-destructive
    /// CreateType/AddColumn that the variant-keyed gate never flags.
    #[test]
    fn rejects_default_value_smuggling_sql() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: INTEGER
        default_value: "0); DROP TABLE users; --"
"#;
        assert!(
            SchemaDoc::from_yaml(yaml).is_err(),
            "a default_value that smuggles SQL must be rejected at the trust boundary"
        );
    }

    /// A `default_value` that introduces an extra column (`0, evil TEXT`) is
    /// rejected: rendered verbatim it would add an undeclared column. Only a
    /// single, well-formed default expression is accepted.
    #[test]
    fn rejects_default_value_with_extra_column() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: INTEGER
        default_value: "0, evil TEXT"
"#;
        assert!(
            SchemaDoc::from_yaml(yaml).is_err(),
            "a default_value that introduces an extra column must be rejected"
        );
    }

    /// A `default_value` carrying any extra column option beyond the default
    /// itself is rejected. Rendered verbatim as ` DEFAULT {default}`, a
    /// trailing `NOT NULL`/`REFERENCES ... ON DELETE CASCADE`/`UNIQUE`/`CHECK`
    /// attaches a column-level constraint the structured schema fields never
    /// declare, smuggling it past the variant-keyed destructive-operation gate.
    #[test]
    fn rejects_default_value_with_extra_column_options() {
        let not_null = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: INTEGER
        default_value: "0 NOT NULL"
"#;
        assert!(
            SchemaDoc::from_yaml(not_null).is_err(),
            "a default_value smuggling a NOT NULL constraint must be rejected"
        );

        let references = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: INTEGER
        default_value: "0 REFERENCES contact ON DELETE CASCADE"
"#;
        assert!(
            SchemaDoc::from_yaml(references).is_err(),
            "a default_value smuggling a REFERENCES ... ON DELETE CASCADE must be rejected"
        );

        let unique = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: INTEGER
        default_value: "0 UNIQUE"
"#;
        assert!(
            SchemaDoc::from_yaml(unique).is_err(),
            "a default_value smuggling a UNIQUE constraint must be rejected"
        );

        let check = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: INTEGER
        default_value: "0 CHECK(priority > 0)"
"#;
        assert!(
            SchemaDoc::from_yaml(check).is_err(),
            "a default_value smuggling a CHECK constraint must be rejected"
        );
    }

    /// A `unique_together` column name that is not a safe identifier is
    /// rejected. Each name is rendered verbatim inside `UNIQUE({cols.join})`,
    /// so an injection payload there would close the constraint list early and
    /// smuggle DDL past the variant-keyed gate.
    #[test]
    fn rejects_unique_together_column_smuggling_sql() {
        let yaml = r#"
types:
  - name: membership
    columns:
      - name: status
        data_type: TEXT
      - name: owner
        data_type: TEXT
    unique_together: [["status", "owner); DROP TABLE users; --"]]
"#;
        assert!(
            SchemaDoc::from_yaml(yaml).is_err(),
            "a unique_together column name that smuggles SQL must be rejected"
        );
    }

    /// A `search_key` that is not a safe identifier is rejected. It is rendered
    /// verbatim into `ALTER TABLE {table} SET SEARCH KEY {col}` by the
    /// SetSearchKey op the differ emits from the desired type's `search_key`,
    /// so an injection payload there smuggles DDL.
    #[test]
    fn rejects_search_key_smuggling_sql() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: TEXT
    search_key: "status); DROP TABLE users; --"
"#;
        assert!(
            SchemaDoc::from_yaml(yaml).is_err(),
            "a search_key that smuggles SQL must be rejected"
        );
    }

    /// The accept side of the default_value guard: legitimate defaults the
    /// engine supports must keep parsing. Covers a single-quoted string
    /// literal, an integer, the CURRENT_TIMESTAMP keyword, and a boolean
    /// literal. If validation rejects any of these it is too strict.
    #[test]
    fn accepts_legitimate_default_values() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: VARCHAR(50)
        default_value: "'active'"
      - name: priority
        data_type: INTEGER
        default_value: "0"
      - name: created
        data_type: TIMESTAMP
        default_value: CURRENT_TIMESTAMP
      - name: active
        data_type: BOOLEAN
        default_value: "TRUE"
"#;
        let doc =
            SchemaDoc::from_yaml(yaml).expect("legitimate default values must keep parsing");
        let cols = &doc.types[0].columns;
        assert_eq!(cols[0].default_value, Some("'active'".to_string()));
        assert_eq!(cols[1].default_value, Some("0".to_string()));
        assert_eq!(cols[2].default_value, Some("CURRENT_TIMESTAMP".to_string()));
        assert_eq!(cols[3].default_value, Some("TRUE".to_string()));
    }

    /// The decisive accept case against a substring blocklist: a `default_value`
    /// whose string-literal CONTENT contains a normally-blocked token is a
    /// single, valid default expression — the tokens are just characters inside
    /// the quoted string, not column options. A real SQL parser accepts these; a
    /// dumb substring blocklist on `NOT NULL`/`DROP`/`;`/`--`/`, evil`/
    /// `REFERENCES` would wrongly reject them. Each must be accepted.
    #[test]
    fn accepts_default_values_with_blocked_tokens_as_string_data() {
        let not_null = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: TEXT
        default_value: "'NOT NULL'"
"#;
        assert!(
            SchemaDoc::from_yaml(not_null).is_ok(),
            "a string-literal default whose content contains NOT NULL is valid and must be accepted"
        );

        let drop = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: TEXT
        default_value: "'DROP it'"
"#;
        assert!(
            SchemaDoc::from_yaml(drop).is_ok(),
            "a string-literal default whose content contains DROP is valid and must be accepted"
        );

        let terminators = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: TEXT
        default_value: "'a; b -- c'"
"#;
        assert!(
            SchemaDoc::from_yaml(terminators).is_ok(),
            "a string-literal default whose content contains ; and -- is valid and must be accepted"
        );

        let comma_evil = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: TEXT
        default_value: "'0, evil'"
"#;
        assert!(
            SchemaDoc::from_yaml(comma_evil).is_ok(),
            "a string-literal default whose content contains , evil is valid and must be accepted"
        );

        let references = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: TEXT
        default_value: "'REFERENCES other'"
"#;
        assert!(
            SchemaDoc::from_yaml(references).is_ok(),
            "a string-literal default whose content contains REFERENCES is valid and must be accepted"
        );
    }

    /// The accept side of the data_type extra-option guard: legitimate bare
    /// types must keep parsing. The new rule that rejects trailing column
    /// options must NOT break parametrized or multi-arg types — `VARCHAR(255)`
    /// (parens) and `ENUM('a','b')` (parens, quotes, comma) are single
    /// well-formed types, not extra column options.
    #[test]
    fn accepts_legitimate_data_types() {
        let text = r#"
types:
  - name: project
    columns:
      - name: notes
        data_type: TEXT
"#;
        assert!(
            SchemaDoc::from_yaml(text).is_ok(),
            "a bare TEXT type must keep parsing"
        );

        let integer = r#"
types:
  - name: project
    columns:
      - name: priority
        data_type: INTEGER
"#;
        assert!(
            SchemaDoc::from_yaml(integer).is_ok(),
            "a bare INTEGER type must keep parsing"
        );

        let varchar = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: VARCHAR(255)
"#;
        assert!(
            SchemaDoc::from_yaml(varchar).is_ok(),
            "a parametrized VARCHAR(255) type must keep parsing"
        );

        let enum_type = r#"
types:
  - name: project
    columns:
      - name: stage
        data_type: "ENUM('a','b')"
"#;
        assert!(
            SchemaDoc::from_yaml(enum_type).is_ok(),
            "a multi-arg ENUM('a','b') type must keep parsing"
        );
    }

    /// The decisive accept case against a substring blocklist on the data_type
    /// guard: an ENUM type whose ALLOWED VALUES are normally-blocked tokens is a
    /// single, valid type with no column options — the keywords are enum members
    /// (quoted string literals), not constraints. A real SQL parser accepts
    /// these; a substring blocklist on `NOT NULL`/`REFERENCES`/`, evil` would
    /// wrongly reject them. Each must be accepted.
    #[test]
    fn accepts_data_types_with_blocked_tokens_as_enum_values() {
        let keyword_members = r#"
types:
  - name: project
    columns:
      - name: stage
        data_type: "ENUM('NOT NULL', 'REFERENCES')"
"#;
        assert!(
            SchemaDoc::from_yaml(keyword_members).is_ok(),
            "an ENUM whose members are the keywords NOT NULL and REFERENCES is a valid type and must be accepted"
        );

        let comma_member = r#"
types:
  - name: project
    columns:
      - name: stage
        data_type: "ENUM('a, evil', 'b')"
"#;
        assert!(
            SchemaDoc::from_yaml(comma_member).is_ok(),
            "an ENUM whose member contains , evil is a valid type and must be accepted"
        );
    }

    /// The accept side of the unique_together guard: a constraint over two
    /// normal identifiers must keep parsing and round-trip onto the schema.
    #[test]
    fn accepts_legitimate_unique_together() {
        let yaml = r#"
types:
  - name: membership
    columns:
      - name: status
        data_type: TEXT
      - name: owner
        data_type: TEXT
    unique_together: [[status, owner]]
"#;
        let doc =
            SchemaDoc::from_yaml(yaml).expect("legitimate unique_together must keep parsing");
        assert_eq!(
            doc.types[0].unique_together,
            Some(vec![vec!["status".to_string(), "owner".to_string()]])
        );
    }

    /// The accept side of the search_key guard: a normal identifier search_key
    /// must keep parsing and round-trip onto the schema.
    #[test]
    fn accepts_legitimate_search_key() {
        let yaml = r#"
types:
  - name: contact
    columns:
      - name: email
        data_type: VARCHAR(255)
    search_key: email
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("legitimate search_key must keep parsing");
        assert_eq!(doc.types[0].search_key, Some("email".to_string()));
    }

    /// The accept side of the guard: every legitimate identifier and type the
    /// engine supports must keep parsing. This is the over-restriction guard —
    /// if validation rejects any of these, it is too strict. Covers VARCHAR(N),
    /// ENUM('a','b') (parens + quotes + comma), INTEGER, BOOLEAN, TEXT, plus an
    /// underscore-leading and a hyphenated identifier (the engine's
    /// `is_safe_sql_identifier` permits hyphens).
    #[test]
    fn accepts_legitimate_identifiers_and_types() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: VARCHAR(255)
      - name: stage
        data_type: ENUM('open', 'closed')
      - name: priority
        data_type: INTEGER
      - name: active
        data_type: BOOLEAN
      - name: notes
        data_type: TEXT
      - name: _internal
        data_type: TEXT
      - name: meeting-minutes
        data_type: TEXT
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("all legitimate inputs must keep parsing");
        assert_eq!(doc.types.len(), 1);
        let cols = &doc.types[0].columns;
        assert_eq!(cols.len(), 7);
        assert_eq!(cols[0].data_type, "VARCHAR(255)");
        assert_eq!(cols[3].data_type, "BOOLEAN");
        assert_eq!(cols[5].name, "_internal");
        assert_eq!(cols[6].name, "meeting-minutes");
    }

    /// A legitimate `rename_from` with a normal identifier is accepted and
    /// still captured as a ColumnRename — the guard must not break the
    /// happy path.
    #[test]
    fn accepts_legitimate_rename_from() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: owner
        data_type: VARCHAR(100)
        rename_from: assignee
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("a normal rename_from must keep parsing");
        assert_eq!(doc.renames.len(), 1);
        assert_eq!(doc.renames[0].from, "assignee");
        assert_eq!(doc.renames[0].to, "owner");
    }

    /// A fully VALID desired doc whose field values contain substrings a
    /// naive heuristic might mistake for error markers or branch keys
    /// (`unterminated` from the malformed-YAML test, `contact` from the
    /// multi-type test) must still parse and read its fields correctly.
    /// An impl that pattern-matches on those fingerprint substrings instead
    /// of parsing would misclassify this valid input and fail. The marker
    /// lives inside a single-quoted SQL string literal so it is also a
    /// well-formed `default_value` under the trust-boundary guard.
    #[test]
    fn valid_doc_with_error_marker_substring_parses() {
        let yaml = r#"
types:
  - name: contact
    columns:
      - name: unterminated
        data_type: TEXT
        default_value: "'[unterminated'"
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("valid doc with marker substrings parses");
        assert_eq!(doc.types.len(), 1);
        let contact = &doc.types[0];
        assert_eq!(contact.table_name, "contact");
        assert_eq!(contact.columns.len(), 1);
        assert_eq!(contact.columns[0].name, "unterminated");
        assert_eq!(contact.columns[0].data_type, "TEXT");
        assert_eq!(
            contact.columns[0].default_value,
            Some("'[unterminated'".to_string())
        );
    }

    /// A real YAML parser is ENCODING-INVARIANT: the same logical document
    /// written in block YAML and in flow/JSON-style YAML must parse to the
    /// identical structure. A line scanner that keys off exact indentation
    /// depth and prefix-matches `key:` cannot read the flow form at all, so
    /// the two parses would diverge (or the flow parse would fail outright).
    /// This is the decisive guard against a structural string scanner
    /// masquerading as a YAML parser.
    #[test]
    fn parses_flow_and_block_forms_equivalently() {
        let block_yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: TEXT
        zone: body
      - name: owner
        data_type: "VARCHAR(100)"
        references: contact
        required: true
    search_key: status
    singleton: true
    unique_together: [[status]]
  - name: contact
    columns:
      - name: email
        data_type: "VARCHAR(255)"
"#;
        let flow_yaml = r#"{types: [{name: project, columns: [{name: status, data_type: TEXT, zone: body}, {name: owner, data_type: "VARCHAR(100)", references: contact, required: true}], search_key: status, singleton: true, unique_together: [[status]]}, {name: contact, columns: [{name: email, data_type: "VARCHAR(255)"}]}]}"#;

        let block = SchemaDoc::from_yaml(block_yaml).expect("block-form YAML parses");
        let flow = SchemaDoc::from_yaml(flow_yaml).expect("flow-form YAML parses");

        // TableSchema/ColumnDef do not derive PartialEq, so compare Debug
        // representations: a real parser yields byte-identical structure for
        // both encodings.
        assert_eq!(
            format!("{:?}", block.types),
            format!("{:?}", flow.types),
            "block and flow YAML must parse to identical structure"
        );

        // Also pin at least one concrete value so the test still fails loudly
        // if BOTH forms parse wrong in the same way (Debug-equal but wrong).
        assert_eq!(block.types.len(), 2);
        assert_eq!(block.types[0].table_name, "project");
        assert_eq!(
            block.types[0].columns[1].references,
            Some("contact".to_string())
        );
        assert_eq!(block.types[0].columns[0].zone, Some(Zone::Body));
        assert!(block.types[0].columns[1].required);
        assert_eq!(block.types[0].search_key, Some("status".to_string()));
        assert!(block.types[0].singleton);
        assert_eq!(
            block.types[0].unique_together,
            Some(vec![vec!["status".to_string()]])
        );
        assert_eq!(block.types[1].table_name, "contact");
        assert_eq!(block.types[1].columns[0].data_type, "VARCHAR(255)");
    }

    /// A column carrying `rename_from: <old>` records a ColumnRename tagging
    /// the type, the old name (`from`), and the column's own `name:` as the
    /// new name (`to`). This is the directive's whole point: the parser must
    /// surface the rename intent separately from the column data.
    #[test]
    fn rename_from_directive_is_captured_as_column_rename() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: owner
        data_type: VARCHAR(100)
        rename_from: assignee
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("doc with rename_from parses");
        assert_eq!(doc.renames.len(), 1, "exactly one rename captured");
        let r = &doc.renames[0];
        assert_eq!(r.table, "project");
        assert_eq!(r.from, "assignee");
        assert_eq!(r.to, "owner");
    }

    /// `rename_from` is a directive, not column data: it must be stripped
    /// before the column is assembled. The resulting ColumnDef carries the
    /// NEW name and its normal attributes, and nothing named `rename_from`
    /// leaks into the column (the existing column parser never sees it).
    #[test]
    fn rename_from_key_is_stripped_from_assembled_column() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: owner
        data_type: VARCHAR(100)
        rename_from: assignee
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("doc with rename_from parses");
        assert_eq!(doc.types.len(), 1);
        let col = &doc.types[0].columns[0];
        // The assembled column has the NEW name and normal attributes.
        assert_eq!(col.name, "owner");
        assert_eq!(col.data_type, "VARCHAR(100)");
        // `rename_from` does not survive as a phantom column or stray value:
        // the type has exactly the one declared column, named `owner`.
        assert_eq!(doc.types[0].columns.len(), 1);
        assert!(
            !doc.types[0]
                .columns
                .iter()
                .any(|c| c.name == "assignee" || c.name == "rename_from"),
            "rename_from must not leak into the column set"
        );
    }

    /// A doc with no `rename_from` anywhere yields an empty renames list.
    /// This pins the default so an impl can't fabricate phantom renames.
    #[test]
    fn doc_without_rename_from_yields_empty_renames() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: status
        data_type: VARCHAR(50)
  - name: contact
    columns:
      - name: email
        data_type: VARCHAR(255)
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("doc without rename_from parses");
        assert!(
            doc.renames.is_empty(),
            "no rename_from anywhere must yield zero renames"
        );
    }

    /// Multiple renames across multiple types are all captured, each tagged
    /// with the type it belongs to. A parser that hardcodes a single rename
    /// or drops the table tag fails here.
    #[test]
    fn multiple_renames_across_types_each_tagged_with_its_table() {
        let yaml = r#"
types:
  - name: project
    columns:
      - name: owner
        data_type: VARCHAR(100)
        rename_from: assignee
      - name: status
        data_type: VARCHAR(50)
  - name: contact
    columns:
      - name: full_name
        data_type: TEXT
        rename_from: name
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("multi-rename doc parses");
        assert_eq!(doc.renames.len(), 2, "both renames captured");

        let project = doc
            .renames
            .iter()
            .find(|r| r.table == "project")
            .expect("project rename present");
        assert_eq!(project.from, "assignee");
        assert_eq!(project.to, "owner");

        let contact = doc
            .renames
            .iter()
            .find(|r| r.table == "contact")
            .expect("contact rename present");
        assert_eq!(contact.from, "name");
        assert_eq!(contact.to, "full_name");
    }

    /// A `rename_from` directive written in FLOW/JSON-style one-line YAML must
    /// be captured identically to its block-form equivalent. A line scanner
    /// that tracks the last `name:`/`- name:` line and pairs it with a later
    /// `rename_from:` line produces ZERO renames here, because flow form has
    /// no line boundaries between keys. Only a real structural YAML parse
    /// recovers the directive, making this the decisive guard against the
    /// scanner cheat.
    #[test]
    fn rename_from_in_flow_form_is_captured() {
        let flow_yaml = r#"{types: [{name: project, columns: [{name: owner, data_type: "VARCHAR(100)", rename_from: assignee}]}]}"#;
        let doc = SchemaDoc::from_yaml(flow_yaml).expect("flow-form rename doc parses");
        assert_eq!(
            doc.renames.len(),
            1,
            "flow-form rename_from must yield exactly one rename"
        );
        let r = &doc.renames[0];
        assert_eq!(r.table, "project");
        assert_eq!(r.from, "assignee");
        assert_eq!(r.to, "owner");
        // The directive is still stripped from the assembled column.
        assert_eq!(doc.types[0].columns.len(), 1);
        assert_eq!(doc.types[0].columns[0].name, "owner");
    }

    /// YAML mappings are unordered: a real structural parse pairs `rename_from`
    /// with the column's `name:` regardless of which key text appears first.
    /// Here `rename_from:` is listed BEFORE `name:` inside the column mapping.
    /// A line scanner pairs `rename_from` with whatever `name` it last saw
    /// (the type's `name: project`, or nothing), corrupting `to`. The captured
    /// rename must still be `from`=old, `to`=the column's own name.
    #[test]
    fn rename_from_before_name_key_is_key_order_invariant() {
        let yaml = r#"
types:
  - name: project
    columns:
      - rename_from: assignee
        data_type: VARCHAR(100)
        name: owner
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("key-order-reversed rename doc parses");
        assert_eq!(doc.renames.len(), 1, "exactly one rename captured");
        let r = &doc.renames[0];
        assert_eq!(r.table, "project");
        assert_eq!(r.from, "assignee");
        assert_eq!(
            r.to, "owner",
            "`to` must be the column's own name, not the type name or empty"
        );
        // The assembled column carries the new name, not the type name.
        assert_eq!(doc.types[0].columns[0].name, "owner");
        assert_eq!(doc.types[0].columns[0].data_type, "VARCHAR(100)");
    }

    /// A column whose `rename_from` VALUE collides with a real structural key
    /// (`name`) is still handled structurally: the directive records the old
    /// name `name`, the column keeps its declared `name:` (`label`), and no
    /// phantom column leaks. A scanner that matches on the literal `name`
    /// token, or that confuses the value `name` with the `name:` key, would
    /// corrupt either the rename or the column. This stops the strip test from
    /// passing vacuously by forcing the value/key distinction.
    #[test]
    fn rename_from_value_colliding_with_key_name_is_handled_structurally() {
        let yaml = r#"
types:
  - name: contact
    columns:
      - name: label
        data_type: TEXT
        rename_from: name
"#;
        let doc = SchemaDoc::from_yaml(yaml).expect("collision doc parses");
        assert_eq!(doc.renames.len(), 1);
        let r = &doc.renames[0];
        assert_eq!(r.table, "contact");
        assert_eq!(r.from, "name", "old name is the literal value `name`");
        assert_eq!(r.to, "label");
        // Exactly one column, named `label`; nothing named `name` or
        // `rename_from` leaks in.
        assert_eq!(doc.types[0].columns.len(), 1);
        assert_eq!(doc.types[0].columns[0].name, "label");
        assert!(
            !doc.types[0]
                .columns
                .iter()
                .any(|c| c.name == "name" || c.name == "rename_from"),
            "neither the rename value nor the directive key may leak into columns"
        );
    }

    use proptest::prelude::*;

    /// One generated column's expected shape: (new_name, data_type,
    /// optional REFERENCES target, optional `rename_from` old name).
    type ExpectedCol = (String, String, Option<String>, Option<String>);
    /// One generated type's expected shape: (type name, its columns).
    type ExpectedType = (String, Vec<ExpectedCol>);

    proptest! {
        /// Generated (non-enumerable) YAML defeats substring-fingerprinting:
        /// the parser must read back every generated type name, column name,
        /// and data_type. A finite if/else cascade keyed on fixed literals
        /// cannot satisfy randomized inputs. Emitting FLOW-form YAML (one
        /// line, no fixed indentation) additionally defeats a line scanner
        /// that keys off indentation depth. Some generated columns also carry
        /// a randomized `rename_from: <old name>`, and the test asserts the
        /// exact set of resulting ColumnRenames (table, from=old, to=new) plus
        /// that each assembled column keeps its NEW name. A line scanner that
        /// pairs `name:`/`rename_from:` lines yields nothing from flow form,
        /// so the rename assertions cannot pass under the cheat.
        #[test]
        fn parses_arbitrary_types_and_columns(
            specs in prop::collection::vec(
                (
                    "[a-z][a-z0-9_]{0,11}",
                    prop::collection::vec(
                        (
                            "[a-z][a-z0-9_]{0,11}",
                            prop::sample::select(vec!["TEXT", "INTEGER", "BOOLEAN", "VARCHAR(64)"]),
                            // Optional REFERENCES target on some columns.
                            prop::option::of("[a-z][a-z0-9_]{0,11}"),
                            // Optional `rename_from: <old name>` on some columns.
                            prop::option::of("[a-z][a-z0-9_]{0,11}"),
                        ),
                        1..=4,
                    ),
                ),
                1..=3,
            )
        ) {
            // Build the expected document, deduping column names within a
            // type by prefixing the column index so assertions are unambiguous.
            // Each column carries (new_name, data_type, refs, rename_from).
            let expected: Vec<ExpectedType> = specs
                .iter()
                .map(|(ty_name, cols)| {
                    let cols = cols
                        .iter()
                        .enumerate()
                        .map(|(i, (col_name, dt, refs, rename))| {
                            (
                                format!("c{i}_{col_name}"),
                                dt.to_string(),
                                refs.clone(),
                                rename.as_ref().map(|old| format!("old{i}_{old}")),
                            )
                        })
                        .collect();
                    (ty_name.clone(), cols)
                })
                .collect();

            // Construct the document in FLOW / JSON-style YAML on one line:
            // no fixed indentation for a line scanner to key off.
            let types_joined = expected
                .iter()
                .map(|(ty_name, cols)| {
                    let cols_joined = cols
                        .iter()
                        .map(|(col_name, dt, refs, rename)| {
                            let mut parts =
                                vec![format!("name: {col_name}"), format!("data_type: \"{dt}\"")];
                            if let Some(r) = refs {
                                parts.push(format!("references: {r}"));
                            }
                            if let Some(old) = rename {
                                parts.push(format!("rename_from: {old}"));
                            }
                            format!("{{{}}}", parts.join(", "))
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{{name: {ty_name}, columns: [{cols_joined}]}}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            let yaml = format!("{{types: [{types_joined}]}}");

            let doc = SchemaDoc::from_yaml(&yaml)
                .expect("generated valid doc parses");

            prop_assert_eq!(doc.types.len(), expected.len());
            for (table, (exp_name, exp_cols)) in doc.types.iter().zip(expected.iter()) {
                prop_assert_eq!(&table.table_name, exp_name);
                prop_assert_eq!(table.columns.len(), exp_cols.len());
                for (col, (exp_col_name, exp_dt, exp_refs, _exp_rename)) in
                    table.columns.iter().zip(exp_cols.iter())
                {
                    // The assembled column carries the NEW name (`to`),
                    // unaffected by any rename_from directive on it.
                    prop_assert_eq!(&col.name, exp_col_name);
                    prop_assert_eq!(&col.data_type, exp_dt);
                    prop_assert_eq!(&col.references, exp_refs);
                }
            }

            // Compute the exact set of renames the document declares and
            // assert the parser surfaced precisely that set. Sorting both
            // sides makes the comparison order-independent.
            let mut expected_renames: Vec<(String, String, String)> = expected
                .iter()
                .flat_map(|(ty_name, cols)| {
                    cols.iter().filter_map(move |(new_name, _dt, _refs, rename)| {
                        rename
                            .as_ref()
                            .map(|old| (ty_name.clone(), old.clone(), new_name.clone()))
                    })
                })
                .collect();
            let mut actual_renames: Vec<(String, String, String)> = doc
                .renames
                .iter()
                .map(|r| (r.table.clone(), r.from.clone(), r.to.clone()))
                .collect();
            expected_renames.sort();
            actual_renames.sort();
            prop_assert_eq!(actual_renames, expected_renames);
        }
    }
}
