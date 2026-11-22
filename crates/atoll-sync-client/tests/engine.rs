//! Devices keeping in step through the real sync server, which runs in this process against the
//! Postgres named by `ATOLL_SYNC_TEST_DATABASE_URL` (see `scripts/sync-server-tests.sh`).
//! Without it the tests say so and pass.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use atoll_core::{IslandColor, IslandId, Timestamp, WorkspaceId};
use atoll_storage::prelude::*;
use atoll_storage::{NewIsland, NewTab, Storage};
use atoll_sync::wire::{PutKey, WireEnvelope};
use atoll_sync::{DataKey, Hardness, KeyEnvelope, SyncId};
use atoll_sync_client::{Api, Choice, Command, Engine, Mode, Report, Setup, TokenError, Tokens};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde::Serialize;

const ISSUER: &str = "https://onyx.test";
const CLIENT: &str = "atoll-test";

struct Fixed(String);

impl Tokens for Fixed {
    fn access_token(&self) -> Result<String, TokenError> {
        Ok(self.0.clone())
    }

    fn refused(&self) {}
}

#[derive(Serialize)]
struct Claims<'a> {
    sub: &'a str,
    iss: &'a str,
    aud: &'a str,
    exp: u64,
}

fn token(account: &str) -> String {
    let key = EncodingKey::from_rsa_pem(include_bytes!(
        "../../atoll-sync-server/tests/fixtures/test-signing-key.pem"
    ))
    .unwrap();
    let mut header = Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test".to_owned());
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    encode(
        &header,
        &Claims {
            sub: account,
            iss: ISSUER,
            aud: CLIENT,
            exp,
        },
        &key,
    )
    .unwrap()
}

