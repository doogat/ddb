use std::fmt;

use super::doogat::Zone;

#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,
    pub references: Option<String>,
    pub zone: Option<Zone>,
    pub required: bool,
    pub search_boost: Option<f64>,
    pub allowed_values: Option<Vec<String>>,
    pub default_value: Option<String>,
}

impl ColumnDef {
    /// Resolve the effective zone, falling back to type-based inference.
    pub fn effective_zone(&self) -> Zone {
        if let Some(ref zone) = self.zone {
            return zone.clone();
        }
        if self.references.is_some() {
            Zone::Reference
        } else if self.is_numeric_or_short_string() {
            Zone::Frontmatter
        } else {
            Zone::Body
        }
    }

    fn is_numeric_or_short_string(&self) -> bool {
        let upper = self.data_type.to_uppercase();
        if matches!(
            upper.as_str(),
            "INTEGER" | "REAL" | "BOOLEAN" | "CHAR" | "TINYTEXT" | "VARCHAR"
        ) {
            return true;
        }
        if upper.starts_with("CHAR(") {
            return true;
        }
        if let Some(rest) = upper.strip_prefix("VARCHAR(") {
            if let Some(num_str) = rest.strip_suffix(')') {
                return num_str.parse::<u64>().is_ok_and(|n| n <= 255);
            }
        }
        if self.allowed_values.is_some() {
            return true;
        }
        false
    }
}

#[derive(Debug, Clone)]
pub struct TableSchema {
    pub table_name: String,
    pub columns: Vec<ColumnDef>,
    pub crdt_strategy: Option<String>,
    pub template_sections: Vec<String>,
    pub folder: bool,
    pub stale_after_days: Option<u32>,
    pub title_template: Option<String>,
    pub origin: Option<String>,
    pub unique_together: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Clone)]
pub enum ConsistencyWarning {
    MalformedYaml {
        path: String,
        error: String,
    },
    CrossZoneDuplicate {
        path: String,
        key: String,
    },
    MissingRequired {
        path: String,
        type_name: String,
        field: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct RebuildReport {
    pub indexed: usize,
    pub tables_materialized: usize,
    pub types_inferred: Vec<String>,
    pub warnings: Vec<ConsistencyWarning>,
}

// ── Consistency auto-fix types ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum TitleSource {
    FirstH1(String),
    Filename(String),
}

#[derive(Debug, Clone)]
pub enum Fix {
    TagsDeduped { removed: Vec<String> },
    TagsSorted,
    TagsStrippedHash { tags: Vec<String> },
    DefaultSet { field: String, value: String },
    TitleDerived { source: TitleSource },
    KeyNormalized { old: String, new: String },
    TitleTrimmed,
    TitleCapitalized,
    H1Aligned { old_h1: String, new_h1: String },
    CrossZoneResolved { key: String, kept_zone: Zone },
    FieldRenamed { old: String, new: String },
    TypeNormalized { old: String, new: String },
    ManualTypedef { type_name: String },
    TitleNonCompliant { expected: String },
    ZoneMigrated { column: String, from: Zone, to: Zone },
}

impl Fix {
    pub fn severity(&self) -> Severity {
        match self {
            Fix::CrossZoneResolved { .. } => Severity::Error,
            Fix::DefaultSet { .. }
            | Fix::TitleDerived { .. }
            | Fix::FieldRenamed { .. }
            | Fix::TitleNonCompliant { .. }
            | Fix::ZoneMigrated { .. } => Severity::Warning,
            Fix::TagsDeduped { .. }
            | Fix::TagsSorted
            | Fix::TagsStrippedHash { .. }
            | Fix::KeyNormalized { .. }
            | Fix::TitleTrimmed
            | Fix::TitleCapitalized
            | Fix::H1Aligned { .. }
            | Fix::TypeNormalized { .. }
            | Fix::ManualTypedef { .. } => Severity::Info,
        }
    }
}

impl fmt::Display for Fix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fix::TagsDeduped { removed } => write!(f, "deduplicated tags: {}", removed.join(", ")),
            Fix::TagsSorted => write!(f, "sorted tags"),
            Fix::TagsStrippedHash { tags } => {
                write!(f, "stripped # from tags: {}", tags.join(", "))
            }
            Fix::DefaultSet { field, value } => write!(f, "set default {field}: {value}"),
            Fix::TitleDerived { source } => match source {
                TitleSource::FirstH1(h) => write!(f, "derived title from H1: {h}"),
                TitleSource::Filename(n) => write!(f, "derived title from filename: {n}"),
            },
            Fix::KeyNormalized { old, new } => write!(f, "normalized key {old} -> {new}"),
            Fix::TitleTrimmed => write!(f, "trimmed title"),
            Fix::TitleCapitalized => write!(f, "capitalized title"),
            Fix::H1Aligned { old_h1, new_h1 } => {
                write!(f, "aligned H1: {old_h1} -> {new_h1}")
            }
            Fix::CrossZoneResolved { key, kept_zone } => {
                write!(
                    f,
                    "resolved cross-zone duplicate: {key} (kept {kept_zone:?})"
                )
            }
            Fix::FieldRenamed { old, new } => write!(f, "renamed field {old} -> {new}"),
            Fix::TypeNormalized { old, new } => write!(f, "normalized type {old} -> {new}"),
            Fix::ManualTypedef { type_name } => write!(
                f,
                "manual typedef '{type_name}' — consider recreating with: ddb query \"CREATE TABLE {type_name} (...)\""
            ),
            Fix::TitleNonCompliant { expected } => {
                write!(f, "title does not match template (expected: {expected})")
            }
            Fix::ZoneMigrated { column, from, to } => {
                write!(f, "migrated column '{column}' from {from:?} to {to:?}")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct DoogatFix {
    pub path: String,
    pub applied: Vec<Fix>,
}

#[derive(Debug, Clone, Default)]
pub struct FixReport {
    pub files_scanned: usize,
    pub files_fixed: usize,
    pub fixes: Vec<DoogatFix>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_zone_varchar_length_cap() {
        let col = |dt: &str| ColumnDef {
            name: "x".into(),
            data_type: dt.into(),
            references: None,
            zone: None,
            required: false,
            search_boost: None,
            allowed_values: None,
            default_value: None,
        };
        assert_eq!(col("VARCHAR").effective_zone(), Zone::Frontmatter);
        assert_eq!(col("VARCHAR(100)").effective_zone(), Zone::Frontmatter);
        assert_eq!(col("VARCHAR(255)").effective_zone(), Zone::Frontmatter);
        assert_eq!(col("VARCHAR(256)").effective_zone(), Zone::Body);
        assert_eq!(col("VARCHAR(1000)").effective_zone(), Zone::Body);
        assert_eq!(col("INTEGER").effective_zone(), Zone::Frontmatter);
        assert_eq!(col("TEXT").effective_zone(), Zone::Body);
    }
}
