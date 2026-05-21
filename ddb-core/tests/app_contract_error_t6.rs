use ddb_core::app_contract::{AppError, AppErrorCategory};
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
