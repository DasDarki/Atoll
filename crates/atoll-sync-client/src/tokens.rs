//! The calls to Onyx and to the sync server that signing in needs. Everything goes through
//! `atoll-net`, so it takes the way the VPN takes when one is on.

use std::time::{Duration, Instant};

use atoll_sync::wire::ServerConfig;
use serde::Deserialize;

use crate::login::Authorized;

const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    /// Onyx no longer accepts this sign-in: it was ended, the account is gone or disabled.
    #[error("signed out: {0}")]
    SignedOut(String),
    /// Onyx or the server could not be reached or asked to wait. The sign-in stays.
    #[error("not reachable: {0}")]
    Unavailable(String),
}

/// What Onyx hands out for a sign-in.
#[derive(Debug, Clone)]
pub struct Tokens {
    pub access: String,
    pub expires_at: Instant,
    pub refresh: String,
}

/// Who is signed in, as far as Atoll asked to know.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Profile {
    pub sub: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    expires_in: u64,
    refresh_token: String,
}

#[derive(Deserialize)]
struct OAuthError {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Asks the sync server where to sign in.
pub fn server_config(server: &str) -> Result<ServerConfig, TokenError> {
    let url = format!("{}/v1/config", server.trim_end_matches('/'));
    let (status, text) = send(atoll_net::agent(TIMEOUT).get(&url).call())?;
    if status != 200 {
        return Err(TokenError::Unavailable(format!(
            "the sync server answered {status}"
        )));
    }
    serde_json::from_str(&text).map_err(|error| TokenError::Unavailable(error.to_string()))
}

/// Exchanges the code of a sign-in for tokens.
pub fn exchange(config: &ServerConfig, authorized: &Authorized) -> Result<Tokens, TokenError> {
    token_request(
        config,
        &[
            ("grant_type", "authorization_code"),
            ("code", &authorized.code),
            ("redirect_uri", &authorized.redirect_uri),
            ("client_id", &config.client_id),
            ("code_verifier", &authorized.verifier),
        ],
    )
}

/// Trades the refresh token for a new pair. The old refresh token is used up by this.
pub fn refresh(config: &ServerConfig, refresh_token: &str) -> Result<Tokens, TokenError> {
    token_request(
        config,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &config.client_id),
        ],
    )
}

/// Ends the sign-in at Onyx, so the device disappears from the sessions of the account.
pub fn revoke(config: &ServerConfig, refresh_token: &str) -> Result<(), TokenError> {
    let url = endpoint(config, "/oauth/revoke");
    let request = atoll_net::agent(TIMEOUT)
        .post(&url)
        .config()
        .http_status_as_error(false)
        .build()
        .send_form([
            ("token", refresh_token),
            ("client_id", config.client_id.as_str()),
        ]);
    let (status, _) = send(request)?;
    if status >= 500 {
        return Err(TokenError::Unavailable(format!("Onyx answered {status}")));
    }
    Ok(())
}

/// Who the access token belongs to.
pub fn profile(config: &ServerConfig, access: &str) -> Result<Profile, TokenError> {
    let url = endpoint(config, "/oauth/userinfo");
    let request = atoll_net::agent(TIMEOUT)
        .get(&url)
        .header("Authorization", &format!("Bearer {access}"))
        .config()
        .http_status_as_error(false)
        .build()
        .call();
    let (status, text) = send(request)?;
    match status {
        200 => {
            serde_json::from_str(&text).map_err(|error| TokenError::Unavailable(error.to_string()))
        }
        401 => Err(TokenError::Unavailable(
            "the access token was not accepted".to_owned(),
        )),
        status => Err(TokenError::Unavailable(format!("Onyx answered {status}"))),
    }
}

fn token_request(config: &ServerConfig, form: &[(&str, &str)]) -> Result<Tokens, TokenError> {
    let url = endpoint(config, "/oauth/token");
    let request = atoll_net::agent(TIMEOUT)
        .post(&url)
        .config()
        .http_status_as_error(false)
        .build()
        .send_form(form.iter().copied());
    let (status, text) = send(request)?;
    classify(status, &text)
}

/// What an answer of the token endpoint means for the sign-in.
fn classify(status: u16, text: &str) -> Result<Tokens, TokenError> {
    if status == 200 {
        let answer: TokenAnswer = serde_json::from_str(text)
            .map_err(|error| TokenError::Unavailable(error.to_string()))?;
        return Ok(Tokens {
            access: answer.access_token,
            expires_at: Instant::now() + Duration::from_secs(answer.expires_in),
            refresh: answer.refresh_token,
        });
    }
    let error: Option<OAuthError> = serde_json::from_str(text).ok();
    match (status, error) {
        (400 | 401, Some(error))
            if matches!(
                error.error.as_str(),
                "invalid_grant" | "invalid_client" | "unauthorized_client"
            ) =>
        {
            Err(TokenError::SignedOut(
                error.error_description.unwrap_or(error.error),
            ))
        }
        (_, error) => Err(TokenError::Unavailable(
            error
                .map(|error| error.error_description.unwrap_or(error.error))
                .unwrap_or_else(|| format!("Onyx answered {status}")),
        )),
    }
}

fn endpoint(config: &ServerConfig, path: &str) -> String {
    format!("{}{path}", config.issuer.trim_end_matches('/'))
}

fn send(
    request: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<(u16, String), TokenError> {
    let mut response = request.map_err(|error| TokenError::Unavailable(error.to_string()))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_string()
        .map_err(|error| TokenError::Unavailable(error.to_string()))?;
    Ok((status, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_used_or_revoked_refresh_token_means_signed_out() {
        let refused = r#"{"error":"invalid_grant","error_description":"The refresh token is invalid, revoked or was replaced by a newer one."}"#;
        assert!(matches!(
            classify(400, refused),
            Err(TokenError::SignedOut(_))
        ));
        let unknown_app = r#"{"error":"invalid_client"}"#;
        assert!(matches!(
            classify(401, unknown_app),
            Err(TokenError::SignedOut(_))
        ));
    }

    #[test]
    fn waiting_and_failures_of_onyx_keep_the_sign_in() {
        assert!(matches!(
            classify(429, r#"{"error":"slow_down"}"#),
            Err(TokenError::Unavailable(_))
        ));
        assert!(matches!(
            classify(502, "Bad Gateway"),
            Err(TokenError::Unavailable(_))
        ));
        assert!(matches!(
            classify(400, "not json"),
            Err(TokenError::Unavailable(_))
        ));
    }

    #[test]
    fn tokens_are_read_with_their_lifetime() {
        let answer = r#"{"access_token":"a","token_type":"Bearer","expires_in":600,"refresh_token":"r","scope":"openid"}"#;
        let tokens = classify(200, answer).unwrap();
        assert_eq!(tokens.access, "a");
        assert_eq!(tokens.refresh, "r");
        let left = tokens.expires_at.duration_since(Instant::now());
        assert!(left > Duration::from_secs(590) && left <= Duration::from_secs(600));
    }
}
