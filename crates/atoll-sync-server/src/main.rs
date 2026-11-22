use anyhow::{Context, Result};
use atoll_sync_server::config::Config;
use atoll_sync_server::{AppState, Verifier, app, db, spawn_pruning};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("health") {
        return health_check();
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "atoll_sync_server=info".into()),
        )
        .init();
    let config = Config::from_environment()?;
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&config.database_url)
        .await
        .context("the database could not be reached")?;
    db::migrate(&pool)
        .await
        .context("the database could not be migrated")?;
    spawn_pruning(pool.clone(), config.tombstone_age);
    let verifier = Verifier::new(&config.issuer, &config.client_id, &config.jwks_url);
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("{} could not be bound", config.listen))?;
    tracing::info!(listen = %config.listen, issuer = config.issuer, "atoll sync server running");
    axum::serve(listener, app(AppState::new(pool, verifier)))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

/// `atoll-sync-server health`: asks the running server whether it is well, for the container
/// health check, which has no curl to do so.
fn health_check() -> Result<()> {
    let listen = std::env::var("ATOLL_SYNC_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    let port = listen.rsplit(':').next().unwrap_or("8080");
    ureq::get(&format!("http://127.0.0.1:{port}/healthz"))
        .call()
        .context("the server does not answer")?;
    Ok(())
}

async fn shutdown() {
    let interrupt = tokio::signal::ctrl_c();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("the terminate signal can be listened to");
    tokio::select! {
        _ = interrupt => {}
        _ = terminate.recv() => {}
    }
    tracing::info!("shutting down");
}
