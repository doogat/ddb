//! Application contract layer — adapter-neutral command/result types.
//!
//! Types in this module MUST NOT depend on adapter crates (`rusqlite`, `git2`,
//! `redb`, `axum`, `async_graphql`). Only domain/shared types from
//! `ddb-core/src/types/` and standard-library types are permitted.
//!
//! The adapter-neutrality invariant is enforced by the integration test
//! `ddb-core/tests/app_contract_adapter_guard.rs`.

mod output;
pub use output::{AppOutput, AppWarning};
