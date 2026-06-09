use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{routing, Extension, Json, Router};
use serde::{Deserialize, Serialize};

use crate::actor::ActorHandle;
use crate::http_error::ErrorBody;
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

    // Extract the (status, machine-readable error code) a NoSQL route returns.
    // Asserting the body code — not just the status — is what proves NoSQL and
    // REST emit the SAME structured error a downstream client keys on; the REST
    // test (`rest_error_delegates_to_shared_http_helper`) pins the same
    // constants, so the two routes are proven equivalent.
    async fn status_and_code(resp: axum::response::Response) -> (StatusCode, String) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("error body collects");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("error body is JSON");
        let code = json["error"]
            .as_str()
            .expect("error body carries a string `error` code")
            .to_string();
        (status, code)
    }

    // NoSQL HTTP errors share REST's mapping via the one helper (PRD 00143
    // Phase 1). The Conflict case is load-bearing: before consolidation the
    // local matcher had no Conflict arm and fell through to 500; the shared
    // helper maps it to 409 with the CONFLICT code.
    #[tokio::test]
    async fn nosql_error_delegates_to_shared_http_helper() {
        async fn expect_mapping(e: DoogatError, want_status: StatusCode, want_code: &str) {
            let (status, code) = status_and_code(nosql_error(e)).await;
            assert_eq!(status, want_status);
            assert_eq!(code, want_code);
        }
        expect_mapping(
            DoogatError::NotFound("x".into()),
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
        )
        .await;
        expect_mapping(
            DoogatError::Validation("x".into()),
            StatusCode::BAD_REQUEST,
            "VALIDATION_ERROR",
        )
        .await;
        expect_mapping(
            DoogatError::Conflict("x".into()),
            StatusCode::CONFLICT,
            "CONFLICT",
        )
        .await;
        expect_mapping(
            DoogatError::SqlEngine("x".into()),
            StatusCode::UNPROCESSABLE_ENTITY,
            "SQL_ERROR",
        )
        .await;
    }
}
