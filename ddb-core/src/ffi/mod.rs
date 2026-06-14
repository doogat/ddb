//! UniFFI facade for the Doogat driver (Swift/Kotlin bindings).
//!
//! Split into `records` (uniffi `Record`/`Error` types + conversions),
//! `driver` (the `DoogatDriver` object and its exported methods), and `tests`.
//! Re-exports keep `ddb_core::ffi::<Item>` paths stable. PRD 00156.

mod driver;
mod records;

#[cfg(test)]
mod tests;

pub use driver::DoogatDriver;
pub use records::{
    AttachmentInfo, ColumnDefRecord, DdbError, DdbErrorContextEntry, PaginatedSearchResult,
    RebuildReport, SearchResult, SqlResultRecord, TypeSchemaRecord,
};
