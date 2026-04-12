/// Search expression AST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchExpr {
    FullText(String),
    FieldEquals { field: String, value: String },
    And(Vec<SearchExpr>),
    Or(Vec<SearchExpr>),
    Not(Box<SearchExpr>),
}

/// Result of compiling a search query into an execution plan.
///
/// `extracted_filters` holds top-level `field=value` filters that can be
/// applied directly against the index. `extracted_negated_filters` holds
/// top-level `NOT field=value` filters. `fts_query` contains whatever
/// remains after extraction, ready to hand to FTS5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPlan {
    pub fts_query: Option<String>,
    pub extracted_filters: Vec<(String, String)>,
    pub extracted_negated_filters: Vec<(String, String)>,
}

// ── Tokenizer ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    FieldFilter { field: String, value: String },
    And,
    Or,
    Not,
    LParen,
    RParen,
}

fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }

        if chars[i] == '(' {
            tokens.push(Token::LParen);
            i += 1;
            continue;
        }

        if chars[i] == ')' {
            tokens.push(Token::RParen);
            i += 1;
            continue;
        }

        // Read a word (alphanum, dot, dash, underscore, etc. -- anything not whitespace/parens/=/: )
        // But also handle field=value and field:value and field:"quoted"
        if chars[i] == '"' {
            i += 1;
            let mut s = String::new();
            while i < len && chars[i] != '"' {
                s.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1;
            }
            tokens.push(Token::Word(s.to_lowercase()));
            continue;
        }

        // accumulate a word (no whitespace, no parens, no = or :)
        let mut word = String::new();
        while i < len
            && !chars[i].is_ascii_whitespace()
            && chars[i] != '('
            && chars[i] != ')'
            && chars[i] != '='
            && chars[i] != ':'
            && chars[i] != '"'
        {
            word.push(chars[i]);
            i += 1;
        }

        if word.is_empty() {
            // stray character we didn't handle - just push it as a word
            word.push(chars[i]);
            i += 1;
            tokens.push(Token::Word(word.to_lowercase()));
            continue;
        }

        if i < len && (chars[i] == '=' || chars[i] == ':') {
            let field = word.to_lowercase();
            i += 1;

            let value = if i < len && chars[i] == '"' {
                i += 1;
                let mut v = String::new();
                while i < len && chars[i] != '"' {
                    v.push(chars[i]);
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
                v.to_lowercase()
            } else {
                let mut v = String::new();
                while i < len
                    && !chars[i].is_ascii_whitespace()
                    && chars[i] != '('
                    && chars[i] != ')'
                {
                    v.push(chars[i]);
                    i += 1;
                }
                v.to_lowercase()
            };

            tokens.push(Token::FieldFilter { field, value });
            continue;
        }

        let lower = word.to_lowercase();
        match lower.as_str() {
            "and" => tokens.push(Token::And),
            "or" => tokens.push(Token::Or),
            "not" => tokens.push(Token::Not),
            _ => tokens.push(Token::Word(lower)),
        }
    }

    tokens
}

