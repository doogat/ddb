use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
    routing, Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::actor::ActorHandle;
use crate::read_pool::ReadPool;
use ddb_core::error::DoogatError;
use ddb_core::service::SORTABLE_COLUMNS;
use ddb_core::types::{ParsedDoogat, SearchFilters, Value as DdbValue};

// ── Query / body types ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub title: String,
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

#[derive(Serialize)]
struct SingleResponse {
    data: DoogatJson,
}

#[derive(Serialize)]
struct SearchResponse {
    data: Vec<SearchHit>,
    total_count: usize,
}

#[derive(Serialize)]
pub struct DoogatJson {
    id: String,
    title: String,
    body: String,
    tags: Vec<String>,
    #[serde(rename = "type")]
    doogat_type: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    frontmatter: BTreeMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    references: BTreeMap<String, Vec<String>>,
    reference_section: String,
}

#[derive(Serialize)]
struct SearchHit {
    id: String,
    title: String,
    snippet: String,
    rank: f64,
}

#[derive(Serialize)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
}

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
    let status = match &e {
        DoogatError::NotFound(_) => StatusCode::NOT_FOUND,
        DoogatError::Validation(_) | DoogatError::InvalidPath(_) => StatusCode::BAD_REQUEST,
        DoogatError::SqlEngine(_) => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let (code, message) = crate::error::classify(&e);
    (
        status,
        Json(ErrorBody {
            error: code.into(),
            message,
        }),
    )
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

async fn list_doogats(
    Extension(read_pool): Extension<ReadPool>,
    Query(raw_params): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorBody>)> {
    // Extract standard params from the raw map
    let doogat_type = raw_params.get("type").cloned();
    let tag = raw_params.get("tag").cloned();
    let q = raw_params.get("q").cloned();
    let backlinks = raw_params.get("backlinks").cloned();
    let (sort_field, sort_desc) = match raw_params.get("sort").map(|s| s.as_str()) {
        Some(raw) => {
            let (desc, field) = if let Some(f) = raw.strip_prefix('-') {
                (true, f)
            } else {
                (false, raw)
            };
            if !SORTABLE_COLUMNS.contains(&field) {
                return Err(rest_error(DoogatError::Validation(format!(
                    "invalid sort field '{field}'; allowed: {}",
                    SORTABLE_COLUMNS.join(", ")
                ))));
            }
            (Some(field.to_string()), Some(desc))
        }
        None => (None, None),
    };
    let page: i64 = raw_params
        .get("page")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1);
    let per_page: i64 = raw_params
        .get("per_page")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let per_page = per_page.clamp(1, 200);

    // Extract field.* params
    let field_filters: Vec<(String, String)> = raw_params
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("field.")
                .filter(|name| !name.is_empty())
                .map(|name| (name.to_string(), v.clone()))
        })
        .collect();

    // Full-text search shortcut
    if let Some(q) = q {
        let limit = per_page as usize;
        let page_usize = page as usize;
        let offset = (page_usize - 1) * limit;
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
        return Ok(Json(
            serde_json::to_value(SearchResponse {
                data: hits,
                total_count: result.total_count,
            })
            .unwrap(),
        ));
    }

    let offset = (page - 1) * per_page;

    let total = read_pool
        .count_doogats(
            doogat_type.clone(),
            tag.clone(),
            backlinks.clone(),
            field_filters.clone(),
        )
        .await
        .map_err(rest_error)?;

    let total_pages = if total == 0 {
        1
    } else {
        (total + per_page - 1) / per_page
    };

    let doogats = read_pool
        .list_doogats(
            doogat_type,
            tag,
            backlinks,
            field_filters,
            Some(per_page),
            Some(offset),
            sort_field,
            sort_desc,
        )
        .await
        .map_err(rest_error)?;

    let data: Vec<DoogatJson> = doogats.iter().map(doogat_to_json).collect();

    Ok(Json(
        serde_json::to_value(ListResponse {
            data,
            pagination: Pagination {
                page,
                per_page,
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
    Ok(Json(SingleResponse {
        data: doogat_to_json(&z),
    }))
}

async fn create_doogat(
    Extension(actor): Extension<ActorHandle>,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<SingleResponse>), (StatusCode, Json<ErrorBody>)> {
    let z = actor
        .create_doogat(body.title, body.body, body.tags, body.doogat_type)
        .await
        .map_err(rest_error)?;
    Ok((
        StatusCode::CREATED,
        Json(SingleResponse {
            data: doogat_to_json(&z),
        }),
    ))
}

async fn update_doogat(
    Extension(actor): Extension<ActorHandle>,
    Path(id): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Result<Json<SingleResponse>, (StatusCode, Json<ErrorBody>)> {
    let z = actor
        .update_doogat(id, body.title, body.body, body.tags, body.doogat_type)
        .await
        .map_err(rest_error)?;
    Ok(Json(SingleResponse {
        data: doogat_to_json(&z),
    }))
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
    use ddb_core::types::{InlineField, Zone, DoogatId, DoogatMeta};

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
