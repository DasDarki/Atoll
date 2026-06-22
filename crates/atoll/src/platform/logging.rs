use tracing_subscriber::EnvFilter;

/// Logs to stderr. The level is controlled with `RUST_LOG`.
pub fn init() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,atoll=info,atoll_engine=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