// ── Parser ──────────────────────────────────────────────────────────
//
// Grammar (precedence low to high):
//   expr     = or_expr
//   or_expr  = and_expr ("OR" and_expr)*
//   and_expr = unary (("AND")? unary)*     -- implicit AND between adjacent terms
//   unary    = "NOT" unary | primary
//   primary  = "(" expr ")" | field_filter | word

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn parse(&mut self) -> Option<SearchExpr> {
        let expr = self.parse_or()?;
        if self.pos < self.tokens.len() {
            return None; // unconsumed tokens = parse error
        }
        Some(expr)
    }

    fn parse_or(&mut self) -> Option<SearchExpr> {
        let mut operands = vec![self.parse_and()?];
        while matches!(self.peek(), Some(Token::Or)) {
            self.advance();
            operands.push(self.parse_and()?);
        }
        if operands.len() == 1 {
            Some(operands.remove(0))
        } else {
            // flatten nested ORs
            let mut flat = Vec::new();
            for op in operands {
                match op {
                    SearchExpr::Or(children) => flat.extend(children),
                    other => flat.push(other),
                }
            }
            Some(SearchExpr::Or(flat))
        }
    }

    fn parse_and(&mut self) -> Option<SearchExpr> {
        let mut operands = vec![self.parse_unary()?];
        loop {
            // explicit AND
            if matches!(self.peek(), Some(Token::And)) {
                self.advance();
                operands.push(self.parse_unary()?);
                continue;
            }
            // implicit AND: next token is a primary-starting token (not OR, not RParen, not EOF)
            match self.peek() {
                Some(Token::Word(_))
                | Some(Token::FieldFilter { .. })
                | Some(Token::Not)
                | Some(Token::LParen) => {
                    operands.push(self.parse_unary()?);
                }
                _ => break,
            }
        }
        if operands.len() == 1 {
            Some(operands.remove(0))
        } else {
            // flatten nested ANDs
            let mut flat = Vec::new();
            for op in operands {
                match op {
                    SearchExpr::And(children) => flat.extend(children),
                    other => flat.push(other),
                }
            }
            Some(SearchExpr::And(flat))
        }
    }

    fn parse_unary(&mut self) -> Option<SearchExpr> {
        if matches!(self.peek(), Some(Token::Not)) {
            self.advance();
            let inner = self.parse_unary()?;
            return Some(SearchExpr::Not(Box::new(inner)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Option<SearchExpr> {
        match self.peek()? {
            Token::LParen => {
                self.advance();
                let expr = self.parse_or()?;
                if !matches!(self.peek(), Some(Token::RParen)) {
                    return None;
                }
                self.advance();
                Some(expr)
            }
            Token::Word(_) => {
                if let Some(Token::Word(w)) = self.advance() {
                    Some(SearchExpr::FullText(w))
                } else {
                    None
                }
            }
            Token::FieldFilter { .. } => {
                if let Some(Token::FieldFilter { field, value }) = self.advance() {
                    Some(SearchExpr::FieldEquals { field, value })
                } else {
                    None
                }
            }
            _ => None, // unexpected token
        }
    }
}

// ── Serializer ──────────────────────────────────────────────────────

fn serialize(expr: &SearchExpr) -> String {
    match expr {
        SearchExpr::FullText(w) => {
            if w.contains(' ') {
                format!("\"{w}\"")
            } else {
                w.clone()
            }
        }
        SearchExpr::FieldEquals { field, value } => {
            if value.contains(' ') {
                format!("{field}=\"{value}\"")
            } else {
                format!("{field}={value}")
            }
        }
        SearchExpr::And(children) => {
            let mut pairs: Vec<(String, String)> = children
                .iter()
                .map(|c| {
                    let sort_key = serialize(c);
                    let display = serialize_and_child(c);
                    (sort_key, display)
                })
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            let parts: Vec<String> = pairs.into_iter().map(|(_, display)| display).collect();
            parts.join(" and ")
        }
        SearchExpr::Or(children) => {
            let parts: Vec<String> = children.iter().map(serialize).collect();
            parts.join(" or ")
        }
        SearchExpr::Not(inner) => {
            format!("not {}", serialize_not_child(inner))
        }
    }
}

/// Wrap child in parens if it's an OR (lower precedence than AND).
fn serialize_and_child(expr: &SearchExpr) -> String {
    match expr {
        SearchExpr::Or(_) => format!("({})", serialize(expr)),
        _ => serialize(expr),
    }
}

/// NOT child: wrap in parens if compound (AND or OR).
fn serialize_not_child(expr: &SearchExpr) -> String {
    match expr {
        SearchExpr::And(_) | SearchExpr::Or(_) => format!("({})", serialize(expr)),
        _ => serialize(expr),
    }
}

// ── Fallback ────────────────────────────────────────────────────────

fn fallback_normalize(query: &str) -> String {
    query.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── FTS5 Serializer ────────────────────────────────────────────────

fn fts_serialize(expr: &SearchExpr) -> String {
    match expr {
        SearchExpr::FullText(w) => {
            if w.contains(' ') {
                format!("\"{w}\"")
            } else {
                w.clone()
            }
        }
        SearchExpr::FieldEquals { value, .. } => {
            if value.contains(' ') {
                format!("\"{value}\"")
            } else {
                value.clone()
            }
        }
        SearchExpr::And(children) => {
            let parts: Vec<String> = children.iter().map(fts_and_child).collect();
            parts.join(" AND ")
        }
        SearchExpr::Or(children) => {
            let parts: Vec<String> = children.iter().map(fts_serialize).collect();
            parts.join(" OR ")
        }
        SearchExpr::Not(inner) => format!("NOT {}", fts_not_child(inner)),
    }
}

/// Wrap child in parens if it's an OR (lower precedence than AND).
fn fts_and_child(expr: &SearchExpr) -> String {
    match expr {
        SearchExpr::Or(_) => format!("({})", fts_serialize(expr)),
        _ => fts_serialize(expr),
    }
}

/// NOT child: wrap in parens if compound (AND or OR).
fn fts_not_child(expr: &SearchExpr) -> String {
    match expr {
        SearchExpr::And(_) | SearchExpr::Or(_) => format!("({})", fts_serialize(expr)),
        _ => fts_serialize(expr),
    }
}

// ── Public API ──────────────────────────────────────────────────────

/// Parse a search query into a `SearchExpr` AST.
/// Returns `None` for empty input or parse failure.
pub fn parse(query: &str) -> Option<SearchExpr> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }
    let tokens = tokenize(trimmed);
    if tokens.is_empty() {
        return None;
    }
    Parser::new(tokens).parse()
}

/// Serialize a `SearchExpr` to valid FTS5 MATCH syntax.
pub fn to_fts_query(expr: &SearchExpr) -> String {
    fts_serialize(expr)
}

/// Partition a `SearchExpr` into positive and negated parts.
/// Returns `(positive_ast, negated_inner_exprs)` where negated_inner_exprs
/// are the inner expressions unwrapped from NOT nodes.
/// Only decomposes top-level AND and standalone NOT; OR nodes pass through intact.
pub fn extract_negations(expr: SearchExpr) -> (Option<SearchExpr>, Vec<SearchExpr>) {
    match expr {
        SearchExpr::Not(inner) => (None, vec![*inner]),
        SearchExpr::And(children) => {
            let mut positives = Vec::new();
            let mut negatives = Vec::new();
            for child in children {
                match child {
                    SearchExpr::Not(inner) => negatives.push(*inner),
                    other => positives.push(other),
                }
            }
            let pos = match positives.len() {
                0 => None,
                1 => Some(positives.remove(0)),
                _ => Some(SearchExpr::And(positives)),
            };
            (pos, negatives)
        }
        other => (Some(other), Vec::new()),
    }
}

/// Compile a search query string into a `SearchPlan`.
///
/// Parses the query into a `SearchExpr` AST and partitions it into three
/// slots on the returned `SearchPlan`:
///
/// - `extracted_filters`: positive `FieldEquals` nodes at the top level (or
///   as children of a top-level `And`) are pulled out and routed to the
///   filter SQL layer.
/// - `extracted_negated_filters`: top-level `Not(FieldEquals)` nodes are
///   pulled out the same way, intended for negation clauses.
/// - `fts_query`: everything else (bare `FullText`, `Or`, nested `And`,
///   `Not` over non-field expressions) is serialized back via
///   `to_fts_query` and passed to FTS5 as the residual MATCH query.
///   `None` means the residual is empty and no FTS MATCH is needed.
///
/// Empty or whitespace-only input returns an empty plan (`fts_query =
/// None`, no filters). The caller decides whether that is an error.
///
/// # Errors
///
/// Returns `Err` for:
/// - unparseable input (`parse()` returned `None`)
/// - bare wildcard-only queries (`*`, `**`, `.*`)
/// - negated field filters on non-tag fields (`NOT url=example.com`)
pub fn compile_search_plan(query: &str) -> Result<SearchPlan, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(SearchPlan {
            fts_query: None,
            extracted_filters: Vec::new(),
            extracted_negated_filters: Vec::new(),
        });
    }

    let expr = parse(trimmed).ok_or_else(|| "invalid search query: unparseable".to_string())?;

    if let SearchExpr::FullText(ref s) = expr {
        let t = s.trim();
        if t == "*" || t == "**" || t == ".*" {
            return Err("invalid search query: bare wildcard not allowed".to_string());
        }
    }

    let mut extracted_filters: Vec<(String, String)> = Vec::new();
    let mut extracted_negated_filters: Vec<(String, String)> = Vec::new();
    let mut remaining: Vec<SearchExpr> = Vec::new();

    match expr {
        SearchExpr::And(children) => {
            for child in children {
                match child {
                    SearchExpr::FieldEquals { field, value } => {
                        extracted_filters.push((field, value));
                    }
                    SearchExpr::Not(inner) => match *inner {
                        SearchExpr::FieldEquals { field, value } => {
                            extracted_negated_filters.push((field, value));
                        }
                        other => {
                            remaining.push(SearchExpr::Not(Box::new(other)));
                        }
                    },
                    other => remaining.push(other),
                }
            }
        }
        SearchExpr::FieldEquals { field, value } => {
            extracted_filters.push((field, value));
        }
        SearchExpr::Not(inner) => match *inner {
            SearchExpr::FieldEquals { field, value } => {
                extracted_negated_filters.push((field, value));
            }
            other => {
                remaining.push(SearchExpr::Not(Box::new(other)));
            }
        },
        other => remaining.push(other),
    }

    // Reject negated field filters on non-tag fields. Only `tag` negation
    // is supported; other fields would silently drop the negation otherwise.
    for (field, _) in &extracted_negated_filters {
        if field != "tag" {
            return Err(format!(
                "NOT is only supported for tag filters, got field: {field}"
            ));
        }
    }

    let fts_query = match remaining.len() {
        0 => None,
        1 => Some(to_fts_query(&remaining.remove(0))),
        _ => Some(to_fts_query(&SearchExpr::And(remaining))),
    };

    Ok(SearchPlan {
        fts_query,
        extracted_filters,
        extracted_negated_filters,
    })
}

