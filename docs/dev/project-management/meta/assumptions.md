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

## 2: create_structured/update_structured on DoogatDriver; remove raw FFI CRUD exports

Tess (initial):
- The `ConflictActionRecord::Error` rejection is asserted as `DdbError::Validation { code: Some(codes::SINGLETON_VIOLATION) }` rather than merely `is_err()`, mirroring the existing raw-path test `ffi_singleton_create_exposes_structured_error_context`.
- `path` is asserted as `ddb/{id}.md` (AGENTS.md storage convention); `path` is not an input, so it cannot "echo" one.
- The `app_config` singleton typedef is the "registered type" fixture, so no unknown-field-on-typed-doogat behavior is exercised.
- A type-less structured create persists arbitrary given fields into frontmatter.
- Field presence/absence is asserted by substring on `read_doogat` raw content, using distinctive key names.

Tess (strengthen):
- `UNREGISTERED_TYPE_BASE_ONLY` has no exported constant (only literals in `service/create.rs`), so the test keeps a local `const` with that literal spelling.
- Scalar frontmatter typing is not pinned by the requirements, so numeric/boolean fields are asserted representation-tolerantly; a `FieldValue::List` must round-trip as a YAML sequence.
- The error variant for `update_structured` on an unknown id is unspecified, so the test accepts `DdbError::NotFound` or `DdbError::Validation` and pins the side effects instead. (Pat raised this as a LOW — see the review note.)
- A create/update through the service updates the SQLite index synchronously (no `reindex()` needed).
- `execute_sql("CREATE TABLE widget (size TEXT)")` registers a second type for the create path.
- `fresh_driver`, `setup_app_config_singleton_typedef` and `head_at` are reachable unchanged from the submodule.

Ivan:
- `create_simple(driver, title, body)` is an acceptable local test helper (6 call sites) for the migrated bundle-export fixtures.
- Migrated fixtures map raw `---\ntitle: X\n---\nBody` to `title: Some("X")`, no tags, no type, `body: Some("Body")`, `ConflictActionRecord::Error`, dropping the raw path's commit message (the structured API has no message parameter and those tests never assert on it).
- `ffi_singleton_create_exposes_structured_error_context` keeps its now-duplicated coverage rather than being deleted.

## 3: Replace lossy FFI error allowlist with policy-derived ffi_variant

Tess (initial):
- `DdbError::Conflict`'s fields are exactly `{ msg: String, code: Option<String>, context: Vec<DdbErrorContextEntry> }`, mirroring the existing `Validation` arm.
- `FfiVariant::NotFound -> DdbError::NotFound { msg }` carries no code/context, so a `Structured { code: SINGLETON_NOT_FOUND }` loses its context at the FFI boundary.
- The Validation arm's code/context rule applies to non-`Structured` sources too (`code: Some(policy code)`, empty context).
- `DoogatError::Toml` now maps to `DdbError::Io`, i.e. `DdbError::Config` is no longer produced.

Tess (strengthen):
- Context entry order on the Conflict/Validation arms is the source `ErrorContext` order (PRD 00129 §6 contract), so full-vector equality is asserted.
- `DoogatError::Structured` may be constructed directly in tests with synthetic code/message/context to cover `codes::ALL`.
- For the `NotFound` and `Io` FFI arms no code/context is asserted, since those variants have no such fields.
- `DoogatError::Redb` is left uncovered (in-contract): it is `#[cfg(feature = "nosql")]` and covering it would need a feature gate in the test file.

Ivan:
- The new `Conflict` variant sits immediately before `VersionMismatch`; uniffi error variants are name-matched in generated bindings, so position is not load-bearing.
- Fixing the unlisted fifth stale assertion in `masterless_b_fails` (ffi/tests.rs:1455) is in scope, being the same precondition class in the same three tests.
- `msg.starts_with("sync: bundle merge failed:")` is the right strength for that site, preserving the original `starts_with` rather than weakening to `contains`.
- Updating two doc comments that named the old variant counts as part of re-pointing.

## 4: Regenerate FFI bindings; add warnings/policy/error-variant parity tests

Orchestrator:
- The parity tests live in `tests/e2e/integration_ffi_warning_parity.rs`, not `ddb-core/src/ffi/tests.rs` as the plan task named: `ddb-core` cannot reach the GraphQL server or the `ddb` binary, so an in-crate test could only compare FFI against the app contract it already calls — the tautology the design doc rejects.
- The CI-breaking `too many arguments (8/7)` lint on `update_structured`, landed by task 2 of this PRD, is 00177's own defect and was fixed inside this task rather than deferred.

Ivan (tests):
- The `TITLE_FROM_TEMPLATE` warning fires when the caller OMITS the title and the typedef has a `title_template` (the dispatch said the inverse); used the omitted-title fixture, matching `graphql_response_warnings_t154.rs`.
- `ConflictActionRecord` is not re-exported from `ddb_core::ffi`, so the argument is built through the public `From<ConflictAction>` impl rather than adding a re-export.
- A static title template `'auto-title'` with no field values is enough; mirrors `integration_warnings_update.rs`.
- `ddb create` has no structured output mode, so the CLI side parses the stderr `warning: <CODE>: <MESSAGE>` line and compares both code and message.
- Each transport side gets its own fresh `DdbTestRepo`; nothing runs in the project root.

Ivan (signature collapse):
- Derived `Debug, Clone, PartialEq` on `UpdateStructuredInput`, matching the neighbouring `FieldEntryValue`; the task did not specify derives.
- Record field order follows the old argument order.
- The single `ddb-server` actor-test timeout seen in a full `test-ci` run is a pre-existing load flake (green on an isolated rerun in 1.11s), not a regression from this change.
