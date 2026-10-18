# Design: Structured fields input (00178)

## Architecture fit

This lands in the app-contract layer (`ddb-core/src/app_contract/`) plus the two
adapter boundaries that still stringify `fields`: GraphQL (`ddb-server/src/schema/`)
and REST (`ddb-server/src/rest.rs`). FFI already takes native scalars via
`FieldEntryValue`/`FieldValue` (00177) and needs no new decode path — see
`## Reuse inventory`. The work is a boundary-decoder swap plus one new core
normalization function; it does not touch `DoogatService`, the git write path,
or the indexer. Per the architecture guardrails, the new normalization function
lives in `ddb-core` and stays adapter-neutral: it takes/returns the existing
`Value`/`BTreeMap` types, never `async_graphql`/`serde_json`/`uniffi` types.

## Module placement

- **New file**: `ddb-core/src/app_contract/fields_normalize.rs` — the one core
  normalization policy (create-null-omit, update-null-unset, dedup,
  set/unset collision check). Re-exported from `ddb-core/src/app_contract/mod.rs`.
- **Edit**: `ddb-server/src/schema/base_types.rs` — replace `parse_fields_json`
  (string-only) with an object decoder over the `JSON` scalar's
  `async_graphql::Value`.
- **Edit**: `ddb-server/src/schema/input.rs` — `fields_map`/`opt_fields_map` route
  through the new object decoder + core normalization instead of
  `parse_fields_json`.
- **Edit**: `ddb-server/src/schema/mutations/singleton.rs` — both `input`
  arguments (`:81`, `:108`) change `TypeRef::named_nn(TypeRef::STRING)` →
  `TypeRef::named_nn("JSON")`; both extraction sites (`:43`, `:94`) change
  `.string()?` → `.object()?.as_index_map()`, then route through
  `decode_json_fields_object` + `normalize_create_fields`/
  `normalize_update_fields`.
- **Edit**: `ddb-server/src/schema/type_defs.rs` — `fields` on
  `CreateDoogatInput` (:437), `CreateManyItemInput` (:448), `UpdateDoogatInput`
  (:466) change `TypeRef::named(TypeRef::STRING)` → `TypeRef::named("JSON")`.
  The `JSON` scalar is already registered (`schema/mod.rs:102`) and already used
  as an output type (`type_defs.rs:140`) — reuse it, do not add a second scalar.
- **Edit**: `ddb-server/src/rest.rs` — `CreateBody.fields`/`UpdateBody.fields`
  change `Option<String>` → `Option<serde_json::Value>`; `create_doogat`/
  `update_doogat` decode via a new `json_value_to_fields_input` helper + core
  normalization instead of `crate::schema::parse_fields_json`.
- **Minimal change**: `ddb-core/src/ffi/driver.rs` — `create_structured`/
  `update_structured` wrap their existing `fields_to_core(...)` result in
  `normalize_create_fields(...)`/pass through `normalize_update_fields` (an
  import plus a call, no shape change). `FieldEntryValue`/`FieldValue`
  (`ddb-core/src/ffi/records.rs`) are unchanged: null/unset intent is already
  explicit via `UpdateStructuredInput.unset_fields` and the simple absence of a
  create key (no `Null` variant needed in the FFI enum, since UniFFI callers
  never emit an entry they want omitted). Routing through the new core
  functions is purely to keep one normalization chokepoint across transports,
  not because FFI needs the null policy (it never produces `None` entries).
- **No change**: `ddb-core/src/app_contract/commands.rs` — `CreateCommand.fields`
  and `UpdateCommand.fields`/`unset_fields` already have the right shapes.

## Interfaces & contracts

### Core: `ddb-core/src/app_contract/fields_normalize.rs`

