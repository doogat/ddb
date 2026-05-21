use ddb_core::app_contract::{AppError, AppErrorCategory};
use ddb_core::error::DoogatError;

// --- Struct field and derive coverage ---

#[test]
fn apperror_exposes_code_message_category_and_field() {
    let err = AppError {
        code: "NOT_FOUND",
        message: String::from("not found: item"),
        category: AppErrorCategory::NotFound,
        field: None,
    };
    assert_eq!(err.code, "NOT_FOUND");
    assert_eq!(err.message, "not found: item");
    assert_eq!(err.category, AppErrorCategory::NotFound);
    assert!(err.field.is_none());
}

#[test]
fn apperror_field_can_hold_a_value() {
    let err = AppError {
        code: "VALIDATION_ERROR",
        message: String::from("validation: bad value"),
        category: AppErrorCategory::InvalidInput,
        field: Some(String::from("email")),
    };
    assert_eq!(err.field, Some(String::from("email")));
}

#[test]
fn apperror_is_cloneable() {
    let err = AppError {
        code: "CONFLICT",
        message: String::from("conflict: duplicate"),
        category: AppErrorCategory::Conflict,
        field: None,
    };
    let cloned = err.clone();
    assert_eq!(cloned, err);
}

#[test]
fn apperror_is_debuggable() {
    let err = AppError {
        code: "INTERNAL_ERROR",
        message: String::from("io: some io failure"),
        category: AppErrorCategory::Internal,
        field: None,
    };
    let debug_str = format!("{:?}", err);
    assert!(debug_str.contains("INTERNAL_ERROR"));
}

#[test]
fn apperror_category_is_cloneable() {
    let cat = AppErrorCategory::NotFound;
    let cloned = cat.clone();
    assert_eq!(cloned, AppErrorCategory::NotFound);
}

#[test]
fn apperror_category_is_debuggable() {
    let debug_str = format!("{:?}", AppErrorCategory::InvalidInput);
    assert!(debug_str.contains("InvalidInput"));
}

#[test]
fn apperror_category_variants_are_eq() {
    assert_eq!(AppErrorCategory::NotFound, AppErrorCategory::NotFound);
    assert_eq!(AppErrorCategory::InvalidInput, AppErrorCategory::InvalidInput);
    assert_eq!(AppErrorCategory::Conflict, AppErrorCategory::Conflict);
    assert_eq!(AppErrorCategory::Internal, AppErrorCategory::Internal);
    assert_ne!(AppErrorCategory::NotFound, AppErrorCategory::Internal);
    assert_ne!(AppErrorCategory::InvalidInput, AppErrorCategory::Conflict);
}

// --- From<DoogatError> mapping: NotFound ---

