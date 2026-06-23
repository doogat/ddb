use thiserror::Error;

pub type Result<T> = std::result::Result<T, DoogatError>;

/// A value attached to a structured error's extensions. Kept JSON-free so
/// `ddb-core` doesn't pull `serde_json` into its required dependency surface
/// (it's optional today, only enabled by the `nosql` feature). The server
/// layer converts to JSON at the GraphQL boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorValue {
    String(String),
    List(Vec<String>),
}

impl ErrorValue {
    pub fn str(s: impl Into<String>) -> Self {
        ErrorValue::String(s.into())
    }

    pub fn list<I, S>(items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        ErrorValue::List(items.into_iter().map(Into::into).collect())
    }
}

/// Ordered key-value extensions attached to a [`DoogatError::Structured`]
/// variant. Order is preserved because per-code field ordering is part of
/// the documented PRD 00129 §6 contract.
pub type ErrorContext = Vec<(String, ErrorValue)>;

#[derive(Debug, Error)]
pub enum DoogatError {
    #[error("git: {0}")]
    Git(String),

    #[error("yaml: {0}")]
    Yaml(String),

    #[error("sql: {0}")]
    Sql(String),

    #[error("automerge: {0}")]
    Automerge(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml: {0}")]
    Toml(String),

    #[error("parse: {0}")]
    Parse(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("validation: {0}")]
    Validation(String),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("sql engine: {0}")]
    SqlEngine(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("sync: {0}")]
    Sync(String),

    #[error("index: {0}")]
    Index(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("version mismatch: repo format v{repo}, driver supports up to v{driver}")]
    VersionMismatch { repo: u32, driver: u32 },

    #[cfg(feature = "nosql")]
    #[error("redb: {0}")]
    Redb(String),

    /// Structured error carrying a stable machine-readable code plus
    /// per-code context fields. Defined in PRD 00129 §6. The `message`
    /// reproduces the legacy English wording (e.g. `"NOT NULL constraint
    /// violated: <table>.<column>"`) so callers still matching messages
    /// don't break; the `code` and `context` are additive.
    #[error("{message}")]
    Structured {
        code: &'static str,
        message: String,
        context: ErrorContext,
    },
}

/// Stable error codes from PRD 00129 §6. Use the constants when matching
/// rather than string literals so a typo is a compile error.
pub mod codes {
    pub const UNIQUE_VIOLATION: &str = "UNIQUE_VIOLATION";
    pub const REFERENCES_VIOLATION: &str = "REFERENCES_VIOLATION";
    pub const NOT_NULL_VIOLATION: &str = "NOT_NULL_VIOLATION";
    pub const UNKNOWN_FIELD: &str = "UNKNOWN_FIELD";
    pub const TYPE_NOT_REGISTERED: &str = "TYPE_NOT_REGISTERED";
    pub const CASCADE_CYCLE: &str = "CASCADE_CYCLE";
    /// PRD 00139 §5: second INSERT into a SINGLETON typedef.
    pub const SINGLETON_VIOLATION: &str = "SINGLETON_VIOLATION";
    /// PRD 00139 §5: typed `update<Type>` against an empty SINGLETON typedef.
    pub const SINGLETON_NOT_FOUND: &str = "SINGLETON_NOT_FOUND";
    /// PRD 00161 §3.5: a multi-typedef schema apply failed partway; the
    /// already-applied ops are listed in the error context.
    pub const SCHEMA_APPLY_PARTIAL: &str = "SCHEMA_APPLY_PARTIAL";
    /// PRD 00161 §3.5: a schema plan contains destructive ops (drop/rename)
    /// and `allow_destructive` was not set.
    pub const SCHEMA_DESTRUCTIVE_BLOCKED: &str = "SCHEMA_DESTRUCTIVE_BLOCKED";
}

