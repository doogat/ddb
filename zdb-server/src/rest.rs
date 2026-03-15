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
use zdb_core::error::ZettelError;
use zdb_core::types::{ParsedZettel, Value as ZdbValue};

// ── Query / body types ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListParams {
    #[serde(rename = "type")]
    pub zettel_type: Option<String>,
    pub tag: Option<String>,
    pub q: Option<String>,
    pub backlinks: Option<String>,
    pub sort: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub title: String,
    pub body: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(rename = "type")]
    pub zettel_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBody {
    pub title: Option<String>,
    pub body: Option<String>,
    pub tags: Option<Vec<String>>,
    #[serde(rename = "type")]
    pub zettel_type: Option<String>,
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
    data: Vec<ZettelJson>,
    pagination: Pagination,
}

#[derive(Serialize)]
struct SingleResponse {
    data: ZettelJson,
}

#[derive(Serialize)]
struct SearchResponse {
    data: Vec<SearchHit>,
    total_count: usize,
}

#[derive(Serialize)]
pub struct ZettelJson {
    id: String,
    title: String,
    body: String,
    tags: Vec<String>,
    #[serde(rename = "type")]
    zettel_type: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    frontmatter: BTreeMap<String, serde_json::Value>,
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

fn zdb_value_to_json(v: ZdbValue) -> serde_json::Value {
    match v {
        ZdbValue::String(s) => serde_json::Value::String(s),
        ZdbValue::Number(n) => serde_json::json!(n),
        ZdbValue::Bool(b) => serde_json::Value::Bool(b),
        ZdbValue::List(l) => {
            serde_json::Value::Array(l.into_iter().map(zdb_value_to_json).collect())
        }
        ZdbValue::Map(m) => serde_json::Value::Object(
            m.into_iter()
                .map(|(k, v)| (k, zdb_value_to_json(v)))
                .collect(),
        ),
    }
}

pub fn zettel_to_json(z: &ParsedZettel) -> ZettelJson {
    ZettelJson {
        id: z.meta.id.as_ref().map(|i| i.0.clone()).unwrap_or_default(),
        title: z.meta.title.clone().unwrap_or_default(),
        body: z.body.clone(),
        tags: z.meta.tags.clone(),
        zettel_type: z.meta.zettel_type.clone(),
        frontmatter: z
            .meta
            .extra
            .iter()
            .map(|(k, v)| (k.clone(), zdb_value_to_json(v.clone())))
            .collect(),
        reference_section: z.reference_section.clone(),
    }
}

fn rest_error(e: ZettelError) -> (StatusCode, Json<ErrorBody>) {
    let (status, code) = match &e {
        ZettelError::NotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
        ZettelError::Validation(_) => (StatusCode::BAD_REQUEST, "VALIDATION_ERROR"),
        ZettelError::InvalidPath(_) => (StatusCode::BAD_REQUEST, "INVALID_PATH"),
        ZettelError::SqlEngine(_) => (StatusCode::UNPROCESSABLE_ENTITY, "SQL_ERROR"),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
    };
    (
        status,
        Json(ErrorBody {
            error: code.into(),
            message: e.to_string(),
        }),
    )
}

// ── Router ───────────────────────────────────────────────────────

pub fn router() -> Router {
    Router::new()
        .route("/zettels", routing::get(list_zettels).post(create_zettel))
        .route(
            "/zettels/{id}",
            routing::get(get_zettel)
                .put(update_zettel)
                .delete(delete_zettel),
        )
}

// ── Handlers ─────────────────────────────────────────────────────

async fn list_zettels(
    Extension(read_pool): Extension<ReadPool>,
    Query(params): Query<ListParams>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorBody>)> {
    // Full-text search shortcut
    if let Some(q) = params.q {
        let limit = params.per_page.unwrap_or(50) as usize;
        let page = params.page.unwrap_or(1).max(1) as usize;
        let offset = (page - 1) * limit;
        let result = read_pool
            .search(q, limit, offset)
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

    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * per_page;

    let total = read_pool
        .count_zettels(
            params.zettel_type.clone(),
            params.tag.clone(),
            params.backlinks.clone(),
            vec![],
        )
        .await
        .map_err(rest_error)?;

    let total_pages = if total == 0 {
        1
    } else {
        (total + per_page - 1) / per_page
    };

    let zettels = read_pool
        .list_zettels(
            params.zettel_type,
            params.tag,
            params.backlinks,
            vec![],
            Some(per_page),
            Some(offset),
        )
        .await
        .map_err(rest_error)?;

    let data: Vec<ZettelJson> = zettels.iter().map(zettel_to_json).collect();

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

async fn get_zettel(
    Extension(read_pool): Extension<ReadPool>,
    Path(id): Path<String>,
) -> Result<Json<SingleResponse>, (StatusCode, Json<ErrorBody>)> {
    let z = read_pool.get_zettel(id).await.map_err(rest_error)?;
    Ok(Json(SingleResponse {
        data: zettel_to_json(&z),
    }))
}

async fn create_zettel(
    Extension(actor): Extension<ActorHandle>,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<SingleResponse>), (StatusCode, Json<ErrorBody>)> {
    let z = actor
        .create_zettel(body.title, body.body, body.tags, body.zettel_type)
        .await
        .map_err(rest_error)?;
    Ok((
        StatusCode::CREATED,
        Json(SingleResponse {
            data: zettel_to_json(&z),
        }),
    ))
}

async fn update_zettel(
    Extension(actor): Extension<ActorHandle>,
    Path(id): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Result<Json<SingleResponse>, (StatusCode, Json<ErrorBody>)> {
    let z = actor
        .update_zettel(id, body.title, body.body, body.tags, body.zettel_type)
        .await
        .map_err(rest_error)?;
    Ok(Json(SingleResponse {
        data: zettel_to_json(&z),
    }))
}

async fn delete_zettel(
    Extension(actor): Extension<ActorHandle>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorBody>)> {
    actor.delete_zettel(id).await.map_err(rest_error)?;
    Ok(StatusCode::NO_CONTENT)
}
