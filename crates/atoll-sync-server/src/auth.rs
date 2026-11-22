//! Checking the access tokens Onyx hands out. They are RS256 JWTs whose keys Onyx publishes, so
//! a token is checked here without asking Onyx: signature, issuer, audience and expiry.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::sync::Mutex;

/// How often the keys of Onyx are asked for again when a token names a key not known yet.
const REFETCH_PAUSE: Duration = Duration::from_secs(60);

/// Leeway for clocks that do not quite agree.
const LEEWAY_SECONDS: u64 = 30;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("no access token")]
    Missing,
    #[error("the access token is not valid: {0}")]
    Invalid(String),
    #[error("the access token is signed with an unknown key")]
    UnknownKey,
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
}

#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    n: Option<String>,
    e: Option<String>,
}

/// Where the signing keys come from.
enum Source {
    Url(String),
    Fixed,
}

pub struct Verifier {
    issuer: String,
    client_id: String,
    source: Source,
    keys: RwLock<HashMap<String, DecodingKey>>,
    fetched: Mutex<Option<Instant>>,
}

impl Verifier {
    /// Checks tokens with the keys published at `jwks_url`, fetched when first needed.
    pub fn new(issuer: &str, client_id: &str, jwks_url: &str) -> Self {
        Self {
            issuer: issuer.to_owned(),
            client_id: client_id.to_owned(),
            source: Source::Url(jwks_url.to_owned()),
            keys: RwLock::default(),
            fetched: Mutex::new(None),
        }
    }

    /// Checks tokens with the given RSA keys only, as `(kid, n, e)` in base64url.
    pub fn with_keys(issuer: &str, client_id: &str, keys: &[(&str, &str, &str)]) -> Self {
        let keys = keys
            .iter()
            .filter_map(|(kid, n, e)| {
                DecodingKey::from_rsa_components(n, e)
                    .ok()
                    .map(|key| ((*kid).to_owned(), key))
            })
            .collect();
        Self {
            issuer: issuer.to_owned(),
            client_id: client_id.to_owned(),
            source: Source::Fixed,
            keys: RwLock::new(keys),
            fetched: Mutex::new(None),
        }
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// The account a token belongs to, its `sub`.
    pub async fn verify(&self, token: &str) -> Result<String, AuthError> {
        let header = decode_header(token).map_err(|error| AuthError::Invalid(error.to_string()))?;
        if header.alg != Algorithm::RS256 {
            return Err(AuthError::Invalid("not signed with RS256".to_owned()));
        }
        let kid = header.kid.unwrap_or_default();
        let key = match self.key(&kid) {
            Some(key) => key,
            None => {
                self.refetch().await;
                self.key(&kid).ok_or(AuthError::UnknownKey)?
            }
        };
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.client_id]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.leeway = LEEWAY_SECONDS;
        let data = decode::<Claims>(token, &key, &validation)
            .map_err(|error| AuthError::Invalid(error.to_string()))?;
        if data.claims.sub.is_empty() {
            return Err(AuthError::Invalid("no subject".to_owned()));
        }
        Ok(data.claims.sub)
    }

    fn key(&self, kid: &str) -> Option<DecodingKey> {
        self.keys.read().ok()?.get(kid).cloned()
    }

    /// Asks Onyx for its keys again, at most once per [`REFETCH_PAUSE`].
    async fn refetch(&self) {
        let Source::Url(url) = &self.source else {
            return;
        };
        let mut fetched = self.fetched.lock().await;
        if fetched.is_some_and(|at| at.elapsed() < REFETCH_PAUSE) {
            return;
        }
        *fetched = Some(Instant::now());
        let url = url.clone();
        let answer = tokio::task::spawn_blocking(move || fetch_keys(&url)).await;
        match answer {
            Ok(Ok(keys)) => {
                tracing::info!(count = keys.len(), "signing keys of Onyx fetched");
                if let Ok(mut known) = self.keys.write() {
                    *known = keys;
                }
            }
            Ok(Err(error)) => tracing::warn!(%error, "signing keys of Onyx could not be fetched"),
            Err(error) => tracing::warn!(%error, "fetching the signing keys stopped"),
        }
    }
}

fn fetch_keys(url: &str) -> anyhow::Result<HashMap<String, DecodingKey>> {
    let text = ureq::get(url)
        .call()?
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_string()?;
    let jwks: Jwks = serde_json::from_str(&text)?;
    Ok(jwks
        .keys
        .into_iter()
        .filter(|key| key.kty == "RSA")
        .filter_map(|key| {
            let decoding =
                DecodingKey::from_rsa_components(key.n.as_deref()?, key.e.as_deref()?).ok()?;
            Some((key.kid.unwrap_or_default(), decoding))
        })
        .collect())
}
