/// Search expression AST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchExpr {
    FullText(String),
    FieldEquals { field: String, value: String },
    And(Vec<SearchExpr>),
    Or(Vec<SearchExpr>),
    Not(Box<SearchExpr>),
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

// ── Public API ──────────────────────────────────────────────────────

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
}
