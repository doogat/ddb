use super::*;

#[test]
fn validate_rename_target_name_accepts_valid_identifiers() {
    assert!(validate_rename_target_name("foo").is_ok());
    assert!(validate_rename_target_name("snake_case").is_ok());
    assert!(validate_rename_target_name("camelCase").is_ok());
    assert!(validate_rename_target_name("Field123").is_ok());
    assert!(validate_rename_target_name("_private").is_ok());
}

#[test]
fn validate_rename_target_name_rejects_empty() {
    let err = validate_rename_target_name("").unwrap_err();
    assert!(err.to_string().contains("empty"));
}

#[test]
fn validate_rename_target_name_rejects_leading_digit() {
    let err = validate_rename_target_name("123abc").unwrap_err();
    assert!(err.to_string().contains("must start with"));
}

#[test]
fn validate_rename_target_name_rejects_hyphen() {
    let err = validate_rename_target_name("with-hyphen").unwrap_err();
    assert!(err.to_string().contains("only letters"));
}

#[test]
fn validate_rename_target_name_rejects_dot_and_space() {
    assert!(validate_rename_target_name("with.dot").is_err());
    assert!(validate_rename_target_name("with space").is_err());
}

#[test]
fn re_create_table_singleton_matches_trailing_marker() {
    let re = re_create_table_singleton();
    assert!(re.is_match("CREATE TABLE x (a INTEGER) SINGLETON"));
    assert!(re.is_match("create table x (a integer) singleton"));
    assert!(re.is_match("CREATE TABLE x (a INTEGER) SINGLETON;"));
    assert!(re.is_match("CREATE TABLE x (a INTEGER) SINGLETON DEFAULT VALUES"));
    assert!(re.is_match("CREATE TABLE x (a INTEGER) SINGLETON   DEFAULT   VALUES"));
    assert!(re.is_match("CREATE TABLE \"x\" (a INTEGER, b TEXT) SINGLETON DEFAULT VALUES;"));
}

#[test]
fn re_create_table_singleton_captures_default_values_marker() {
    let re = re_create_table_singleton();
    let caps = re
        .captures("CREATE TABLE x (a INTEGER) SINGLETON DEFAULT VALUES")
        .unwrap();
    assert!(caps.get(1).is_some(), "DEFAULT VALUES must be captured");
    let caps_no_dv = re.captures("CREATE TABLE x (a INTEGER) SINGLETON").unwrap();
    assert!(
        caps_no_dv.get(1).is_none(),
        "bare SINGLETON must leave group 1 empty"
    );
}

#[test]
fn re_create_table_singleton_rejects_non_marker_uses() {
    let re = re_create_table_singleton();
    // Plain CREATE TABLE without SINGLETON.
    assert!(!re.is_match("CREATE TABLE x (a INTEGER)"));
    // SINGLETON inside the column list (not a marker).
    assert!(!re.is_match("CREATE TABLE x (singleton INTEGER)"));
    // Marker without closing paren preceding (anchored to `)`).
    assert!(!re.is_match("SELECT 'SINGLETON'"));
}

#[test]
fn re_set_singleton_matches_quoted_and_bare_table_names() {
    let re = re_set_singleton();
    assert!(re.is_match("ALTER TABLE x SET SINGLETON"));
    assert!(re.is_match("ALTER TABLE x SET SINGLETON;"));
    assert!(re.is_match("alter table \"meeting-minutes\" set singleton"));
    assert!(re.is_match("ALTER TABLE  app_config   SET   SINGLETON   ;"));

    let caps = re.captures("ALTER TABLE app_config SET SINGLETON").unwrap();
    assert!(caps.get(1).is_none(), "bare name uses group 2");
    assert_eq!(caps.get(2).unwrap().as_str(), "app_config");

    let caps = re
        .captures("ALTER TABLE \"meeting-minutes\" SET SINGLETON")
        .unwrap();
    assert_eq!(caps.get(1).unwrap().as_str(), "meeting-minutes");
    assert!(caps.get(2).is_none());
}