```rust
use std::collections::BTreeMap;
use crate::error::{DoogatError, Result};
use crate::types::Value;

/// Decoded field entries before null-policy is applied. `None` = the adapter
/// saw an explicit null for this key; `Some(v)` = a concrete value.
pub type RawFieldsInput = BTreeMap<String, Option<Value>>;

/// Create-path normalization: nulls are omitted (no key written). Returns the
/// typed fields map ready for `CreateCommand.fields`.
pub fn normalize_create_fields(raw: RawFieldsInput) -> BTreeMap<String, Value> {
    raw.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect()
}

/// Update-path normalization: nulls become unsets, merged with
/// `explicit_unset` (deduplicated). A key present as both a concrete value
/// AND in `explicit_unset` is a collision, rejected as `DoogatError::Validation`.
/// Returns `(fields, unset_fields)` ready for `UpdateCommand`.
pub fn normalize_update_fields(
    raw: RawFieldsInput,
    explicit_unset: Vec<String>,
) -> Result<(BTreeMap<String, Value>, Vec<String>)> {
    let mut fields = BTreeMap::new();
    let mut unsets: std::collections::BTreeSet<String> = explicit_unset.into_iter().collect();
    for (k, v) in raw {
        match v {
            Some(val) => {
                if unsets.contains(&k) {
                    return Err(DoogatError::Validation(format!(
                        "field '{k}' is both set and unset in the same update"
                    )));
                }
                fields.insert(k, val);
            }
            None => {
                unsets.insert(k);
            }
        }
    }
    Ok((fields, unsets.into_iter().collect()))
}
```

Re-export both from `ddb-core/src/app_contract/mod.rs` alongside `CreateCommand`/
`UpdateCommand`.

### GraphQL: `ddb-server/src/schema/base_types.rs`

Replace `parse_fields_json(json_str: &str)` with:

```rust
/// Decode a `JSON`-scalar `async_graphql::Value::Object` into
/// `app_contract::RawFieldsInput`. Rejects a `GqlValue::String` (the old
/// JSON-encoded-string shape) with `InvalidInput` so a caller still sending
/// the stringified form gets a clear migration error, not a silent reparse.
pub(crate) fn decode_json_fields_object(
    obj: &async_graphql::indexmap::IndexMap<async_graphql::Name, GqlValue>,
) -> Result<ddb_core::app_contract::RawFieldsInput, String> {
    obj.iter().map(|(k, v)| Ok((k.to_string(), gql_scalar_to_opt_value(v)?))).collect()
}

fn gql_scalar_to_opt_value(v: &GqlValue) -> Result<Option<Value>, String> {
    Ok(match v {
        GqlValue::Null => None,
        GqlValue::String(s) => Some(Value::String(s.clone())),
        GqlValue::Number(n) => Some(Value::Number(
            n.as_f64().ok_or_else(|| "non-finite number in fields".to_string())?,
        )),
        GqlValue::Boolean(b) => Some(Value::Bool(*b)),
        GqlValue::List(items) => Some(Value::List(
            items.iter().map(gql_scalar_to_value_strict).collect::<Result<_, _>>()?,
        )),
        GqlValue::Object(map) => Some(Value::Map(
            map.iter()
                .map(|(k, v)| Ok((k.to_string(), gql_scalar_to_value_strict(v)?)))
                .collect::<Result<_, String>>()?,
        )),
        other => return Err(format!("unsupported fields value: {other:?}")),
    })
}

/// Same as `gql_scalar_to_opt_value` but rejects `Null` (no null inside a
/// list/nested object — null is only a top-level per-field signal).
fn gql_scalar_to_value_strict(v: &GqlValue) -> Result<Value, String> { /* delegates, errors on Null */ }
```

`parse_fields_json` is deleted; its two remaining direct callers
(`schema/mutations/singleton.rs:43,94`) move to `decode_json_fields_object` +
`normalize_create_fields`/`normalize_update_fields` (singleton create/update
have no separate null-vs-unset distinction documented in the PRD, so they use
the same create/update normalization as the generic path — singleton update
goes through `normalize_update_fields` with `explicit_unset: vec![]` since
singleton has no `unsetFields` arg today).

**Singleton's own `input` argument must change too** (confirmed by reading
`singleton.rs:81,108`): both call sites declare
`InputValue::new("input", TypeRef::named_nn(TypeRef::STRING))` and extract via
`ctx.args.try_get("input")?.string()?` (`:43,94`) — a GraphQL string, not an
object, today. Both declarations change to
`InputValue::new("input", TypeRef::named_nn("JSON"))`, and the extraction
changes to `ctx.args.try_get("input")?.object()?.as_index_map()` so the result
is the `&IndexMap<Name, GqlValue>` `decode_json_fields_object` expects.

