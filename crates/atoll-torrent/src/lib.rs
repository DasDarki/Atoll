//! Torrents, carried the same way as everything else Atoll fetches.
//!
//! The work is done by `librqbit`. What this crate adds is the one rule Atoll insists on: a
//! torrent only ever runs while a way out stands that hides where the user is. There are two such
//! ways, and they are not the same thing.
//!
//! Through Atoll's own VPN the torrent speaks TCP through the SOCKS bridge, and that is all it
//! may do: the parts of BitTorrent that use UDP - the distributed table and the trackers that
//! speak it - cannot go through a bridge that only carries TCP, and a client that tried would
//! hand out the user's address behind the VPN's back. They are therefore switched off, which
//! costs peers.
//!
//! Through a VPN of the system every socket is tied to its device, which the kernel then enforces:
//! the table, the trackers, uTP and incoming connections all work, and nothing can leave while
//! the device is gone. That is the better way for torrents, and the reason this crate knows about
//! network devices at all.

mod announce;
mod device;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use atoll_i18n::t;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ConnectionOptions, ListenerMode,
    ListenerOptions, Magnet, Session, SessionOptions, TorrentStatsState, generate_peer_id,
    torrent_from_bytes,
};

pub use device::{Device, devices};

/// How often the state of the torrents is read for the user interface.
const POLL: Duration = Duration::from_millis(900);

/// Our own number for a torrent, handed out before `librqbit` knows it.
pub type TorrentId = u64;

/// Where a torrent's traffic goes. Without one of these nothing runs at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// Through Atoll's own VPN, on the port of its SOCKS bridge. Only TCP fits through it.
    Bridge(u16),
    /// Through a VPN of the system: every socket is bound to that device.
    Device(String),
}

impl Route {
    /// Whether the parts of BitTorrent that speak UDP can be used at all.
    pub fn carries_udp(&self) -> bool {
        matches!(self, Route::Device(_))
    }

    /// Whether the way out is still there.
    pub fn stands(&self) -> bool {
        match self {
            Route::Bridge(_) => true,
            Route::Device(name) => device::is_up(name),
        }
    }
}

/// What one torrent is doing, as the download list shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub id: TorrentId,
    pub name: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
    /// Bytes per second.
    pub speed: u64,
    pub peers: usize,
    pub finished: bool,
    pub paused: bool,
    pub error: Option<String>,
    /// Where the files landed, for opening them afterwards.
    pub folder: Option<PathBuf>,
}

/// What the torrents are told to do.
enum Command {
    Add {
        id: TorrentId,
        source: Source,
        directory: PathBuf,
    },
    Pause(TorrentId),
    Resume(TorrentId),
    Forget {
        id: TorrentId,
        delete_files: bool,
    },
    Stop,
}

/// Where a torrent comes from.
#[derive(Debug, Clone)]
pub enum Source {
    /// A magnet address or the address of a torrent file.
    Link(String),
    /// The contents of a torrent file.
    File(Vec<u8>),
}

#[derive(Debug, Default)]
struct Shared {
    states: Mutex<Vec<State>>,
    trouble: Mutex<Option<String>>,
}

impl Shared {
    fn says(&self, trouble: impl Into<String>) {
        *self.trouble.lock().unwrap_or_else(PoisonError::into_inner) = Some(trouble.into());
    }
}

/// The torrents of this run. Dropping it stops everything.
pub struct Torrents {
    orders: tokio::sync::mpsc::UnboundedSender<Command>,
    shared: Arc<Shared>,
    next: std::cell::Cell<TorrentId>,
    route: Route,
}

impl Torrents {
    /// Starts a session that carries everything the given way.
    pub fn start(
        route: Route,
        directory: PathBuf,
        session_directory: PathBuf,
        upload: bool,
    ) -> Self {
        let (orders, taken) = tokio::sync::mpsc::unbounded_channel();
        let shared = Arc::new(Shared::default());
        let carrier = route.clone();
        let theirs = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("atoll-torrent".to_owned())
            .spawn(move || run(carrier, directory, session_directory, upload, theirs, taken));
        if let Err(error) = spawned {
            shared.says(error.to_string());
        }
        Self {
            orders,
            shared,
            next: std::cell::Cell::new(1),
            route,
        }
    }

    pub fn route(&self) -> &Route {
        &self.route
    }

    /// Takes on a torrent and answers with the number the user interface knows it by.
    pub fn add(&self, source: Source, directory: PathBuf) -> TorrentId {
        let id = self.next.get();
        self.next.set(id + 1);
        let _ = self.orders.send(Command::Add {
            id,
            source,
            directory,
        });
        id
    }