#[test]
fn re_set_singleton_rejects_unrelated_alter_forms() {
    let re = re_set_singleton();
    assert!(!re.is_match("ALTER TABLE x SET SINGLETONS"));
    assert!(!re.is_match("ALTER TABLE x SET SINGLETON foo"));
    assert!(!re.is_match("ALTER TABLE x DROP SINGLETON"));
    assert!(!re.is_match("ALTER TABLE x SET SEARCH KEY foo"));
}

#[test]
fn re_drop_singleton_matches_quoted_and_bare_table_names() {
    let re = re_drop_singleton();
    assert!(re.is_match("ALTER TABLE x DROP SINGLETON"));
    assert!(re.is_match("ALTER TABLE x DROP SINGLETON;"));
    assert!(re.is_match("alter table \"meeting-minutes\" drop singleton"));

    let caps = re
        .captures("ALTER TABLE app_config DROP SINGLETON")
        .unwrap();
    assert_eq!(caps.get(2).unwrap().as_str(), "app_config");
}

#[test]
fn re_drop_singleton_rejects_unrelated_alter_forms() {
    let re = re_drop_singleton();
    assert!(!re.is_match("ALTER TABLE x SET SINGLETON"));
    assert!(!re.is_match("ALTER TABLE x DROP SINGLETONS"));
    assert!(!re.is_match("ALTER TABLE x DROP SINGLETON foo"));
}

#[test]
fn validate_rename_target_name_rejects_reserved_names() {
    assert!(validate_rename_target_name("doogats")
        .unwrap_err()
        .to_string()
        .contains("reserved"));
    assert!(validate_rename_target_name("_typedef")
        .unwrap_err()
        .to_string()
        .contains("reserved"));
    assert!(validate_rename_target_name("_ddb_locks")
        .unwrap_err()
        .to_string()
        .contains("reserved"));
    assert!(validate_rename_target_name("sqlite_master")
        .unwrap_err()
        .to_string()
        .contains("reserved"));
}

// --- strip_inline_zones tests ---
//
// PRD 00160: `strip_inline_zones` returns a table-name -> (column -> zone)
// map. Single-table tests dig into the one table they declare.

#[test]
fn strip_inline_zones_single_zoned_column_maps_and_strips_token() {
    use crate::types::Zone;
    let sql = "CREATE TABLE notes (descr TEXT ZONE frontmatter)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(map.len(), 1);
    assert_eq!(map["notes"]["descr"], Zone::Frontmatter);
    assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
    // Type and column name survive
    assert!(cleaned.to_lowercase().contains("descr"));
    assert!(cleaned.to_lowercase().contains("text"));
}

#[test]
fn strip_inline_zones_maps_two_zoned_columns() {
    use crate::types::Zone;
    let sql = "CREATE TABLE t (title TEXT ZONE frontmatter, body TEXT ZONE body)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(map["t"].len(), 2);
    assert_eq!(map["t"]["title"], Zone::Frontmatter);
    assert_eq!(map["t"]["body"], Zone::Body);
    // Neither ZONE token survives
    assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
    assert!(!cleaned.to_lowercase().contains("zone body"));
}

#[test]
fn strip_inline_zones_preserves_enum_comma() {
    use crate::types::Zone;
    let sql = "CREATE TABLE t (status ENUM('a','b') ZONE frontmatter, id INTEGER)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(map["t"].len(), 1);
    assert_eq!(map["t"]["status"], Zone::Frontmatter);
    // The ENUM comma must be inside the cleaned column definition, not split
    assert!(cleaned.contains("ENUM('a','b')") || cleaned.contains("ENUM('a', 'b')"));
    assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
}

