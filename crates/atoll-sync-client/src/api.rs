//! The calls to the sync server, blocking, for the sync thread. An access token the server
//! refuses is renewed once and the call tried again.

use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::time::Duration;

use atoll_sync::SyncId;
use atoll_sync::wire::{
    Change, Changes, DEVICE_HEADER, Device, DeviceName, ErrorBody, ErrorCode, Event, Push, Pushed,
    PutKey, Snapshot, StoredKey,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::auth::Auth;
use crate::tokens::TokenError;

const TIMEOUT: Duration = Duration::from_secs(30);

/// How long one event stream stays open before it is opened anew.
const STREAM_LIFETIME: Duration = Duration::from_secs(30 * 60);

/// Where the access tokens for the server come from.
pub trait Tokens: Send + Sync {
    fn access_token(&self) -> Result<String, TokenError>;
    /// The server refused the last token, the next one has to be fresh.
    fn refused(&self);
}

impl Tokens for Auth {
    fn access_token(&self) -> Result<String, TokenError> {
        Auth::access_token(self)
    }

    fn refused(&self) {
        self.access_refused();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    #[error("signed out")]
    SignedOut,
    #[error("the changes are gone, the snapshot has to be read")]
    ResyncRequired,
    #[error("sync is not set up for this account")]
    NoKey,
    #[error("another device set the key first")]
    KeyConflict,
    #[error("the sync server cannot be reached: {0}")]
    Unavailable(String),
    #[error("the sync server refused: {0}")]
    Refused(String),
}

impl From<TokenError> for ApiError {
    fn from(error: TokenError) -> Self {
        match error {
            TokenError::SignedOut(_) => ApiError::SignedOut,
            TokenError::Unavailable(reason) => ApiError::Unavailable(reason),
        }
    }
}

#[derive(Clone)]
pub struct Api {
    server: String,
    device: SyncId,
    tokens: Arc<dyn Tokens>,
}

impl Api {
    pub fn new(server: &str, device: SyncId, tokens: Arc<dyn Tokens>) -> Self {
        Self {
            server: server.trim_end_matches('/').to_owned(),
            device,
            tokens,
        }
    }

    pub fn device(&self) -> SyncId {
        self.device
    }

    /// The wrapped key, `None` while sync is not set up for the account.
    pub fn key(&self) -> Result<Option<StoredKey>, ApiError> {
        match self.call::<(), StoredKey>("GET", "/v1/key", None) {
            Ok(key) => Ok(Some(key)),
            Err(ApiError::NoKey) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn put_key(&self, put: &PutKey) -> Result<StoredKey, ApiError> {
        self.call("PUT", "/v1/key", Some(put))
    }

    pub fn push(&self, changes: Vec<Change>) -> Result<i64, ApiError> {
        let pushed: Pushed = self.call("POST", "/v1/changes", Some(&Push { changes }))?;
        Ok(pushed.revision)
    }

    pub fn changes(&self, since: i64) -> Result<Changes, ApiError> {
        self.call::<(), _>("GET", &format!("/v1/changes?since={since}"), None)
    }

    pub fn snapshot(&self) -> Result<Snapshot, ApiError> {
        self.call::<(), _>("GET", "/v1/snapshot", None)
    }

    pub fn devices(&self) -> Result<Vec<Device>, ApiError> {
        self.call::<(), _>("GET", "/v1/devices", None)
    }

    pub fn name_device(&self, name: &DeviceName) -> Result<(), ApiError> {
        self.call_empty("PUT", "/v1/devices/self", Some(name))
    }

    pub fn forget_device(&self, device: SyncId) -> Result<(), ApiError> {
        self.call_empty::<()>("DELETE", &format!("/v1/devices/{device}"), None)
    }

    /// Removes everything the account keeps on the server, the key included.
    pub fn reset(&self) -> Result<(), ApiError> {
        self.call_empty::<()>("DELETE", "/v1/account", None)
    }

    /// Listens to the event stream until it ends, calling `changed` with every revision told.
    /// Returns when the stream closes, fails or `changed` answers `false`.
    pub fn listen(&self, mut changed: impl FnMut(i64) -> bool) -> Result<(), ApiError> {
        let token = self.tokens.access_token()?;
        let response = atoll_net::agent(STREAM_LIFETIME)
            .get(&self.url("/v1/events"))
            .header("Authorization", &format!("Bearer {token}"))
            .header(DEVICE_HEADER, &self.device.to_string())
            .config()
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|error| ApiError::Unavailable(error.to_string()))?;
        let status = response.status().as_u16();
        if status == 401 {
            self.tokens.refused();
            return Err(ApiError::Unavailable("the token was refused".to_owned()));
        }
        if status != 200 {
            return Err(ApiError::Unavailable(format!(
                "the event stream answered {status}"
            )));
        }
        let reader = BufReader::new(response.into_body().into_reader());
        for line in reader.lines() {
            let line = line.map_err(|error| ApiError::Unavailable(error.to_string()))?;
            if let Some(data) = line.strip_prefix("data:")
                && let Ok(event) = serde_json::from_str::<Event>(data.trim())
                && !changed(event.revision)
            {
                return Ok(());
            }
        }
        Ok(())
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.server)
    }

    fn call<B: Serialize, T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, ApiError> {
        let text = self.exchange(method, path, body)?;
        serde_json::from_str(&text)
            .map_err(|error| ApiError::Refused(format!("unexpected answer: {error}")))
    }

    fn call_empty<B: Serialize>(
        &self,
        method: &str,
        path: &str,
        body: Option<&B>,
    ) -> Result<(), ApiError> {
        self.exchange(method, path, body).map(|_| ())
    }

    /// Sends one request, renewing the token once when the server refuses it.
    fn exchange<B: Serialize>(
        &self,
        method: &str,
        path: &str,
        body: Option<&B>,
    ) -> Result<String, ApiError> {
        let body =
            body.map(|body| serde_json::to_string(body).expect("a request can always be written"));
        for attempt in 0..2 {
            let token = self.tokens.access_token()?;
            let (status, text) = self.send(method, path, &token, body.as_deref())?;
            if status == 401 && attempt == 0 {
                self.tokens.refused();
                continue;
            }
            return classify(status, text);
        }
        Err(ApiError::SignedOut)
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<&str>,
    ) -> Result<(u16, String), ApiError> {
        let agent = atoll_net::agent(TIMEOUT);
        let url = self.url(path);
        let authorization = format!("Bearer {token}");
        let device = self.device.to_string();
        let answer = match (method, body) {
            ("GET", _) => agent
                .get(&url)
                .header("Authorization", &authorization)
                .header(DEVICE_HEADER, &device)
                .config()
                .http_status_as_error(false)
                .build()
                .call(),
            ("DELETE", _) => agent
                .delete(&url)
                .header("Authorization", &authorization)
                .header(DEVICE_HEADER, &device)
                .config()
                .http_status_as_error(false)
                .build()
                .call(),
            (method, body) => {
                let request = if method == "PUT" {
                    agent.put(&url)
                } else {
                    agent.post(&url)
                };
                request
                    .header("Authorization", &authorization)
                    .header(DEVICE_HEADER, &device)
                    .header("Content-Type", "application/json")
                    .config()
                    .http_status_as_error(false)
                    .build()
                    .send(body.unwrap_or("{}"))
            }
        };
        let mut response = answer.map_err(|error| ApiError::Unavailable(error.to_string()))?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_string()
            .map_err(|error| ApiError::Unavailable(error.to_string()))?;
        Ok((status, text))
    }
}

/// What an answer of the server means.
fn classify(status: u16, text: String) -> Result<String, ApiError> {
    if (200..300).contains(&status) {
        return Ok(text);
    }
    let body: Option<ErrorBody> = serde_json::from_str(&text).ok();
    match body.map(|body| (body.error, body.message)) {
        Some((ErrorCode::Unauthorized, _)) => Err(ApiError::SignedOut),
        Some((ErrorCode::ResyncRequired, _)) => Err(ApiError::ResyncRequired),
        Some((ErrorCode::NoKey, _)) => Err(ApiError::NoKey),
        Some((ErrorCode::KeyConflict, _)) => Err(ApiError::KeyConflict),
        Some((ErrorCode::RateLimited | ErrorCode::Internal, message)) => {
            Err(ApiError::Unavailable(message))
        }
        Some((_, message)) => Err(ApiError::Refused(message)),
        None if status >= 500 || status == 429 => Err(ApiError::Unavailable(format!(
            "the server answered {status}"
        ))),
        None => Err(ApiError::Refused(format!("the server answered {status}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_of_the_server_are_told_apart() {
        assert_eq!(classify(200, "{}".to_owned()), Ok("{}".to_owned()));
        let gone = r#"{"error":"resync_required","message":"read the snapshot"}"#.to_owned();
        assert_eq!(classify(410, gone), Err(ApiError::ResyncRequired));
        let none = r#"{"error":"no_key","message":"not set up"}"#.to_owned();
        assert_eq!(classify(404, none), Err(ApiError::NoKey));
        assert!(matches!(
            classify(502, "Bad Gateway".to_owned()),
            Err(ApiError::Unavailable(_))
        ));
        assert!(matches!(
            classify(
                429,
                r#"{"error":"rate_limited","message":"slow down"}"#.to_owned()
            ),
            Err(ApiError::Unavailable(_))
        ));
    }
}
