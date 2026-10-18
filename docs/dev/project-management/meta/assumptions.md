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
