use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};

/// What the server is told through its environment.
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub listen: SocketAddr,
    /// The Onyx issuer, exactly as it appears in the tokens.
    pub issuer: String,
    /// Where the signing keys of Onyx are published.
    pub jwks_url: String,
    /// The client id of Atoll in Onyx, the audience of the tokens.
    pub client_id: String,
    /// How long removed records are kept so devices that were offline learn about them.
    pub tombstone_age: Duration,
}

impl Config {
    pub fn from_environment() -> Result<Self> {
        let required =
            |name: &str| std::env::var(name).with_context(|| format!("{name} is not set"));
        let issuer = required("ATOLL_SYNC_ONYX_URL")?
            .trim_end_matches('/')
            .to_owned();
        let jwks_url =
            std::env::var("ATOLL_SYNC_JWKS_URL").unwrap_or_else(|_| format!("{issuer}/oauth/jwks"));
        let tombstone_days: u64 = std::env::var("ATOLL_SYNC_TOMBSTONE_DAYS")
            .ok()
            .map(|days| days.parse())
            .transpose()
            .context("ATOLL_SYNC_TOMBSTONE_DAYS is no number of days")?
            .unwrap_or(180);
        Ok(Self {
            database_url: required("ATOLL_SYNC_DATABASE_URL")?,
            listen: std::env::var("ATOLL_SYNC_LISTEN")
                .unwrap_or_else(|_| "0.0.0.0:8080".to_owned())
                .parse()
                .context("ATOLL_SYNC_LISTEN is no address")?,
            issuer,
            jwks_url,
            client_id: required("ATOLL_SYNC_CLIENT_ID")?,
            tombstone_age: Duration::from_secs(tombstone_days * 86_400),
        })
    }
}
