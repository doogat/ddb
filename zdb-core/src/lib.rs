//! # zdb-core
//!
//! Core library for Doogat ZettelDB — a hybrid Git-CRDT decentralized Zettelkasten database.
//!
//! ## Modules
//!
//! - [`attachments`] — File attachment CRUD on `reference/{id}/`
//! - [`bundle`] — Bundle export/import for air-gapped sync
//! - [`bundled_types`] — Built-in type definition templates (project, contact)
//! - [`compaction`] — CRDT temp cleanup and git gc
//! - [`consistency`] — Detect and auto-fix zettel data quality issues
//! - [`crdt_resolver`] — Automerge CRDT conflict resolution
//! - [`error`] — Error types and Result alias
//! - [`ffi`] — UniFFI facade (ZettelDriver) for Swift/Kotlin bindings
//! - [`git_ops`] — Git repository operations (CRUD, merge, remote sync)
//! - [`hlc`] — Hybrid Logical Clock for causal ordering
//! - [`indexer`] — SQLite FTS5 search index, type inference, materialization
//! - [`maintenance`] — Git maintenance runner and auto-trigger
//! - [`parser`] — Parse and serialize three-zone Markdown zettels
//! - [`service`] — Unified orchestration layer (ZettelService) for CLI, FFI, and server
//! - [`sql_engine`] — SQL DDL/DML translation (tables as zettel types)
//! - [`sync_manager`] — Multi-device sync orchestration
//! - [`traits`] — Core trait abstractions (ZettelSource, ZettelStore, ZettelIndex)
//! - [`types`] — Shared data structures
//!
//! Feature-gated:
//! - `nosql` — redb-based key-value index for fast lookups (requires `nosql` feature)

uniffi::setup_scaffolding!();

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
pub mod service;
pub mod sql_engine;
pub mod sync_manager;
pub mod traits;
pub mod types;

#[cfg(feature = "nosql")]
pub mod nosql;