#[test]
fn strip_inline_zones_quoted_identifier_key_strips_quotes() {
    use crate::types::Zone;
    let sql = r#"CREATE TABLE t ("long-desc" TEXT ZONE frontmatter)"#;
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(map["t"].len(), 1);
    assert_eq!(map["t"]["long-desc"], Zone::Frontmatter);
    assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
}

#[test]
fn strip_inline_zones_non_create_table_is_noop() {
    let sql = "INSERT INTO t (a) VALUES (1)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(cleaned, sql);
    assert!(map.is_empty());
}

#[test]
fn strip_inline_zones_rejects_invalid_zone_value() {
    let sql = "CREATE TABLE t (x TEXT ZONE sidebar)";
    let err = strip_inline_zones(sql).unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid zone: sidebar (use frontmatter, body, or reference)"
    );
}

#[test]
fn strip_inline_zones_column_named_zone_does_not_misfire() {
    use crate::types::Zone;
    // A column named `zone` with no ZONE attribute -> empty map
    let sql = "CREATE TABLE t (zone TEXT)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert!(map.is_empty(), "column named 'zone' must not misfire");
    assert!(
        cleaned.to_lowercase().contains("zone text") || cleaned.to_lowercase().contains("zone")
    );

    // A quoted string default that contains 'zone' must not misfire
    let sql2 = "CREATE TABLE t (descr TEXT DEFAULT 'zone body')";
    let (cleaned2, map2) = strip_inline_zones(sql2).unwrap();
    assert!(
        map2.is_empty(),
        "quoted literal 'zone body' must not misfire"
    );
    assert!(!cleaned2.is_empty());

    // A column named `zone` with a ZONE attribute -> map key `zone`, value Body
    let sql3 = "CREATE TABLE t (zone TEXT ZONE body)";
    let (cleaned3, map3) = strip_inline_zones(sql3).unwrap();
    assert_eq!(map3["t"].len(), 1);
    assert_eq!(map3["t"]["zone"], Zone::Body);
    assert!(
        !cleaned3.to_lowercase().contains("zone body") || {
            // Only allowed if "zone body" in cleaned is the "zone TEXT" column def,
            // not the ZONE attribute. A stricter check: the ZONE keyword appearing
            // as an attribute token should be absent after the column name.
            cleaned3.to_lowercase().matches("zone").count() == 1
        }
    );
}

#[test]
fn strip_inline_zones_case_insensitive_keyword_and_value() {
    use crate::types::Zone;
    let sql = "CREATE TABLE t (descr TEXT zone FRONTMATTER)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(map["t"].len(), 1);
    assert_eq!(map["t"]["descr"], Zone::Frontmatter);
    assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
}

#[test]
fn strip_inline_zones_no_zone_create_table_is_idempotent() {
    let sql = "CREATE TABLE t (id INTEGER, name TEXT)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(cleaned, sql);
    assert!(map.is_empty());
}

#[test]
fn strip_inline_zones_reference_zone_value_accepted() {
    use crate::types::Zone;
    let sql = "CREATE TABLE t (owner_id INTEGER ZONE reference)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(map["t"].len(), 1);
    assert_eq!(map["t"]["owner_id"], Zone::Reference);
    assert!(!cleaned.to_lowercase().contains("zone reference"));
}

#[test]
fn strip_inline_zones_strips_every_table_in_multi_statement_batch() {
    use crate::types::Zone;
    // PRD 00160 Critical (blind review): a multi-statement batch must have
    // EVERY table's inline ZONE stripped and attributed to the right table,
    // not just the first. Same column name in two tables with DIFFERENT
    // zones proves per-table keying (a flat column-keyed map would collide).
    let sql = "CREATE TABLE a (descr TEXT ZONE frontmatter); \
               CREATE TABLE b (descr TEXT ZONE body)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(map["a"]["descr"], Zone::Frontmatter);
    assert_eq!(map["b"]["descr"], Zone::Body);
    // No ZONE attribute token survives in either table (would fail sqlparser).
    assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
    assert!(!cleaned.to_lowercase().contains("zone body"));
}