/// Starts the sync server on a free port and answers with its address.
fn server() -> Option<String> {
    let Ok(url) = std::env::var("ATOLL_SYNC_TEST_DATABASE_URL") else {
        eprintln!("ATOLL_SYNC_TEST_DATABASE_URL is not set, the engine tests are skipped");
        return None;
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let pool = sqlx::PgPool::connect(&url).await.unwrap();
            atoll_sync_server::db::migrate(&pool).await.unwrap();
            let n =
                include_str!("../../atoll-sync-server/tests/fixtures/test-signing-key.n").trim();
            let verifier =
                atoll_sync_server::Verifier::with_keys(ISSUER, CLIENT, &[("test", n, "AQAB")]);
            let app = atoll_sync_server::app(atoll_sync_server::AppState::new(pool, verifier));
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    Some(address)
}

/// Sets sync up for `account` with `key`, as the first device does.
fn set_up(server: &str, account: &str, key: &DataKey) {
    let api = Api::new(server, SyncId::random(), Arc::new(Fixed(token(account))));
    let envelope = KeyEnvelope::wrap(
        key,
        "geheim",
        Hardness {
            memory_kib: 64,
            passes: 1,
        },
    )
    .unwrap();
    api.put_key(&PutKey {
        envelope: WireEnvelope::from(&envelope),
        replaces: None,
    })
    .unwrap();
}

/// One device: a database file and the sync thread working on it.
struct Device {
    path: PathBuf,
    engine: Engine,
    reports: Arc<Mutex<Vec<Report>>>,
}

impl Device {
    fn start(
        directory: &Path,
        name: &str,
        server: &str,
        account: &str,
        key: &DataKey,
        mode: Mode,
    ) -> Self {
        let path = directory.join(format!("{name}.db"));
        drop(Storage::open(&path).unwrap());
        Self::attach(path, server, account, key, mode)
    }

    fn attach(path: PathBuf, server: &str, account: &str, key: &DataKey, mode: Mode) -> Self {
        let api = Api::new(server, SyncId::random(), Arc::new(Fixed(token(account))));
        let reports = Arc::new(Mutex::new(Vec::new()));
        let sink = reports.clone();
        let engine = Engine::start(
            Setup {
                database: path.clone(),
                api,
                key: key.clone(),
                mode,
                account: format!("{account}@test"),
                device_name: "Test".to_owned(),
            },
            move |report| sink.lock().unwrap().push(report),
        )
        .unwrap();
        Self {
            path,
            engine,
            reports,
        }
    }

    fn storage(&self) -> Storage {
        Storage::open(&self.path).unwrap()
    }

    fn reports(&self) -> Vec<Report> {
        self.reports.lock().unwrap().clone()
    }
}

fn directory(name: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("atoll-engine-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn island(storage: &Storage, workspace: WorkspaceId, name: &str) -> IslandId {
    storage
        .create_island(NewIsland {
            workspace_id: workspace,
            name,
            color: IslandColor::Rose,
            index: None,
            created_at: Timestamp(0),
        })
        .unwrap()
        .id
}

fn tab(storage: &Storage, island: IslandId, url: &str) {
    storage
        .create_tab(NewTab {
            private: false,
            island_id: island,
            url,
            title: None,
            index: None,
            created_at: Timestamp(1),
        })
        .unwrap();
}

type Layout = Vec<(String, Vec<(String, Vec<String>)>)>;

fn layout(storage: &Storage) -> Layout {
    storage
        .workspaces()
        .unwrap()
        .into_iter()
        .map(|workspace| {
            let islands = storage
                .islands(workspace.id)
                .unwrap()
                .into_iter()
                .map(|island| {
                    let tabs = storage
                        .tabs_in_island(island.id)
                        .unwrap()
                        .into_iter()
                        .map(|tab| tab.url)
                        .collect();
                    (island.name, tabs)
                })
                .collect();
            (workspace.name, islands)
        })
        .collect()
}

/// Waits until `condition` holds, for at most twenty seconds.
fn eventually(what: &str, condition: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("{what} did not happen in time");
}

#[test]
fn two_devices_keep_in_step_both_ways() {
    let Some(server) = server() else { return };
    let directory = directory("both-ways");
    let account = SyncId::random().to_string();
    let key = DataKey::generate();
    set_up(&server, &account, &key);

    let laptop_path = directory.join("laptop.db");
    {
        let laptop = Storage::open(&laptop_path).unwrap();
        let work = laptop.create_workspace("Arbeit").unwrap();
        let docs = island(&laptop, work.id, "Doku");
        tab(&laptop, docs, "https://a.example/");
        tab(&laptop, docs, "https://b.example/");
    }
    let laptop = Device::attach(laptop_path, &server, &account, &key, Mode::ReadWrite);
    let phone = Device::start(
        &directory,
        "phone",
        &server,
        &account,
        &key,
        Mode::ReadWrite,
    );

    eventually("the phone holding what the laptop has", || {
        layout(&phone.storage()) == layout(&laptop.storage())
    });

    {
        let storage = laptop.storage();
        let work = storage.workspaces().unwrap()[0].id;
        let docs = storage.islands(work).unwrap()[0].id;
        storage.rename_island(docs, "Dokumente").unwrap();
        tab(&storage, docs, "https://c.example/");
    }
    eventually("a rename and a new tab reaching the phone", || {
        layout(&phone.storage())[0].1[0]
            == (
                "Dokumente".to_owned(),
                vec![
                    "https://a.example/".to_owned(),
                    "https://b.example/".to_owned(),
                    "https://c.example/".to_owned(),
                ],
            )
    });

    {
        let storage = phone.storage();
        let work = storage.workspaces().unwrap()[0].id;
        let docs = storage.islands(work).unwrap()[0].id;
        let first = storage.tabs_in_island(docs).unwrap()[0].id;
        storage.delete_tab(first).unwrap();
    }
    eventually("a tab closed on the phone closing on the laptop", || {
        layout(&laptop.storage())[0].1[0].1 == ["https://b.example/", "https://c.example/"]
    });
    assert!(
        laptop.storage().sync_outbox().unwrap().is_empty(),
        "nothing echoes back"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_reading_device_mirrors_and_drops_its_own_changes() {
    let Some(server) = server() else { return };
    let directory = directory("reading");
    let account = SyncId::random().to_string();
    let key = DataKey::generate();
    set_up(&server, &account, &key);

    let laptop_path = directory.join("laptop.db");
    {
        let laptop = Storage::open(&laptop_path).unwrap();
        let work = laptop.create_workspace("Arbeit").unwrap();
        let docs = island(&laptop, work.id, "Doku");
        tab(&laptop, docs, "https://a.example/");
    }
    let laptop = Device::attach(laptop_path, &server, &account, &key, Mode::ReadWrite);
    let reader = Device::start(&directory, "reader", &server, &account, &key, Mode::Read);
    eventually("the reader mirroring", || {
        layout(&reader.storage()) == layout(&laptop.storage())
    });

    {
        let storage = reader.storage();
        let work = storage.workspaces().unwrap()[0].id;
        storage.rename_workspace(work, "Nur hier").unwrap();
    }
    reader.engine.send(Command::Now);
    eventually("the reader's own change being undone", || {
        layout(&reader.storage()) == layout(&laptop.storage())
    });
    assert_eq!(
        layout(&laptop.storage())[0].0,
        "Arbeit",
        "nothing went to the server"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn reading_on_demand_waits_until_asked() {
    let Some(server) = server() else { return };
    let directory = directory("on-demand");
    let account = SyncId::random().to_string();
    let key = DataKey::generate();
    set_up(&server, &account, &key);
    let laptop_path = directory.join("laptop.db");
    {
        let laptop = Storage::open(&laptop_path).unwrap();
        let work = laptop.create_workspace("Arbeit").unwrap();
        island(&laptop, work.id, "Doku");
    }
    let laptop = Device::attach(laptop_path, &server, &account, &key, Mode::ReadWrite);
    let tablet = Device::start(
        &directory,
        "tablet",
        &server,
        &account,
        &key,
        Mode::ReadOnDemand,
    );
    eventually("the laptop sending", || {
        laptop.storage().sync_outbox().unwrap().is_empty() && !laptop.reports().is_empty()
    });
    std::thread::sleep(Duration::from_secs(4));
    assert!(
        layout(&tablet.storage()).is_empty(),
        "nothing came without being asked"
    );
    tablet.engine.send(Command::Mirror);
    eventually("the tablet mirroring when asked", || {
        layout(&tablet.storage()) == layout(&laptop.storage())
    });
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn when_both_sides_hold_data_the_user_chooses() {
    let Some(server) = server() else { return };
    let directory = directory("choice");
    let account = SyncId::random().to_string();
    let key = DataKey::generate();
    set_up(&server, &account, &key);

    let laptop_path = directory.join("laptop.db");
    {
        let laptop = Storage::open(&laptop_path).unwrap();
        let work = laptop.create_workspace("Allgemein").unwrap();
        let docs = island(&laptop, work.id, "Laptop");
        tab(&laptop, docs, "https://laptop.example/");
    }
    let laptop = Device::attach(laptop_path, &server, &account, &key, Mode::ReadWrite);
    eventually("the laptop setting the server up", || {
        !laptop.reports().is_empty()
    });

    let desk_path = directory.join("desk.db");
    {
        let desk = Storage::open(&desk_path).unwrap();
        let work = desk.create_workspace("Allgemein").unwrap();
        let docs = island(&desk, work.id, "Schreibtisch");
        tab(&desk, docs, "https://desk.example/");
    }
    let desk = Device::attach(desk_path, &server, &account, &key, Mode::ReadWrite);
    eventually("the desk asking what to do", || {
        desk.reports().iter().any(|report| {
            matches!(
                report,
                Report::NeedsChoice {
                    local_tabs: 1,
                    server_tabs: 1
                }
            )
        })
    });
    desk.engine.send(Command::Link(Choice::Merge));
    eventually("both devices holding both islands in one workspace", || {
        let wanted = |layout: Layout| {
            layout.len() == 1 && {
                let mut names: Vec<String> =
                    layout[0].1.iter().map(|(name, _)| name.clone()).collect();
                names.sort();
                names == ["Laptop", "Schreibtisch"]
            }
        };
        wanted(layout(&desk.storage())) && wanted(layout(&laptop.storage()))
    });
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_reset_elsewhere_stops_a_device_before_it_touches_anything() {
    let Some(server) = server() else { return };
    let directory = directory("reset");
    let account = SyncId::random().to_string();
    let key = DataKey::generate();
    set_up(&server, &account, &key);

    let laptop_path = directory.join("laptop.db");
    {
        let laptop = Storage::open(&laptop_path).unwrap();
        let work = laptop.create_workspace("Arbeit").unwrap();
        let docs = island(&laptop, work.id, "Doku");
        tab(&laptop, docs, "https://a.example/");
    }
    let laptop = Device::attach(laptop_path, &server, &account, &key, Mode::ReadWrite);
    eventually("the laptop sending", || {
        laptop
            .reports()
            .iter()
            .any(|report| matches!(report, Report::Synced { .. }))
    });
    let before = layout(&laptop.storage());

    let elsewhere = Api::new(&server, SyncId::random(), Arc::new(Fixed(token(&account))));
    elsewhere.reset().unwrap();
    set_up(&server, &account, &DataKey::generate());
    laptop.engine.send(Command::Now);
    eventually("the laptop noticing the new key", || {
        laptop
            .reports()
            .iter()
            .any(|report| matches!(report, Report::KeyChanged))
    });
    assert_eq!(
        layout(&laptop.storage()),
        before,
        "nothing was mirrored away"
    );
    let _ = std::fs::remove_dir_all(directory);
}