The **update** field (`:43-70`) is the one that must thread unsets:
`normalize_update_fields(raw, vec![])` returns `(fields, unsets)`, and
`UpdateDoogatParams.unset_fields` at `:70` — today hardcoded to `vec![]` —
changes to the `unsets` this call returns, or a `{"note": null}` input is
silently dropped (computed, then discarded) instead of unsetting `note`, which
would violate the PRD's "on update `note` is unset ... identical on
GraphQL/REST/FFI" requirement. The **upsert** field (`:94`, create-only, no
`unset_fields` on its path) uses `normalize_create_fields` only, matching its
existing fields-only signature.

### GraphQL: `ddb-server/src/schema/input.rs`

```rust
pub(crate) fn fields_map(obj: &GqlObject) -> Result<BTreeMap<String, DdbValue>, String> {
    match obj.get("fields") {
        Some(GqlValue::Object(map)) => {
            let raw = decode_json_fields_object(map)?;
            Ok(normalize_create_fields(raw))
        }
        Some(GqlValue::String(_)) => Err(
            "fields must be a JSON object, not a JSON-encoded string (migrate the \
             GraphQL variable type to JSON)".to_string(),
        ),
        _ => Ok(BTreeMap::new()),
    }
}

/// Update path: returns fields AND the null-derived + explicit unsets, merged.
pub(crate) fn fields_and_unsets(
    obj: &GqlObject,
) -> Result<(BTreeMap<String, DdbValue>, Vec<String>), String> {
    let explicit_unset = string_list(obj, "unsetFields");
    let raw = match obj.get("fields") {
        Some(GqlValue::Object(map)) => decode_json_fields_object(map)?,
        Some(GqlValue::String(_)) => return Err(
            "fields must be a JSON object, not a JSON-encoded string".to_string(),
        ),
        _ => BTreeMap::new(),
    };
    normalize_update_fields(raw, explicit_unset).map_err(|e| e.to_string())
}
```

`opt_fields_map` (batch-update "leave unchanged" semantics) keeps its `None`-
when-absent contract but is renamed `opt_fields_and_unsets` returning
`Option<(BTreeMap<String, DdbValue>, Vec<String>)>`; its two call sites in
`schema/mutations/operations.rs` (:103) already branch on `Some`/`None`, so the
branch body changes from `fields` alone to `(fields, unsets)` merged into the
existing `unset_fields` the caller already threads through.

### GraphQL: `ddb-server/src/schema/type_defs.rs`

Three `InputValue::new("fields", TypeRef::named(TypeRef::STRING))` become
`InputValue::new("fields", TypeRef::named("JSON"))` at lines 437, 448, 466 (no
other field on those three `InputObject`s changes).

### REST: `ddb-server/src/rest.rs`

```rust
#[derive(Debug, Deserialize)]
pub struct CreateBody {
    // ... unchanged ...
    pub fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBody {
    // ... unchanged ...
    pub fields: Option<serde_json::Value>,
    #[serde(rename = "unsetFields")]
    pub unset_fields: Option<Vec<String>>,
}

/// Decode a REST JSON body's `fields` object into `RawFieldsInput`. Rejects
/// anything other than `serde_json::Value::Object` (including a JSON string,
/// so a caller still sending the old encoded-string shape gets `InvalidInput`
/// rather than a silent reparse) and any non-finite number.
fn json_value_to_fields_input(v: serde_json::Value) -> Result<RawFieldsInput, String> {
    let serde_json::Value::Object(map) = v else {
        return Err("fields must be a JSON object".to_string());
    };
    map.into_iter().map(|(k, v)| Ok((k, json_scalar_to_opt_value(v)?))).collect()
}

fn json_scalar_to_opt_value(v: serde_json::Value) -> Result<Option<DdbValue>, String> {
    use serde_json::Value as J;
    Ok(match v {
        J::Null => None,
        J::String(s) => Some(DdbValue::String(s)),
        J::Number(n) => Some(DdbValue::Number(
            n.as_f64().ok_or_else(|| "non-finite number in fields".to_string())?,
        )),
        J::Bool(b) => Some(DdbValue::Bool(b)),
        J::Array(items) => Some(DdbValue::List(
            items.into_iter().map(json_scalar_to_value_strict).collect::<Result<_, _>>()?,
        )),
        J::Object(map) => Some(DdbValue::Map(
            map.into_iter()
                .map(|(k, v)| Ok((k, json_scalar_to_value_strict(v)?)))
                .collect::<Result<_, String>>()?,
        )),
    })
}
```