#[test]
fn strip_inline_zones_batch_with_unzoned_first_table_strips_later_table() {
    use crate::types::Zone;
    // The exact Critical scenario: the first table has no inline zone, a
    // later one does. The later table's ZONE must still be stripped.
    let sql = "CREATE TABLE a (x TEXT); CREATE TABLE b (descr TEXT ZONE frontmatter)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert!(!map.contains_key("a"), "unzoned table absent from map");
    assert_eq!(map["b"]["descr"], Zone::Frontmatter);
    assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
    // The first table's body is untouched.
    assert!(cleaned.contains("CREATE TABLE a (x TEXT)"));
}

// PRD 00160 rework (review cycle 1): regression tests for the defects the
// multi-reviewer cycle found in strip_inline_zones.

#[test]
fn strip_inline_zones_trailing_whitespace_before_delimiter_still_maps() {
    use crate::types::Zone;
    // Trailing whitespace before the closing paren / comma must NOT cause the
    // ZONE token to be skipped. (Bug: leading_ws counted trailing whitespace,
    // over-skipping past ZONE so it survived to sqlparser as a parse error.)
    let sql = "CREATE TABLE t (a TEXT ZONE body          , b TEXT)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(map["t"].len(), 1, "trailing whitespace must not lose the ZONE");
    assert_eq!(map["t"]["a"], Zone::Body);
    assert!(!cleaned.to_lowercase().contains("zone body"));

    // Also before the closing paren.
    let sql2 = "CREATE TABLE t (a TEXT ZONE frontmatter          )";
    let (_c2, map2) = strip_inline_zones(sql2).unwrap();
    assert_eq!(map2["t"].get("a"), Some(&Zone::Frontmatter));
}

#[test]
fn strip_inline_zones_does_not_panic_on_unterminated_quoted_identifier() {
    // Malformed user SQL must not panic (AGENTS.md no-panic guardrail).
    // (Bug: name_end for an unterminated quoted identifier exceeded seg.len(),
    // making &seg[name_end..] an out-of-bounds slice.)
    let sql = "CREATE TABLE t (\"unterminated TEXT)";
    // Must return (Ok or Err) without panicking.
    let _ = strip_inline_zones(sql);
}

#[test]
fn strip_inline_zones_does_not_panic_on_unbalanced_parens() {
    // A stray ) before any ( must not underflow the depth counter.
    let sql = "CREATE TABLE t )";
    let _ = strip_inline_zones(sql);
    let sql2 = "CREATE TABLE t (a TEXT))";
    let _ = strip_inline_zones(sql2);
}

#[test]
fn strip_inline_zones_time_zone_type_is_not_treated_as_inline_zone() {
    // `TIMESTAMP WITH TIME ZONE` is a standard type; the ZONE inside it must
    // NOT be parsed as an inline zone attribute (which previously errored with
    // `invalid zone: not`). A genuine inline ZONE on another column still maps.
    let sql = "CREATE TABLE t (created TIMESTAMP WITH TIME ZONE NOT NULL)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert!(map.is_empty(), "TIME ZONE must not yield an inline zone");
    assert_eq!(cleaned, sql, "no-zone input round-trips unchanged");

    // A real inline ZONE alongside a TIME ZONE typed column still maps, and
    // the TIME ZONE token is left intact.
    let sql2 = "CREATE TABLE t (created TIMESTAMP WITH TIME ZONE NOT NULL, descr TEXT ZONE frontmatter)";
    let (cleaned2, map2) = strip_inline_zones(sql2).unwrap();
    use crate::types::Zone;
    assert_eq!(map2["t"].get("descr"), Some(&Zone::Frontmatter));
    assert!(!map2["t"].contains_key("created"));
    assert!(cleaned2.to_uppercase().contains("TIME ZONE"));
}