#[test]
fn not_found_error_maps_to_not_found_category() {
    let doogat_err = DoogatError::NotFound(String::from("item 42"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::NotFound);
}

#[test]
fn not_found_error_maps_to_not_found_code() {
    let doogat_err = DoogatError::NotFound(String::from("item 42"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "NOT_FOUND");
}

#[test]
fn not_found_error_message_uses_display_string() {
    let doogat_err = DoogatError::NotFound(String::from("item 42"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "not found: item 42");
}

#[test]
fn not_found_error_field_is_none() {
    let doogat_err = DoogatError::NotFound(String::from("item 42"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: Validation ---

#[test]
fn validation_error_maps_to_invalid_input_category() {
    let doogat_err = DoogatError::Validation(String::from("bad field"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::InvalidInput);
}

#[test]
fn validation_error_maps_to_validation_error_code() {
    let doogat_err = DoogatError::Validation(String::from("bad field"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "VALIDATION_ERROR");
}

#[test]
fn validation_error_message_uses_display_string() {
    let doogat_err = DoogatError::Validation(String::from("bad field"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "validation: bad field");
}

#[test]
fn validation_error_field_is_none() {
    let doogat_err = DoogatError::Validation(String::from("bad field"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: Parse ---

#[test]
fn parse_error_maps_to_invalid_input_category() {
    let doogat_err = DoogatError::Parse(String::from("unexpected token"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::InvalidInput);
}

#[test]
fn parse_error_maps_to_parse_error_code() {
    let doogat_err = DoogatError::Parse(String::from("unexpected token"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "PARSE_ERROR");
}

#[test]
fn parse_error_message_uses_display_string() {
    let doogat_err = DoogatError::Parse(String::from("unexpected token"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "parse: unexpected token");
}

#[test]
fn parse_error_field_is_none() {
    let doogat_err = DoogatError::Parse(String::from("unexpected token"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: InvalidPath ---

#[test]
fn invalid_path_error_maps_to_invalid_input_category() {
    let doogat_err = DoogatError::InvalidPath(String::from("/bad/path"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::InvalidInput);
}

#[test]
fn invalid_path_error_maps_to_invalid_path_code() {
    let doogat_err = DoogatError::InvalidPath(String::from("/bad/path"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "INVALID_PATH");
}

#[test]
fn invalid_path_error_message_uses_display_string() {
    let doogat_err = DoogatError::InvalidPath(String::from("/bad/path"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "invalid path: /bad/path");
}

#[test]
fn invalid_path_error_field_is_none() {
    let doogat_err = DoogatError::InvalidPath(String::from("/bad/path"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: BadRequest ---

#[test]
fn bad_request_error_maps_to_invalid_input_category() {
    let doogat_err = DoogatError::BadRequest(String::from("missing required param"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::InvalidInput);
}

#[test]
fn bad_request_error_maps_to_bad_request_code() {
    let doogat_err = DoogatError::BadRequest(String::from("missing required param"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "BAD_REQUEST");
}

#[test]
fn bad_request_error_message_uses_display_string() {
    let doogat_err = DoogatError::BadRequest(String::from("missing required param"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "bad request: missing required param");
}

#[test]
fn bad_request_error_field_is_none() {
    let doogat_err = DoogatError::BadRequest(String::from("missing required param"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: Conflict ---

#[test]
fn conflict_error_maps_to_conflict_category() {
    let doogat_err = DoogatError::Conflict(String::from("duplicate key"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Conflict);
}

#[test]
fn conflict_error_maps_to_conflict_code() {
    let doogat_err = DoogatError::Conflict(String::from("duplicate key"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "CONFLICT");
}

#[test]
fn conflict_error_message_uses_display_string() {
    let doogat_err = DoogatError::Conflict(String::from("duplicate key"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "conflict: duplicate key");
}

#[test]
fn conflict_error_field_is_none() {
    let doogat_err = DoogatError::Conflict(String::from("duplicate key"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: Git ---

#[test]
fn git_error_maps_to_internal_category() {
    let doogat_err = DoogatError::Git(String::from("repo locked"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Internal);
}

#[test]
fn git_error_maps_to_internal_error_code() {
    let doogat_err = DoogatError::Git(String::from("repo locked"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "INTERNAL_ERROR");
}

#[test]
fn git_error_message_uses_display_string() {
    let doogat_err = DoogatError::Git(String::from("repo locked"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "git: repo locked");
}

#[test]
fn git_error_field_is_none() {
    let doogat_err = DoogatError::Git(String::from("repo locked"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: Yaml ---

#[test]
fn yaml_error_maps_to_internal_category() {
    let doogat_err = DoogatError::Yaml(String::from("invalid yaml"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Internal);
}

#[test]
fn yaml_error_maps_to_internal_error_code() {
    let doogat_err = DoogatError::Yaml(String::from("invalid yaml"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "INTERNAL_ERROR");
}

#[test]
fn yaml_error_message_uses_display_string() {
    let doogat_err = DoogatError::Yaml(String::from("invalid yaml"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "yaml: invalid yaml");
}

#[test]
fn yaml_error_field_is_none() {
    let doogat_err = DoogatError::Yaml(String::from("invalid yaml"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: Sql ---

#[test]
fn sql_error_maps_to_internal_category() {
    let doogat_err = DoogatError::Sql(String::from("table missing"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Internal);
}

#[test]
fn sql_error_maps_to_internal_error_code() {
    let doogat_err = DoogatError::Sql(String::from("table missing"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "INTERNAL_ERROR");
}

#[test]
fn sql_error_message_uses_display_string() {
    let doogat_err = DoogatError::Sql(String::from("table missing"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "sql: table missing");
}

#[test]
fn sql_error_field_is_none() {
    let doogat_err = DoogatError::Sql(String::from("table missing"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: Automerge ---

#[test]
fn automerge_error_maps_to_internal_category() {
    let doogat_err = DoogatError::Automerge(String::from("merge failed"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Internal);
}

#[test]
fn automerge_error_maps_to_internal_error_code() {
    let doogat_err = DoogatError::Automerge(String::from("merge failed"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "INTERNAL_ERROR");
}

#[test]
fn automerge_error_message_uses_display_string() {
    let doogat_err = DoogatError::Automerge(String::from("merge failed"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "automerge: merge failed");
}

#[test]
fn automerge_error_field_is_none() {
    let doogat_err = DoogatError::Automerge(String::from("merge failed"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: Io ---

#[test]
fn io_error_maps_to_internal_category() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
    let doogat_err = DoogatError::Io(io_err);
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Internal);
}

#[test]
fn io_error_maps_to_internal_error_code() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
    let doogat_err = DoogatError::Io(io_err);
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "INTERNAL_ERROR");
}

#[test]
fn io_error_message_uses_display_string() {
    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
    let expected_msg = format!("io: {}", io_err);
    let doogat_err = DoogatError::Io(io_err);
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, expected_msg);
}

#[test]
fn io_error_field_is_none() {
    let io_err = std::io::Error::new(std::io::ErrorKind::Other, "other");
    let doogat_err = DoogatError::Io(io_err);
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: Toml ---

#[test]
fn toml_error_maps_to_internal_category() {
    let doogat_err = DoogatError::Toml(String::from("bad toml"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Internal);
}

#[test]
fn toml_error_maps_to_internal_error_code() {
    let doogat_err = DoogatError::Toml(String::from("bad toml"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "INTERNAL_ERROR");
}

#[test]
fn toml_error_message_uses_display_string() {
    let doogat_err = DoogatError::Toml(String::from("bad toml"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "toml: bad toml");
}

#[test]
fn toml_error_field_is_none() {
    let doogat_err = DoogatError::Toml(String::from("bad toml"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: Sync ---

#[test]
fn sync_error_maps_to_internal_category() {
    let doogat_err = DoogatError::Sync(String::from("peer unreachable"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Internal);
}

#[test]
fn sync_error_maps_to_internal_error_code() {
    let doogat_err = DoogatError::Sync(String::from("peer unreachable"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "INTERNAL_ERROR");
}

#[test]
fn sync_error_message_uses_display_string() {
    let doogat_err = DoogatError::Sync(String::from("peer unreachable"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "sync: peer unreachable");
}

#[test]
fn sync_error_field_is_none() {
    let doogat_err = DoogatError::Sync(String::from("peer unreachable"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: Index ---

#[test]
fn index_error_maps_to_internal_category() {
    let doogat_err = DoogatError::Index(String::from("index corrupt"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Internal);
}

#[test]
fn index_error_maps_to_internal_error_code() {
    let doogat_err = DoogatError::Index(String::from("index corrupt"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "INTERNAL_ERROR");
}

#[test]
fn index_error_message_uses_display_string() {
    let doogat_err = DoogatError::Index(String::from("index corrupt"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "index: index corrupt");
}

#[test]
fn index_error_field_is_none() {
    let doogat_err = DoogatError::Index(String::from("index corrupt"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: SqlEngine ---

#[test]
fn sql_engine_error_maps_to_internal_category() {
    let doogat_err = DoogatError::SqlEngine(String::from("unsupported operation"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Internal);
}

#[test]
fn sql_engine_error_maps_to_internal_error_code() {
    let doogat_err = DoogatError::SqlEngine(String::from("unsupported operation"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "INTERNAL_ERROR");
}

#[test]
fn sql_engine_error_message_uses_display_string() {
    let doogat_err = DoogatError::SqlEngine(String::from("unsupported operation"));
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "sql engine: unsupported operation");
}

#[test]
fn sql_engine_error_field_is_none() {
    let doogat_err = DoogatError::SqlEngine(String::from("unsupported operation"));
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: VersionMismatch ---

#[test]
fn version_mismatch_error_maps_to_internal_category() {
    let doogat_err = DoogatError::VersionMismatch { repo: 2, driver: 1 };
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Internal);
}

#[test]
fn version_mismatch_error_maps_to_internal_error_code() {
    let doogat_err = DoogatError::VersionMismatch { repo: 2, driver: 1 };
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "INTERNAL_ERROR");
}

#[test]
fn version_mismatch_error_message_uses_display_string() {
    let doogat_err = DoogatError::VersionMismatch { repo: 3, driver: 2 };
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "version mismatch: repo format v3, driver supports up to v2");
}

#[test]
fn version_mismatch_error_field_is_none() {
    let doogat_err = DoogatError::VersionMismatch { repo: 2, driver: 1 };
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

// --- From<DoogatError> mapping: Structured ---

#[test]
fn structured_singleton_not_found_maps_to_not_found_category() {
    let doogat_err = DoogatError::Structured {
        code: "SINGLETON_NOT_FOUND",
        message: String::from("singleton missing"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::NotFound);
}

#[test]
fn structured_singleton_not_found_uses_sc_as_code() {
    let doogat_err = DoogatError::Structured {
        code: "SINGLETON_NOT_FOUND",
        message: String::from("singleton missing"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert_eq!(err.code, "SINGLETON_NOT_FOUND");
}

#[test]
fn structured_singleton_not_found_uses_sm_as_message() {
    let doogat_err = DoogatError::Structured {
        code: "SINGLETON_NOT_FOUND",
        message: String::from("singleton missing"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "singleton missing");
}

#[test]
fn structured_unique_violation_maps_to_conflict_category() {
    let doogat_err = DoogatError::Structured {
        code: "UNIQUE_VIOLATION",
        message: String::from("unique constraint failed"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Conflict);
    assert_eq!(err.code, "UNIQUE_VIOLATION");
}

#[test]
fn structured_singleton_violation_maps_to_conflict_category() {
    let doogat_err = DoogatError::Structured {
        code: "SINGLETON_VIOLATION",
        message: String::from("only one allowed"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Conflict);
    assert_eq!(err.code, "SINGLETON_VIOLATION");
}

#[test]
fn structured_references_violation_maps_to_conflict_category() {
    let doogat_err = DoogatError::Structured {
        code: "REFERENCES_VIOLATION",
        message: String::from("reference broken"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Conflict);
    assert_eq!(err.code, "REFERENCES_VIOLATION");
}

#[test]
fn structured_cascade_cycle_maps_to_conflict_category() {
    let doogat_err = DoogatError::Structured {
        code: "CASCADE_CYCLE",
        message: String::from("cycle detected"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Conflict);
    assert_eq!(err.code, "CASCADE_CYCLE");
}

#[test]
fn structured_not_null_violation_maps_to_invalid_input_category() {
    let doogat_err = DoogatError::Structured {
        code: "NOT_NULL_VIOLATION",
        message: String::from("field is required"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::InvalidInput);
    assert_eq!(err.code, "NOT_NULL_VIOLATION");
}

#[test]
fn structured_unknown_field_maps_to_invalid_input_category() {
    let doogat_err = DoogatError::Structured {
        code: "UNKNOWN_FIELD",
        message: String::from("field does not exist"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::InvalidInput);
    assert_eq!(err.code, "UNKNOWN_FIELD");
}

#[test]
fn structured_type_not_registered_maps_to_invalid_input_category() {
    let doogat_err = DoogatError::Structured {
        code: "TYPE_NOT_REGISTERED",
        message: String::from("type unknown"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::InvalidInput);
    assert_eq!(err.code, "TYPE_NOT_REGISTERED");
}

#[test]
fn structured_unknown_code_maps_to_internal_category() {
    let doogat_err = DoogatError::Structured {
        code: "SOME_FUTURE_CODE",
        message: String::from("unexpected error"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert_eq!(err.category, AppErrorCategory::Internal);
    assert_eq!(err.code, "SOME_FUTURE_CODE");
}

#[test]
fn structured_error_field_is_none() {
    let doogat_err = DoogatError::Structured {
        code: "SINGLETON_NOT_FOUND",
        message: String::from("singleton missing"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert!(err.field.is_none());
}

#[test]
fn structured_message_is_sm_not_display_wrapper() {
    // Display for Structured is the message itself; code must equal sc, not "INTERNAL_ERROR"
    let doogat_err = DoogatError::Structured {
        code: "UNIQUE_VIOLATION",
        message: String::from("exact message text"),
        context: vec![],
    };
    let err = AppError::from(doogat_err);
    assert_eq!(err.message, "exact message text");
    assert_ne!(err.code, "INTERNAL_ERROR");
}
