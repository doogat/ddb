use axum::{
    extract::{Path, Query},
    http::StatusCode,
    routing, Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::actor::ActorHandle;
use crate::read_pool::ReadPool;
use ddb_core::error::DoogatError;
use ddb_core::service::SORTABLE_COLUMNS;
use ddb_core::app_contract::{PageRequest, SearchCommand};
use ddb_core::types::{ConflictAction, ListFilter, ParsedDoogat, Value as DdbValue};

// ── Query / body types ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub title: Option<String>,
    pub body: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(rename = "type")]
    pub doogat_type: Option<String>,
    pub fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBody {
    pub title: Option<String>,
    pub body: Option<String>,
    pub tags: Option<Vec<String>>,
    #[serde(rename = "type")]
    pub doogat_type: Option<String>,
    pub fields: Option<serde_json::Value>,
    #[serde(rename = "unsetFields")]
    pub unset_fields: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct ApplySchemaBody {
    pub schema: String,
    #[serde(rename = "dryRun", default)]
    pub dry_run: bool,
    #[serde(rename = "allowDestructive", default)]
    pub allow_destructive: bool,
}

// ── Response types ───────────────────────────────────────────────

#[derive(Serialize)]
struct Pagination {
    page: i64,
    per_page: i64,
    total: i64,
    total_pages: i64,
}

#[derive(Serialize)]
struct ListResponse {
    data: Vec<DoogatJson>,
    pagination: Pagination,
}

/// A single warning entry in a REST response. `code` is a stable identifier;
/// `message` is human-readable. Always present in `SingleResponse.warnings`
/// (never skipped when empty) so REST clients can rely on the field.
#[derive(Serialize)]
pub struct WarningJson {
    pub code: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct SingleResponse {
    pub data: DoogatJson,
    pub warnings: Vec<WarningJson>,
}

impl SingleResponse {
    fn new(data: DoogatJson, warnings: Vec<WarningJson>) -> Self {
        Self { data, warnings }
    }
}

/// REST envelope for `POST /schema/apply`: the apply report under `data`, plus
/// the always-present `warnings` array (same envelope shape as the other
/// mutating REST routes).
#[derive(Serialize)]
pub struct SchemaApplyResponse {
    pub data: ddb_core::schema_diff::plan::SchemaApplyReport,
    pub warnings: Vec<WarningJson>,
}

/// Map app-contract warnings into the REST `WarningJson` wire shape. Shared by
/// the create and update handlers so the conversion stays single-sourced.
fn warnings_to_json(warnings: Vec<ddb_core::app_contract::AppWarning>) -> Vec<WarningJson> {
    warnings
        .into_iter()
        .map(|w| WarningJson {
            code: w.code.to_string(),
            message: w.message,
        })
        .collect()
}

#[derive(Serialize)]
struct SearchResponse {
    data: Vec<SearchHit>,
    total_count: usize,
    warnings: Vec<WarningJson>,
}

#[derive(Serialize)]
pub struct DoogatJson {
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    #[serde(rename = "type")]
    pub doogat_type: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub frontmatter: BTreeMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub references: BTreeMap<String, Vec<String>>,
    pub reference_section: String,
}

/// One REST search hit: every core `SearchResult` field, serialized from the
/// core type (no transport-local field subset).
#[derive(Serialize)]
struct SearchHit {
    id: String,
    title: String,
    path: String,
    snippet: String,
    rank: f64,
    updated_at: String,
    tags: Vec<String>,
    #[serde(rename = "type")]
    doogat_type: Option<String>,
    fields: BTreeMap<String, String>,
    created_at: Option<String>,
}

impl From<ddb_core::types::SearchResult> for SearchHit {
    fn from(r: ddb_core::types::SearchResult) -> Self {
        Self {
            id: r.id,
            title: r.title,
            path: r.path,
            snippet: r.snippet,
            rank: r.rank,
            updated_at: r.updated_at,
            tags: r.tags,
            doogat_type: r.doogat_type,
            fields: r.fields.unwrap_or_default(),
            created_at: r.created_at,
        }
    }
}

// `ErrorBody` now lives in `http_error` (one shape for every HTTP surface).
// Re-exported here so the pre-existing `ddb_server::rest::ErrorBody` public
// path keeps resolving after PRD 00143 moved the type out of this module.
pub use crate::http_error::ErrorBody;

// ── Conversions ──────────────────────────────────────────────────

fn ddb_value_to_json(v: DdbValue) -> serde_json::Value {
    match v {
        DdbValue::String(s) => serde_json::Value::String(s),
        DdbValue::Number(n) => serde_json::json!(n),
        DdbValue::Bool(b) => serde_json::Value::Bool(b),
        DdbValue::List(l) => {
            serde_json::Value::Array(l.into_iter().map(ddb_value_to_json).collect())
        }
        DdbValue::Map(m) => serde_json::Value::Object(
            m.into_iter()
                .map(|(k, v)| (k, ddb_value_to_json(v)))
                .collect(),
        ),
    }
}

pub fn doogat_to_json(z: &ParsedDoogat) -> DoogatJson {
    // Group reference-zone inline fields by key into arrays
    let mut references: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for field in &z.inline_fields {
        if matches!(field.zone, ddb_core::types::Zone::Reference) {
            references
                .entry(field.key.clone())
                .or_default()
                .push(field.value.clone());
        }
    }

    DoogatJson {
        id: z.meta.id.as_ref().map(|i| i.0.clone()).unwrap_or_default(),
        title: z.meta.title.clone().unwrap_or_default(),
        body: z.body.clone(),
        tags: z.meta.tags.clone(),
        doogat_type: z.meta.doogat_type.clone(),
        frontmatter: z
            .meta
            .extra
            .iter()
            .map(|(k, v)| (k.clone(), ddb_value_to_json(v.clone())))
            .collect(),
        references,
        reference_section: z.reference_section.clone(),
    }
}

/// Decode the REST `fields` JSON object into the core fields input. Each
/// top-level key maps to `Some(value)`, or to `None` for an explicit `null`.
/// Any non-object value is rejected (the old encoded-string shape included).
fn json_value_to_fields_input(
    v: serde_json::Value,
) -> Result<BTreeMap<String, Option<DdbValue>>, String> {
    match v {
        serde_json::Value::Object(map) => map
            .into_iter()
            .map(|(k, v)| json_scalar_to_opt_value(v).map(|v| (k, v)))
            .collect(),
        other => Err(format!("fields must be a JSON object, got {other}")),
    }
}

/// Decode one top-level field value: JSON `null` is the per-field `None`
/// signal; anything else decodes strictly.
fn json_scalar_to_opt_value(v: serde_json::Value) -> Result<Option<DdbValue>, String> {
    match v {
        serde_json::Value::Null => Ok(None),
        other => json_scalar_to_value_strict(other).map(Some),
    }
}

/// Decode a JSON value with native types kept; a `null` nested inside a
/// list or object is rejected.
fn json_scalar_to_value_strict(v: serde_json::Value) -> Result<DdbValue, String> {
    use serde_json::Value as J;
    match v {
        J::Null => Err("null is only allowed as a top-level field value".to_string()),
        J::Bool(b) => Ok(DdbValue::Bool(b)),
        J::Number(n) => n
            .as_f64()
            .map(DdbValue::Number)
            .ok_or_else(|| format!("number {n} is not representable")),
        J::String(s) => Ok(DdbValue::String(s)),
        J::Array(items) => items
            .into_iter()
            .map(json_scalar_to_value_strict)
            .collect::<Result<_, _>>()
            .map(DdbValue::List),
        J::Object(map) => map
            .into_iter()
            .map(|(k, v)| json_scalar_to_value_strict(v).map(|v| (k, v)))
            .collect::<Result<_, _>>()
            .map(DdbValue::Map),
    }
}

fn rest_error(e: DoogatError) -> (StatusCode, Json<ErrorBody>) {
    crate::http_error::http_error_response(e)
}

// ── Router ───────────────────────────────────────────────────────

pub fn router() -> Router {
    Router::new()
        .route("/doogats", routing::get(list_doogats).post(create_doogat))
        .route(
            "/doogats/{id}",
            routing::get(get_doogat)
                .put(update_doogat)
                .delete(delete_doogat),
        )
        .route("/schema/apply", routing::post(apply_schema))
}

// ── Handlers ─────────────────────────────────────────────────────

struct ListParams {
    doogat_type: Option<String>,
    tag: Option<String>,
    backlinks: Option<String>,
    sort_field: Option<String>,
    sort_desc: Option<bool>,
    page: i64,
    per_page: i64,
    field_filters: Vec<(String, String)>,
}

type RestResult<T> = Result<T, (StatusCode, Json<ErrorBody>)>;

fn parse_sort_param(raw: &str) -> RestResult<(Option<String>, Option<bool>)> {
    let (desc, field) = if let Some(f) = raw.strip_prefix('-') {
        (Some(true), f)
    } else {
        (None, raw)
    };
    if !SORTABLE_COLUMNS.contains(&field) {
        return Err(rest_error(DoogatError::Validation(format!(
            "invalid sort field '{field}'; allowed: {}",
            SORTABLE_COLUMNS.join(", ")
        ))));
    }
    Ok((Some(field.to_string()), desc))
}

fn parse_list_params(
    raw: &std::collections::HashMap<String, String>,
) -> Result<ListParams, (StatusCode, Json<ErrorBody>)> {
    let (sort_field, sort_desc) = match raw.get("sort") {
        Some(s) => parse_sort_param(s)?,
        None => (None, None),
    };
    let page: i64 = match raw.get("page") {
        Some(v) => v.parse().map_err(|_| {
            rest_error(DoogatError::Validation(format!(
                "invalid page value '{v}'; must be a positive integer"
            )))
        })?,
        None => 1,
    }
    .max(1);
    let per_page: i64 = match raw.get("per_page") {
        Some(v) => v.parse().map_err(|_| {
            rest_error(DoogatError::Validation(format!(
                "invalid per_page value '{v}'; must be an integer 1-200"
            )))
        })?,
        None => 50,
    };
    let field_filters = raw
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("field.")
                .filter(|name| !name.is_empty())
                .map(|name| (name.to_string(), v.clone()))
        })
        .collect();

    Ok(ListParams {
        doogat_type: raw.get("type").cloned(),
        tag: raw.get("tag").cloned(),
        backlinks: raw.get("backlinks").cloned(),
        sort_field,
        sort_desc,
        page,
        per_page: per_page.clamp(1, 200),
        field_filters,
    })
}

