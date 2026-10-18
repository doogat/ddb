# Assumptions ledger — 00177-ffi-crud-through-contract-v1

## 1: FFI structured record types + BaseOnly doc comment

Tess (initial):
- The new functions are visible from `crate::ffi::tests::records_tests` at `crate::ffi::records::*` (at least `pub(super)`).
- "Degrades to a logged, dropped element" means a Map inside a List is removed and its siblings keep their order. For a top-level Map only asserted that it does not panic, because the return value is not specified.
- The round-trip is checked by comparing core `Value`s, because it is unknown whether `FieldValue` derives `PartialEq`.
- `UpdateResultRecord::from_output` is fed a create output (same `AppOutput<ParsedDoogat>` type; a plain update produces no warnings).
- The record field `path` equals `ParsedDoogat.path`, expected to be `ddb/{id}.md` per repo conventions.
- `RebuildWarningRecord.code` and `.message` are `String`s.

Tess (strengthen):
- Record fields are `id: String`, `title/doogat_type: Option<String>`, `tags: Vec<String>`, `path: String`, and warnings items with `code: String` and `message: String`.
- `from_output` can be called from the `ffi::tests` submodule, as the existing tests already assume.
- `FieldValue` is not assumed to derive `Clone` or `Debug`.
- `svc.update` keeps tag order as given and puts the id in `rec.id`.

Ivan:
- A top-level `Value::Map` passed to `core_to_field_value` becomes an empty `FieldValue::List`, with a `tracing::warn!`.
- A missing `meta.id` in `from_output` becomes an empty string (`unwrap_or_default`) instead of a panic.
- `core_value` has only the variants String, Number, Bool, List and Map.
- The new conversion functions and `from_output` are `pub(super)`, matching the existing `SchemaApplyReportRecord::from_output`.
- `core_to_field_scalar` is `#[cfg(test)]` too, because its only caller is `core_to_field_value`, which is `#[cfg(test)]`.