`create_doogat`/`update_doogat` (rest.rs:441-493) change their `match body.fields`
block from `parse_fields_json(&json_str)` to
`json_value_to_fields_input(v).and_then(|raw| normalize_create_fields(raw))` /
`normalize_update_fields(raw, body.unset_fields.unwrap_or_default())`, mapping
`DoogatError::Validation`/decode-`String` errors to `rest_error` exactly as
today.

## Data flow

1. Adapter (GraphQL dynamic resolver / REST `axum::Json` deserialize / FFI
   `Vec<FieldEntryValue>`) receives the wire payload.
2. Adapter decodes its native shape into `RawFieldsInput`
   (`BTreeMap<String, Option<Value>>`) — `None` marks an explicit top-level
   null; FFI's decode (`fields_to_core`, unchanged) always produces `Some`
   entries since there is no FFI null variant, so it is routed through
   `normalize_create_fields`/`normalize_update_fields` as a degenerate case
   (no nulls to resolve) purely to keep one chokepoint, not because FFI needs
   the policy.
3. Core `normalize_create_fields`/`normalize_update_fields` (new, in
   `app_contract`) applies the single null policy and the set/unset collision
   check, producing `(BTreeMap<String, Value>, Vec<String>)` (update) or
   `BTreeMap<String, Value>` (create).
4. The adapter builds `CreateCommand`/`UpdateCommand` exactly as before and
   calls `DoogatService::create`/`update` — nothing downstream of the command
   construction changes.

## Reuse inventory

- `async_graphql`'s `JSON` scalar is already registered (`schema/mod.rs:102`)
  and already used as an **output** type for `SearchHit.fields`
  (`type_defs.rs:140`) — reused directly as the **input** type; no new scalar.
- `ddb-core/src/app_contract/commands.rs`'s `UpdateCommand.unset_fields:
  Vec<String>` already exists (added for the CLI's `--unset`) — the null-derived
  unsets merge into this existing field; no new command field.
- `ddb-core/src/ffi/records.rs`'s `FieldEntryValue`/`FieldValue` (PRD 00177)
  already give FFI native-scalar input with no stringify step — confirmed by
  reading `fields_to_core`/`field_value_to_core`; no FFI change needed beyond
  routing create/update through the new core normalization for a single
  chokepoint.
- `ddb-server/src/rest.rs:171`'s `ddb_value_to_json` (the existing
  `DdbValue → serde_json::Value` output converter) has no inverse; greps tried:
  `from_json_value`, `json_value_to`, `serde_json::Value` decode helpers in both
  `ddb-server/src` and `ddb-core/src` — nothing found, so
  `json_value_to_fields_input`/`json_scalar_to_opt_value` in this design are new.
- Greps tried for an existing null-aware fields decoder before writing
  `fields_normalize.rs`: `Option<Value>`, `unset`, `null.*field`, `nullable` across
  `ddb-core/src/app_contract/` and `ddb-server/src/schema/` — nothing found.

## Alternatives considered

1. **Smallest-diff: keep `fields` as `String` on the wire, but have the server
   parse it as JSON-with-native-types instead of String-only (drop the
   `BTreeMap<String, String>` cast in `parse_fields_json`).** Rejected: the PRD
   explicitly requires accepting an **object**, and GraphQL variables typed as
   `String` cannot carry a JSON object — callers would still JSON.stringify
   their object into a string, which is exactly the round-trip this PRD
   removes. This only fixes REST (where `Option<String>` already holds a raw
   JSON string a caller could in principle send unparsed) and does nothing for
   GraphQL's `String`-typed input.
2. **New typed GraphQL input object (`FieldEntry { key: String, value: FieldValueUnion }`)
   mirroring the FFI `FieldEntryValue` shape**, instead of the `JSON` scalar.
   Rejected: GraphQL has no native union-of-scalars input type without a
   custom scalar anyway, and the repo already has a working `JSON` scalar
   registered and tested as an output type — inventing a second typed-input
   shape duplicates work the scalar already does and diverges from the
   existing output convention for the same data.
3. **Chosen: reuse the registered `JSON` scalar for input, decode its
   `async_graphql::Value` into the new adapter-neutral `RawFieldsInput`, and
   add one core normalization function shared by GraphQL/REST/FFI.** This is
   the smallest change that satisfies "one core policy, boundary-owned
   decoders" from the PRD's Behavior section, and reuses the scalar mechanism
   already proven in this codebase.

## Risks & edge cases

- **GraphQL variable migration break (PRD-acknowledged, no deprecation
  window)**: any existing `$fields: String` operation fails schema
  validation the moment the input type changes to `JSON`. Mitigation is
  documentation/examples/fixtures migration in Phase 2, not code — tracked by
  the PRD's own Risks section.
- **Number precision**: `Value::Number(f64)` cannot exactly represent large
  integers past 2^53; a REST/GraphQL JSON integer field beyond that range
  silently loses precision on decode. Pre-existing limitation of `Value`
  (unchanged by this PRD — `DdbValue::Number` is already `f64` for every other
  write path); not introduced here, not fixed here.
- **Likely next changes**: (1) a future PRD may want nested `Value::Map`/`List`
  fields to carry nulls inside them (not just top-level) — this design's
  `gql_scalar_to_value_strict`/`json_scalar_to_value_strict` reject nested null
  today, which is a deliberate narrow scope, not an accidental gap, and the
  next PRD would widen `Value` itself (adding a `Null` variant) rather than
  patching around it; (2) singleton create/update (`schema/mutations/singleton.rs`)
  may eventually want its own `unsetFields` argument — this design already
  routes it through `normalize_update_fields` so adding that argument later is
  a one-line `string_list` read, not a new decode path; (3) the REST surface
  may want the same strict-reject-on-string-fields error surfaced as a
  specific `INVALID_FIELDS_SHAPE` error code rather than the generic
  `Validation`/`InvalidInput` category — deferred until a consumer needs to
  distinguish it.

## Test strategy outline

- **Core unit tests** (`ddb-core/src/app_contract/fields_normalize.rs`,
  colocated `#[cfg(test)]`): native bool/number Value variants pass through
  unchanged; a `None` entry is omitted on create; a `None` entry becomes an
  unset on update; an explicit `unset_fields` entry merges and dedupes with a
  null-derived one; a key both set (`Some`) and in `explicit_unset` returns
  `DoogatError::Validation`.