#[test]
fn strip_inline_zones_still_errors_on_genuine_bad_value() {
    // The TIME ZONE carve-out must not swallow a real typo: `ZONE sidebar`
    // still errors clearly.
    let sql = "CREATE TABLE t (x TEXT ZONE sidebar)";
    let err = strip_inline_zones(sql).unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid zone: sidebar (use frontmatter, body, or reference)"
    );
}

#[test]
fn strip_inline_zones_quoted_paren_before_zone() {
    use crate::types::Zone;
    // A single-quoted '(' before the inline ZONE must not desync the paren
    // scanner that finds the column-list bounds.
    let sql = "CREATE TABLE t (a TEXT DEFAULT '(' ZONE frontmatter)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(map["t"].get("a"), Some(&Zone::Frontmatter));
    assert!(cleaned.contains("DEFAULT '('"), "quoted default preserved");
    assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
}

#[test]
fn strip_inline_zones_inline_zone_after_time_zone_type() {
    use crate::types::Zone;
    // A `TIMESTAMP WITH TIME ZONE` column that ALSO declares an inline zone:
    // the TIME ZONE type keyword is skipped, but the trailing real ZONE must
    // still be mapped (the carve-out must not swallow it).
    let sql = "CREATE TABLE t (created TIMESTAMP WITH TIME ZONE ZONE body)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(map["t"].get("created"), Some(&Zone::Body));
    // The TIME ZONE type keyword survives; the trailing inline ZONE is excised.
    assert!(cleaned.to_uppercase().contains("TIME ZONE"));
    assert!(!cleaned.to_lowercase().contains("zone body"));
}

// PRD 00160 doubt review: regression tests for the two correctness bugs the
// skeptical final review found in strip_inline_zones.

#[test]
fn strip_inline_zones_leading_non_create_statement_strips_later_table() {
    use crate::types::Zone;
    // A batch whose FIRST statement is not CREATE TABLE (e.g. a CREATE TYPE
    // for an enum, or a BEGIN) must still have a later table's inline ZONE
    // stripped. (Bug: the guard was anchored `^\s*CREATE TABLE`, so any
    // leading non-CREATE-TABLE statement made the whole batch a no-op and the
    // ZONE token reached sqlparser as a parse error.)
    let sql = "CREATE TYPE mood AS ENUM ('a','b'); \
               CREATE TABLE t (m TEXT ZONE frontmatter)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    assert_eq!(
        map["t"]["m"],
        Zone::Frontmatter,
        "later table's ZONE must be stripped despite a leading non-CREATE statement"
    );
    assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
    // The leading statement is untouched.
    assert!(cleaned.contains("CREATE TYPE mood AS ENUM ('a','b')"));
}

#[test]
fn strip_inline_zones_create_table_inside_string_literal_does_not_panic() {
    use crate::types::Zone;
    // A column default that is a string literal CONTAINING `CREATE TABLE ...`
    // must not panic. (Bug: the preamble regex matched the fake CREATE TABLE
    // inside the literal, whose open offset fell before the cursor, so the
    // slice `&sql[cursor..=open]` panicked with begin > end. No-panic guardrail.)
    let sql = "CREATE TABLE t (note TEXT DEFAULT 'CREATE TABLE fake (x TEXT ZONE body)', summary TEXT ZONE frontmatter)";
    let (cleaned, map) = strip_inline_zones(sql).unwrap();
    // Only the real table/column is mapped; the literal's fake table is not.
    assert_eq!(map["t"].len(), 1);
    assert_eq!(map["t"]["summary"], Zone::Frontmatter);
    assert!(
        !map.contains_key("fake"),
        "a CREATE TABLE inside a string literal must not be mapped"
    );
    // The literal default is preserved verbatim.
    assert!(
        cleaned.contains("'CREATE TABLE fake (x TEXT ZONE body)'"),
        "literal default preserved verbatim"
    );
    // The real inline ZONE attribute is excised.
    assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
}
