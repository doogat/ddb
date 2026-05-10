use std::fmt;

use super::doogat::Zone;

/// SQL `ON DELETE` action for a `REFERENCES` column. Default is
/// [`OnDeleteAction::Restrict`] — the parent delete is rejected while
/// referencing rows exist (issue #10 / commit 5a55296). PRD 00129 §2 adds
/// [`OnDeleteAction::Cascade`] as an opt-in.
///
/// `SET NULL`, `SET DEFAULT`, and `ON UPDATE` are out of scope for v1
/// per PRD 00129 §Out of scope; the DDL parser rejects them with a
/// clear message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OnDeleteAction {
    #[default]
    Restrict,
    Cascade,
}

impl OnDeleteAction {
    /// Stable lowercase identifier used in the typedef YAML serialization.
    pub fn as_typedef_str(self) -> &'static str {
        match self {
            OnDeleteAction::Restrict => "restrict",
            OnDeleteAction::Cascade => "cascade",
        }
    }

    /// Parse the typedef YAML form. Unknown strings fall back to RESTRICT
    /// to preserve the safer default on legacy / malformed typedefs.
    pub fn parse_typedef_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "cascade" => OnDeleteAction::Cascade,
            _ => OnDeleteAction::Restrict,
        }
    }
}

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
    /// PRD 00129 §2: `ON DELETE` action for a REFERENCES column. Defaults
    /// to RESTRICT (the existing #10 behavior). Only meaningful when
    /// `references.is_some()`.
    pub on_delete: OnDeleteAction,
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
    /// Column to match against in `field=val` substring searches that
    /// resolve through this typedef. `None` falls back to `title`. Set via
    /// `ALTER TABLE <name> SET SEARCH KEY <col>` and reset via
    /// `ALTER TABLE <name> DROP SEARCH KEY` (ddb#15 follow-up: jink
    /// categories use `fqn` as the user-facing identifier rather than the
    /// leaf `title`).
    pub search_key: Option<String>,
    /// PRD 00139 §1: SINGLETON typedef flag. When `true`, the typedef may
    /// hold at most one materialized row. Enforced by three layers
    /// (validator + SQL DML pre-check + materializer UNIQUE index) and
    /// surfaces a per-type singular GraphQL query field plus
    /// `update<Type>` / `upsert<Type>` mutations. Defaults to `false`;
    /// set via `CREATE TABLE x (...) SINGLETON` or
    /// `ALTER TABLE x SET SINGLETON`, cleared via
    /// `ALTER TABLE x DROP SINGLETON`. Serialized into typedef YAML only
    /// when `true` so non-singleton typedefs remain byte-identical.
    pub singleton: bool,
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
    TagsDeduped {
        removed: Vec<String>,
    },
    TagsSorted,
    TagsStrippedHash {
        tags: Vec<String>,
    },
    DefaultSet {
        field: String,
        value: String,
    },
    TitleDerived {
        source: TitleSource,
    },
    KeyNormalized {
        old: String,
        new: String,
    },
    TitleTrimmed,
    TitleCapitalized,
    H1Aligned {
        old_h1: String,
        new_h1: String,
    },
    CrossZoneResolved {
        key: String,
        kept_zone: Zone,
    },
    FieldRenamed {
        old: String,
        new: String,
    },
    TypeNormalized {
        old: String,
        new: String,
    },
    ManualTypedef {
        type_name: String,
    },
    TitleNonCompliant {
        expected: String,
    },
    ZoneMigrated {
        column: String,
        from: Zone,
        to: Zone,
    },
    SingletonConflictResolved {
        table: String,
        winner: String,
        losers: Vec<String>,
    },
}

impl Fix {
    pub fn severity(&self) -> Severity {
        match self {
            Fix::CrossZoneResolved { .. } => Severity::Error,
            Fix::DefaultSet { .. }
            | Fix::TitleDerived { .. }
            | Fix::FieldRenamed { .. }
            | Fix::TitleNonCompliant { .. }
            | Fix::ZoneMigrated { .. }
            | Fix::SingletonConflictResolved { .. } => Severity::Warning,
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
            Fix::SingletonConflictResolved {
                table,
                winner,
                losers,
            } => write!(
                f,
                "resolved singleton conflict in {table}: kept {winner}, quarantined {}",
                losers.join(", ")
            ),
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
            on_delete: OnDeleteAction::Restrict,
        };
        assert_eq!(col("VARCHAR").effective_zone(), Zone::Frontmatter);
        assert_eq!(col("VARCHAR(100)").effective_zone(), Zone::Frontmatter);
        assert_eq!(col("VARCHAR(255)").effective_zone(), Zone::Frontmatter);
        assert_eq!(col("VARCHAR(256)").effective_zone(), Zone::Body);
        assert_eq!(col("VARCHAR(1000)").effective_zone(), Zone::Body);
        assert_eq!(col("INTEGER").effective_zone(), Zone::Frontmatter);
        assert_eq!(col("TEXT").effective_zone(), Zone::Body);
    }

    #[test]
    fn singleton_field_defaults_false_on_new_schema() {
        // PRD 00139 §1: every freshly constructed TableSchema starts non-singleton.
        let schema = TableSchema {
            table_name: "x".into(),
            columns: vec![],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
            unique_together: None,
            search_key: None,
            singleton: false,
        };
        assert!(!schema.singleton);
    }
}
