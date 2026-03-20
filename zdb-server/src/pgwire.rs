use std::fmt::Debug;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::sink::Sink;
use futures_util::stream;
use tokio::net::TcpListener;

use rand::Rng as _;

use pgwire::api::auth::md5pass::{hash_md5_password, Md5PasswordAuthStartupHandler};
use pgwire::api::auth::{AuthSource, DefaultServerParameterProvider, LoginInfo, Password};
use pgwire::api::copy::NoopCopyHandler;
use pgwire::api::query::{PlaceholderExtendedQueryHandler, SimpleQueryHandler};
use pgwire::api::results::{DataRowEncoder, FieldFormat, FieldInfo, QueryResponse, Response, Tag};
use pgwire::api::{ClientInfo, PgWireHandlerFactory, Type};
use pgwire::error::{PgWireError, PgWireResult};
use pgwire::messages::PgWireBackendMessage;
use pgwire::tokio::process_socket;

use sqlparser::ast::Statement;
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

use zdb_core::sql_engine::SqlResult;

use crate::actor::ActorHandle;
use crate::read_pool::ReadPool;
use crate::reload::SchemaReloader;

/// True when `sql` is a single pure SELECT statement.
///
/// Conservative: multi-statement batches, INSERT...SELECT,
/// CREATE TABLE AS SELECT, and EXPLAIN all return false.
pub(crate) fn is_select_only(sql: &str) -> bool {
    let dialect = GenericDialect {};
    match Parser::parse_sql(&dialect, sql) {
        Ok(stmts) if stmts.len() == 1 => matches!(stmts[0], Statement::Query(_)),
        _ => false,
    }
}

// -- Auth --

#[derive(Debug)]
struct ZdbAuthSource {
    token: String,
}

#[async_trait]
impl AuthSource for ZdbAuthSource {
    async fn get_password(&self, login_info: &LoginInfo) -> PgWireResult<Password> {
        // Random 4-byte salt per connection, per PG protocol spec
        let salt: Vec<u8> = rand::thread_rng().gen::<[u8; 4]>().to_vec();
        let user = login_info.user().unwrap_or("zdb");
        let hashed = hash_md5_password(user, &self.token, &salt);
        Ok(Password::new(Some(salt), hashed.as_bytes().to_vec()))
    }
}

// -- Query handler --

struct ZdbBackend {
    actor: ActorHandle,
    read_pool: ReadPool,
    reloader: Arc<SchemaReloader>,
}

#[async_trait]
impl SimpleQueryHandler for ZdbBackend {
    async fn do_query<'a, 'b: 'a, C>(
        &'b self,
        _client: &mut C,
        query: &'a str,
    ) -> PgWireResult<Vec<Response<'a>>>
    where
        C: ClientInfo + Sink<PgWireBackendMessage> + Unpin + Send + Sync,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        let (result, upper) = if is_select_only(query) {
            let r = self
                .read_pool
                .execute_select(query.to_string())
                .await
                .map_err(|e| PgWireError::ApiError(Box::new(e)))?;
            (r, String::new())
        } else {
            let r = self
                .actor
                .execute_sql(query.to_string())
                .await
                .map_err(|e| PgWireError::ApiError(Box::new(e)))?;

            let upper = query.to_uppercase();
            if upper.contains("CREATE TABLE")
                || upper.contains("DROP TABLE")
                || upper.contains("ALTER TABLE")
            {
                self.reloader.trigger_reload_and_wait().await;
            }
            (r, upper)
        };

        let response = match result {
            SqlResult::Rows { columns, rows } => {
                let schema = Arc::new(
                    columns
                        .iter()
                        .map(|name| {
                            FieldInfo::new(
                                name.clone(),
                                None,
                                None,
                                Type::VARCHAR,
                                FieldFormat::Text,
                            )
                        })
                        .collect::<Vec<_>>(),
                );

                let data_rows: Vec<PgWireResult<_>> = rows
                    .iter()
                    .map(|row| {
                        let mut encoder = DataRowEncoder::new(schema.clone());
                        for val in row {
                            let v: &str = val;
                            encoder
                                .encode_field(&Some(v))
                                .map_err(|e| PgWireError::ApiError(Box::new(e)))?;
                        }
                        encoder.finish()
                    })
                    .collect();

                Response::Query(QueryResponse::new(schema, stream::iter(data_rows)))
            }
            SqlResult::Affected(n) => {
                let tag = command_tag_for_query(&upper);
                Response::Execution(Tag::new(tag).with_rows(n))
            }
            SqlResult::Ok(msg) => {
                let tag = normalize_ok_tag(&upper, &msg);
                Response::Execution(Tag::new(&tag))
            }
        };

        Ok(vec![response])
    }
}

/// Derive PG command tag from the SQL query for `SqlResult::Affected`.
fn command_tag_for_query(upper_query: &str) -> &'static str {
    if upper_query.starts_with("UPDATE") {
        "UPDATE"
    } else if upper_query.starts_with("DELETE") {
        "DELETE"
    } else if upper_query.starts_with("INSERT") {
        "INSERT"
    } else {
        "OK"
    }
}

