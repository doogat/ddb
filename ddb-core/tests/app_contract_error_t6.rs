use ddb_core::app_contract::{AppError, AppErrorCategory, AppErrorDetail};
use ddb_core::error::{codes, DoogatError};

fn assert_app_error(
    source: DoogatError,
    code: &'static str,
    category: AppErrorCategory,
    message: &str,
) {
    assert_eq!(
        AppError::from(source),
        AppError {
            code,
            message: message.to_string(),
            category,
            field: None,
        }
    );
}

#[test]
fn maps_legacy_client_errors_to_stable_app_errors() {
    for (source, code, category, message) in [
        (
            DoogatError::NotFound("item 42".into()),
            "NOT_FOUND",
            AppErrorCategory::NotFound,
            "not found: item 42",
        ),
        (
            DoogatError::Validation("bad field".into()),
            "VALIDATION_ERROR",
            AppErrorCategory::InvalidInput,
            "validation: bad field",
        ),
        (
            DoogatError::Parse("unexpected token".into()),
            "PARSE_ERROR",
            AppErrorCategory::InvalidInput,
            "parse: unexpected token",
        ),
        (
            DoogatError::InvalidPath("/bad/path".into()),
            "INVALID_PATH",
            AppErrorCategory::InvalidInput,
            "invalid path: /bad/path",
        ),
        (
            DoogatError::BadRequest("missing required param".into()),
            "BAD_REQUEST",
            AppErrorCategory::InvalidInput,
            "bad request: missing required param",
        ),
        (
            DoogatError::Conflict("duplicate key".into()),
            "CONFLICT",
            AppErrorCategory::Conflict,
            "conflict: duplicate key",
        ),
    ] {
        assert_app_error(source, code, category, message);
    }
}

#[test]
fn maps_internal_core_errors_to_internal_app_errors() {
    for (source, message) in [
        (DoogatError::Git("repo locked".into()), "git: repo locked"),
        (
            DoogatError::Yaml("invalid yaml".into()),
            "yaml: invalid yaml",
        ),
        (
            DoogatError::Sql("table missing".into()),
            "sql: table missing",
        ),
        (
            DoogatError::Automerge("merge failed".into()),
            "automerge: merge failed",
        ),
        (DoogatError::Toml("bad toml".into()), "toml: bad toml"),
        (
            DoogatError::SqlEngine("unsupported operation".into()),
            "sql engine: unsupported operation",
        ),
        (
            DoogatError::Sync("peer unreachable".into()),
            "sync: peer unreachable",
        ),
        (
            DoogatError::Index("index corrupt".into()),
            "index: index corrupt",
        ),
        (
            DoogatError::VersionMismatch { repo: 3, driver: 2 },
            "version mismatch: repo format v3, driver supports up to v2",
        ),
    ] {
        assert_app_error(
            source,
            "INTERNAL_ERROR",
            AppErrorCategory::Internal,
            message,
        );
    }

    assert_app_error(
        DoogatError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "access denied",
        )),
        "INTERNAL_ERROR",
        AppErrorCategory::Internal,
        "io: access denied",
    );
}

#[test]
fn maps_structured_error_codes_to_app_categories() {
    for (code, category) in [
        (codes::SINGLETON_NOT_FOUND, AppErrorCategory::NotFound),
        (codes::UNIQUE_VIOLATION, AppErrorCategory::Conflict),
        (codes::SINGLETON_VIOLATION, AppErrorCategory::Conflict),
        (codes::REFERENCES_VIOLATION, AppErrorCategory::Conflict),
        (codes::CASCADE_CYCLE, AppErrorCategory::Conflict),
        (codes::NOT_NULL_VIOLATION, AppErrorCategory::InvalidInput),
        (codes::UNKNOWN_FIELD, AppErrorCategory::InvalidInput),
        (codes::TYPE_NOT_REGISTERED, AppErrorCategory::InvalidInput),
        ("SOME_FUTURE_CODE", AppErrorCategory::Internal),
    ] {
        assert_app_error(
            DoogatError::Structured {
                code,
                message: "exact message text".into(),
                context: vec![],
            },
            code,
            category,
            "exact message text",
        );
    }
}

// ── Group A: AppError preserves Structured.context in details ─────────────────

/// Helper: find a detail entry by key in AppError.details.
fn detail<'a>(err: &'a AppError, key: &str) -> &'a AppErrorDetail {
    err.details
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("missing details key '{key}' in {err:?}"))
}

#[test]
fn app_error_from_unique_violation_preserves_table_columns_values() {
    let source = DoogatError::unique_violation("widgets", ["name"], ["foo"]);
    let err = AppError::from(source);
    assert_eq!(err.code, codes::UNIQUE_VIOLATION);
    assert_eq!(err.category, AppErrorCategory::Conflict);
    // Single column → field populated with the column name.
    assert_eq!(err.field, Some("name".to_string()));
    // details carries the full context.
    assert_eq!(
        detail(&err, "table"),
        &AppErrorDetail::String("widgets".to_string())
    );
    assert_eq!(
        detail(&err, "columns"),
        &AppErrorDetail::List(vec!["name".to_string()])
    );
    assert_eq!(
        detail(&err, "values"),
        &AppErrorDetail::List(vec!["foo".to_string()])
    );
}

#[test]
fn app_error_from_unique_violation_multi_column_leaves_field_none() {
    let source = DoogatError::unique_violation("widgets", ["name", "version"], ["a", "b"]);
    let err = AppError::from(source);
    assert_eq!(err.code, codes::UNIQUE_VIOLATION);
    // Multiple columns → no single column to point at.
    assert_eq!(err.field, None);
    // details still carries columns and values.
    assert_eq!(
        detail(&err, "columns"),
        &AppErrorDetail::List(vec!["name".to_string(), "version".to_string()])
    );
    assert_eq!(
        detail(&err, "values"),
        &AppErrorDetail::List(vec!["a".to_string(), "b".to_string()])
    );
}

#[test]
fn app_error_from_singleton_violation_preserves_table_and_existing_id() {
    let source = DoogatError::singleton_violation("cfg", "20260101120000");
    let err = AppError::from(source);
    assert_eq!(err.code, codes::SINGLETON_VIOLATION);
    assert_eq!(err.category, AppErrorCategory::Conflict);
    // No single column involved → field is None.
    assert_eq!(err.field, None);
    assert_eq!(
        detail(&err, "table"),
        &AppErrorDetail::String("cfg".to_string())
    );
    assert_eq!(
        detail(&err, "existing_id"),
        &AppErrorDetail::String("20260101120000".to_string())
    );
}

#[test]
fn app_error_from_not_null_violation_populates_field_with_column() {
    let source = DoogatError::not_null_violation("doogats", "title");
    let err = AppError::from(source);
    assert_eq!(err.code, codes::NOT_NULL_VIOLATION);
    assert_eq!(err.category, AppErrorCategory::InvalidInput);
    assert_eq!(err.field, Some("title".to_string()));
    assert_eq!(
        detail(&err, "table"),
        &AppErrorDetail::String("doogats".to_string())
    );
    assert_eq!(
        detail(&err, "column"),
        &AppErrorDetail::String("title".to_string())
    );
}

#[test]
fn app_error_from_type_not_registered_carries_type_name() {
    let source = DoogatError::type_not_registered("ghost");
    let err = AppError::from(source);
    assert_eq!(err.code, codes::TYPE_NOT_REGISTERED);
    assert_eq!(err.category, AppErrorCategory::InvalidInput);
    assert_eq!(
        detail(&err, "type"),
        &AppErrorDetail::String("ghost".to_string())
    );
}
