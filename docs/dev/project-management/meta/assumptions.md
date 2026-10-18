# Assumptions ledger — 00178-structured-fields-input-v1

## 1: Core fields normalization (app_contract) + FFI routing

Tess (initial):
- The order of the returned unset list is not specified, so the tests sort it before comparing.
- Explicit unsets with no matching key in `raw` pass through into the unset list (my reading of "merges").
- A doogat's `DoogatError::Validation` reaches FFI as `DdbError::Validation { .. }`, based on the mapping in the existing `error_parity_tests.rs`.
- A rejected FFI collision writes no commit and leaves stored fields unchanged (not stated, but "must be rejected" implies it).
- Duplicates within `explicit_unset` itself are not specified, so they are not tested initially (later added by the strengthen pass).

Tess (strengthen):
- The collision error must contain the key name as a substring, per the design doc's message `field '{k}' is both set and unset in the same update`.
- For the dedup and explicit-only tests, the unset list is compared in exact order, safe because each case returns a single element.
- Named the test-local type alias `NormalizedUpdate` so it does not clash with the existing `app_contract::results::UpdateResult`.

Ivan:
- `DoogatError::Validation` takes a `String`.
- Unset order is explicit `unset_fields` first, then null-derived keys, in input order; the tests only pin order for single-source cases.
- A key that is both null and explicitly unset is not a collision — kept once, as the tests require.

## 3: GraphQL: fields to JSON scalar, decode through core normalization, update singleton mutations

Tess (initial):
- `decode_json_fields_object` returns `Result<RawFieldsInput, String>` with `RawFieldsInput: Debug + PartialEq`.
- Error messages for nested-null rejection are not checked initially, only `is_err()`.

Tess (strengthen):
- The error from `decode_json_fields_object`/`fields_map`/`fields_and_unsets` implements `Debug`, so the key check uses `format!("{err:?}")`.
- Renaming a test's key from `l` to `tags_list` counts as strengthening the test, not changing its logic.
- `fields_map` and `fields_and_unsets` return `Result<_, String>`.

Ivan:
- `opt_fields_and_unsets` returns `None` whenever `fields` is absent, even if `unsetFields` is present; the batch-update call site uses `unsetFields` on its own in that case.
- The nested-null error names the full path (`key[i]` / `key.sub`), not just the top-level key.
- The `normalize_update_fields` error in the singleton `update_*` resolver maps through `to_graphql_error`.

## 4: Conformance: create-with-typed-fields step (GraphQL leg; REST deferred)

Ivan:
- The fixture is GraphQL-only (`interfaces: [Graphql]`), because the CLI driver sends fields as strings.
- The test is added to `cross_driver_crud.rs` next to `filtered_search`'s test rather than in a new file.
- Reading the stored frontmatter from the test repo's work tree is an acceptable way to assert native types, alongside SQL `typeof`, since the existing search-hit path turns every field into a string.
- For `priority`, "numeric" means it parses as f64 3.0, because core renders it as `3.0`.
- Fixing the GraphQL driver's legacy string encoding of `fields` is in scope, because the fixture cannot run without it (task 3 made the server reject the old stringified form, which had already broken `filtered_search_returns_same_hits_and_full_field_set_on_cli_and_graphql` on master).

Known gap recorded in the task: the test cannot distinguish a native boolean from a stringified one at this layer, because core coerces both to the column's declared type before either the stored frontmatter or the SQL index see them. A regression here would need an untyped field or a server-level unit test to catch.

## 2: REST: accept object fields through boundary decoder + core normalization

Tess (initial):
- `json_scalar_to_opt_value` returns `Result<Option<DdbValue>, String>`; compared with `assert_eq!`, assuming `DdbValue: PartialEq + Debug` (already true for existing `Value` tests).
- `json_value_to_fields_input` returns `ddb_core::app_contract::RawFieldsInput` (`BTreeMap<String, Option<DdbValue>>`) with `Debug`.
- Nested-null rejection is tested only through `json_value_to_fields_input`, since the task does not give `json_scalar_to_value_strict`'s exact signature.
- Rejection tests check only `is_err()`, not error text.
- Strict nested conversion rejects null at every depth, read from "the same conversion but rejects J::Null".

Devon:
- Changing the field types broke `create_doogat`/`update_doogat` compilation; patched locally with `parse_fields_json(&v.to_string())` only to compile for the exploit run, then reverted via `git checkout`.
- Read "run once per exploit" as satisfied by one combined run with all 8 exploits present at once.
- Took the 10 tests in `rest::tests` as the whole target; `rest::search_tests` is a separate module not selected by the runner filter.

Tess (strengthen):
- `json_scalar_to_opt_value` recurses into lists; `["a", null]` returns an error, and its error type supports `PartialEq`.
- The exact `f64` for 9007199254740993 is `9_007_199_254_740_993_u64 as f64` (2^53); this case alone doesn't catch an f32 path since f32 also rounds it to 2^53 (the 0.1 and 1e300 cases do).
- A JSON integer inside a nested list/object decodes to `DdbValue::Number` the same as at the top level.

Ivan:
- Could not read `normalize_create_fields`/`normalize_update_fields` signatures directly (not in file list); derived them from compiler errors. Create takes `BTreeMap<String, Option<Value>>` and returns `BTreeMap<String, Value>` without failing; update takes that map plus `Vec<String>` and returns `Result<(fields, unset_fields), DoogatError>`.
- Decoder errors are plain `String`s (has `PartialEq`/`Debug`).
- A JSON number that cannot become f64 is a validation error (only reachable with serde_json's `arbitrary_precision` feature).