/// Derive PG command tag from the SQL query for `SqlResult::Ok`.
/// INSERT returns the zettel ID (not a descriptive message), so we use the
/// query string to determine the tag rather than parsing the message.
fn normalize_ok_tag(upper_query: &str, _msg: &str) -> String {
    if upper_query.starts_with("CREATE TABLE") || upper_query.starts_with("CREATE  TABLE") {
        "CREATE TABLE".to_string()
    } else if upper_query.starts_with("DROP TABLE") || upper_query.starts_with("DROP  TABLE") {
        "DROP TABLE".to_string()
    } else if upper_query.starts_with("ALTER TABLE") || upper_query.starts_with("ALTER  TABLE") {
        "ALTER TABLE".to_string()
    } else if upper_query.starts_with("INSERT") {
        "INSERT 0 1".to_string()
    } else {
        "OK".to_string()
    }
}

// -- Server glue --

struct ZdbPgHandlers {
    auth: Arc<Md5PasswordAuthStartupHandler<ZdbAuthSource, DefaultServerParameterProvider>>,
    query: Arc<ZdbBackend>,
}

impl PgWireHandlerFactory for ZdbPgHandlers {
    type StartupHandler =
        Md5PasswordAuthStartupHandler<ZdbAuthSource, DefaultServerParameterProvider>;
    type SimpleQueryHandler = ZdbBackend;
    type ExtendedQueryHandler = PlaceholderExtendedQueryHandler;
    type CopyHandler = NoopCopyHandler;

    fn simple_query_handler(&self) -> Arc<Self::SimpleQueryHandler> {
        self.query.clone()
    }

    fn extended_query_handler(&self) -> Arc<Self::ExtendedQueryHandler> {
        Arc::new(PlaceholderExtendedQueryHandler)
    }

    fn startup_handler(&self) -> Arc<Self::StartupHandler> {
        self.auth.clone()
    }

    fn copy_handler(&self) -> Arc<Self::CopyHandler> {
        Arc::new(NoopCopyHandler)
    }
}

pub async fn start(
    actor: ActorHandle,
    read_pool: ReadPool,
    token: String,
    reloader: Arc<SchemaReloader>,
    bind: &str,
    port: u16,
) -> std::io::Result<()> {
    let auth_source = Arc::new(ZdbAuthSource { token });
    let mut params = DefaultServerParameterProvider::default();
    params.server_version = "ZettelDB 0.1".to_owned();

    let auth = Arc::new(Md5PasswordAuthStartupHandler::new(
        auth_source,
        Arc::new(params),
    ));

    let query = Arc::new(ZdbBackend {
        actor,
        read_pool,
        reloader,
    });
    let handlers = Arc::new(ZdbPgHandlers { auth, query });

    let addr = format!("{bind}:{port}");
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "pgwire listening");
    eprintln!("pgwire listening on {addr}");

    loop {
        let (socket, _) = listener.accept().await?;
        let handlers = handlers.clone();
        tokio::spawn(async move {
            if let Err(e) = process_socket(socket, None, handlers).await {
                tracing::warn!(%e, "pgwire connection error");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tag_for_query_maps_dml() {
        assert_eq!(command_tag_for_query("UPDATE"), "UPDATE");
        assert_eq!(command_tag_for_query("DELETE FROM books"), "DELETE");
        assert_eq!(command_tag_for_query("INSERT INTO"), "INSERT");
        assert_eq!(command_tag_for_query("SOMETHING ELSE"), "OK");
    }

    #[test]
    fn is_select_only_plain_select() {
        assert!(is_select_only("SELECT * FROM books"));
        assert!(is_select_only(
            "select id, title from books where year > 2020"
        ));
    }

    #[test]
    fn is_select_only_cte() {
        assert!(is_select_only(
            "WITH recent AS (SELECT * FROM books WHERE year > 2020) SELECT * FROM recent"
        ));
    }

    #[test]
    fn is_select_only_rejects_insert_select() {
        assert!(!is_select_only(
            "INSERT INTO archive SELECT * FROM books WHERE year < 2000"
        ));
    }

    #[test]
    fn is_select_only_rejects_create_table_as_select() {
        assert!(!is_select_only(
            "CREATE TABLE archive AS SELECT * FROM books"
        ));
    }

    #[test]
    fn is_select_only_rejects_ddl() {
        assert!(!is_select_only("CREATE TABLE book (id TEXT, title TEXT)"));
        assert!(!is_select_only("DROP TABLE book"));
        assert!(!is_select_only("ALTER TABLE book ADD COLUMN year TEXT"));
    }

    #[test]
    fn is_select_only_rejects_multi_statement() {
        assert!(!is_select_only("SELECT 1; SELECT 2"));
    }

    #[test]
    fn is_select_only_rejects_explain() {
        assert!(!is_select_only("EXPLAIN SELECT * FROM books"));
        assert!(!is_select_only("EXPLAIN ANALYZE SELECT * FROM books"));
    }

    #[test]
    fn normalize_ok_tag_maps_ddl_and_insert() {
        assert_eq!(
            normalize_ok_tag("CREATE TABLE book", "table book created"),
            "CREATE TABLE"
        );
        assert_eq!(normalize_ok_tag("DROP TABLE book", ""), "DROP TABLE");
        assert_eq!(
            normalize_ok_tag("ALTER TABLE book ADD COLUMN year", ""),
            "ALTER TABLE"
        );
        assert_eq!(
            normalize_ok_tag("INSERT INTO books", "20260303123456"),
            "INSERT 0 1"
        );
        assert_eq!(normalize_ok_tag("UNKNOWN STMT", "something"), "OK");
    }
}
