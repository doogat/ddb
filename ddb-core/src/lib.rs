//! # ddb-core
//!
//! Core library for Doogat Doogat DB — a hybrid Git-CRDT decentralized Ddb database.
//!
//! ## Modules
//!
//! - [`app_contract`] — Adapter-neutral application command/result contract
//! - [`attachments`] — File attachment CRUD on `reference/{id}/`
//! - [`bundle`] — Bundle export/import for air-gapped sync
//! - [`bundled_types`] — Built-in type definition templates (project, contact)
//! - [`compaction`] — CRDT temp cleanup and git gc
//! - [`consistency`] — Detect and auto-fix doogat data quality issues
//! - [`crdt_resolver`] — Automerge CRDT conflict resolution
//! - [`error`] — Error types and Result alias
//! - [`ffi`] — UniFFI facade (DoogatDriver) for Swift/Kotlin bindings
//! - [`git_ops`] — Git repository operations (CRUD, merge, remote sync)
//! - [`hlc`] — Hybrid Logical Clock for causal ordering
//! - [`indexer`] — SQLite FTS5 search index, type inference, materialization
//! - [`maintenance`] — Git maintenance runner and auto-trigger
//! - [`parser`] — Parse and serialize three-zone Markdown doogats
//! - [`search_query`] — Search query parsing and normalization
//! - [`service`] — Unified orchestration layer (DoogatService) for CLI, FFI, and server
//! - [`sql_engine`] — SQL DDL/DML translation (tables as doogat types)
//! - [`sync_manager`] — Multi-device sync orchestration
//! - [`traits`] — Core trait abstractions (DoogatSource, DoogatStore, DoogatIndex,
//!   GitBackend supertrait + sub-traits: GitRemote, GitMerge, GitHistory,
//!   GitBinary, GitRename, GitDesktopHooks)
//! - [`types`] — Shared data structures
//!
//! Feature-gated:
//! - `nosql` — redb-based key-value index for fast lookups (requires `nosql` feature)

uniffi::setup_scaffolding!();

pub mod app_contract;
pub mod attachments;
pub mod bundle;
pub mod bundled_types;
pub mod compaction;
pub mod consistency;
pub mod crdt_resolver;
pub mod error;
pub mod ffi;
pub mod git_ops;
pub mod hlc;
pub mod indexer;
pub mod maintenance;
pub mod parser;
pub mod search_query;
pub mod service;
pub mod sql_engine;
pub mod sync_manager;
pub mod traits;
pub mod types;

#[cfg(feature = "nosql")]
pub mod nosql;
