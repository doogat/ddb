use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use async_graphql::dynamic::Schema;
use async_graphql::http::ALL_WEBSOCKET_PROTOCOLS;
use async_graphql_axum::{GraphQLProtocol, GraphQLWebSocket};
use axum::extract::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;

use crate::auth::{validate_bearer, AuthToken};

/// WebSocket upgrade handler for GraphQL subscriptions.
///
/// Dual-path auth: native clients send `Authorization: Bearer <token>` on the
/// HTTP upgrade request; browser clients omit the header and authenticate via
/// the `connection_init` payload (`{ "Authorization": "Bearer <token>" }`).
///
/// Keepalive: async-graphql sends periodic pings per the graphql-ws protocol.
/// `keepalive_timeout` closes the connection if a pong isn't received within 30s.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    protocol: GraphQLProtocol,
    Extension(shared_schema): Extension<Arc<ArcSwap<Schema>>>,
    Extension(auth): Extension<AuthToken>,
    headers: axum::http::HeaderMap,
) -> Response {
    let header_value = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let has_header = !header_value.is_empty();

    // Header present but invalid — reject at upgrade time
    if has_header && !validate_bearer(header_value, &auth.0) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let schema = (*shared_schema.load_full()).clone();

    if has_header {
        // Header auth valid — permissive on_connection_init
        ws.protocols(ALL_WEBSOCKET_PROTOCOLS)
            .on_upgrade(move |stream| {
                GraphQLWebSocket::new(stream, schema, protocol)
                    .on_connection_init(|_| async { Ok(async_graphql::Data::default()) })
                    .keepalive_timeout(Duration::from_secs(30))
                    .serve()
            })
            .into_response()
    } else {
        // No header — require token in connection_init payload
        let expected = auth.0.clone();
        ws.protocols(ALL_WEBSOCKET_PROTOCOLS)
            .on_upgrade(move |stream| {
                GraphQLWebSocket::new(stream, schema, protocol)
                    .on_connection_init(move |payload| async move {
                        let token = payload
                            .get("Authorization")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if validate_bearer(token, &expected) {
                            Ok(async_graphql::Data::default())
                        } else if token.is_empty() {
                            Err(async_graphql::Error::new("Missing authentication"))
                        } else {
                            Err(async_graphql::Error::new("Invalid authentication"))
                        }
                    })
                    .keepalive_timeout(Duration::from_secs(30))
                    .serve()
            })
            .into_response()
    }
}
