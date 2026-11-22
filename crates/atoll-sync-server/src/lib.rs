//! The Atoll sync server: keeps the sealed records of every account, hands out what changed and
//! tells connected devices when something did. Accounts are the `sub` of Onyx access tokens.

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
mod limit;
mod routes;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use sqlx::PgPool;
use tokio::sync::broadcast;

pub use auth::Verifier;
pub use limit::Limiter;

/// What the handlers share.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub verifier: Arc<Verifier>,
    /// Every stored change as `(account, revision)`, for the event streams.
    pub events: broadcast::Sender<(String, i64)>,
    pub limiter: Arc<Limiter>,
}

impl AppState {
    pub fn new(pool: PgPool, verifier: Verifier) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            pool,
            verifier: Arc::new(verifier),
            events,
            limiter: Arc::new(Limiter::default()),
        }
    }
}

/// The routes of the server.
pub fn app(state: AppState) -> Router {
    routes::router(state)
}

/// Removes old tombstones now and then, for as long as the server runs.
pub fn spawn_pruning(pool: PgPool, age: Duration) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            match db::prune(&pool, age).await {
                Ok(0) => {}
                Ok(removed) => tracing::info!(removed, "old tombstones removed"),
                Err(error) => tracing::warn!(%error, "old tombstones could not be removed"),
            }
        }
    });
}
