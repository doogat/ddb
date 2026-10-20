use regex::Regex;
use sqlparser::ast::Expr;
use std::sync::OnceLock;

use crate::error::{DoogatError, Result};

// --- Regex statics ---

pub(crate) fn re_set_zone() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+SET\s+ZONE\s+(frontmatter|body|reference)\s+FOR\s+(?:"([^"]+)"|(\w[\w-]*))\s*;?\s*$"#).expect("valid regex")
    })
}

pub(crate) fn re_set_title_template() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+SET\s+TITLE\s+TEMPLATE\s+'([^']+)'\s*;?\s*$"#).expect("valid regex")
    })
}

pub(crate) fn re_drop_title_template() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+DROP\s+TITLE\s+TEMPLATE\s*;?\s*$"#).expect("valid regex")
    })
}

pub(crate) fn re_set_search_key() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+SET\s+SEARCH\s+KEY\s+(?:"([^"]+)"|(\w[\w-]*))\s*;?\s*$"#).expect("valid regex")
    })
}

pub(crate) fn re_drop_search_key() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+DROP\s+SEARCH\s+KEY\s*;?\s*$"#,
        )
        .expect("valid regex")
    })
}

/// PRD 00139 §2: detect the trailing `SINGLETON [DEFAULT VALUES]` marker on
/// a CREATE TABLE source string. Anchored to the closing paren of the
/// column list so the marker can only appear after the table body, not
/// inside a column name.
///
/// Match group 1 captures the `DEFAULT VALUES` suffix when present so the
/// caller can branch on whether to auto-seed (T7).
pub(crate) fn re_create_table_singleton() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\)\s*SINGLETON(\s+DEFAULT\s+VALUES)?\s*;?\s*$"#).expect("valid regex")
    })
}

/// PRD 00139 §8: `ALTER TABLE x SET SINGLETON` matcher. Mirrors the
/// quoted/bare table-name capture shape of `re_set_search_key`.
pub(crate) fn re_set_singleton() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+SET\s+SINGLETON\s*;?\s*$"#)
            .expect("valid regex")
    })
}

/// PRD 00139 §8: `ALTER TABLE x DROP SINGLETON` matcher.
pub(crate) fn re_drop_singleton() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+DROP\s+SINGLETON\s*;?\s*$"#,
        )
        .expect("valid regex")
    })
}

/// `ALTER TABLE x ADD UNIQUE (a, b)` matcher. Group 3 is the column list.
pub(crate) fn re_add_unique() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+ADD\s+UNIQUE\s*\(([^)]+)\)\s*;?\s*$"#,
        )
        .expect("valid regex")
    })
}

/// `ALTER TABLE x DROP UNIQUE (a, b)` matcher. Group 3 is the column list.
pub(crate) fn re_drop_unique() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+DROP\s+UNIQUE\s*\(([^)]+)\)\s*;?\s*$"#,
        )
        .expect("valid regex")
    })
}

pub(crate) fn re_unfilled_placeholder() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{[^}]+\}").expect("valid regex"))
}

// --- WHERE clause extraction ---

pub(crate) fn extract_where_id(selection: &Option<Expr>) -> Result<String> {
    match selection {
        Some(Expr::BinaryOp { left, op, right }) => {
            if format!("{op}") != "=" {
                return Err(DoogatError::SqlEngine(
                    "only WHERE id = '<value>' supported".into(),
                ));
            }
            let col = match left.as_ref() {
                Expr::Identifier(ident) => ident.value.to_lowercase(),
                _ => {
                    return Err(DoogatError::SqlEngine(
                        "WHERE clause must be id = '<value>'".into(),
                    ))
                }
            };
            if col != "id" {
                return Err(DoogatError::SqlEngine(
                    "only WHERE id = '<value>' supported".into(),
                ));
            }
            super::eval::expr_to_string(right)
        }
        _ => Err(DoogatError::SqlEngine(
            "WHERE id = '<value>' required".into(),
        )),
    }
}

/// Try to parse an expression as `column = 'value'`, returning (column_name, value).
fn extract_eq_pair(expr: &Expr) -> Option<(String, String)> {
    let Expr::BinaryOp { left, op, right } = expr else {
        return None;
    };
    if format!("{op}") != "=" {
        return None;
    }
    let Expr::Identifier(ident) = left.as_ref() else {
        return None;
    };
    Some((
        ident.value.to_lowercase(),
        super::eval::expr_to_string(right).ok()?,
    ))
}

/// Extract two column values from a WHERE clause like
/// `{col1} = 'val1' AND {col2} = 'val2'`.
pub(crate) fn extract_junction_where(
    selection: &Option<Expr>,
    col1_name: &str,
    col2_name: &str,
) -> Result<(String, String)> {
    let err = || {
        DoogatError::SqlEngine(format!(
            "junction DELETE requires WHERE {col1_name} = '...' AND {col2_name} = '...'"
        ))
    };

    let Some(Expr::BinaryOp { left, op, right }) = selection else {
        return Err(err());
    };
    if format!("{op}") != "AND" {
        return Err(err());
    }

    let mut val1 = None;
    let mut val2 = None;
    for side in [left.as_ref(), right.as_ref()] {
        if let Some((col, val)) = extract_eq_pair(side) {
            if col == col1_name {
                val1 = Some(val);
            } else if col == col2_name {
                val2 = Some(val);
            }
        }
    }

    match (val1, val2) {
        (Some(v1), Some(v2)) => Ok((v1, v2)),
        _ => Err(err()),
    }
}