impl DoogatError {
    /// `UNIQUE(col, ...)` constraint violation. The message reproduces
    /// SQLite's `"UNIQUE constraint failed: <table>.<col>[, <table>.<col>]..."`
    /// format so callers like jink that today match on
    /// `msg.contains("UNIQUE constraint")` keep working; the structured
    /// `extensions.code` and per-column context replace string matching.
    pub fn unique_violation(
        table: impl Into<String>,
        columns: impl IntoIterator<Item = impl Into<String>>,
        values: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let table = table.into();
        let cols: Vec<String> = columns.into_iter().map(Into::into).collect();
        let vals: Vec<String> = values.into_iter().map(Into::into).collect();
        let qualified = cols
            .iter()
            .map(|c| format!("{table}.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        DoogatError::Structured {
            code: codes::UNIQUE_VIOLATION,
            message: format!("UNIQUE constraint failed: {qualified}"),
            context: vec![
                ("table".into(), ErrorValue::String(table)),
                ("columns".into(), ErrorValue::List(cols)),
                ("values".into(), ErrorValue::List(vals)),
            ],
        }
    }

    /// `NOT NULL REFERENCES` parent delete blocked by RESTRICT (or attempted
    /// against a column whose `ON DELETE` action is RESTRICT). Message
    /// matches the existing wording from `materialize.rs::check_restrict_blocks_delete`.
    pub fn references_violation(
        table: impl Into<String>,
        column: impl Into<String>,
        referencing_table: impl Into<String>,
        referencing_id: impl Into<String>,
    ) -> Self {
        let table = table.into();
        let column = column.into();
        let referencing_table = referencing_table.into();
        let referencing_id = referencing_id.into();
        DoogatError::Structured {
            code: codes::REFERENCES_VIOLATION,
            message: format!(
                "cannot delete '{table}': NOT NULL REFERENCES from {referencing_table}.{column} in row '{referencing_id}'"
            ),
            context: vec![
                ("table".into(), ErrorValue::String(table)),
                ("column".into(), ErrorValue::String(column)),
                (
                    "referencing_table".into(),
                    ErrorValue::String(referencing_table),
                ),
                (
                    "referencing_id".into(),
                    ErrorValue::String(referencing_id),
                ),
            ],
        }
    }

    /// INSERT-time dangling FK: a typed column with `REFERENCES <type>` was
    /// supplied a value that doesn't match any row in the target type's
    /// materialized table. Distinct from `references_violation`, which
    /// covers delete-time RESTRICT blocking.
    pub fn dangling_reference(
        table: impl Into<String>,
        column: impl Into<String>,
        target_table: impl Into<String>,
        target_id: impl Into<String>,
    ) -> Self {
        let table = table.into();
        let column = column.into();
        let target_table = target_table.into();
        let target_id = target_id.into();
        DoogatError::Structured {
            code: codes::REFERENCES_VIOLATION,
            message: format!(
                "{table}.{column} references non-existent {target_table} '{target_id}'"
            ),
            context: vec![
                ("table".into(), ErrorValue::String(table)),
                ("column".into(), ErrorValue::String(column)),
                ("target_table".into(), ErrorValue::String(target_table)),
                ("target_id".into(), ErrorValue::String(target_id)),
            ],
        }
    }

    /// Required column missing on INSERT, or `SET col = NULL` on a NOT NULL
    /// column on UPDATE. Message matches the PRD 00122 wording emitted from
    /// `dml.rs::check_not_null`.
    pub fn not_null_violation(table: impl Into<String>, column: impl Into<String>) -> Self {
        let table = table.into();
        let column = column.into();
        DoogatError::Structured {
            code: codes::NOT_NULL_VIOLATION,
            message: format!("NOT NULL constraint violated: {table}.{column}"),
            context: vec![
                ("table".into(), ErrorValue::String(table)),
                ("column".into(), ErrorValue::String(column)),
            ],
        }
    }

    /// `fields` JSON has a key that's not in the typedef columns. Message
    /// matches the PRD 00122 wording from `dml.rs::check_unknown_columns`
    /// so pre-existing match-on-message callers keep working.
    pub fn unknown_field(table: impl Into<String>, field: impl Into<String>) -> Self {
        let table = table.into();
        let field = field.into();
        DoogatError::Structured {
            code: codes::UNKNOWN_FIELD,
            message: format!("unknown column: {table}.{field}"),
            context: vec![
                ("table".into(), ErrorValue::String(table)),
                ("unknown_field".into(), ErrorValue::String(field)),
            ],
        }
    }

    /// `input.type` references a typedef name that doesn't exist. PRD 00129
    /// §1 — replaces the previous "silently create only the base row"
    /// behavior with an explicit rejection.
    pub fn type_not_registered(type_name: impl Into<String>) -> Self {
        let type_name = type_name.into();
        DoogatError::Structured {
            code: codes::TYPE_NOT_REGISTERED,
            message: format!("type \"{type_name}\" is not a registered typedef"),
            context: vec![("type".into(), ErrorValue::String(type_name))],
        }
    }

    /// CASCADE delete graph contains a cycle. Lists the tables involved in
    /// the cycle so the caller can fix the typedef. PRD 00129 §2.
    pub fn cascade_cycle(tables: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let tables: Vec<String> = tables.into_iter().map(Into::into).collect();
        let joined = tables.join(", ");
        DoogatError::Structured {
            code: codes::CASCADE_CYCLE,
            message: format!("cascade delete would form a cycle through {joined}"),
            context: vec![("tables".into(), ErrorValue::List(tables))],
        }
    }

    /// PRD 00139 §5: a second INSERT into a SINGLETON typedef. Carries the
    /// existing row's id so the caller can decide whether to switch to
    /// `update<Type>` / `upsert<Type>`. Mirrors the `unique_violation` shape
    /// (`code` + `table` context) but adds `existing_id` since SINGLETON
    /// always identifies one specific blocker row.
    pub fn singleton_violation(table: impl Into<String>, existing_id: impl Into<String>) -> Self {
        let table = table.into();
        let existing_id = existing_id.into();
        DoogatError::Structured {
            code: codes::SINGLETON_VIOLATION,
            message: format!(
                "SINGLETON constraint violated: {table} already holds row {existing_id}"
            ),
            context: vec![
                ("table".into(), ErrorValue::String(table)),
                ("existing_id".into(), ErrorValue::String(existing_id)),
            ],
        }
    }

    /// PRD 00139 §5: typed `update<Type>` (no id) hit an empty SINGLETON
    /// typedef. Caller should switch to `upsert<Type>` or first
    /// `create<Type>`. Distinct from `NotFound(String)` so GraphQL clients
    /// can branch on `extensions.code`.
    pub fn singleton_not_found(table: impl Into<String>) -> Self {
        let table = table.into();
        DoogatError::Structured {
            code: codes::SINGLETON_NOT_FOUND,
            message: format!("SINGLETON typedef {table} has no row to update"),
            context: vec![("table".into(), ErrorValue::String(table))],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_get<'a>(err: &'a DoogatError, key: &str) -> &'a ErrorValue {
        let DoogatError::Structured { context, .. } = err else {
            panic!("expected Structured variant, got: {err:?}");
        };
        context
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("missing context key '{key}' in {err:?}"))
    }

