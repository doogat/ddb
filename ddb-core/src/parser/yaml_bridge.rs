use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::error::DoogatError;
use crate::types::{DoogatId, DoogatMeta, Value};

/// Shared regex for fenced code block markers (``` or ~~~).
pub(super) fn fence_regex() -> &'static Regex {
    static FENCE_RE: OnceLock<Regex> = OnceLock::new();
    FENCE_RE.get_or_init(|| Regex::new(r"^(?:`{3,}|~{3,})").expect("valid regex: fence marker"))
}

/// Shared regex for inline code spans (`...`).
pub(super) fn inline_code_regex() -> &'static Regex {
    static INLINE_CODE_RE: OnceLock<Regex> = OnceLock::new();
    INLINE_CODE_RE.get_or_init(|| Regex::new(r"`[^`]+`").expect("valid regex: inline code"))
}

impl From<serde_yaml::Error> for DoogatError {
    fn from(e: serde_yaml::Error) -> Self {
        Self::Yaml(e.to_string())
    }
}

/// Internal struct for YAML deserialization. Converts to public DoogatMeta at the boundary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct RawDoogatMeta {
    pub id: Option<DoogatId>,
    pub title: Option<String>,
    pub date: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub doogat_type: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

impl From<RawDoogatMeta> for DoogatMeta {
    fn from(raw: RawDoogatMeta) -> Self {
        DoogatMeta {
            id: raw.id,
            title: raw.title,
            date: raw.date,
            doogat_type: raw.doogat_type,
            tags: raw.tags,
            extra: raw
                .extra
                .into_iter()
                .map(|(k, v)| (k, from_serde_yaml(v)))
                .collect(),
        }
    }
}

pub(crate) fn from_serde_yaml(v: serde_yaml::Value) -> Value {
    match v {
        serde_yaml::Value::String(s) => Value::String(s),
        serde_yaml::Value::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        serde_yaml::Value::Bool(b) => Value::Bool(b),
        serde_yaml::Value::Sequence(seq) => {
            Value::List(seq.into_iter().map(from_serde_yaml).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let m = map
                .into_iter()
                .filter_map(|(k, v)| k.as_str().map(|ks| (ks.to_string(), from_serde_yaml(v))))
                .collect();
            Value::Map(m)
        }
        serde_yaml::Value::Null | serde_yaml::Value::Tagged(_) => Value::String(String::new()),
    }
}

pub(super) fn to_serde_yaml(v: &Value) -> serde_yaml::Value {
    match v {
        Value::String(s) => serde_yaml::Value::String(s.clone()),
        Value::Number(n) => serde_yaml::Value::Number(serde_yaml::Number::from(*n)),
        Value::Bool(b) => serde_yaml::Value::Bool(*b),
        Value::List(list) => serde_yaml::Value::Sequence(list.iter().map(to_serde_yaml).collect()),
        Value::Map(map) => {
            let m: serde_yaml::Mapping = map
                .iter()
                .map(|(k, v)| (serde_yaml::Value::String(k.clone()), to_serde_yaml(v)))
                .collect();
            serde_yaml::Value::Mapping(m)
        }
    }
}

/// Quote a YAML string value if it contains special characters.
pub(crate) fn yaml_quote(s: &str) -> String {
    if s.contains(':')
        || s.contains('[')
        || s.contains(']')
        || s.contains('{')
        || s.contains('}')
        || s.contains('#')
        || s.contains("[[")
    {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Serialize a YAML value as a frontmatter field, handling complex types (sequences, mappings)
/// with proper block-style indentation.
pub(super) fn serialize_yaml_value(out: &mut String, key: &str, value: &serde_yaml::Value) {
    match value {
        serde_yaml::Value::Sequence(_) | serde_yaml::Value::Mapping(_) => {
            // Use serde_yaml to serialize the full key-value pair as a YAML mapping,
            // then strip the trailing newline and append.
            let mut map = serde_yaml::Mapping::new();
            map.insert(serde_yaml::Value::String(key.into()), value.clone());
            let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(map)).unwrap_or_default();
            out.push_str(&yaml);
        }
        _ => {
            let yaml_val = serde_yaml::to_string(value).unwrap_or_default();
            let yaml_val = yaml_val.trim().trim_end_matches('\n');
            out.push_str(&format!("{key}: {yaml_val}\n"));
        }
    }
}
