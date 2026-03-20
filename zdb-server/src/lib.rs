pub mod actor;
pub mod auth;
pub mod config;
pub mod error;
pub mod events;
pub mod filter;
pub mod maintenance;
pub mod nosql_api;
pub mod pgwire;
pub mod read_pool;
pub mod reload;
pub mod rest;
pub mod schema;
pub mod ws;

use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use async_graphql::dynamic::Schema;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::http::header::HeaderName;
use axum::{middleware, Extension, Router};

use actor::ActorHandle;
use auth::AuthToken;
use config::ServerConfig;
use events::EventBus;

/// Run the GraphQL server.
pub async fn run(
    repo_path: PathBuf,
    port: Option<u16>,
    pg_port: Option<u16>,
    bind: Option<&str>,
    playground: bool,
) -> std::io::Result<()> {
    let cfg = ServerConfig::load(port, pg_port, bind);

    // Auth
    let token = auth::load_or_create_token(&cfg.token_file)?;
    tracing::info!(path = %cfg.token_file.display(), "auth token");

    // Attachment file serving
    let attachment_root = repo_path.join("reference");

    // Actor
    let event_bus = EventBus::new();
    let actor = ActorHandle::spawn(repo_path.clone(), event_bus)
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    // Read pool for concurrent read-only queries
    let read_pool = read_pool::ReadPool::new(repo_path, cfg.read_pool_size)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    tracing::info!(slots = cfg.read_pool_size, "read pool");

    // Fetch type schemas for dynamic schema generation
    let type_schemas = actor.get_type_schemas().await.unwrap_or_default();
    let type_count = type_schemas.len();

    // Build GraphQL schema with hot-reload support (two-phase init)
    let rest_actor = actor.clone();
    let (reloader, shared_schema) = reload::SchemaReloader::new(actor.clone(), read_pool.clone());
    let gql_schema = match schema::build_schema(
        actor,
        read_pool.clone(),
        type_schemas,
        Some(reloader.clone()),
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(%e, "failed to build initial GraphQL schema");
            return Err(std::io::Error::other(e));
        }
    };
    reloader.store_initial(gql_schema);

    // Auth-gated routes
    let mut auth_routes = Router::new()
        .route("/graphql", axum::routing::post(graphql_handler))
        .route(
            "/attachments/{zettel_id}/{filename}",
            axum::routing::get(serve_attachment),
        )
        .nest("/rest", rest::router())
        .nest("/nosql", nosql_api::router())
        .layer(Extension(attachment_root));

    if playground {
        let playground_token = token.clone();
        auth_routes = auth_routes.route(
            "/graphql",
            axum::routing::get(move || {
                let t = playground_token.clone();
                async move {
                    axum::response::Html(async_graphql::http::playground_source(
                        async_graphql::http::GraphQLPlaygroundConfig::new("/graphql")
                            .with_header("Authorization", &format!("Bearer {t}")),
                    ))
                }
            }),
        );
    }

    let auth_routes = auth_routes.layer(middleware::from_fn(auth::require_auth));

    // WebSocket route — auth handled in ws_handler via header or connection_init payload
    let ws_routes = Router::new().route("/ws", axum::routing::get(ws::ws_handler));

    // Background maintenance
    if cfg.maintenance_enabled {
        let maint_actor = rest_actor.clone();
        let interval = cfg.maintenance_interval_secs;
        tokio::spawn(async move {
            maintenance::maintenance_loop(maint_actor, interval).await;
        });
        tracing::info!(interval_secs = cfg.maintenance_interval_secs, "maintenance enabled");
    }

    let pg_actor = rest_actor.clone();
    let pg_token = token.clone();
    let pg_reloader = reloader.clone();
    let pg_read_pool = read_pool.clone();

    // Merge routers — shared extensions available to all routes
    let app = auth_routes
        .merge(ws_routes)
        .layer(Extension(AuthToken(token)))
        .layer(Extension(rest_actor))
        .layer(Extension(read_pool))
        .layer(Extension(shared_schema))
        .layer(axum::middleware::map_response(
            |mut res: axum::response::Response| async {
                res.headers_mut().insert(
                    HeaderName::from_static("x-experimental"),
                    "true".parse().unwrap(),
                );
                res
            },
        ));

    let addr = format!("{}:{}", cfg.bind, cfg.port);
    tracing::info!(%addr, "listening");
    tracing::info!(count = type_count, "type schemas loaded");

    let listener = tokio::net::TcpListener::bind(&addr).await?;

    let pg = pgwire::start(
        pg_actor,
        pg_read_pool,
        pg_token,
        pg_reloader,
        &cfg.bind,
        cfg.pg_port,
    );

    tokio::select! {
        r = axum::serve(listener, app) => r?,
        r = pg => r?,
    };
    Ok(())
}

async fn graphql_handler(
    Extension(schema): Extension<Arc<ArcSwap<Schema>>>,
    req: GraphQLRequest,
) -> GraphQLResponse {
    let schema = schema.load();
    schema.execute(req.into_inner()).await.into()
}

async fn serve_attachment(
    Extension(attachment_root): Extension<PathBuf>,
    axum::extract::Path((zettel_id, filename)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    // Prevent path traversal: zettel_id must be 14 digits, filename must be clean
    let id_ok = zettel_id.len() == 14 && zettel_id.chars().all(|c| c.is_ascii_digit());
    let name_ok = !filename.is_empty()
        && !filename.contains("..")
        && !filename.contains('/')
        && !filename.contains('\\');
    if !id_ok || !name_ok {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }

    let file_path = attachment_root.join(&zettel_id).join(&filename);
    match tokio::fs::read(&file_path).await {
        Ok(bytes) => {
            let mime = zdb_core::types::AttachmentInfo::mime_from_filename(&filename);
            (StatusCode::OK, [(header::CONTENT_TYPE, mime)], bytes).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "attachment not found").into_response(),
    }
}
