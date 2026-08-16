//! Application contract layer — adapter-neutral command/result types.
//!
//! Types in this module MUST NOT depend on adapter crates (`rusqlite`, `git2`,
//! `redb`, `axum`, `async_graphql`). Only domain/shared types from
//! `ddb-core/src/types/` and standard-library types are permitted.
//!
//! The adapter-neutrality invariant is enforced by the integration test
//! `ddb-core/tests/app_contract_adapter_guard.rs`.

mod error;
pub use error::{AppError, AppErrorCategory, AppErrorDetail, SCHEMA_UNSUPPORTED_CHANGE};

mod output;
pub use output::{AppOutput, AppWarning};
pub(crate) use output::{
    describe_consistency_warning_code, summarize_reindex_warnings,
};
#[cfg(test)]
pub(crate) use output::REINDEX_SKIPPED_FILES;

pub fn describe_consistency_warning(warning: &crate::types::ConsistencyWarning) -> String {
    output::describe_consistency_warning(warning)
}

mod commands;
pub use commands::{
    ApplySchemaCommand, CreateCommand, DeleteCommand, ReadCommand, SearchCommand,
    UnregisteredTypePolicy, UpdateCommand,
};

mod results;
pub use results::{
    AppSearchResult, BrokenBacklink, CreateResult, DeleteResult, ListResult, ReadResult,
    UpdateResult,
};
