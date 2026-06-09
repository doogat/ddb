use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{routing, Extension, Json, Router};
use serde::{Deserialize, Serialize};

use crate::actor::ActorHandle;
use crate::rest::ErrorBody;
use ddb_core::error::DoogatError;

#[derive(Deserialize)]
pub struct ScanParams {
    #[serde(rename = "type")]
    pub doogat_type: Option<String>,
    pub tag: Option<String>,
}

#[derive(Serialize)]
struct IdsResponse {
    ids: Vec<String>,
}

fn nosql_error(e: DoogatError) -> axum::response::Response {
    crate::http_error::http_error_response(e).into_response()
}

pub fn router() -> Router {
    Router::new()
        .route("/{id}", routing::get(get_doogat))
        .route("/", routing::get(scan))
        .route("/{id}/backlinks", routing::get(backlinks))
}

async fn get_doogat(
    Extension(actor): Extension<ActorHandle>,
    Path(id): Path<String>,
) -> axum::response::Response {
    match actor.nosql_get(id).await {
        Ok(Some(z)) => {
            let json = crate::rest::doogat_to_json(&z);
            Json(json).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorBody {
                error: "NOT_FOUND".into(),
                message: "doogat not found".into(),
            }),
        )
            .into_response(),
        Err(e) => nosql_error(e),
    }
}

async fn scan(
    Extension(actor): Extension<ActorHandle>,
    Query(params): Query<ScanParams>,
) -> axum::response::Response {
    let result = match (params.doogat_type, params.tag) {
        (Some(_), Some(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: "BAD_REQUEST".into(),
                    message: "specify ?type= or ?tag=, not both".into(),
                }),
            )
                .into_response();
        }
        (Some(t), None) => actor.nosql_scan_type(t).await,
        (None, Some(tag)) => actor.nosql_scan_tag(tag).await,
        (None, None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: "BAD_REQUEST".into(),
                    message: "specify ?type= or ?tag=".into(),
                }),
            )
                .into_response();
        }
    };

    match result {
        Ok(ids) => Json(IdsResponse { ids }).into_response(),
        Err(e) => nosql_error(e),
    }
}

async fn backlinks(
    Extension(actor): Extension<ActorHandle>,
    Path(id): Path<String>,
) -> axum::response::Response {
    match actor.nosql_backlinks(id).await {
        Ok(ids) => Json(IdsResponse { ids }).into_response(),
        Err(e) => nosql_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NoSQL HTTP errors now share REST's mapping via the one helper
    // (PRD 00143 Phase 1). The Conflict case is the load-bearing assertion:
    // before consolidation the local matcher had no Conflict arm and fell
    // through to 500; the shared helper maps it to 409.
    #[test]
    fn nosql_error_delegates_to_shared_http_helper() {
        assert_eq!(
            nosql_error(DoogatError::NotFound("x".into())).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            nosql_error(DoogatError::Validation("x".into())).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            nosql_error(DoogatError::Conflict("x".into())).status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            nosql_error(DoogatError::SqlEngine("x".into())).status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}
