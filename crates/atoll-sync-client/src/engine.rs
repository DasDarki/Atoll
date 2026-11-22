//! The sync thread. It has a database connection of its own, so the windows never wait for the
//! network or for records being taken over, and a second thread that listens to the event stream
//! of the server and wakes it when another device changed something.
//!
//! What it does depends on the mode: reading and writing sends what changed here two seconds
//! after the last change and takes over what changed elsewhere; reading mirrors the server; on
//! demand and off do nothing by themselves. What it did is told through [`Report`].

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use atoll_storage::prelude::*;
use atoll_storage::{Applied, Incoming, Storage};
use atoll_sync::wire::{Change, DeviceName, Platform, Stored};
use atoll_sync::{DataKey, Kind, Record, SyncId};

use crate::api::{Api, ApiError};

const CURSOR_KEY: &str = "sync_cursor";
const LINKED_KEY: &str = "sync_linked";

/// How often the thread looks whether something is due.
const TICK: Duration = Duration::from_secs(2);
/// How long changes have to rest before they are sent, so a page loading does not send its
/// address five times.
const SETTLE_MILLIS: i64 = 2000;
/// How often changes are fetched even when the event stream said nothing.
const PULL_EVERY: Duration = Duration::from_secs(60);
/// How long to wait after the server could not be reached.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// The most changes sent at once.
const PUSH_CHUNK: usize = 500;

/// In which direction this device keeps in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// Every change is sent, every change elsewhere taken over.
    ReadWrite,
    /// The server is mirrored as soon as it changes, changes here are discarded.
    Read,
    /// The server is mirrored only when asked.
    ReadOnDemand,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::ReadWrite => "read_write",
            Mode::Read => "read",
            Mode::ReadOnDemand => "read_on_demand",
        }
    }

    pub fn parse(name: &str) -> Option<Mode> {
        [Mode::Off, Mode::ReadWrite, Mode::Read, Mode::ReadOnDemand]
            .into_iter()
            .find(|mode| mode.as_str() == name)
    }

    fn listens(self) -> bool {
        matches!(self, Mode::ReadWrite | Mode::Read)
    }

    fn code(self) -> u8 {
        self as u8
    }

    fn from_code(code: u8) -> Mode {
        match code {
            1 => Mode::ReadWrite,
            2 => Mode::Read,
            3 => Mode::ReadOnDemand,
            _ => Mode::Off,
        }
    }
}

/// What to do when this device and the server both hold data the first time they meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Keep both; workspaces of the same name become one.
    Merge,
    /// Take the server's state, drop what is here.
    TakeServer,
    /// Put this device's state on the server, drop what is there.
    Upload,
}

pub enum Command {
    /// The event stream said something changed.
    Wake,
    /// Send and fetch now, or mirror in the reading modes.
    Now,
    /// Mirror the server once, whatever the mode.
    Mirror,
    Mode(Mode),
    Link(Choice),
    Stop,
}

/// What the thread tells the windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// A round ended: how many changes went out and what came in.
    Synced { sent: usize, applied: Applied },
    /// Both sides hold data; the user has to choose with [`Command::Link`].
    NeedsChoice {
        local_tabs: usize,
        server_tabs: usize,
    },
    /// The server could not be reached or refused, the thread tries again.
    Failed(String),
    /// The sign-in is gone, the thread stopped.
    SignedOut,
}

/// Everything the thread needs.
pub struct Setup {
    pub database: PathBuf,
    pub api: Api,
    pub key: DataKey,
    pub mode: Mode,
    /// Who and where this device is linked to, to know whether it met this account before.
    pub account: String,
    pub device_name: String,
}

pub struct Engine {
    commands: Sender<Command>,
    stop: Arc<AtomicBool>,
    mode: Arc<AtomicU8>,
    threads: Vec<JoinHandle<()>>,
}

