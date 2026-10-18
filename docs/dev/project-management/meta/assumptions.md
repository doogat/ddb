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
