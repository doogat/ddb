use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use async_graphql::dynamic::{FieldFuture, Schema, TypeRef};
use tokio::sync::Notify;

use crate::actor::ActorHandle;
use crate::read_pool::ReadPool;
use crate::schema;

/// Orchestrates hot schema reload after typedef mutations.
pub struct SchemaReloader {
    shared: Arc<ArcSwap<Schema>>,
    trigger: Notify,
    done: Notify,
    version: AtomicU64,
}

impl SchemaReloader {
    /// Create a reloader and spawn its background reload task.
    ///
    /// The returned `ArcSwap` is initially empty (placeholder schema).
    /// Call [`Self::store_initial`] after building the real schema.
    pub fn new(actor: ActorHandle, read_pool: ReadPool) -> (Arc<Self>, Arc<ArcSwap<Schema>>) {
        // Placeholder schema — replaced by store_initial before serving requests
        let placeholder = Schema::build("Query", None, None)
            .register(async_graphql::dynamic::Object::new("Query").field(
                async_graphql::dynamic::Field::new(
                    "_placeholder",
                    async_graphql::dynamic::TypeRef::named(TypeRef::BOOLEAN),
                    |_| FieldFuture::new(async { Ok(None::<async_graphql::dynamic::FieldValue>) }),
                ),
            ))
            .finish()
            .expect("placeholder schema");
        let shared = Arc::new(ArcSwap::from_pointee(placeholder));

        let reloader = Arc::new(Self {
            shared: shared.clone(),
            trigger: Notify::new(),
            done: Notify::new(),
            version: AtomicU64::new(1),
        });

        // Spawn background reload loop
        let r = reloader.clone();
        tokio::spawn(async move {
            Self::reload_loop(r, actor, read_pool).await;
        });

        (reloader, shared)
    }

    /// Store the initial schema built at startup.
    pub fn store_initial(&self, schema: Schema) {
        self.shared.store(Arc::new(schema));
    }

    /// Signal the background task to rebuild the schema and wait for completion.
    ///
    /// Returns `false` if the reload did not complete within 5 seconds.
    pub async fn trigger_reload_and_wait(&self) -> bool {
        // Register waiter before triggering so we don't miss the completion signal
        let done = self.done.notified();
        self.trigger.notify_one();
        if tokio::time::timeout(Duration::from_secs(5), done)
            .await
            .is_err()
        {
            tracing::warn!("schema reload timed out after 5s");
            return false;
        }
        true
    }

    /// Current schema version (increments on each successful reload).
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    async fn reload_loop(this: Arc<Self>, actor: ActorHandle, read_pool: ReadPool) {
        loop {
            this.trigger.notified().await;

            // Coalesce rapid signals — drain any pending notifications
            tokio::task::yield_now().await;

            let type_schemas = match actor.get_type_schemas().await {
                Ok(ts) => ts,
                Err(e) => {
                    tracing::error!(%e, "schema reload: failed to get type schemas");
                    this.done.notify_waiters();
                    continue;
                }
            };

            let new_schema = match schema::build_schema(
                actor.clone(),
                read_pool.clone(),
                type_schemas,
                Some(Arc::clone(&this)),
            ) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(%e, "schema reload: build failed");
                    this.done.notify_waiters();
                    continue;
                }
            };
            this.shared.store(Arc::new(new_schema));
            this.version.fetch_add(1, Ordering::Relaxed);
            tracing::info!(version = this.version.load(Ordering::Relaxed), "schema reloaded");
            this.done.notify_waiters();
        }
    }
}
