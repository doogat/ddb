use axum::extract::{Path, Query};
use axum::{routing, Extension, Router};
use serde::{Deserialize, Serialize};

use crate::actor::ActorHandle;
use crate::rest::ErrorBody;

#[derive(Deserialize)]
pub struct ScanParams {
    #[serde(rename = "type")]
    pub zettel_type: Option<String>,
    pub tag: Option<String>,
}

#[derive(Serialize)]
struct IdsResponse {
    ids: Vec<String>,
}

pub fn router() -> Router {
    Router::new()
        .route("/{id}", routing::get(get_zettel))
        .route("/", routing::get(scan))
        .route("/{id}/backlinks", routing::get(backlinks))
}

async fn get_zettel(
    Extension(actor): Extension<ActorHandle>,
    Path(id): Path<String>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::Json;

    match actor.nosql_get(id).await {
        Ok(Some(z)) => {
            let json = crate::rest::zettel_to_json(&z);
            Json(json).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorBody {
                error: "not_found".into(),
                message: "zettel not found".into(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: "nosql_error".into(),
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn scan(
    Extension(actor): Extension<ActorHandle>,
    Query(params): Query<ScanParams>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::Json;

    let result = match (params.zettel_type, params.tag) {
        (Some(_), Some(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: "bad_request".into(),
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
                    error: "bad_request".into(),
                    message: "specify ?type= or ?tag=".into(),
                }),
            )
                .into_response();
        }
    };

    match result {
        Ok(ids) => Json(IdsResponse { ids }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: "nosql_error".into(),
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn backlinks(
    Extension(actor): Extension<ActorHandle>,
    Path(id): Path<String>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::Json;

    match actor.nosql_backlinks(id).await {
        Ok(ids) => Json(IdsResponse { ids }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: "nosql_error".into(),
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}
