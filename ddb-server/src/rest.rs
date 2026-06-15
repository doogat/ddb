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
use ddb_core::types::{ConflictAction, ListFilter, ParsedDoogat, SearchFilters, Value as DdbValue};

// ── Query / body types ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub title: Option<String>,
    pub body: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(rename = "type")]
    pub doogat_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBody {
    pub title: Option<String>,
    pub body: Option<String>,
    pub tags: Option<Vec<String>>,
    #[serde(rename = "type")]
    pub doogat_type: Option<String>,
    pub fields: Option<String>,
    #[serde(rename = "unsetFields")]
    pub unset_fields: Option<Vec<String>>,
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

#[derive(Serialize)]
struct SearchResponse {
    data: Vec<SearchHit>,
    total_count: usize,
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

#[derive(Serialize)]
struct SearchHit {
    id: String,
    title: String,
    snippet: String,
    rank: f64,
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
}

// ── Handlers ─────────────────────────────────────────────────────

struct ListParams {
    doogat_type: Option<String>,
    tag: Option<String>,
    q: Option<String>,
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
        q: raw.get("q").cloned(),
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
    let params = parse_list_params(&raw_params)?;

    if let Some(q) = params.q {
        return handle_search(read_pool, q, params.page, params.per_page).await;
    }

    fetch_paginated_list(read_pool, params).await
}

async fn handle_search(
    read_pool: ReadPool,
    q: String,
    page: i64,
    per_page: i64,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorBody>)> {
    let limit = per_page as usize;
    let offset = (page as usize - 1) * limit;
    let result = read_pool
        .search(q, limit, offset, SearchFilters::default())
        .await
        .map_err(rest_error)?;
    let hits: Vec<SearchHit> = result
        .hits
        .into_iter()
        .map(|r| SearchHit {
            id: r.id,
            title: r.title,
            snippet: r.snippet,
            rank: r.rank,
        })
        .collect();
    Ok(Json(
        serde_json::to_value(SearchResponse {
            data: hits,
            total_count: result.total_count,
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
    let output = actor
        .create_doogat(
            body.title,
            body.body,
            body.tags,
            body.doogat_type,
            std::collections::BTreeMap::new(),
            ConflictAction::Error,
        )
        .await
        .map_err(rest_error)?;
    let warnings: Vec<WarningJson> = output
        .warnings
        .into_iter()
        .map(|w| WarningJson {
            code: w.code.to_string(),
            message: w.message,
        })
        .collect();
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
    let fields = match body.fields {
        Some(json_str) => crate::schema::parse_fields_json(&json_str)
            .map_err(|msg| rest_error(ddb_core::error::DoogatError::Validation(msg)))?,
        None => std::collections::BTreeMap::new(),
    };
    let unset_fields = body.unset_fields.unwrap_or_default();
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
    let warnings: Vec<WarningJson> = output
        .warnings
        .into_iter()
        .map(|w| WarningJson {
            code: w.code.to_string(),
            message: w.message,
        })
        .collect();
    Ok(Json(SingleResponse::new(doogat_to_json(&output.value), warnings)))
}

async fn delete_doogat(
    Extension(actor): Extension<ActorHandle>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorBody>)> {
    actor.delete_doogat(id).await.map_err(rest_error)?;
    Ok(StatusCode::NO_CONTENT)
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
}