impl Engine {
    /// Starts the thread and its listener. `report` is called on the sync thread.
    pub fn start(setup: Setup, report: impl Fn(Report) + Send + 'static) -> std::io::Result<Self> {
        let (commands, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mode = Arc::new(AtomicU8::new(setup.mode.code()));
        let listener = {
            let api = setup.api.clone();
            let commands = commands.clone();
            let stop = stop.clone();
            let mode = mode.clone();
            std::thread::Builder::new()
                .name("sync-listener".to_owned())
                .spawn(move || listen(&api, &commands, &stop, &mode))?
        };
        let worker = {
            let mode = mode.clone();
            std::thread::Builder::new()
                .name("sync".to_owned())
                .spawn(move || match Worker::new(setup, Box::new(report), mode) {
                    Ok(worker) => worker.run(receiver),
                    Err(error) => {
                        tracing::error!(%error, "the sync thread could not open the database")
                    }
                })?
        };
        Ok(Self {
            commands,
            stop,
            mode,
            threads: vec![worker, listener],
        })
    }

    pub fn send(&self, command: Command) {
        if let Command::Mode(mode) = &command {
            self.mode.store(mode.code(), Ordering::SeqCst);
        }
        let _ = self.commands.send(command);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.commands.send(Command::Stop);
        for thread in self.threads.drain(..) {
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

/// Keeps the event stream open while the mode wants it and wakes the sync thread on news.
fn listen(api: &Api, commands: &Sender<Command>, stop: &AtomicBool, mode: &AtomicU8) {
    let mut pause = Duration::from_secs(2);
    while !stop.load(Ordering::SeqCst) {
        if !Mode::from_code(mode.load(Ordering::SeqCst)).listens() {
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let result = api.listen(|_| {
            if stop.load(Ordering::SeqCst)
                || !Mode::from_code(mode.load(Ordering::SeqCst)).listens()
            {
                return false;
            }
            commands.send(Command::Wake).is_ok()
        });
        match result {
            Ok(()) => pause = Duration::from_secs(2),
            Err(error) => {
                tracing::debug!(%error, "the event stream closed");
                pause = (pause * 2).min(Duration::from_secs(60));
            }
        }
        let until = Instant::now() + pause;
        while Instant::now() < until && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

struct Worker {
    storage: Storage,
    api: Api,
    key: DataKey,
    mode: Arc<AtomicU8>,
    account: String,
    device_name: String,
    report: Box<dyn Fn(Report) + Send>,
    woken: bool,
    last_pull: Option<Instant>,
    resting_until: Option<Instant>,
    asked_for_choice: bool,
    named: bool,
}

impl Worker {
    fn new(
        setup: Setup,
        report: Box<dyn Fn(Report) + Send>,
        mode: Arc<AtomicU8>,
    ) -> Result<Self, atoll_storage::StorageError> {
        Ok(Self {
            storage: Storage::open(&setup.database)?,
            api: setup.api,
            key: setup.key,
            mode,
            account: setup.account,
            device_name: setup.device_name,
            report,
            woken: true,
            last_pull: None,
            resting_until: None,
            asked_for_choice: false,
            named: false,
        })
    }

    fn mode(&self) -> Mode {
        Mode::from_code(self.mode.load(Ordering::SeqCst))
    }

    fn run(mut self, commands: Receiver<Command>) {
        loop {
            let command = commands.recv_timeout(TICK);
            let outcome = match command {
                Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(Command::Wake) => {
                    self.woken = true;
                    self.tick()
                }
                Ok(Command::Now) => {
                    self.resting_until = None;
                    match self.mode() {
                        Mode::ReadWrite => self.read_write(true),
                        Mode::Read | Mode::ReadOnDemand => self.mirror(),
                        Mode::Off => Ok(()),
                    }
                }
                Ok(Command::Mirror) => {
                    self.resting_until = None;
                    self.mirror()
                }
                Ok(Command::Mode(_)) => {
                    self.asked_for_choice = false;
                    self.woken = true;
                    self.tick()
                }
                Ok(Command::Link(choice)) => self.link(choice),
                Err(RecvTimeoutError::Timeout) => self.tick(),
            };
            match outcome {
                Ok(()) => {}
                Err(ApiError::SignedOut) => {
                    (self.report)(Report::SignedOut);
                    return;
                }
                Err(error) => {
                    tracing::info!(%error, "sync round failed");
                    self.resting_until = Some(Instant::now() + RETRY_AFTER);
                    (self.report)(Report::Failed(error.to_string()));
                }
            }
        }
    }

    fn tick(&mut self) -> Result<(), ApiError> {
        if self
            .resting_until
            .is_some_and(|until| Instant::now() < until)
        {
            return Ok(());
        }
        self.resting_until = None;
        match self.mode() {
            Mode::ReadWrite => self.read_write(false),
            Mode::Read => {
                if self.pull_due() {
                    if self
                        .storage
                        .sync_outbox()
                        .map_err(storage_error)?
                        .is_empty()
                    {
                        self.pull()
                    } else {
                        self.mirror()
                    }
                } else {
                    Ok(())
                }
            }
            Mode::ReadOnDemand | Mode::Off => Ok(()),
        }
    }

    fn pull_due(&self) -> bool {
        self.woken || self.last_pull.is_none_or(|at| at.elapsed() >= PULL_EVERY)
    }

    fn read_write(&mut self, now: bool) -> Result<(), ApiError> {
        if !self.linked()? {
            if self.asked_for_choice {
                return Ok(());
            }
            return self.first_meeting();
        }
        self.name_once();
        let sent = self.push(now)?;
        if now || sent > 0 || self.pull_due() {
            self.pull_reporting(sent)?;
        }
        Ok(())
    }

    fn name_once(&mut self) {
        if self.named {
            return;
        }
        let name = DeviceName {
            name: self.device_name.clone(),
            platform: Platform::Desktop,
        };
        match self.api.name_device(&name) {
            Ok(()) => self.named = true,
            Err(error) => tracing::debug!(%error, "the device could not be named"),
        }
    }

    fn linked(&self) -> Result<bool, ApiError> {
        Ok(self
            .storage
            .setting(LINKED_KEY)
            .map_err(storage_error)?
            .as_deref()
            == Some(self.account.as_str()))
    }

    fn cursor(&self) -> Result<i64, ApiError> {
        Ok(self
            .storage
            .setting_i64(CURSOR_KEY)
            .map_err(storage_error)?
            .unwrap_or(0))
    }

    /// The first round with this account: decides alone where that is clear, otherwise asks.
    fn first_meeting(&mut self) -> Result<(), ApiError> {
        let snapshot = self.api.snapshot()?;
        let server_tabs = snapshot
            .records
            .iter()
            .filter(|record| record.kind == Kind::Tab)
            .count();
        let local = self.storage.sync_records().map_err(storage_error)?;
        let local_tabs = local
            .iter()
            .filter(|(kind, _, record)| {
                *kind == Kind::ArchivedTab
                    || matches!(record, Record::Tab(tab) if !tab.url.is_empty())
            })
            .count();
        if snapshot.records.is_empty() {
            return self.link(Choice::Upload);
        }
        if local_tabs == 0 {
            return self.link(Choice::TakeServer);
        }
        self.asked_for_choice = true;
        (self.report)(Report::NeedsChoice {
            local_tabs,
            server_tabs,
        });
        Ok(())
    }

    /// Joins this device to the account in the way chosen.
    fn link(&mut self, choice: Choice) -> Result<(), ApiError> {
        let snapshot = self.api.snapshot()?;
        let server = self.open_all(&snapshot.records);
        let server_ids: HashSet<(Kind, SyncId)> =
            server.iter().map(|(kind, id, _)| (*kind, *id)).collect();
        let mut applied = Applied::default();
        match choice {
            Choice::TakeServer => {
                applied = self
                    .storage
                    .transaction(|transaction| {
                        let applied = transaction.mirror_sync(server.clone())?;
                        transaction.clear_sync_outbox()?;
                        Ok(applied)
                    })
                    .map_err(storage_error)?;
            }
            Choice::Merge => {
                let workspaces = self.storage.workspaces().map_err(storage_error)?;
                applied = self
                    .storage
                    .transaction(|transaction| {
                        let mut adopted = HashSet::new();
                        for (kind, id, record) in &server {
                            let (Kind::Workspace, Record::Workspace(remote)) = (kind, record)
                            else {
                                continue;
                            };
                            if transaction.local_id(Kind::Workspace, *id)?.is_some() {
                                continue;
                            }
                            for local in workspaces.iter().filter(|local| local.name == remote.name)
                            {
                                let local_id = sync_id_of_workspace(transaction, local.id.0)?;
                                if !adopted.contains(&local.id)
                                    && !server_ids.contains(&(Kind::Workspace, local_id))
                                {
                                    transaction.adopt_workspace(local.id, *id)?;
                                    adopted.insert(local.id);
                                    break;
                                }
                            }
                        }
                        let incoming = server
                            .iter()
                            .cloned()
                            .map(|(kind, id, record)| Incoming {
                                kind,
                                id,
                                record: Some(record),
                            })
                            .collect();
                        transaction.apply_sync(incoming)
                    })
                    .map_err(storage_error)?;
                self.send_all(|kind, id| !server_ids.contains(&(kind, id)))?;
            }
            Choice::Upload => {
                let local: HashSet<(Kind, SyncId)> = self
                    .storage
                    .sync_records()
                    .map_err(storage_error)?
                    .into_iter()
                    .map(|(kind, id, _)| (kind, id))
                    .collect();
                let removals: Vec<Change> = server_ids
                    .iter()
                    .filter(|known| !local.contains(known))
                    .map(|(kind, id)| Change {
                        kind: *kind,
                        id: *id,
                        data: None,
                    })
                    .collect();
                for chunk in removals.chunks(PUSH_CHUNK) {
                    self.api.push(chunk.to_vec())?;
                }
                self.send_all(|_, _| true)?;
            }
        }
        self.storage
            .transaction(|transaction| {
                transaction.clear_sync_outbox()?;
                transaction.set_setting(CURSOR_KEY, &snapshot.cursor.to_string())?;
                transaction.set_setting(LINKED_KEY, &self.account)
            })
            .map_err(storage_error)?;
        self.asked_for_choice = false;
        self.last_pull = Some(Instant::now());
        tracing::info!(?choice, "this device joined the account");
        (self.report)(Report::Synced { sent: 0, applied });
        Ok(())
    }

    /// Sends every record here that `wanted` picks, as one would after joining.
    fn send_all(&mut self, wanted: impl Fn(Kind, SyncId) -> bool) -> Result<usize, ApiError> {
        self.storage
            .transaction(|transaction| transaction.give_order_keys())
            .map_err(storage_error)?;
        let changes: Vec<Change> = self
            .storage
            .sync_records()
            .map_err(storage_error)?
            .into_iter()
            .filter(|(kind, id, _)| wanted(*kind, *id))
            .map(|(_, id, record)| Change {
                kind: record.kind(),
                id,
                data: Some(self.key.seal_record(id, &record)),
            })
            .collect();
        for chunk in changes.chunks(PUSH_CHUNK) {
            self.api.push(chunk.to_vec())?;
        }
        Ok(changes.len())
    }

    /// Sends what changed here, once it has rested, or at once when asked to.
    fn push(&mut self, now: bool) -> Result<usize, ApiError> {
        let entries = self.storage.sync_outbox().map_err(storage_error)?;
        if entries.is_empty() {
            return Ok(0);
        }
        let newest = entries
            .iter()
            .map(|entry| entry.changed_at)
            .max()
            .unwrap_or_default();
        if !now && unix_millis() - newest < SETTLE_MILLIS {
            return Ok(0);
        }
        let rekeyed = self
            .storage
            .transaction(|transaction| transaction.give_order_keys())
            .map_err(storage_error)?;
        let mut due: BTreeSet<(usize, Kind, SyncId)> = BTreeSet::new();
        for entry in &entries {
            if let (Some(kind), Ok(id)) =
                (Kind::parse(&entry.kind), entry.sync_id.parse::<SyncId>())
            {
                due.insert((rank(kind), kind, id));
            }
        }
        for (kind, id) in rekeyed {
            due.insert((rank(kind), kind, id));
        }
        let mut changes = Vec::with_capacity(due.len());
        for (_, kind, id) in due {
            let record = self.storage.sync_record(kind, id).map_err(storage_error)?;
            changes.push(Change {
                kind,
                id,
                data: record.map(|record| self.key.seal_record(id, &record)),
            });
        }
        for chunk in changes.chunks(PUSH_CHUNK) {
            self.api.push(chunk.to_vec())?;
        }
        self.storage
            .forget_outbox(&entries)
            .map_err(storage_error)?;
        Ok(changes.len())
    }

    fn pull_reporting(&mut self, sent: usize) -> Result<(), ApiError> {
        match self.pull_changes() {
            Ok(applied) => {
                (self.report)(Report::Synced { sent, applied });
                Ok(())
            }
            Err(ApiError::ResyncRequired) => {
                self.push(true)?;
                self.mirror_reporting(sent)
            }
            Err(error) => Err(error),
        }
    }

    fn pull(&mut self) -> Result<(), ApiError> {
        self.pull_reporting(0)
    }

    /// Takes over what changed elsewhere since the cursor.
    fn pull_changes(&mut self) -> Result<Applied, ApiError> {
        self.woken = false;
        let mut cursor = self.cursor()?;
        let mut total = Applied::default();
        loop {
            let page = self.api.changes(cursor)?;
            let own = self.api.device();
            let incoming: Vec<Incoming> = page
                .changes
                .iter()
                .filter(|change| change.device != own)
                .filter_map(|change| self.open(change))
                .collect();
            let next = page.cursor;
            let applied = self
                .storage
                .transaction(|transaction| {
                    let applied = transaction.apply_sync(incoming)?;
                    transaction.set_setting(CURSOR_KEY, &next.to_string())?;
                    Ok(applied)
                })
                .map_err(storage_error)?;
            merge(&mut total, applied);
            cursor = next;
            if !page.more {
                break;
            }
        }
        self.last_pull = Some(Instant::now());
        Ok(total)
    }

    fn mirror(&mut self) -> Result<(), ApiError> {
        self.mirror_reporting(0)
    }

    /// Makes this device hold exactly what the server holds.
    fn mirror_reporting(&mut self, sent: usize) -> Result<(), ApiError> {
        self.woken = false;
        let snapshot = self.api.snapshot()?;
        let records = self.open_all(&snapshot.records);
        let applied = self
            .storage
            .transaction(|transaction| {
                let applied = transaction.mirror_sync(records)?;
                transaction.clear_sync_outbox()?;
                transaction.set_setting(CURSOR_KEY, &snapshot.cursor.to_string())?;
                Ok(applied)
            })
            .map_err(storage_error)?;
        self.last_pull = Some(Instant::now());
        (self.report)(Report::Synced { sent, applied });
        Ok(())
    }

    /// Opens one stored change. One that does not open, sealed with another key or changed on
    /// the way, is passed over.
    fn open(&self, stored: &Stored) -> Option<Incoming> {
        match &stored.data {
            None => Some(Incoming {
                kind: stored.kind,
                id: stored.id,
                record: None,
            }),
            Some(sealed) => match self.key.open_record(stored.kind, stored.id, sealed) {
                Ok(record) => Some(Incoming {
                    kind: stored.kind,
                    id: stored.id,
                    record: Some(record),
                }),
                Err(error) => {
                    tracing::warn!(%error, kind = stored.kind.as_str(), "a record could not be opened");
                    None
                }
            },
        }
    }

    fn open_all(&self, stored: &[Stored]) -> Vec<(Kind, SyncId, Record)> {
        stored
            .iter()
            .filter_map(|stored| self.open(stored))
            .filter_map(|incoming| {
                incoming
                    .record
                    .map(|record| (incoming.kind, incoming.id, record))
            })
            .collect()
    }
}

fn sync_id_of_workspace<S: Connected + ?Sized>(
    store: &S,
    local: i64,
) -> atoll_storage::StorageResult<SyncId> {
    let text: String = store.connection().query_row(
        "SELECT sync_id FROM workspace WHERE id = ?1",
        [local],
        |row| row.get(0),
    )?;
    Ok(text.parse().unwrap_or_else(|_| SyncId::random()))
}

fn rank(kind: Kind) -> usize {
    Kind::ALL
        .iter()
        .position(|known| *known == kind)
        .unwrap_or(0)
}

fn merge(total: &mut Applied, applied: Applied) {
    total.changed += applied.changed;
    total.orphans += applied.orphans;
    total.moved_tabs.extend(applied.moved_tabs);
    total.removed_workspaces.extend(applied.removed_workspaces);
}

fn storage_error(error: atoll_storage::StorageError) -> ApiError {
    ApiError::Refused(format!("the database failed: {error}"))
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
