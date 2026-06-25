use std::collections::BTreeMap;

use crate::error::{DoogatError, Result};
use crate::parser::from_serde_yaml;
use crate::sql_engine::schema_from_parsed;
use crate::types::{DoogatMeta, ParsedDoogat, Value};

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
            types.push(schema_from_parsed(&parsed)?);
        }

        Ok(SchemaDoc { types, renames })
    }
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

    /// A fully VALID desired doc whose field values contain substrings a
    /// naive heuristic might mistake for error markers or branch keys
    /// (`unterminated` from the malformed-YAML test, `contact` from the
    /// multi-type test) must still parse and read its fields correctly.
    /// An impl that pattern-matches on those fingerprint substrings instead
    /// of parsing would misclassify this valid input and fail.
    #[test]
    fn valid_doc_with_error_marker_substring_parses() {
        let yaml = r#"
types:
  - name: contact
    columns:
      - name: unterminated
        data_type: TEXT
        default_value: "[unterminated"
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
            Some("[unterminated".to_string())
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