    fn code_of(err: &DoogatError) -> &'static str {
        let DoogatError::Structured { code, .. } = err else {
            panic!("expected Structured variant, got: {err:?}");
        };
        code
    }

    #[test]
    fn unique_violation_sets_code_and_lists_columns_and_values() {
        let err = DoogatError::unique_violation(
            "category-membership",
            ["link", "category"],
            ["20260416120000", "20260416120001"],
        );
        assert_eq!(code_of(&err), "UNIQUE_VIOLATION");
        assert!(
            err.to_string()
                .contains("UNIQUE constraint failed: category-membership.link"),
            "message: {err}"
        );
        assert_eq!(
            ctx_get(&err, "table"),
            &ErrorValue::String("category-membership".into())
        );
        assert_eq!(
            ctx_get(&err, "columns"),
            &ErrorValue::List(vec!["link".into(), "category".into()])
        );
        assert_eq!(
            ctx_get(&err, "values"),
            &ErrorValue::List(vec!["20260416120000".into(), "20260416120001".into()])
        );
    }

    #[test]
    fn references_violation_sets_code_and_includes_blocker_context() {
        let err = DoogatError::references_violation(
            "20260416120000",
            "link",
            "category-membership",
            "20260416130000",
        );
        assert_eq!(code_of(&err), "REFERENCES_VIOLATION");
        assert_eq!(
            err.to_string(),
            "cannot delete '20260416120000': NOT NULL REFERENCES from category-membership.link in row '20260416130000'"
        );
        assert_eq!(
            ctx_get(&err, "referencing_table"),
            &ErrorValue::String("category-membership".into())
        );
        assert_eq!(
            ctx_get(&err, "referencing_id"),
            &ErrorValue::String("20260416130000".into())
        );
    }

    #[test]
    fn not_null_violation_matches_prd_00122_wording() {
        let err = DoogatError::not_null_violation("link", "url");
        assert_eq!(code_of(&err), "NOT_NULL_VIOLATION");
        assert_eq!(err.to_string(), "NOT NULL constraint violated: link.url");
        assert_eq!(ctx_get(&err, "table"), &ErrorValue::String("link".into()));
        assert_eq!(ctx_get(&err, "column"), &ErrorValue::String("url".into()));
    }

    #[test]
    fn unknown_field_matches_prd_00122_wording() {
        let err = DoogatError::unknown_field("link", "bogus");
        assert_eq!(code_of(&err), "UNKNOWN_FIELD");
        assert_eq!(err.to_string(), "unknown column: link.bogus");
        assert_eq!(
            ctx_get(&err, "unknown_field"),
            &ErrorValue::String("bogus".into())
        );
    }

    #[test]
    fn type_not_registered_carries_type_name() {
        let err = DoogatError::type_not_registered("widget");
        assert_eq!(code_of(&err), "TYPE_NOT_REGISTERED");
        assert_eq!(
            err.to_string(),
            "type \"widget\" is not a registered typedef"
        );
        assert_eq!(ctx_get(&err, "type"), &ErrorValue::String("widget".into()));
    }

    #[test]
    fn cascade_cycle_lists_tables_in_order() {
        let err = DoogatError::cascade_cycle(["a", "b", "a"]);
        assert_eq!(code_of(&err), "CASCADE_CYCLE");
        assert_eq!(
            err.to_string(),
            "cascade delete would form a cycle through a, b, a"
        );
        assert_eq!(
            ctx_get(&err, "tables"),
            &ErrorValue::List(vec!["a".into(), "b".into(), "a".into()])
        );
    }

    #[test]
    fn singleton_violation_carries_table_and_existing_id() {
        let err = DoogatError::singleton_violation("app_config", "20260510120000");
        assert_eq!(code_of(&err), "SINGLETON_VIOLATION");
        assert_eq!(
            err.to_string(),
            "SINGLETON constraint violated: app_config already holds row 20260510120000"
        );
        assert_eq!(
            ctx_get(&err, "table"),
            &ErrorValue::String("app_config".into())
        );
        assert_eq!(
            ctx_get(&err, "existing_id"),
            &ErrorValue::String("20260510120000".into())
        );
    }

    #[test]
    fn singleton_not_found_carries_table() {
        let err = DoogatError::singleton_not_found("app_config");
        assert_eq!(code_of(&err), "SINGLETON_NOT_FOUND");
        assert_eq!(
            err.to_string(),
            "SINGLETON typedef app_config has no row to update"
        );
        assert_eq!(
            ctx_get(&err, "table"),
            &ErrorValue::String("app_config".into())
        );
    }

    #[test]
    fn legacy_validation_variant_unchanged() {
        // Sanity: existing `Validation(String)` constructor continues to
        // produce its legacy `"validation: ..."` Display so other call
        // sites that haven't migrated to the structured constructors keep
        // working.
        let err = DoogatError::Validation("some-message".into());
        assert_eq!(err.to_string(), "validation: some-message");
    }
}
