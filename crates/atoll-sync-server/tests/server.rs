//! Runs the server against a real Postgres, named by `ATOLL_SYNC_TEST_DATABASE_URL`, see
//! `scripts/sync-server-tests.sh`. Without it the tests say so and pass, so `cargo test` keeps
//! working where no database is at hand.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use atoll_sync::wire::{
    Change, Changes, DEVICE_HEADER, Device, ErrorBody, ErrorCode, Push, Pushed, PutKey, Snapshot,
    StoredKey, WireEnvelope,
};
use atoll_sync::{Kind, SyncId};
use atoll_sync_server::{AppState, Verifier, app, db};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use jsonwebtoken::{EncodingKey, Header, encode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::PgPool;
use tower::ServiceExt;

const ISSUER: &str = "https://onyx.test";
const CLIENT: &str = "atoll-test";
const KEY_ID: &str = "test";

#[derive(Serialize)]
struct Claims<'a> {
    sub: &'a str,
    iss: &'a str,
    aud: &'a str,
    exp: u64,
}

struct Server {
    app: Router,
    pool: PgPool,
}

async fn server() -> Option<Server> {
    let Ok(url) = std::env::var("ATOLL_SYNC_TEST_DATABASE_URL") else {
        eprintln!("ATOLL_SYNC_TEST_DATABASE_URL is not set, the server tests are skipped");
        return None;
    };
    let pool = PgPool::connect(&url)
        .await
        .expect("the test database answers");
    db::migrate(&pool)
        .await
        .expect("the test database can be migrated");
    let n = include_str!("fixtures/test-signing-key.n").trim();
    let verifier = Verifier::with_keys(ISSUER, CLIENT, &[(KEY_ID, n, "AQAB")]);
    Some(Server {
        app: app(AppState::new(pool.clone(), verifier)),
        pool,
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn token_for(account: &str, audience: &str, expires: u64) -> String {
    let key = EncodingKey::from_rsa_pem(include_bytes!("fixtures/test-signing-key.pem")).unwrap();
    let mut header = Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(KEY_ID.to_owned());
    encode(
        &header,
        &Claims {
            sub: account,
            iss: ISSUER,
            aud: audience,
            exp: expires,
        },
        &key,
    )
    .unwrap()
}

/// A fresh account, so tests sharing the database never see each other's data.
struct Account {
    token: String,
    device: SyncId,
}

impl Account {
    fn new() -> Self {
        Self::named(&SyncId::random().to_string())
    }

    fn named(sub: &str) -> Self {
        Self {
            token: token_for(sub, CLIENT, now() + 600),
            device: SyncId::random(),
        }
    }

    fn on_another_device(&self) -> Self {
        Self {
            token: self.token.clone(),
            device: SyncId::random(),
        }
    }
}

async fn call(
    server: &Server,
    account: Option<&Account>,
    method: &str,
    path: &str,
    body: Option<String>,
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(account) = account {
        request = request
            .header("authorization", format!("Bearer {}", account.token))
            .header(DEVICE_HEADER, account.device.to_string());
    }
    let request = match body {
        Some(body) => request
            .header("content-type", "application/json")
            .body(Body::from(body)),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = server.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec())
}

async fn json<T: DeserializeOwned>(
    server: &Server,
    account: &Account,
    method: &str,
    path: &str,
    body: Option<String>,
) -> (StatusCode, T) {
    let (status, bytes) = call(server, Some(account), method, path, body).await;
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("{status}: {error}: {}", String::from_utf8_lossy(&bytes)));
    (status, value)
}

fn change(kind: Kind, id: SyncId, data: Option<&[u8]>) -> Change {
    Change {
        kind,
        id,
        data: data.map(<[u8]>::to_vec),
    }
}

async fn push(server: &Server, account: &Account, changes: Vec<Change>) -> i64 {
    let body = serde_json::to_string(&Push {
        changes,
        key_id: String::new(),
    })
    .unwrap();
    let (status, pushed): (_, Pushed) =
        json(server, account, "POST", "/v1/changes", Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    pushed.revision
}

async fn changes(server: &Server, account: &Account, since: i64) -> (StatusCode, Vec<u8>) {
    call(
        server,
        Some(account),
        "GET",
        &format!("/v1/changes?since={since}"),
        None,
    )
    .await
}

#[tokio::test]
async fn only_valid_tokens_for_atoll_get_in() {
    let Some(server) = server().await else { return };
    let (status, _) = call(&server, None, "GET", "/v1/snapshot", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let mut other_app = Account::new();
    other_app.token = token_for("someone", "another-app", now() + 600);
    let (status, body) = call(&server, Some(&other_app), "GET", "/v1/snapshot", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let error: ErrorBody = serde_json::from_slice(&body).unwrap();
    assert_eq!(error.error, ErrorCode::Unauthorized);

    let mut expired = Account::new();
    expired.token = token_for("someone", CLIENT, now() - 3600);
    let (status, _) = call(&server, Some(&expired), "GET", "/v1/snapshot", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let mut forged = Account::new();
    let mut parts: Vec<String> = forged.token.split('.').map(str::to_owned).collect();
    parts[1] = parts[1].chars().rev().collect();
    forged.token = parts.join(".");
    let (status, _) = call(&server, Some(&forged), "GET", "/v1/snapshot", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let account = Account::new();
    let request = Request::builder()
        .uri("/v1/snapshot")
        .header("authorization", format!("Bearer {}", account.token))
        .body(Body::empty())
        .unwrap();
    let response = server.app.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "without a device"
    );
}

#[tokio::test]
async fn devices_learn_where_to_sign_in_without_being_signed_in() {
    let Some(server) = server().await else { return };
    let (status, body) = call(&server, None, "GET", "/v1/config", None).await;
    assert_eq!(status, StatusCode::OK);
    let config: atoll_sync::wire::ServerConfig = serde_json::from_slice(&body).unwrap();
    assert_eq!(config.issuer, ISSUER);
    assert_eq!(config.client_id, CLIENT);
}

#[tokio::test]
async fn changes_come_back_in_order_and_page_by_page() {
    let Some(server) = server().await else { return };
    let account = Account::new();
    let first = SyncId::random();
    let revision = push(
        &server,
        &account,
        vec![change(Kind::Tab, first, Some(b"eins"))],
    )
    .await;
    assert_eq!(revision, 1);

    let many: Vec<Change> = (0..600)
        .map(|_| change(Kind::Tab, SyncId::random(), Some(b"viele")))
        .collect();
    assert_eq!(push(&server, &account, many).await, 601);

    let (status, body) = changes(&server, &account, 0).await;
    assert_eq!(status, StatusCode::OK);
    let page: Changes = serde_json::from_slice(&body).unwrap();
    assert_eq!(page.changes.len(), 500);
    assert!(page.more);
    assert_eq!(page.cursor, 500);
    assert_eq!(page.changes[0].id, first);
    assert_eq!(page.changes[0].data.as_deref(), Some(&b"eins"[..]));
    assert_eq!(page.changes[0].device, account.device);
    let (_, body) = changes(&server, &account, page.cursor).await;
    let rest: Changes = serde_json::from_slice(&body).unwrap();
    assert_eq!(rest.changes.len(), 101);
    assert!(!rest.more);
    assert_eq!(rest.cursor, 601);
    let (_, body) = changes(&server, &account, rest.cursor).await;
    let nothing: Changes = serde_json::from_slice(&body).unwrap();
    assert!(nothing.changes.is_empty());
    assert_eq!(nothing.cursor, 601, "the cursor stays where it was");
}

#[tokio::test]
async fn a_removal_reaches_the_changes_but_not_the_snapshot() {
    let Some(server) = server().await else { return };
    let account = Account::new();
    let kept = SyncId::random();
    let gone = SyncId::random();
    push(
        &server,
        &account,
        vec![
            change(Kind::Island, kept, Some(b"bleibt")),
            change(Kind::Island, gone, Some(b"geht")),
        ],
    )
    .await;
    let laptop = account.on_another_device();
    push(&server, &laptop, vec![change(Kind::Island, gone, None)]).await;

    let (_, body) = changes(&server, &account, 2).await;
    let page: Changes = serde_json::from_slice(&body).unwrap();
    assert_eq!(page.changes.len(), 1);
    assert_eq!(page.changes[0].id, gone);
    assert_eq!(page.changes[0].data, None);
    assert_eq!(page.changes[0].device, laptop.device);

    let (_, snapshot): (_, Snapshot) = json(&server, &account, "GET", "/v1/snapshot", None).await;
    assert_eq!(snapshot.cursor, 3);
    let ids: Vec<SyncId> = snapshot.records.iter().map(|record| record.id).collect();
    assert_eq!(ids, [kept]);
}

#[tokio::test]
async fn accounts_never_see_each_other() {
    let Some(server) = server().await else { return };
    let alice = Account::new();
    let bob = Account::new();
    let id = SyncId::random();
    push(
        &server,
        &alice,
        vec![change(Kind::Workspace, id, Some(b"alice"))],
    )
    .await;
    push(
        &server,
        &bob,
        vec![change(Kind::Workspace, id, Some(b"bob"))],
    )
    .await;

    let (_, snapshot): (_, Snapshot) = json(&server, &alice, "GET", "/v1/snapshot", None).await;
    assert_eq!(snapshot.records.len(), 1);
    assert_eq!(snapshot.records[0].data.as_deref(), Some(&b"alice"[..]));
    assert_eq!(snapshot.cursor, 1, "each account counts its own revisions");
}

#[tokio::test]
async fn pushes_at_the_same_time_get_revisions_in_the_order_they_become_visible() {
    let Some(server) = server().await else { return };
    let account = Account::new();
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let app = server.app.clone();
        let token = account.token.clone();
        let device = SyncId::random();
        tasks.push(tokio::spawn(async move {
            let body = serde_json::to_string(&Push {
                changes: (0..20)
                    .map(|_| change(Kind::Tab, SyncId::random(), Some(b"x")))
                    .collect(),
                key_id: String::new(),
            })
            .unwrap();
            let request = Request::builder()
                .method("POST")
                .uri("/v1/changes")
                .header("authorization", format!("Bearer {token}"))
                .header(DEVICE_HEADER, device.to_string())
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            app.oneshot(request).await.unwrap().status()
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), StatusCode::OK);
    }
    let mut revisions = Vec::new();
    let mut since = 0;
    loop {
        let (_, body) = changes(&server, &account, since).await;
        let page: Changes = serde_json::from_slice(&body).unwrap();
        revisions.extend(page.changes.iter().map(|change| change.revision));
        since = page.cursor;
        if !page.more {
            break;
        }
    }
    assert_eq!(revisions, (1..=240).collect::<Vec<i64>>());
}

#[tokio::test]
async fn the_wrapped_key_is_set_once_and_replaced_only_knowingly() {
    let Some(server) = server().await else { return };
    let account = Account::new();
    let (status, body) = call(&server, Some(&account), "GET", "/v1/key", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        serde_json::from_slice::<ErrorBody>(&body).unwrap().error,
        ErrorCode::NoKey
    );

    let envelope = WireEnvelope {
        key_id: "0123456789abcdef0123456789abcdef".to_owned(),
        salt: vec![7; 16],
        memory_kib: 65536,
        passes: 3,
        wrapped: vec![1; 73],
    };
    let put = |replaces| {
        serde_json::to_string(&PutKey {
            envelope: envelope.clone(),
            replaces,
        })
        .unwrap()
    };
    let (status, stored): (_, StoredKey) =
        json(&server, &account, "PUT", "/v1/key", Some(put(None))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stored.version, 1);

    let (status, _) = call(&server, Some(&account), "PUT", "/v1/key", Some(put(None))).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a second device setting up loses the race"
    );
    let (status, replaced): (_, StoredKey) =
        json(&server, &account, "PUT", "/v1/key", Some(put(Some(1)))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replaced.version, 2);
    let (status, _) = call(
        &server,
        Some(&account),
        "PUT",
        "/v1/key",
        Some(put(Some(1))),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "replacing an old version");

    let (_, read): (_, StoredKey) = json(&server, &account, "GET", "/v1/key", None).await;
    assert_eq!(read.envelope, envelope);
    assert_eq!(read.version, 2);
}

#[tokio::test]
async fn pushes_sealed_with_a_key_the_account_no_longer_keeps_are_refused() {
    let Some(server) = server().await else { return };
    let account = Account::new();
    let key_id = "0123456789abcdef0123456789abcdef";
    let sealed_with = |key_id: &str| {
        serde_json::to_string(&Push {
            changes: vec![change(Kind::Tab, SyncId::random(), Some(b"x"))],
            key_id: key_id.to_owned(),
        })
        .unwrap()
    };
    let (status, body) = call(
        &server,
        Some(&account),
        "POST",
        "/v1/changes",
        Some(sealed_with(key_id)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "no key is kept yet");
    assert_eq!(
        serde_json::from_slice::<ErrorBody>(&body).unwrap().error,
        ErrorCode::KeyChanged
    );

    let put = serde_json::to_string(&PutKey {
        envelope: WireEnvelope {
            key_id: key_id.to_owned(),
            salt: vec![7; 16],
            memory_kib: 65536,
            passes: 3,
            wrapped: vec![1; 73],
        },
        replaces: None,
    })
    .unwrap();
    let (status, _) = call(&server, Some(&account), "PUT", "/v1/key", Some(put)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &server,
        Some(&account),
        "POST",
        "/v1/changes",
        Some(sealed_with(key_id)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &server,
        Some(&account),
        "POST",
        "/v1/changes",
        Some(sealed_with("ffffffffffffffffffffffffffffffff")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "another key");

    let (status, _) = call(&server, Some(&account), "DELETE", "/v1/account", None).await;
    assert!(status.is_success());
    let (status, _) = call(
        &server,
        Some(&account),
        "POST",
        "/v1/changes",
        Some(sealed_with(key_id)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "the account was reset");
}

#[tokio::test]
async fn resetting_empties_the_account_and_sends_every_device_to_the_snapshot() {
    let Some(server) = server().await else { return };
    let account = Account::new();
    let revision = push(
        &server,
        &account,
        vec![change(Kind::Tab, SyncId::random(), Some(b"x"))],
    )
    .await;
    let envelope = WireEnvelope {
        key_id: "0123456789abcdef0123456789abcdef".to_owned(),
        salt: vec![7; 16],
        memory_kib: 64,
        passes: 1,
        wrapped: vec![1; 73],
    };
    let body = serde_json::to_string(&PutKey {
        envelope,
        replaces: None,
    })
    .unwrap();
    call(&server, Some(&account), "PUT", "/v1/key", Some(body)).await;

    let (status, _) = call(&server, Some(&account), "DELETE", "/v1/account", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = changes(&server, &account, revision).await;
    assert_eq!(
        status,
        StatusCode::GONE,
        "even a device that was up to date"
    );
    assert_eq!(
        serde_json::from_slice::<ErrorBody>(&body).unwrap().error,
        ErrorCode::ResyncRequired
    );
    let (_, snapshot): (_, Snapshot) = json(&server, &account, "GET", "/v1/snapshot", None).await;
    assert!(snapshot.records.is_empty());
    let (status, _) = call(&server, Some(&account), "GET", "/v1/key", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = changes(&server, &account, snapshot.cursor).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "from the snapshot on it goes on as usual"
    );
}

#[tokio::test]
async fn old_tombstones_go_and_devices_behind_them_read_the_snapshot() {
    let Some(server) = server().await else { return };
    let sub = SyncId::random().to_string();
    let account = Account::named(&sub);
    let id = SyncId::random();
    push(&server, &account, vec![change(Kind::Tab, id, Some(b"x"))]).await;
    push(&server, &account, vec![change(Kind::Tab, id, None)]).await;
    sqlx::query("UPDATE record SET changed_at = now() - interval '200 days' WHERE account = $1")
        .bind(&sub)
        .execute(&server.pool)
        .await
        .unwrap();

    let removed = db::prune(&server.pool, Duration::from_secs(180 * 86_400))
        .await
        .unwrap();
    assert!(removed >= 1);
    let (status, _) = changes(&server, &account, 0).await;
    assert_eq!(status, StatusCode::GONE);
    let (status, _) = changes(&server, &account, 2).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a device past the tombstone missed nothing"
    );
}

#[tokio::test]
async fn devices_are_listed_named_and_forgotten() {
    let Some(server) = server().await else { return };
    let laptop = Account::new();
    let phone = laptop.on_another_device();
    let body = r#"{"name":"Pixel","platform":"android"}"#.to_owned();
    let (status, _) = call(&server, Some(&phone), "PUT", "/v1/devices/self", Some(body)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, devices): (_, Vec<Device>) = json(&server, &laptop, "GET", "/v1/devices", None).await;
    assert_eq!(devices.len(), 2);
    let me = devices.iter().find(|device| device.current).unwrap();
    assert_eq!(me.id, laptop.device);
    let other = devices.iter().find(|device| !device.current).unwrap();
    assert_eq!(other.name, "Pixel");

    let path = format!("/v1/devices/{}", phone.device);
    let (status, _) = call(&server, Some(&laptop), "DELETE", &path, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, devices): (_, Vec<Device>) = json(&server, &laptop, "GET", "/v1/devices", None).await;
    assert_eq!(devices.len(), 1);
}

#[tokio::test]
async fn oversized_pushes_are_refused() {
    let Some(server) = server().await else { return };
    let account = Account::new();
    let too_many: Vec<Change> = (0..1001)
        .map(|_| change(Kind::Tab, SyncId::random(), Some(b"x")))
        .collect();
    let body = serde_json::to_string(&Push {
        changes: too_many,
        key_id: String::new(),
    })
    .unwrap();
    let (status, _) = call(&server, Some(&account), "POST", "/v1/changes", Some(body)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    let huge = vec![0u8; 64 * 1024 + 1];
    let body = serde_json::to_string(&Push {
        changes: vec![change(Kind::Tab, SyncId::random(), Some(&huge))],
        key_id: String::new(),
    })
    .unwrap();
    let (status, _) = call(&server, Some(&account), "POST", "/v1/changes", Some(body)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    let body = serde_json::to_string(&Push {
        changes: Vec::new(),
        key_id: String::new(),
    })
    .unwrap();
    let (status, _) = call(&server, Some(&account), "POST", "/v1/changes", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_event_stream_tells_where_the_account_stands_and_what_changes() {
    let Some(server) = server().await else { return };
    let account = Account::new();
    push(
        &server,
        &account,
        vec![change(Kind::Tab, SyncId::random(), Some(b"x"))],
    )
    .await;
    let request = Request::builder()
        .uri("/v1/events")
        .header("authorization", format!("Bearer {}", account.token))
        .header(DEVICE_HEADER, account.device.to_string())
        .body(Body::empty())
        .unwrap();
    let response = server.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let mut next = async || {
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("an event arrives")
            .unwrap()
            .unwrap();
        String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap()
    };
    assert_eq!(next().await, "event: changed\ndata: {\"revision\":1}\n\n");

    let elsewhere = Account::new();
    push(
        &server,
        &elsewhere,
        vec![change(Kind::Tab, SyncId::random(), Some(b"y"))],
    )
    .await;
    push(
        &server,
        &account.on_another_device(),
        vec![change(Kind::Tab, SyncId::random(), Some(b"z"))],
    )
    .await;
    assert_eq!(
        next().await,
        "event: changed\ndata: {\"revision\":2}\n\n",
        "another account's change is not told"
    );
}
