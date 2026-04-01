/// Search expression AST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchExpr {
    FullText(String),
    FieldEquals { field: String, value: String },
    And(Vec<SearchExpr>),
    Or(Vec<SearchExpr>),
    Not(Box<SearchExpr>),
}

/// Normalize a search query to canonical form.
/// On parse failure, falls back to lowercase + whitespace collapse.
pub fn normalize(_query: &str) -> String {
    String::new() // stub — tests should fail
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PRD test vectors ────────────────────────────────────────────

    #[test]
    fn field_equals_lowercased() {
        assert_eq!(normalize("Tag=Svelte"), "tag=svelte");
    }

    #[test]
    fn and_operands_sorted_alphabetically() {
        assert_eq!(normalize("b AND a"), "a and b");
    }

    #[test]
    fn whitespace_collapsed_and_implicit_and() {
        assert_eq!(normalize("  meeting   minutes  "), "meeting and minutes");
    }

    #[test]
    fn field_filters_sorted_by_serialized_form() {
        assert_eq!(
            normalize("Tag=svelte AND category=work.portals"),
            "category=work.portals and tag=svelte"
        );
    }

    #[test]
    fn field_filters_sorted_regardless_of_input_order() {
        assert_eq!(
            normalize("category=work.portals AND Tag=svelte"),
            "category=work.portals and tag=svelte"
        );
    }

    #[test]
    fn case_insensitive_words_with_whitespace() {
        assert_eq!(normalize("  MEETING   Minutes  "), "meeting and minutes");
    }

    #[test]
    fn implicit_and_between_bare_words() {
        assert_eq!(normalize("meeting minutes"), "meeting and minutes");
    }

    #[test]
    fn parenthesized_or_with_and() {
        assert_eq!(normalize("(a OR b) AND c"), "(a or b) and c");
    }

    #[test]
    fn not_with_field_filter() {
        assert_eq!(normalize("NOT tag=archive"), "not tag=archive");
    }

    #[test]
    fn invalid_query_falls_back_to_lowercase_whitespace_collapse() {
        assert_eq!(normalize(")))bad((( query"), ")))bad((( query");
    }

    // ── Additional behaviors ────────────────────────────────────────

    #[test]
    fn empty_string_returns_empty() {
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn whitespace_only_returns_empty() {
        assert_eq!(normalize("   "), "");
    }

    #[test]
    fn single_word_lowercased() {
        assert_eq!(normalize("Hello"), "hello");
    }

    #[test]
    fn colon_syntax_same_as_equals() {
        assert_eq!(normalize("field:value"), "field=value");
    }

    #[test]
    fn quoted_string_in_field_filter_preserves_spaces() {
        assert_eq!(
            normalize("title:\"meeting minutes\""),
            "title=meeting minutes"
        );
    }

    #[test]
    fn equals_quoted_value_preserves_spaces() {
        assert_eq!(
            normalize("field=\"quoted value\""),
            "field=quoted value"
        );
    }

    #[test]
    fn or_preserves_operand_order() {
        assert_eq!(normalize("a OR b"), "a or b");
    }

    #[test]
    fn nested_not() {
        assert_eq!(normalize("NOT (a OR b)"), "not (a or b)");
    }

    #[test]
    fn complex_nested_and_operands_sorted() {
        // AND operands sorted: "(a or b)" < "(c or d)" alphabetically
        assert_eq!(
            normalize("(a OR b) AND (c OR d)"),
            "(a or b) and (c or d)"
        );
    }

    #[test]
    fn complex_nested_and_operands_sorted_reversed_input() {
        // Even if input order is reversed, AND sort puts them in order
        assert_eq!(
            normalize("(c OR d) AND (a OR b)"),
            "(a or b) and (c or d)"
        );
    }

    #[test]
    fn deeply_nested_recursive_normalization() {
        assert_eq!(
            normalize("NOT (a AND (b OR c))"),
            "not (a and (b or c))"
        );
    }

    #[test]
    fn implicit_and_with_field_filters_sorted() {
        assert_eq!(normalize("tag=a status=b"), "status=b and tag=a");
    }

    #[test]
    fn and_flattening() {
        // Nested ANDs should flatten: (a AND b) AND c -> a and b and c (sorted)
        assert_eq!(normalize("(a AND b) AND c"), "a and b and c");
    }

    #[test]
    fn or_flattening() {
        // Nested ORs should flatten: (a OR b) OR c -> a or b or c
        assert_eq!(normalize("(a OR b) OR c"), "a or b or c");
    }

    #[test]
    fn mixed_operators_and_binds_tighter() {
        // a AND b OR c => (a AND b) OR c due to precedence
        // "a and b" is a single AND group, then OR with c
        assert_eq!(normalize("a AND b OR c"), "a and b or c");
    }

    #[test]
    fn case_insensitive_and_operator() {
        assert_eq!(normalize("a and b"), "a and b");
    }

    #[test]
    fn case_insensitive_and_operator_mixed_case() {
        assert_eq!(normalize("a And b"), "a and b");
    }

    #[test]
    fn case_insensitive_and_operator_upper() {
        assert_eq!(normalize("a AND b"), "a and b");
    }

    #[test]
    fn or_operator_case_insensitive() {
        assert_eq!(normalize("a or b"), "a or b");
        assert_eq!(normalize("a Or b"), "a or b");
        assert_eq!(normalize("a OR b"), "a or b");
    }

    #[test]
    fn not_operator_case_insensitive() {
        assert_eq!(normalize("not a"), "not a");
        assert_eq!(normalize("Not a"), "not a");
        assert_eq!(normalize("NOT a"), "not a");
    }
}