    pub fn pause(&self, id: TorrentId) {
        let _ = self.orders.send(Command::Pause(id));
    }

    pub fn resume(&self, id: TorrentId) {
        let _ = self.orders.send(Command::Resume(id));
    }

    pub fn forget(&self, id: TorrentId, delete_files: bool) {
        let _ = self.orders.send(Command::Forget { id, delete_files });
    }

    /// What every torrent is doing right now.
    pub fn states(&self) -> Vec<State> {
        self.shared
            .states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// What went wrong, once, so the user hears it.
    pub fn take_trouble(&self) -> Option<String> {
        self.shared
            .trouble
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

impl Drop for Torrents {
    fn drop(&mut self) {
        let _ = self.orders.send(Command::Stop);
    }
}

/// The life of the torrent thread.
fn run(
    route: Route,
    directory: PathBuf,
    session_directory: PathBuf,
    upload: bool,
    shared: Arc<Shared>,
    orders: tokio::sync::mpsc::UnboundedReceiver<Command>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            shared.says(error.to_string());
            return;
        }
    };
    runtime.block_on(async move {
        let session = match build_session(&route, &directory, &session_directory, upload).await {
            Ok(session) => session,
            Err(error) => {
                shared.says(t!("Die Torrents konnten nicht gestartet werden: {}", error));
                return;
            }
        };
        serve(session, shared, !route.carries_udp(), orders).await;
    });
}

/// Puts the session together, with everything the chosen way does not allow switched off.
async fn build_session(
    route: &Route,
    directory: &Path,
    session_directory: &Path,
    upload: bool,
) -> Result<Arc<Session>, String> {
    let mut options = SessionOptions {
        // The table and the trackers that speak it only ever run on a device of the system,
        // where the kernel keeps them inside the VPN.
        dht: if route.carries_udp() {
            SessionOptions::default().dht
        } else {
            None
        },
        disable_local_service_discovery: true,
        fastresume: true,
        ipv4_only: true,
        ..SessionOptions::default()
    };
    options.disable_upload = !upload;
    match route {
        Route::Bridge(port) => {
            options.connect = Some(ConnectionOptions {
                proxy_url: Some(format!("socks5://127.0.0.1:{port}")),
                enable_tcp: true,
                peer_opts: None,
            });
            // Nothing may listen: an incoming connection would come from outside the VPN.
            options.listen = None;
        }
        Route::Device(name) => {
            options.bind_device_name = Some(name.clone());
            options.listen = Some(ListenerOptions {
                mode: ListenerMode::TcpAndUtp,
                ipv4_only: true,
                ..ListenerOptions::default()
            });
        }
    }
    let _ = session_directory;
    Session::new_with_opts(directory.to_path_buf(), options)
        .await
        .map_err(|error| error.to_string())
}

/// Takes the orders and keeps the state the user interface reads.
async fn serve(
    session: Arc<Session>,
    shared: Arc<Shared>,
    through_the_bridge: bool,
    mut orders: tokio::sync::mpsc::UnboundedReceiver<Command>,
) {
    let mut known: HashMap<TorrentId, usize> = HashMap::new();
    let mut names: HashMap<TorrentId, (String, PathBuf)> = HashMap::new();
    let mut ticker = tokio::time::interval(POLL);
    loop {
        tokio::select! {
            order = orders.recv() => {
                match order {
                    Some(Command::Add { id, source, directory }) => {
                        let added = add_one(&session, source, &directory, through_the_bridge).await;
                        match added {
                            Ok((theirs, name)) => {
                                known.insert(id, theirs);
                                names.insert(id, (name, directory));
                            }
                            Err(trouble) => {
                                shared.says(trouble);
                                names.remove(&id);
                            }
                        }
                    }
                    Some(Command::Pause(id)) => {
                        if let Some(handle) = known.get(&id).and_then(|theirs| session.get((*theirs).into()))
                            && let Err(error) = session.pause(&handle).await
                        {
                            tracing::debug!(%error, "a torrent could not be paused");
                        }
                    }
                    Some(Command::Resume(id)) => {
                        if let Some(handle) = known.get(&id).and_then(|theirs| session.get((*theirs).into()))
                            && let Err(error) = session.unpause(&handle).await
                        {
                            shared.says(t!("Der Torrent läuft nicht weiter: {}", error));
                        }
                    }
                    Some(Command::Forget { id, delete_files }) => {
                        if let Some(theirs) = known.remove(&id)
                            && let Err(error) = session.delete(theirs.into(), delete_files).await
                        {
                            tracing::debug!(%error, "a torrent could not be removed");
                        }
                        names.remove(&id);
                    }
                    Some(Command::Stop) | None => return,
                }
            }
            _ = ticker.tick() => {}
        }

        let mut states = Vec::with_capacity(known.len());
        for (id, theirs) in &known {
            let Some(handle) = session.get((*theirs).into()) else {
                continue;
            };
            let stats = handle.stats();
            let (name, directory) = names
                .get(id)
                .cloned()
                .unwrap_or_else(|| (String::new(), PathBuf::new()));
            let live = stats.live.as_ref();
            states.push(State {
                id: *id,
                name: handle.name().unwrap_or(name),
                done_bytes: stats.progress_bytes,
                total_bytes: stats.total_bytes,
                speed: live.map_or(0, |live| live.download_speed.as_bytes()),
                peers: live.map_or(0, |live| live.snapshot.peer_stats.live as usize),
                finished: stats.finished,
                paused: matches!(
                    stats.state,
                    TorrentStatsState::Paused | TorrentStatsState::Initializing { paused: true }
                ),
                error: stats.error.clone(),
                folder: Some(directory),
            });
        }
        states.sort_by_key(|state| state.id);
        *shared.states.lock().unwrap_or_else(PoisonError::into_inner) = states;
    }
}

/// Hands one torrent to the session.
///
/// On the way through Atoll's own VPN the trackers are asked here instead of by the client, see
/// [`announce`], and the client is given the peers and told to leave the trackers alone.
async fn add_one(
    session: &Arc<Session>,
    source: Source,
    directory: &Path,
    through_the_bridge: bool,
) -> Result<(usize, String), String> {
    let torrent = match &source {
        Source::Link(link) => AddTorrent::from_url(link.as_str()),
        Source::File(bytes) => AddTorrent::from_bytes(bytes.clone()),
    };
    let peers = if through_the_bridge {
        found_peers(&source).await
    } else {
        None
    };
    let options = AddTorrentOptions {
        output_folder: Some(directory.display().to_string()),
        overwrite: true,
        disable_trackers: peers.is_some(),
        initial_peers: peers,
        ..AddTorrentOptions::default()
    };
    let answer = session
        .add_torrent(torrent, Some(options))
        .await
        .map_err(|error| {
            t!(
                "Der Torrent konnte nicht geladen werden: {}",
                format!("{error:#}")
            )
        })?;
    match answer {
        AddTorrentResponse::Added(id, handle) | AddTorrentResponse::AlreadyManaged(id, handle) => {
            let name = handle.name().unwrap_or_default();
            Ok((id, name))
        }
        AddTorrentResponse::ListOnly(_) => {
            Err(t!("Der Torrent wurde nur gelesen, nicht geladen").to_owned())
        }
    }
}

/// Asks the trackers of a torrent for peers, off the runtime because the asking blocks.
async fn found_peers(source: &Source) -> Option<Vec<std::net::SocketAddr>> {
    let (info_hash, trackers) = what_to_ask_for(source)?;
    if trackers.is_empty() {
        tracing::debug!("the torrent names no tracker Atoll could ask through the VPN");
        return Some(Vec::new());
    }
    let peer_id = generate_peer_id(b"-AT0001-").0;
    let found =
        tokio::task::spawn_blocking(move || announce::peers_of(&info_hash, &peer_id, &trackers))
            .await
            .unwrap_or_default();
    tracing::info!(
        peers = found.len(),
        "the trackers were asked through the VPN"
    );
    Some(found)
}

/// What a torrent says about itself: which hash it has and which trackers it names.
fn what_to_ask_for(source: &Source) -> Option<([u8; 20], Vec<String>)> {
    match source {
        Source::Link(link) if link.starts_with("magnet:") => {
            let magnet = Magnet::parse(link).ok()?;
            let hash = magnet.as_id20()?.0;
            Some((hash, magnet.trackers.clone()))
        }
        // An address that is not a magnet is a torrent file Atoll has not read yet.
        Source::Link(_) => None,
        Source::File(bytes) => {
            let torrent = torrent_from_bytes(bytes).ok()?;
            let trackers = torrent
                .iter_announce()
                .map(|tracker| String::from_utf8_lossy(tracker.as_ref()).into_owned())
                .collect();
            Some((torrent.info_hash.0, trackers))
        }
    }
}
