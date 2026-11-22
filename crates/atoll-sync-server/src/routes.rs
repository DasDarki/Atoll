use std::convert::Infallible;
use std::time::Duration;

use atoll_sync::SyncId;
use atoll_sync::wire::{
    Changes, DEVICE_HEADER, Device, DeviceName, ErrorCode, Event, Push, Pushed, PutKey, Snapshot,
};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::sse::{self, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use futures_util::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;

use crate::AppState;
use crate::db;
use crate::error::ApiError;

/// The most changes one push may carry.
const MAX_CHANGES: usize = 1000;
/// The most changes one page holds.
const PAGE: i64 = 500;
/// The largest sealed record accepted.
const MAX_RECORD: usize = 64 * 1024;
/// The largest request body accepted.
const MAX_BODY: usize = 8 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/v1/changes", get(read_changes).post(push))
        .route("/v1/snapshot", get(snapshot))
        .route("/v1/events", get(events))
        .route("/v1/key", get(read_key).put(write_key))
        .route("/v1/account", delete(reset))
        .route("/v1/devices", get(list_devices))
        .route("/v1/devices/self", put(name_device))
        .route("/v1/devices/{id}", delete(forget_device))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(state)
}

/// The account and device a request comes from, checked.
pub struct Caller {
    account: String,
    device: SyncId,
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or_else(|| ApiError::unauthorized("no access token"))?;
        let account = state
            .verifier
            .verify(token)
            .await
            .map_err(|error| ApiError::unauthorized(error.to_string()))?;
        let device: SyncId = parts
            .headers
            .get(DEVICE_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| {
                ApiError::bad_request(format!("the header {DEVICE_HEADER} is missing"))
            })?;
        if !state.limiter.allow(&account) {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                ErrorCode::RateLimited,
                "too many requests, slow down",
            ));
        }
        db::touch(&state.pool, &account, device).await?;
        Ok(Self { account, device })
    }
}

async fn health(State(state): State<AppState>) -> Response {
    match sqlx::query("SELECT 1").execute(&state.pool).await {
        Ok(_) => "ok".into_response(),
        Err(error) => {
            tracing::warn!(%error, "health check failed");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

async fn push(
    State(state): State<AppState>,
    caller: Caller,
    Json(push): Json<Push>,
) -> Result<Json<Pushed>, ApiError> {
    if push.changes.is_empty() {
        return Err(ApiError::bad_request("no changes"));
    }
    if push.changes.len() > MAX_CHANGES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::TooLarge,
            format!("at most {MAX_CHANGES} changes per push"),
        ));
    }
    if push.changes.iter().any(|change| {
        change
            .data
            .as_ref()
            .is_some_and(|data| data.len() > MAX_RECORD)
    }) {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::TooLarge,
            format!("a record may hold at most {MAX_RECORD} bytes"),
        ));
    }
    let revision = db::push(&state.pool, &caller.account, caller.device, &push.changes).await?;
    let _ = state.events.send((caller.account, revision));
    Ok(Json(Pushed { revision }))
}

#[derive(Deserialize)]
struct Since {
    #[serde(default)]
    since: i64,
}

async fn read_changes(
    State(state): State<AppState>,
    caller: Caller,
    Query(Since { since }): Query<Since>,
) -> Result<Json<Changes>, ApiError> {
    if since < db::pruned_before(&state.pool, &caller.account).await? {
        return Err(ApiError::new(
            StatusCode::GONE,
            ErrorCode::ResyncRequired,
            "changes this old are gone, read the snapshot",
        ));
    }
    let mut changes = db::changes(&state.pool, &caller.account, since, PAGE + 1).await?;
    let more = changes.len() as i64 > PAGE;
    changes.truncate(PAGE as usize);
    let cursor = changes.last().map_or(since, |change| change.revision);
    Ok(Json(Changes {
        changes,
        cursor,
        more,
    }))
}

async fn snapshot(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Snapshot>, ApiError> {
    let (records, cursor) = db::snapshot(&state.pool, &caller.account).await?;
    Ok(Json(Snapshot { records, cursor }))
}

/// A stream that says "something changed, up to revision N" for the asking account, starting
/// with where the account stands now. A device that falls behind only misses notices, never
/// changes: it reads the changes itself.
async fn events(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>, ApiError> {
    let current = db::current_revision(&state.pool, &caller.account).await?;
    let account = caller.account;
    let pool = state.pool.clone();
    let first = stream::once(async move { notice(current) });
    let following = BroadcastStream::new(state.events.subscribe()).filter_map(move |message| {
        let account = account.clone();
        let pool = pool.clone();
        async move {
            match message {
                Ok((changed, revision)) if changed == account => Some(notice(revision)),
                Ok(_) => None,
                Err(_) => db::current_revision(&pool, &account).await.ok().map(notice),
            }
        }
    });
    Ok(Sse::new(first.chain(following))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(25))))
}

fn notice(revision: i64) -> Result<sse::Event, Infallible> {
    Ok(sse::Event::default()
        .event("changed")
        .json_data(Event { revision })
        .unwrap_or_default())
}

async fn read_key(State(state): State<AppState>, caller: Caller) -> Result<Response, ApiError> {
    match db::key(&state.pool, &caller.account).await? {
        Some(key) => Ok(Json(key).into_response()),
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            ErrorCode::NoKey,
            "sync is not set up for this account yet",
        )),
    }
}

async fn write_key(
    State(state): State<AppState>,
    caller: Caller,
    Json(put): Json<PutKey>,
) -> Result<Response, ApiError> {
    if put.envelope.salt.len() != 16
        || put.envelope.wrapped.is_empty()
        || put.envelope.wrapped.len() > 1024
    {
        return Err(ApiError::bad_request("the wrapped key is not well formed"));
    }
    match db::put_key(&state.pool, &caller.account, &put.envelope, put.replaces).await? {
        Some(version) => Ok(Json(atoll_sync::wire::StoredKey {
            envelope: put.envelope,
            version,
        })
        .into_response()),
        None => Err(ApiError::new(
            StatusCode::CONFLICT,
            ErrorCode::KeyConflict,
            "the key was set or changed by another device",
        )),
    }
}

async fn reset(State(state): State<AppState>, caller: Caller) -> Result<StatusCode, ApiError> {
    let revision = db::reset(&state.pool, &caller.account).await?;
    tracing::info!(account = caller.account, "sync reset by the account");
    let _ = state.events.send((caller.account, revision));
    Ok(StatusCode::NO_CONTENT)
}

async fn list_devices(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<Device>>, ApiError> {
    let devices = db::devices(&state.pool, &caller.account)
        .await?
        .into_iter()
        .map(|(id, name, platform, last_seen_at)| Device {
            current: id == caller.device,
            id,
            name,
            platform,
            last_seen_at,
        })
        .collect();
    Ok(Json(devices))
}

async fn name_device(
    State(state): State<AppState>,
    caller: Caller,
    Json(name): Json<DeviceName>,
) -> Result<StatusCode, ApiError> {
    if name.name.trim().is_empty() || name.name.chars().count() > 100 {
        return Err(ApiError::bad_request(
            "a device name has 1 to 100 characters",
        ));
    }
    db::name_device(&state.pool, &caller.account, caller.device, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn forget_device(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id: SyncId = id
        .parse()
        .map_err(|_| ApiError::bad_request("no device id"))?;
    if db::forget_device(&state.pool, &caller.account, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Ok(StatusCode::NOT_FOUND)
    }
}
