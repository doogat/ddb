//! # zdb-core
//!
//! Core library for Doogat ZettelDB — a hybrid Git-CRDT decentralized Zettelkasten database.
//!
//! ## Modules
//!
//! - [`parser`] — Parse and serialize three-zone Markdown zettels
//! - [`git_ops`] — Git repository operations (CRUD, merge, remote sync)
//! - [`crdt_resolver`] — Automerge CRDT conflict resolution
//! - [`indexer`] — SQLite FTS5 search index, type inference, materialization
//! - [`sql_engine`] — SQL DDL/DML translation (tables as zettel types)
//! - [`bundled_types`] — Built-in type definition templates (project, contact)
//! - [`sync_manager`] — Multi-device sync orchestration
//! - [`compaction`] — CRDT temp cleanup and git gc
//! - [`types`] — Shared data structures
//! - [`error`] — Error types and Result alias

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
pub mod sql_engine;
pub mod sync_manager;
pub mod traits;
pub mod types;

#[cfg(feature = "nosql")]
pub mod nosql;