/// Validate and compile a search query, rejecting truly empty plans.
///
/// Wraps [`compile_search_plan`] with the additional constraint that the
/// resulting plan must contain at least one of: an FTS query, extracted
/// filters, or extracted negated filters. This ensures `search()` and
/// `normalizeSearchQuery` agree on the set of valid queries structurally
/// rather than by duplicating the check in each caller.
///
/// Callers with external filters (types, tag, where) should handle the
/// `Err` case themselves when those filters are present, since external
/// filters rescue an otherwise-empty query.
pub fn validate_and_compile(query: &str) -> Result<SearchPlan, String> {
    let plan = compile_search_plan(query)?;
    if plan.fts_query.is_none()
        && plan.extracted_filters.is_empty()
        && plan.extracted_negated_filters.is_empty()
    {
        return Err(format!("invalid search query: {query}"));
    }
    Ok(plan)
}

/// Normalize a search query to canonical form.
/// On parse failure, falls back to lowercase + whitespace collapse.
pub fn normalize(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let tokens = tokenize(trimmed);
    if tokens.is_empty() {
        return String::new();
    }

    let mut parser = Parser::new(tokens);
    match parser.parse() {
        Some(expr) => serialize(&expr),
        None => fallback_normalize(trimmed),
    }
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
            "title=\"meeting minutes\""
        );
    }

    #[test]
    fn equals_quoted_value_preserves_spaces() {
        assert_eq!(
            normalize("field=\"quoted value\""),
            "field=\"quoted value\""
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

    // ── Idempotency ────────────────────────────────────────────────

    #[test]
    fn normalize_is_idempotent() {
        let inputs = [
            "Tag=Svelte",
            "b AND a",
            "  meeting   minutes  ",
            "Tag=svelte AND category=work.portals",
            "(a OR b) AND c",
            "NOT tag=archive",
            ")))bad((( query",
            "field=\"quoted value\"",
            "title:\"meeting minutes\"",
            "a AND b OR c",
            "(c OR d) AND (a OR b)",
            "NOT (a AND (b OR c))",
            "\"meeting minutes\"",
        ];
        for input in &inputs {
            let once = normalize(input);
            let twice = normalize(&once);
            assert_eq!(once, twice, "not idempotent for input: {input}");
        }
    }

    // ── Edge cases from PRD risks ──────────────────────────────────

    #[test]
    fn standalone_quoted_string_preserves_quotes() {
        assert_eq!(
            normalize("\"meeting minutes\""),
            "\"meeting minutes\""
        );
    }

    #[test]
    fn empty_field_value_fallback() {
        // field= with empty value: treated as field filter with empty value
        let result = normalize("field=");
        assert_eq!(result, "field=");
    }

    #[test]
    fn empty_field_name_parses_as_terms() {
        // =value with no field: = becomes a word, value becomes a word, implicit AND
        let result = normalize("=value");
        assert_eq!(result, "= and value");
    }

    // ── parse() tests ──────────────────────────────────────────────────

    #[test]
    fn parse_simple_word() {
        assert_eq!(parse("hello"), Some(SearchExpr::FullText("hello".into())));
    }

    #[test]
    fn parse_two_words_implicit_and() {
        assert_eq!(
            parse("a b"),
            Some(SearchExpr::And(vec![
                SearchExpr::FullText("a".into()),
                SearchExpr::FullText("b".into()),
            ]))
        );
    }

    #[test]
    fn parse_explicit_and() {
        assert_eq!(
            parse("a AND b"),
            Some(SearchExpr::And(vec![
                SearchExpr::FullText("a".into()),
                SearchExpr::FullText("b".into()),
            ]))
        );
    }

    #[test]
    fn parse_or() {
        assert_eq!(
            parse("a OR b"),
            Some(SearchExpr::Or(vec![
                SearchExpr::FullText("a".into()),
                SearchExpr::FullText("b".into()),
            ]))
        );
    }

    #[test]
    fn parse_not() {
        assert_eq!(
            parse("NOT a"),
            Some(SearchExpr::Not(Box::new(SearchExpr::FullText("a".into()))))
        );
    }

    #[test]
    fn parse_complex_negation() {
        assert_eq!(
            parse("important NOT meeting"),
            Some(SearchExpr::And(vec![
                SearchExpr::FullText("important".into()),
                SearchExpr::Not(Box::new(SearchExpr::FullText("meeting".into()))),
            ]))
        );
    }

    #[test]
    fn parse_field_filter() {
        assert_eq!(
            parse("tag=svelte"),
            Some(SearchExpr::FieldEquals {
                field: "tag".into(),
                value: "svelte".into(),
            })
        );
    }

    #[test]
    fn parse_quoted_phrase() {
        assert_eq!(
            parse("\"meeting minutes\""),
            Some(SearchExpr::FullText("meeting minutes".into()))
        );
    }

    #[test]
    fn parse_nested_parens() {
        assert_eq!(
            parse("(a OR b) AND c"),
            Some(SearchExpr::And(vec![
                SearchExpr::Or(vec![
                    SearchExpr::FullText("a".into()),
                    SearchExpr::FullText("b".into()),
                ]),
                SearchExpr::FullText("c".into()),
            ]))
        );
    }

    #[test]
    fn parse_returns_none_on_bad_input() {
        assert_eq!(parse(")))bad((("), None);
    }

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse(""), None);
    }

    // ── to_fts_query() tests ───────────────────────────────────────────

    #[test]
    fn to_fts_query_single_word() {
        assert_eq!(to_fts_query(&SearchExpr::FullText("hello".into())), "hello");
    }

    #[test]
    fn to_fts_query_quoted_phrase() {
        assert_eq!(
            to_fts_query(&SearchExpr::FullText("meeting minutes".into())),
            "\"meeting minutes\""
        );
    }

    #[test]
    fn to_fts_query_and() {
        let expr = SearchExpr::And(vec![
            SearchExpr::FullText("a".into()),
            SearchExpr::FullText("b".into()),
        ]);
        assert_eq!(to_fts_query(&expr), "a AND b");
    }

    #[test]
    fn to_fts_query_or() {
        let expr = SearchExpr::Or(vec![
            SearchExpr::FullText("a".into()),
            SearchExpr::FullText("b".into()),
        ]);
        assert_eq!(to_fts_query(&expr), "a OR b");
    }

    #[test]
    fn to_fts_query_nested() {
        let expr = SearchExpr::And(vec![
            SearchExpr::FullText("a".into()),
            SearchExpr::Or(vec![
                SearchExpr::FullText("b".into()),
                SearchExpr::FullText("c".into()),
            ]),
        ]);
        assert_eq!(to_fts_query(&expr), "a AND (b OR c)");
    }

    #[test]
    fn to_fts_query_field_equals_uses_value() {
        let expr = SearchExpr::FieldEquals {
            field: "tag".into(),
            value: "svelte".into(),
        };
        assert_eq!(to_fts_query(&expr), "svelte");
    }

    #[test]
    fn to_fts_query_not() {
        let expr = SearchExpr::Not(Box::new(SearchExpr::FullText("a".into())));
        assert_eq!(to_fts_query(&expr), "NOT a");
    }

    #[test]
    fn to_fts_query_and_with_not() {
        let expr = SearchExpr::And(vec![
            SearchExpr::FullText("a".into()),
            SearchExpr::Not(Box::new(SearchExpr::FullText("b".into()))),
        ]);
        assert_eq!(to_fts_query(&expr), "a AND NOT b");
    }

    #[test]
    fn to_fts_query_not_compound() {
        let expr = SearchExpr::Not(Box::new(SearchExpr::And(vec![
            SearchExpr::FullText("a".into()),
            SearchExpr::FullText("b".into()),
        ])));
        assert_eq!(to_fts_query(&expr), "NOT (a AND b)");
    }

    // ── extract_negations() tests ─────────────────────────────────────

    #[test]
    fn extract_negations_standalone_fulltext() {
        let (pos, negs) = extract_negations(SearchExpr::FullText("a".into()));
        assert_eq!(pos, Some(SearchExpr::FullText("a".into())));
        assert!(negs.is_empty());
    }

    #[test]
    fn extract_negations_standalone_field_equals() {
        let (pos, negs) = extract_negations(SearchExpr::FieldEquals {
            field: "tag".into(),
            value: "x".into(),
        });
        assert_eq!(
            pos,
            Some(SearchExpr::FieldEquals {
                field: "tag".into(),
                value: "x".into(),
            })
        );
        assert!(negs.is_empty());
    }

    #[test]
    fn extract_negations_standalone_not() {
        let (pos, negs) =
            extract_negations(SearchExpr::Not(Box::new(SearchExpr::FullText("a".into()))));
        assert_eq!(pos, None);
        assert_eq!(negs, vec![SearchExpr::FullText("a".into())]);
    }

    #[test]
    fn extract_negations_and_with_one_not() {
        let (pos, negs) = extract_negations(SearchExpr::And(vec![
            SearchExpr::FullText("important".into()),
            SearchExpr::Not(Box::new(SearchExpr::FullText("meeting".into()))),
        ]));
        assert_eq!(pos, Some(SearchExpr::FullText("important".into())));
        assert_eq!(negs, vec![SearchExpr::FullText("meeting".into())]);
    }

    #[test]
    fn extract_negations_and_with_multiple_nots() {
        let (pos, negs) = extract_negations(SearchExpr::And(vec![
            SearchExpr::FullText("a".into()),
            SearchExpr::Not(Box::new(SearchExpr::FullText("b".into()))),
            SearchExpr::Not(Box::new(SearchExpr::FullText("c".into()))),
        ]));
        assert_eq!(pos, Some(SearchExpr::FullText("a".into())));
        assert_eq!(
            negs,
            vec![
                SearchExpr::FullText("b".into()),
                SearchExpr::FullText("c".into()),
            ]
        );
    }

    #[test]
    fn extract_negations_and_all_negative() {
        let (pos, negs) = extract_negations(SearchExpr::And(vec![
            SearchExpr::Not(Box::new(SearchExpr::FullText("a".into()))),
            SearchExpr::Not(Box::new(SearchExpr::FullText("b".into()))),
        ]));
        assert_eq!(pos, None);
        assert_eq!(
            negs,
            vec![
                SearchExpr::FullText("a".into()),
                SearchExpr::FullText("b".into()),
            ]
        );
    }

    #[test]
    fn extract_negations_and_multiple_positives_after_removing_nots() {
        let (pos, negs) = extract_negations(SearchExpr::And(vec![
            SearchExpr::FullText("a".into()),
            SearchExpr::FullText("b".into()),
            SearchExpr::Not(Box::new(SearchExpr::FullText("c".into()))),
        ]));
        assert_eq!(
            pos,
            Some(SearchExpr::And(vec![
                SearchExpr::FullText("a".into()),
                SearchExpr::FullText("b".into()),
            ]))
        );
        assert_eq!(negs, vec![SearchExpr::FullText("c".into())]);
    }

    #[test]
    fn extract_negations_not_with_compound_inner() {
        let (pos, negs) = extract_negations(SearchExpr::Not(Box::new(SearchExpr::And(vec![
            SearchExpr::FullText("a".into()),
            SearchExpr::FullText("b".into()),
        ]))));
        assert_eq!(pos, None);
        assert_eq!(
            negs,
            vec![SearchExpr::And(vec![
                SearchExpr::FullText("a".into()),
                SearchExpr::FullText("b".into()),
            ])]
        );
    }

    #[test]
    fn extract_negations_or_not_decomposed() {
        let (pos, negs) = extract_negations(SearchExpr::Or(vec![
            SearchExpr::FullText("a".into()),
            SearchExpr::Not(Box::new(SearchExpr::FullText("b".into()))),
        ]));
        assert_eq!(
            pos,
            Some(SearchExpr::Or(vec![
                SearchExpr::FullText("a".into()),
                SearchExpr::Not(Box::new(SearchExpr::FullText("b".into()))),
            ]))
        );
        assert!(negs.is_empty());
    }

    #[test]
    fn extract_negations_not_field_equals() {
        let (pos, negs) = extract_negations(SearchExpr::Not(Box::new(SearchExpr::FieldEquals {
            field: "tag".into(),
            value: "archive".into(),
        })));
        assert_eq!(pos, None);
        assert_eq!(
            negs,
            vec![SearchExpr::FieldEquals {
                field: "tag".into(),
                value: "archive".into(),
            }]
        );
    }

    #[test]
    fn extract_negations_and_with_not_field_equals() {
        let (pos, negs) = extract_negations(SearchExpr::And(vec![
            SearchExpr::FullText("important".into()),
            SearchExpr::Not(Box::new(SearchExpr::FieldEquals {
                field: "tag".into(),
                value: "archive".into(),
            })),
        ]));
        assert_eq!(pos, Some(SearchExpr::FullText("important".into())));
        assert_eq!(
            negs,
            vec![SearchExpr::FieldEquals {
                field: "tag".into(),
                value: "archive".into(),
            }]
        );
    }

    // ── compile_search_plan() tests ────────────────────────────────────

    #[test]
    fn compile_empty_returns_no_fts_no_filters() {
        let plan = compile_search_plan("").expect("empty query should be Ok");
        assert_eq!(plan.fts_query, None);
        assert!(plan.extracted_filters.is_empty());
        assert!(plan.extracted_negated_filters.is_empty());

        let plan = compile_search_plan("   ").expect("whitespace-only query should be Ok");
        assert_eq!(plan.fts_query, None);
        assert!(plan.extracted_filters.is_empty());
        assert!(plan.extracted_negated_filters.is_empty());
    }

    #[test]
    fn compile_single_field_equals_no_fts() {
        let plan = compile_search_plan("tag=rust").expect("valid field filter should be Ok");
        assert_eq!(plan.fts_query, None);
        assert_eq!(
            plan.extracted_filters,
            vec![("tag".to_string(), "rust".to_string())]
        );
        assert!(plan.extracted_negated_filters.is_empty());
    }

    #[test]
    fn compile_colon_syntax_treated_same_as_equals() {
        let plan = compile_search_plan("tag:rust").expect("colon syntax should be Ok");
        assert_eq!(plan.fts_query, None);
        assert_eq!(
            plan.extracted_filters,
            vec![("tag".to_string(), "rust".to_string())]
        );
        assert!(plan.extracted_negated_filters.is_empty());
    }

    #[test]
    fn compile_dotted_value_preserved() {
        let plan =
            compile_search_plan("category=work.dev").expect("dotted value should be Ok");
        assert_eq!(plan.fts_query, None);
        assert_eq!(
            plan.extracted_filters,
            vec![("category".to_string(), "work.dev".to_string())]
        );
        assert!(plan.extracted_negated_filters.is_empty());
    }

    #[test]
    fn compile_and_of_two_field_equals() {
        // Explicit AND form.
        let plan = compile_search_plan("tag=rust AND category=work")
            .expect("AND of two field filters should be Ok");
        assert_eq!(plan.fts_query, None);
        // Assumption: extraction walks the AST AND children in order, preserving the
        // parser's flattened-child order (which follows input order for explicit AND).
        assert_eq!(
            plan.extracted_filters,
            vec![
                ("tag".to_string(), "rust".to_string()),
                ("category".to_string(), "work".to_string()),
            ]
        );
        assert!(plan.extracted_negated_filters.is_empty());

        // Implicit AND form should behave identically.
        let plan = compile_search_plan("tag=rust category=work")
            .expect("implicit AND of two field filters should be Ok");
        assert_eq!(plan.fts_query, None);
        assert_eq!(
            plan.extracted_filters,
            vec![
                ("tag".to_string(), "rust".to_string()),
                ("category".to_string(), "work".to_string()),
            ]
        );
        assert!(plan.extracted_negated_filters.is_empty());
    }

    #[test]
    fn compile_field_equals_and_text() {
        let plan = compile_search_plan("tag=rust meeting")
            .expect("field filter + text should be Ok");
        assert_eq!(
            plan.extracted_filters,
            vec![("tag".to_string(), "rust".to_string())]
        );
        assert!(plan.extracted_negated_filters.is_empty());
        assert_eq!(plan.fts_query, Some("meeting".to_string()));
    }

    #[test]
    fn compile_field_equals_and_multiple_text() {
        let plan = compile_search_plan("tag=rust meeting notes")
            .expect("field filter + multiple text terms should be Ok");
        assert_eq!(
            plan.extracted_filters,
            vec![("tag".to_string(), "rust".to_string())]
        );
        assert!(plan.extracted_negated_filters.is_empty());
        // The remaining two text terms should be rewrapped in an And and serialized
        // via to_fts_query, producing "meeting AND notes".
        let expected = to_fts_query(&SearchExpr::And(vec![
            SearchExpr::FullText("meeting".into()),
            SearchExpr::FullText("notes".into()),
        ]));
        assert_eq!(plan.fts_query, Some(expected));
    }

    #[test]
    fn compile_single_full_text_passes_through() {
        let plan = compile_search_plan("hello").expect("single word should be Ok");
        assert!(plan.extracted_filters.is_empty());
        assert!(plan.extracted_negated_filters.is_empty());
        assert_eq!(plan.fts_query, Some("hello".to_string()));
    }

    #[test]
    fn compile_not_field_equals_extracted_as_negated() {
        let plan =
            compile_search_plan("NOT tag=archive").expect("NOT field filter should be Ok");
        assert_eq!(plan.fts_query, None);
        assert!(plan.extracted_filters.is_empty());
        assert_eq!(
            plan.extracted_negated_filters,
            vec![("tag".to_string(), "archive".to_string())]
        );
    }

    #[test]
    fn compile_mixed_positive_and_not_field_equals() {
        let plan = compile_search_plan("tag=rust NOT tag=archive")
            .expect("positive + NOT field filter should be Ok");
        assert_eq!(plan.fts_query, None);
        assert_eq!(
            plan.extracted_filters,
            vec![("tag".to_string(), "rust".to_string())]
        );
        assert_eq!(
            plan.extracted_negated_filters,
            vec![("tag".to_string(), "archive".to_string())]
        );
    }

    #[test]
    fn compile_or_of_field_equals_not_decomposed() {
        let plan = compile_search_plan("tag=rust OR tag=svelte")
            .expect("OR of field filters should be Ok");
        // OR at the top level is NOT decomposed - filters flow through intact.
        assert!(plan.extracted_filters.is_empty());
        assert!(plan.extracted_negated_filters.is_empty());
        // Expected fts_query is whatever to_fts_query produces for the parsed OR AST.
        let parsed = parse("tag=rust OR tag=svelte").expect("parses");
        let expected = to_fts_query(&parsed);
        assert_eq!(plan.fts_query, Some(expected));
    }

    // Error path ────────────────────────────────────────────────────────

    #[test]
    fn compile_non_tag_negated_field_rejected() {
        let err = compile_search_plan("NOT url=example.com")
            .expect_err("NOT on non-tag field should be rejected");
        assert!(
            err.contains("NOT") && err.contains("tag"),
            "error message should mention NOT/tag limitation, got: {err}"
        );
    }

    #[test]
    fn compile_bare_asterisk_rejected() {
        let err = compile_search_plan("*").expect_err("bare * should be rejected");
        assert!(
            err.contains("wildcard"),
            "error message should mention 'wildcard', got: {err}"
        );
    }

    #[test]
    fn compile_bare_double_asterisk_rejected() {
        let err = compile_search_plan("**").expect_err("bare ** should be rejected");
        assert!(
            err.contains("wildcard"),
            "error message should mention 'wildcard', got: {err}"
        );
    }

    #[test]
    fn compile_bare_dot_asterisk_rejected() {
        let err = compile_search_plan(".*").expect_err("bare .* should be rejected");
        assert!(
            err.contains("wildcard"),
            "error message should mention 'wildcard', got: {err}"
        );
    }

    #[test]
    fn compile_unparseable_rejected() {
        let err =
            compile_search_plan(")))bad(((").expect_err("unparseable query should be rejected");
        assert!(
            err.contains("unparseable"),
            "error message should mention 'unparseable', got: {err}"
        );
    }

    #[test]
    fn compile_bare_and_operator_rejected() {
        let err = compile_search_plan("AND").expect_err("bare AND should be rejected");
        assert!(
            err.contains("unparseable"),
            "error message should mention 'unparseable', got: {err}"
        );
    }

    #[test]
    fn compile_bare_or_operator_rejected() {
        let err = compile_search_plan("OR").expect_err("bare OR should be rejected");
        assert!(
            err.contains("unparseable"),
            "error message should mention 'unparseable', got: {err}"
        );
    }

    #[test]
    fn compile_bare_not_operator_rejected() {
        let err = compile_search_plan("NOT").expect_err("bare NOT should be rejected");
        assert!(
            err.contains("unparseable"),
            "error message should mention 'unparseable', got: {err}"
        );
    }

    // Issue #6 group C2: error-class consistency. Invalid search inputs must
    // produce an error string starting with "invalid search query" (not
    // "internal error"). Integration coverage in section 18h pins this at the
    // GraphQL surface; this test pins it at the Rust level.
    #[test]
    fn error_class_consistency_issue_6_c2() {
        let bad_inputs = &["*", "**", ".*", "AND", "OR", "NOT", "(unbalanced"];
        for q in bad_inputs {
            let err = compile_search_plan(q);
            match err {
                Err(msg) => {
                    assert!(
                        msg.starts_with("invalid search query")
                            || msg.contains("unparseable"),
                        "bad input {q:?} should produce 'invalid search query' or \
                         'unparseable' error, got: {msg}"
                    );
                }
                Ok(_) => {
                    // Some inputs may be valid after all (NOT, OR alone could
                    // parse differently). If compile_search_plan accepts them,
                    // that's fine; we're only pinning that rejections use the
                    // right error class.
                }
            }
        }
    }

    // Issue #6 group C1: the contract is that every input compile_search_plan
    // accepts produces a normalized form compile_search_plan also accepts. PRD
    // 00121 fixed the original inconsistency where normalizeSearchQuery
    // accepted `tag=rust` while search() rejected it. This test pins the
    // contract at the unit level in addition to the GraphQL-surface check
    // already in integration.sh section 18h.
    #[test]
    fn normalize_and_search_accept_same_inputs_issue_6_c1() {
        // Curated inputs covering the patterns jink uses + the bugs from #6.
        // Each input must (a) compile as-is and (b) still compile after
        // passing through normalize().
        let cases = &[
            "tag=rust",
            "category=work.dev",
            "tag:rust",
            "Hello World",
            "rust AND crdt",
            "rust OR python",
            "NOT tag=python",
            "tag=rust AND category=work.dev",
            "\"phrase query\"",
            "meeting minutes",
            "(a OR b) AND c",
        ];

        for q in cases {
            let raw = compile_search_plan(q);
            assert!(
                raw.is_ok(),
                "compile_search_plan rejected curated input {q:?}: {raw:?}"
            );
            let normalized = normalize(q);
            let round_trip = compile_search_plan(&normalized);
            assert!(
                round_trip.is_ok(),
                "normalized form {normalized:?} of {q:?} round-trips into a rejection: {round_trip:?}"
            );
        }
    }

    // ── validate_and_compile ───────────────────────────────────────

    #[test]
    fn validate_and_compile_valid_query() {
        let plan = validate_and_compile("hello world").unwrap();
        assert!(plan.fts_query.is_some());
    }

    #[test]
    fn validate_and_compile_empty_string() {
        let err = validate_and_compile("").unwrap_err();
        assert!(err.contains("invalid search query"), "got: {err}");
    }

    #[test]
    fn validate_and_compile_whitespace_only() {
        let err = validate_and_compile("   ").unwrap_err();
        assert!(err.contains("invalid search query"), "got: {err}");
    }

    #[test]
    fn validate_and_compile_bare_wildcard() {
        let err = validate_and_compile("*").unwrap_err();
        assert!(err.contains("bare wildcard"), "got: {err}");
    }

    #[test]
    fn validate_and_compile_non_tag_negation() {
        let err = validate_and_compile("NOT url=example.com").unwrap_err();
        assert!(err.contains("NOT is only supported for tag"), "got: {err}");
    }

    #[test]
    fn validate_and_compile_field_only() {
        let plan = validate_and_compile("tag=rust").unwrap();
        assert!(plan.fts_query.is_none());
        assert_eq!(plan.extracted_filters.len(), 1);
        assert_eq!(plan.extracted_filters[0], ("tag".into(), "rust".into()));
    }

    #[test]
    fn validate_and_compile_mixed_query() {
        let plan = validate_and_compile("hello tag=rust").unwrap();
        assert!(plan.fts_query.is_some());
        assert_eq!(plan.extracted_filters.len(), 1);
    }
}
