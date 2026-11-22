//! Staying signed in. One `Auth` per program holds the refresh token and hands out access tokens,
//! renewing them shortly before they run out. Renewing is done by one caller at a time: Onyx
//! answers a refresh token used twice by ending the sign-in, so two windows renewing at once
//! must never happen.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use atoll_sync::wire::ServerConfig;

use crate::login::Authorized;
use crate::store::{Saved, TokenStore};
use crate::tokens::{self, Profile, TokenError, Tokens};

/// An access token closer to its end than this is renewed before it is handed out.
const RENEW_BEFORE: Duration = Duration::from_secs(60);

pub struct Auth {
    server: String,
    config: ServerConfig,
    store: TokenStore,
    state: Mutex<State>,
}

struct State {
    refresh: String,
    access: Option<(String, Instant)>,
}

impl Auth {
    /// Finishes a sign-in: exchanges the code, keeps the refresh token and asks who signed in.
    pub fn sign_in(
        server: &str,
        config: ServerConfig,
        authorized: &Authorized,
        store: TokenStore,
    ) -> Result<(Self, Profile), TokenError> {
        let tokens = tokens::exchange(&config, authorized)?;
        let auth = Self::from_tokens(server, config, store, &tokens);
        let profile = tokens::profile(&auth.config, &tokens.access)?;
        Ok((auth, profile))
    }

    /// The sign-in of the last run, if there was one.
    pub fn resume(store: TokenStore) -> Option<Self> {
        let saved = store.load()?;
        Some(Self {
            server: saved.server,
            config: ServerConfig {
                issuer: saved.issuer,
                client_id: saved.client_id,
            },
            state: Mutex::new(State {
                refresh: saved.refresh,
                access: None,
            }),
            store,
        })
    }

    fn from_tokens(server: &str, config: ServerConfig, store: TokenStore, tokens: &Tokens) -> Self {
        let auth = Self {
            server: server.to_owned(),
            config,
            store,
            state: Mutex::new(State {
                refresh: tokens.refresh.clone(),
                access: Some((tokens.access.clone(), tokens.expires_at)),
            }),
        };
        auth.keep(&tokens.refresh);
        auth
    }

    /// The sync server this sign-in belongs to.
    pub fn server(&self) -> &str {
        &self.server
    }

    /// A valid access token, renewed when needed. Blocks while renewing, so it is only asked
    /// for away from the user interface. On [`TokenError::SignedOut`] the kept sign-in is gone.
    pub fn access_token(&self) -> Result<String, TokenError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((access, expires_at)) = &state.access
            && expires_at.saturating_duration_since(Instant::now()) > RENEW_BEFORE
        {
            return Ok(access.clone());
        }
        match tokens::refresh(&self.config, &state.refresh) {
            Ok(tokens) => {
                self.keep(&tokens.refresh);
                state.refresh = tokens.refresh;
                state.access = Some((tokens.access.clone(), tokens.expires_at));
                Ok(tokens.access)
            }
            Err(TokenError::SignedOut(reason)) => {
                tracing::info!(reason, "the sign-in was ended");
                self.store.clear();
                Err(TokenError::SignedOut(reason))
            }
            Err(error) => Err(error),
        }
    }

    /// Forgets the access token, after the server refused it, so the next call renews it.
    pub fn access_refused(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.access = None;
    }

    /// Who is signed in.
    pub fn profile(&self) -> Result<Profile, TokenError> {
        let access = self.access_token()?;
        tokens::profile(&self.config, &access)
    }

    /// Ends the sign-in at Onyx and forgets it here, also when Onyx cannot be reached.
    pub fn sign_out(&self) {
        let refresh = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .refresh
            .clone();
        if let Err(error) = tokens::revoke(&self.config, &refresh) {
            tracing::warn!(%error, "the sign-in could not be ended at Onyx");
        }
        self.store.clear();
    }

    /// Keeps the refresh token before it is used, so a crash never loses the newest one.
    fn keep(&self, refresh: &str) {
        let saved = Saved {
            server: self.server.clone(),
            issuer: self.config.issuer.clone(),
            client_id: self.config.client_id.clone(),
            refresh: refresh.to_owned(),
        };
        if let Err(error) = self.store.save(&saved) {
            tracing::error!(%error, "the sign-in could not be kept for the next start");
        }
    }
}
