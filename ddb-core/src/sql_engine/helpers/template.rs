use regex::Regex;
use std::sync::OnceLock;

use crate::error::{DoogatError, Result};

fn re_title_placeholder() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{([^{}]*)\}").expect("valid regex"))
}

pub(crate) fn is_safe_sql_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A placeholder token parsed from a `title_template`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplatePlaceholder {
    /// The full placeholder string including braces, e.g. `"{col.field}"`.
    pub raw: String,
    /// The column segment (before the dot, or the whole token if no dot).
    pub col: String,
    /// The field segment when the placeholder uses dotted form.
    pub field: Option<String>,
}

/// Parse all `{col}` and `{col.field}` placeholders from a `title_template`.
///
/// Rejects multi-hop paths (`{a.b.c}`) and malformed identifiers. Returns
/// placeholders in the order they appear in the template.
pub fn parse_title_template(tmpl: &str) -> Result<Vec<TemplatePlaceholder>> {
    let mut out = Vec::new();
    for cap in re_title_placeholder().captures_iter(tmpl) {
        let raw = cap.get(0).expect("regex group 0").as_str().to_string();
        let inner = cap.get(1).expect("regex group 1").as_str();
        let parts: Vec<&str> = inner.split('.').collect();
        match parts.as_slice() {
            [col] => {
                if !is_safe_sql_identifier(col) {
                    return Err(DoogatError::SqlEngine(format!(
                        "title_template has malformed placeholder {raw}"
                    )));
                }
                out.push(TemplatePlaceholder {
                    raw,
                    col: (*col).to_string(),
                    field: None,
                });
            }
            [col, field] => {
                if !is_safe_sql_identifier(col) || !is_safe_sql_identifier(field) {
                    return Err(DoogatError::SqlEngine(format!(
                        "title_template has malformed placeholder {raw}"
                    )));
                }
                out.push(TemplatePlaceholder {
                    raw,
                    col: (*col).to_string(),
                    field: Some((*field).to_string()),
                });
            }
            _ => {
                return Err(DoogatError::SqlEngine(format!(
                    "title_template {raw} uses multi-hop path; only one-level REFERENCES dereferencing is supported"
                )));
            }
        }
    }
    Ok(out)
}
