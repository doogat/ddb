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

    use proptest::prelude::*;

    proptest! {
        /// Generated (non-enumerable) YAML defeats substring-fingerprinting:
        /// the parser must read back every generated type name, column name,
        /// and data_type. A finite if/else cascade keyed on fixed literals
        /// cannot satisfy randomized inputs. Emitting FLOW-form YAML (one
        /// line, no fixed indentation) additionally defeats a line scanner
        /// that keys off indentation depth.
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
                        ),
                        1..=4,
                    ),
                ),
                1..=3,
            )
        ) {
            // Build the expected document, deduping column names within a
            // type by prefixing the column index so assertions are unambiguous.
            let expected: Vec<(String, Vec<(String, String, Option<String>)>)> = specs
                .iter()
                .map(|(ty_name, cols)| {
                    let cols = cols
                        .iter()
                        .enumerate()
                        .map(|(i, (col_name, dt, refs))| {
                            (format!("c{i}_{col_name}"), dt.to_string(), refs.clone())
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
                        .map(|(col_name, dt, refs)| match refs {
                            Some(r) => {
                                format!("{{name: {col_name}, data_type: \"{dt}\", references: {r}}}")
                            }
                            None => format!("{{name: {col_name}, data_type: \"{dt}\"}}"),
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
                for (col, (exp_col_name, exp_dt, exp_refs)) in
                    table.columns.iter().zip(exp_cols.iter())
                {
                    prop_assert_eq!(&col.name, exp_col_name);
                    prop_assert_eq!(&col.data_type, exp_dt);
                    prop_assert_eq!(&col.references, exp_refs);
                }
            }
        }
    }
}