async fn list_doogats(
    Extension(read_pool): Extension<ReadPool>,
    Query(raw_params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorBody>)> {
    // Search decodes its own page window (validated, unclamped); only the plain
    // list path keeps `parse_list_params`'s 1-200 clamp.
    if let Some(q) = raw_params.get("q") {
        let cmd = SearchCommand {
            query: q.clone(),
            page: decode_search_page(&raw_params)?,
            types: raw_params.get("type").map(|t| vec![t.clone()]),
            tag: raw_params.get("tag").cloned(),
        };
        return handle_search(read_pool, cmd).await;
    }

    let params = parse_list_params(&raw_params)?;
    fetch_paginated_list(read_pool, params).await
}

/// Decode REST `page`/`per_page` into a validated [`PageRequest`]. `page`
/// defaults to 1 and must be >= 1; `per_page` defaults to
/// [`PageRequest::DEFAULT_LIMIT`]. Out-of-range values are 400
/// `VALIDATION_ERROR`, never clamped.
fn decode_search_page(raw: &std::collections::HashMap<String, String>) -> RestResult<PageRequest> {
    let page: i64 = match raw.get("page") {
        Some(v) => v.parse().map_err(|_| {
            rest_error(DoogatError::Validation(format!(
                "invalid page value '{v}'; must be a positive integer"
            )))
        })?,
        None => 1,
    };
    if page < 1 {
        return Err(rest_error(DoogatError::Validation(format!(
            "page must be >= 1, got {page}"
        ))));
    }
    let per_page: Option<i64> = match raw.get("per_page") {
        Some(v) => Some(v.parse().map_err(|_| {
            rest_error(DoogatError::Validation(format!(
                "invalid per_page value '{v}'; must be an integer 1-{}",
                PageRequest::MAX_LIMIT
            )))
        })?),
        None => None,
    };
    let per_page_for_offset = per_page.unwrap_or(PageRequest::DEFAULT_LIMIT as i64);
    let offset = per_page_for_offset.checked_mul(page - 1).ok_or_else(|| {
        rest_error(DoogatError::Validation(format!(
            "page {page} with per_page {per_page_for_offset} overflows the result offset"
        )))
    })?;
    PageRequest::decode(per_page, Some(offset)).map_err(rest_error)
}

async fn handle_search(
    read_pool: ReadPool,
    cmd: SearchCommand,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorBody>)> {
    let output = read_pool.search_command(cmd).await.map_err(rest_error)?;
    let warnings = warnings_to_json(output.warnings);
    Ok(Json(
        serde_json::to_value(SearchResponse {
            data: output.value.items.into_iter().map(SearchHit::from).collect(),
            total_count: output.value.total,
            warnings,
        })
        .unwrap(),
    ))
}

async fn fetch_paginated_list(
    read_pool: ReadPool,
    params: ListParams,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorBody>)> {
    let offset = (params.page - 1) * params.per_page;

    let total = read_pool
        .count_doogats(
            params.doogat_type.clone(),
            params.tag.clone(),
            params.backlinks.clone(),
            params.field_filters.clone(),
        )
        .await
        .map_err(rest_error)?;

    let total_pages = if total == 0 {
        1
    } else {
        (total + params.per_page - 1) / params.per_page
    };

    let doogats = read_pool
        .list_doogats(ListFilter {
            doogat_type: params.doogat_type,
            tag: params.tag,
            backlinks_of: params.backlinks,
            field_filters: params.field_filters,
            limit: Some(params.per_page),
            offset: Some(offset),
            sort_field: params.sort_field,
            sort_desc: params.sort_desc,
        })
        .await
        .map_err(rest_error)?;

    let data: Vec<DoogatJson> = doogats.iter().map(doogat_to_json).collect();

    Ok(Json(
        serde_json::to_value(ListResponse {
            data,
            pagination: Pagination {
                page: params.page,
                per_page: params.per_page,
                total,
                total_pages,
            },
        })
        .unwrap(),
    ))
}

async fn get_doogat(
    Extension(read_pool): Extension<ReadPool>,
    Path(id): Path<String>,
) -> Result<Json<SingleResponse>, (StatusCode, Json<ErrorBody>)> {
    let z = read_pool.get_doogat(id).await.map_err(rest_error)?;
    Ok(Json(SingleResponse::new(doogat_to_json(&z), vec![])))
}

async fn create_doogat(
    Extension(actor): Extension<ActorHandle>,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<SingleResponse>), (StatusCode, Json<ErrorBody>)> {
    let raw = match body.fields {
        Some(v) => json_value_to_fields_input(v)
            .map_err(|msg| rest_error(DoogatError::Validation(msg)))?,
        None => BTreeMap::new(),
    };
    let fields = ddb_core::app_contract::normalize_create_fields(raw);
    let output = actor
        .create_doogat(
            body.title,
            body.body,
            body.tags,
            body.doogat_type,
            fields,
            ConflictAction::Error,
        )
        .await
        .map_err(rest_error)?;
    let warnings = warnings_to_json(output.warnings);
    Ok((
        StatusCode::CREATED,
        Json(SingleResponse::new(doogat_to_json(&output.value), warnings)),
    ))
}

async fn update_doogat(
    Extension(actor): Extension<ActorHandle>,
    Path(id): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Result<Json<SingleResponse>, (StatusCode, Json<ErrorBody>)> {
    let raw = match body.fields {
        Some(v) => json_value_to_fields_input(v)
            .map_err(|msg| rest_error(DoogatError::Validation(msg)))?,
        None => BTreeMap::new(),
    };
    let (fields, unset_fields) = ddb_core::app_contract::normalize_update_fields(
        raw,
        body.unset_fields.unwrap_or_default(),
    )
    .map_err(rest_error)?;
    let output = actor
        .update_doogat(crate::actor::UpdateDoogatParams {
            id,
            title: body.title,
            body: body.body,
            tags: body.tags,
            doogat_type: body.doogat_type,
            fields,
            unset_fields,
        })
        .await
        .map_err(rest_error)?;
    let warnings = warnings_to_json(output.warnings);
    Ok(Json(SingleResponse::new(doogat_to_json(&output.value), warnings)))
}

async fn delete_doogat(
    Extension(actor): Extension<ActorHandle>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorBody>)> {
    actor.delete_doogat(id).await.map_err(rest_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn apply_schema(
    Extension(actor): Extension<ActorHandle>,
    Json(body): Json<ApplySchemaBody>,
) -> Result<Json<SchemaApplyResponse>, (StatusCode, Json<ErrorBody>)> {
    let output = actor
        .apply_schema(body.schema, body.dry_run, body.allow_destructive)
        .await
        .map_err(rest_error)?;
    let warnings = warnings_to_json(output.warnings);
    Ok(Json(SchemaApplyResponse {
        data: output.value,
        warnings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddb_core::types::{DoogatId, DoogatMeta, InlineField, Zone};

    // rest_error must stay in lockstep with the shared HTTP helper so REST and
    // NoSQL produce one error shape (PRD 00143 Phase 1). Pinning (status, code)
    // against the documented constants is the regression signal; the NoSQL test
    // (`nosql_error_delegates_to_shared_http_helper`) pins the same constants on
    // the other route, so the two are proven equivalent without a tautology.
    #[test]
    fn rest_error_delegates_to_shared_http_helper() {
        fn check(make: impl Fn() -> DoogatError, want_status: StatusCode, want_code: &str) {
            let (status, body) = rest_error(make());
            assert_eq!(status, want_status);
            assert_eq!(body.0.error, want_code);
        }
        check(
            || DoogatError::NotFound("x".into()),
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
        );
        check(
            || DoogatError::Conflict("x".into()),
            StatusCode::CONFLICT,
            "CONFLICT",
        );
        check(
            || DoogatError::BadRequest("x".into()),
            StatusCode::BAD_REQUEST,
            "BAD_REQUEST",
        );
        // VALIDATION_ERROR is also pinned by the NoSQL test, so both routes
        // assert the same code for a validation failure (cross-route parity).
        check(
            || DoogatError::Validation("x".into()),
            StatusCode::BAD_REQUEST,
            "VALIDATION_ERROR",
        );
        check(
            || DoogatError::SqlEngine("x".into()),
            StatusCode::UNPROCESSABLE_ENTITY,
            "SQL_ERROR",
        );
    }

    #[test]
    fn rest_json_reference_arrays() {
        let z = ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260301140100".into())),
                title: Some("Test".into()),
                date: None,
                tags: vec![],
                doogat_type: Some("bookmark".into()),
                extra: std::collections::BTreeMap::new(),
            },
            body: String::new(),
            sections: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            inline_fields: vec![
                InlineField {
                    key: "category".into(),
                    value: "20260301120100".into(),
                    zone: Zone::Reference,
                },
                InlineField {
                    key: "category".into(),
                    value: "20260301120101".into(),
                    zone: Zone::Reference,
                },
                InlineField {
                    key: "author".into(),
                    value: "20260301120200".into(),
                    zone: Zone::Reference,
                },
            ],
            reference_section: String::new(),
            path: "ddb/20260301140100.md".into(),
            updated_at: None,
        };

        let json = doogat_to_json(&z);
        assert_eq!(
            json.references.get("category").unwrap(),
            &vec!["20260301120100".to_string(), "20260301120101".to_string()]
        );
        assert_eq!(
            json.references.get("author").unwrap(),
            &vec!["20260301120200".to_string()]
        );
    }

    // ── fields boundary decoder (PRD 00178) ──────────────────────────

    use serde_json::json;

    #[test]
    fn fields_object_decodes_native_values_without_string_coercion() {
        let raw = json_value_to_fields_input(json!({
            "s": "true",
            "n": 42,
            "f": -1.5,
            "b_true": true,
            "b_false": false,
            "l": ["a", 2, true],
            "m": {"inner": -1.5, "flag": false, "Inner": "upper"},
            "tenth": 0.1,
            "huge": 1e300,
            "big_int": 9_007_199_254_740_993_u64,
            "n_str": "42",
            "inf": "inf",
            "ll": [[1, 2], []],
            "lm": [{"k": 1}],
            "MixedCase": "v",
        }))
        .expect("a JSON object of scalars and containers must decode");

        let mut nested = BTreeMap::new();
        nested.insert("inner".to_string(), DdbValue::Number(-1.5));
        nested.insert("flag".to_string(), DdbValue::Bool(false));
        nested.insert("Inner".to_string(), DdbValue::String("upper".into()));
        let mut list_item_map = BTreeMap::new();
        list_item_map.insert("k".to_string(), DdbValue::Number(1.0));
        let expected = [
            ("s", DdbValue::String("true".into())),
            ("n", DdbValue::Number(42.0)),
            ("f", DdbValue::Number(-1.5)),
            ("b_true", DdbValue::Bool(true)),
            ("b_false", DdbValue::Bool(false)),
            (
                "l",
                DdbValue::List(vec![
                    DdbValue::String("a".into()),
                    DdbValue::Number(2.0),
                    DdbValue::Bool(true),
                ]),
            ),
            ("m", DdbValue::Map(nested)),
            // f64 precision: none of these survive an f32 round trip.
            ("tenth", DdbValue::Number(0.1)),
            ("huge", DdbValue::Number(1e300)),
            ("big_int", DdbValue::Number(9_007_199_254_740_993_u64 as f64)),
            // Numeric-looking strings stay strings.
            ("n_str", DdbValue::String("42".into())),
            ("inf", DdbValue::String("inf".into())),
            // Nested lists and objects inside lists keep their shape.
            (
                "ll",
                DdbValue::List(vec![
                    DdbValue::List(vec![DdbValue::Number(1.0), DdbValue::Number(2.0)]),
                    DdbValue::List(vec![]),
                ]),
            ),
            ("lm", DdbValue::List(vec![DdbValue::Map(list_item_map)])),
            // Keys keep their case, top-level and nested.
            ("MixedCase", DdbValue::String("v".into())),
        ];
        assert_eq!(raw.len(), expected.len(), "no key may be dropped, got {raw:?}");
        for (key, want) in expected {
            assert_eq!(
                raw.get(key),
                Some(&Some(want)),
                "'{key}' must keep its native JSON type, got {raw:?}"
            );
        }
    }

    #[test]
    fn fields_empty_values_stay_present_not_null() {
        let raw = json_value_to_fields_input(json!({"s": "", "l": [], "m": {}}))
            .expect("empty values must decode");
        assert_eq!(raw.get("s"), Some(&Some(DdbValue::String(String::new()))));
        assert_eq!(raw.get("l"), Some(&Some(DdbValue::List(vec![]))));
        assert_eq!(raw.get("m"), Some(&Some(DdbValue::Map(BTreeMap::new()))));
    }

    #[test]
    fn fields_empty_object_decodes_to_no_entries() {
        let raw = json_value_to_fields_input(json!({})).expect("{} must decode");
        assert!(raw.is_empty(), "got {raw:?}");
    }

    #[test]
    fn fields_top_level_null_becomes_a_none_entry_not_a_dropped_key() {
        let raw = json_value_to_fields_input(json!({"gone": null, "kept": 7}))
            .expect("a top-level null is a valid per-field signal");
        assert_eq!(
            raw.get("gone"),
            Some(&None),
            "a null field must stay as an explicit None so create omits and update unsets it"
        );
        assert_eq!(raw.get("kept"), Some(&Some(DdbValue::Number(7.0))));
        assert_eq!(raw.len(), 2, "got {raw:?}");

        assert_eq!(
            json_scalar_to_opt_value(serde_json::Value::Null),
            Ok(None),
            "a top-level JSON null maps to None"
        );
        assert_eq!(
            json_scalar_to_opt_value(json!("x")),
            Ok(Some(DdbValue::String("x".into()))),
            "a non-null value must decode to Some"
        );
        assert_eq!(
            json_scalar_to_opt_value(json!(0.1)),
            Ok(Some(DdbValue::Number(0.1)))
        );
        assert_eq!(
            json_scalar_to_opt_value(json!(false)),
            Ok(Some(DdbValue::Bool(false)))
        );
        assert_eq!(
            json_scalar_to_opt_value(json!(["a", 1])),
            Ok(Some(DdbValue::List(vec![
                DdbValue::String("a".into()),
                DdbValue::Number(1.0),
            ])))
        );
        let nested_null = json_scalar_to_opt_value(json!(["a", null]));
        assert!(
            nested_null.is_err(),
            "a null nested in a list is not a top-level signal, got {nested_null:?}"
        );
    }

    #[test]
    fn fields_rejects_null_nested_inside_a_list_or_object() {
        for (case, fields) in [
            ("null in list", json!({"l": ["a", null]})),
            ("null as only list item", json!({"l": [null]})),
            ("null in object", json!({"m": {"inner": null}})),
            ("null in list inside list", json!({"l": [[1, null]]})),
            ("null in object inside list", json!({"l": [{"k": null}]})),
            ("null in object inside object", json!({"m": {"a": {"b": null}}})),
            ("null in list inside object", json!({"m": {"l": [null]}})),
            ("null in list three lists deep", json!({"l": [[[null]]]})),
        ] {
            let result = json_value_to_fields_input(fields);
            assert!(
                result.is_err(),
                "{case}: null is only a top-level signal and must be rejected, got {result:?}"
            );
        }
    }

    #[test]
    fn fields_rejects_any_non_object_top_level_value() {
        for (case, fields) in [
            ("old encoded-string shape", json!("{\"a\":1}")),
            ("plain string", json!("hello")),
            ("number", json!(5)),
            ("array", json!([{"a": 1}])),
            ("bool true", json!(true)),
            ("bool false", json!(false)),
            ("null", serde_json::Value::Null),
        ] {
            let result = json_value_to_fields_input(fields);
            assert!(
                result.is_err(),
                "{case}: fields must be a JSON object, never reparsed, got {result:?}"
            );
        }
    }

    #[test]
    fn create_body_accepts_a_fields_object_and_reads_null_or_absent_as_none() {
        let body: CreateBody =
            serde_json::from_value(json!({"title": "t", "fields": {"n": 1, "b": true}}))
                .expect("CreateBody must accept fields as a JSON object");
        assert_eq!(body.fields, Some(json!({"n": 1, "b": true})));

        let body: CreateBody = serde_json::from_value(json!({"title": "t", "fields": null}))
            .expect("fields: null must deserialize");
        assert_eq!(body.fields, None, "fields: null means no fields");

        let body: CreateBody =
            serde_json::from_value(json!({"title": "t"})).expect("absent fields must deserialize");
        assert_eq!(body.fields, None, "absent fields means no fields");
    }

    #[test]
    fn update_body_accepts_a_fields_object_and_keeps_unset_fields_rename() {
        let body: UpdateBody = serde_json::from_value(json!({
            "fields": {"theme": "dark", "legacy": null},
            "unsetFields": ["old"],
        }))
        .expect("UpdateBody must accept fields as a JSON object");
        assert_eq!(body.fields, Some(json!({"theme": "dark", "legacy": null})));
        assert_eq!(body.unset_fields, Some(vec!["old".to_string()]));

        let body: UpdateBody = serde_json::from_value(json!({"fields": null}))
            .expect("fields: null must deserialize");
        assert_eq!(body.fields, None, "fields: null means no fields");
        assert_eq!(body.unset_fields, None);

        let body: UpdateBody =
            serde_json::from_value(json!({"title": "t"})).expect("absent fields must deserialize");
        assert_eq!(body.fields, None, "absent fields means no fields");
    }
}

#[cfg(test)]
mod search_tests;