- **GraphQL unit tests** (`schema/input.rs`, `schema/base_types.rs`): object
  literal with mixed scalar/bool/number/null decodes correctly; a
  `GqlValue::String` fields value (the old shape) is rejected with an error
  naming the migration; list/nested-object fields round-trip.
- **REST unit/integration tests** (`ddb-server/src/rest.rs` or
  `ddb-server/tests/`): `CreateBody`/`UpdateBody` deserialize a JSON object
  `fields`; a JSON string `fields` is rejected; null on update unsets, null on
  create omits.
- **Conformance** (PRD 00148 harness, Phase 2 task): create-with-typed-fields
  across GraphQL + REST asserts a boolean lands as a boolean column.
- **E2E** (Rust harness, `tests/e2e/`, Phase 2 task): object-form round-trip,
  null/unset behavior, rejected encoded strings — per the PRD's Critical
  Scenarios.
- Per-task local gate stays `cargo build` + `cargo clippy --workspace
  --all-targets` + `cargo test-ci`; e2e/conformance authored but not locally
  executed (delegated to CI), per `AGENTS.md`'s Definition of Done.

## Review log

- non-blocker: set/unset collision check silently dedupes a null-derived unset against an identical explicit `unsetFields` entry rather than flagging the redundancy — acceptable per PRD, no code change.
- non-blocker: design/PRD line-number citations drift a few lines from the current checkout (rest.rs, operations.rs) — close enough to locate, not exact.
- question: should sending `fields: {"x": null}` together with `unsetFields: ["x"]` warn on the redundancy instead of silently merging? Left as PRD-aligned silent dedup; revisit only if a caller reports confusion.

dispatch 1 (claude): cardinal-sin 0, blocker 1, non-blocker 2, question 1
dispatch 2 (claude-fallback): cardinal-sin 0, blocker 1, non-blocker 0, question 0

Both blockers found (dispatch 1: singleton input-type mismatch; dispatch 2: singleton update unset-threading) were fixed in the doc. No cardinal sins or open blockers remain after dispatch 2 — dispatch 3 was not run.
