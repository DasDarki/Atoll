//! Atoll Sync on this device: the key the records are sealed with, the sync thread and what it
//! reports. Signing in lives in `account`; this part starts once the account is signed in.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use atoll_storage::Applied;
use atoll_storage::StorageError;
use atoll_storage::prelude::*;
use atoll_sync::wire::{Device, DeviceName, Platform, PutKey, StoredKey};
use atoll_sync::{DataKey, Hardness, KeyEnvelope, SyncError, SyncId};
use atoll_sync_client::{Api, ApiError, Auth, Choice, Command, Engine, Mode, Report, Setup};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Browser;
use super::adblock::relative_age;
use super::preferences::PreferenceKey;
use super::windows::WindowManager;
use crate::platform::tasks::run_in_background;
use crate::ui::{SettingsState, SyncDeviceItem};
use atoll_i18n::t;

/// Where the id of this device for the sync server is kept.
const DEVICE_ID_KEY: &str = "sync_device_id";

/// The shortest passphrase accepted.
const MIN_PASSPHRASE: usize = 8;

/// A page the user did something in this recently is not sent elsewhere by another device.
const QUIET_PAGE: Duration = Duration::from_secs(10);

/// A page sent to an address this recently is not sent again by another device. Pages that
/// redirect to a new address on every load would otherwise bounce between the devices.
const SETTLING_PAGE: Duration = Duration::from_secs(30);

type Listener = Box<dyn Fn(u64, Report)>;

thread_local! {
    static LISTENER: RefCell<Option<Listener>> = const { RefCell::new(None) };
}

/// Where the key of the account stands on this device.
#[derive(Default)]
enum KeyState {
    /// Not signed in, or not asked yet.
    #[default]
    Unknown,
    Checking,
    /// The server could not be asked.
    Unreachable(String),
    /// Sync was never set up for the account.
    Missing,
    /// Sync is set up, this device does not hold the key yet.
    Locked(StoredKey),
    /// A passphrase is being turned into a key.
    Working,
    Ready,
}

/// Atoll Sync on this device, the same for every window.
#[derive(Default)]
pub struct SyncState {
    key: KeyState,
    engine: Option<Engine>,
    /// Counts the sync threads started, so reports of a stopped one are passed over.
    generation: u64,
    last_synced: Option<Instant>,
    failure: Option<String>,
    /// Tabs here and on the server while the user has not chosen how they come together.
    choice: Option<(usize, usize)>,
    devices: Vec<Device>,
    /// The recovery key while it is shown.
    recovery_key: Option<String>,
    /// The name the server was last told for this device.
    device_name_sent: Option<String>,
    error: String,
}

impl Browser {
    pub fn sync_action(&self, action: &str) {
        match action {
            "retry" => self.check_sync_key(),
            "set-up" => self.set_up_sync(),
            "unlock" => self.unlock_sync(),
            "now" => self.sync_now(),
            "mirror" => self.mirror_from_server(),
            "choose" => self.ask_sync_choice(),
            "change-passphrase" => self.change_sync_passphrase(),
            "show-recovery" => self.show_recovery_key(),
            "copy-recovery" => {
                let key = self.sync.borrow().recovery_key.clone();
                if let Some(key) = key {
                    self.copy_text(&key);
                }
            }
            "hide-recovery" => {
                self.sync.borrow_mut().recovery_key = None;
                self.show_sync_everywhere();
            }
            "reset" => self.reset_sync(),
            other => tracing::warn!(action = other, "unknown sync action"),
        }
    }

    /// Picks sync up after signing in: with the key kept on this device it starts at once,
    /// otherwise the server is asked whether sync was set up.
    pub(super) fn sync_signed_in(&self) {
        match self.token_store().load_key() {
            Some(_) => {
                self.sync.borrow_mut().key = KeyState::Ready;
                self.start_sync_engine();
                self.show_sync_everywhere();
            }
            None => self.check_sync_key(),
        }
    }

    /// Stops the sync thread and forgets the key, after signing out.
    pub(super) fn sync_signed_out(&self) {
        self.token_store().clear_key();
        let generation = self.sync.borrow().generation + 1;
        *self.sync.borrow_mut() = SyncState {
            generation,
            ..SyncState::default()
        };
        self.show_sync_everywhere();
    }

    fn sync_api(&self, auth: &Arc<Auth>) -> Api {
        let tokens: Arc<dyn atoll_sync_client::Tokens> = auth.clone();
        Api::new(auth.server(), self.sync_device_id(), tokens)
    }

    /// The id of this device, made up once.
    fn sync_device_id(&self) -> SyncId {
        if let Ok(Some(text)) = self.storage.setting(DEVICE_ID_KEY)
            && let Ok(id) = text.parse()
        {
            return id;
        }
        let id = SyncId::random();
        if let Err(error) = self.storage.set_setting(DEVICE_ID_KEY, &id.to_string()) {
            tracing::warn!(%error, "the id of this device could not be kept");
        }
        id
    }

    /// Asks the server whether sync is set up for the account.
    fn check_sync_key(&self) {
        let Some(auth) = self.signed_in_auth() else {
            return;
        };
        self.sync.borrow_mut().key = KeyState::Checking;
        self.show_sync_everywhere();
        let api = self.sync_api(&auth);
        let this = self.this();
        run_in_background(
            "sync-key",
            move || api.key(),
            move |answer| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                if browser.signed_in_auth().is_none() {
                    return;
                }
                let key = match answer {
                    Ok(None) => KeyState::Missing,
                    Ok(Some(stored)) => KeyState::Locked(stored),
                    Err(ApiError::SignedOut) => {
                        browser.signed_out_by_server();
                        return;
                    }
                    Err(error) => KeyState::Unreachable(error.to_string()),
                };
                browser.sync.borrow_mut().key = key;
                browser.show_sync_everywhere();
            },
        );
    }

    /// The passphrase typed twice, or why it cannot be taken.
    fn new_passphrase(&self) -> Option<String> {
        let window = self.window()?;
        let state = window.global::<SettingsState>();
        let passphrase = state.get_sync_passphrase().to_string();
        let error = if passphrase.chars().count() < MIN_PASSPHRASE {
            Some(t!(
                "Die Passphrase braucht mindestens {} Zeichen.",
                MIN_PASSPHRASE
            ))
        } else if passphrase != state.get_sync_passphrase_repeat().as_str() {
            Some(t!("Die beiden Passphrasen stimmen nicht überein.").to_owned())
        } else {
            None
        };
        match error {
            Some(error) => {
                self.sync_error(&error);
                None
            }
            None => Some(passphrase),
        }
    }

    /// Sets sync up for the account: makes the key, wraps it with the passphrase and puts it on
    /// the server, unless another device was quicker.
    fn set_up_sync(&self) {
        let Some(auth) = self.signed_in_auth() else {
            return;
        };
        if !matches!(self.sync.borrow().key, KeyState::Missing) {
            return;
        }
        let Some(passphrase) = self.new_passphrase() else {
            return;
        };
        self.sync_working();
        let api = self.sync_api(&auth);
        let this = self.this();
        run_in_background(
            "sync-set-up",
            move || -> Result<DataKey, ApiError> {
                let key = DataKey::generate();
                let envelope = KeyEnvelope::wrap(&key, &passphrase, Hardness::default())
                    .map_err(|error| ApiError::Refused(error.to_string()))?;
                api.put_key(&PutKey {
                    envelope: (&envelope).into(),
                    replaces: None,
                })?;
                Ok(key)
            },
            move |result| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                match result {
                    Ok(key) => {
                        browser.sync.borrow_mut().recovery_key = Some(key.recovery_key());
                        browser.key_unlocked(&key);
                    }
                    Err(ApiError::KeyConflict) => {
                        browser.check_sync_key();
                        browser.sync_error(t!(
                            "Ein anderes Gerät hat Sync gerade eingerichtet. Entsperre mit dessen Passphrase."
                        ));
                    }
                    Err(ApiError::SignedOut) => browser.signed_out_by_server(),
                    Err(error) => {
                        browser.sync.borrow_mut().key = KeyState::Missing;
                        browser.sync_error(&t!("Einrichten fehlgeschlagen: {}", error));
                    }
                }
            },
        );
    }

    /// Opens the key with the passphrase or the recovery key typed in.
    fn unlock_sync(&self) {
        let stored = match &self.sync.borrow().key {
            KeyState::Locked(stored) => stored.clone(),
            _ => return,
        };
        let Some(window) = self.window() else { return };
        let secret = window
            .global::<SettingsState>()
            .get_sync_passphrase()
            .to_string();
        if secret.trim().is_empty() {
            self.sync_error(t!(
                "Gib die Passphrase oder den Wiederherstellungsschlüssel ein."
            ));
            return;
        }
        self.sync_working();
        let this = self.this();
        let kept = stored.clone();
        run_in_background(
            "sync-unlock",
            move || unlock(&stored, &secret),
            move |result| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                match result {
                    Ok(key) => browser.key_unlocked(&key),
                    Err(error) => {
                        browser.sync.borrow_mut().key = KeyState::Locked(kept);
                        let message = match error {
                            SyncError::WrongKey | SyncError::InvalidRecoveryKey => {
                                t!("Passphrase oder Wiederherstellungsschlüssel passt nicht.")
                                    .to_owned()
                            }
                            other => t!("Entsperren fehlgeschlagen: {}", other),
                        };
                        browser.sync_error(&message);
                    }
                }
            },
        );
    }

    /// Keeps the key on this device and starts the sync thread.
    fn key_unlocked(&self, key: &DataKey) {
        if let Err(error) = self.token_store().save_key(key) {
            tracing::error!(%error, "the sync key could not be kept");
            self.sync.borrow_mut().key = KeyState::Missing;
            self.sync_error(&t!(
                "Der Schlüssel konnte nicht gespeichert werden: {}",
                error
            ));
            return;
        }
        self.sync.borrow_mut().key = KeyState::Ready;
        self.clear_passphrase_fields();
        self.sync_error("");
        self.start_sync_engine();
        self.refresh_sync_devices();
        self.show_toast(t!("Atoll Sync ist eingerichtet"));
    }

    fn sync_working(&self) {
        self.sync.borrow_mut().key = KeyState::Working;
        self.sync_error("");
        self.show_sync_everywhere();
    }

    fn clear_passphrase_fields(&self) {
        self.for_all_windows(|browser| {
            if let Some(window) = browser.window() {
                let state = window.global::<SettingsState>();
                state.set_sync_passphrase("".into());
                state.set_sync_passphrase_repeat("".into());
                state.set_sync_changing_passphrase(false);
            }
        });
    }

    /// Wraps the key with a new passphrase. The other devices keep their key and go on.
    fn change_sync_passphrase(&self) {
        let Some(auth) = self.signed_in_auth() else {
            return;
        };
        let Some(key) = self.token_store().load_key() else {
            return;
        };
        let Some(passphrase) = self.new_passphrase() else {
            return;
        };
        self.sync_error("");
        let api = self.sync_api(&auth);
        let this = self.this();
        run_in_background(
            "sync-passphrase",
            move || -> Result<(), ApiError> {
                let stored = api.key()?.ok_or(ApiError::NoKey)?;
                if !stored.envelope.key_id.is_empty() && stored.envelope.key_id != key.id() {
                    return Err(ApiError::KeyChanged);
                }
                let envelope = KeyEnvelope::wrap(&key, &passphrase, Hardness::default())
                    .map_err(|error| ApiError::Refused(error.to_string()))?;
                api.put_key(&PutKey {
                    envelope: (&envelope).into(),
                    replaces: Some(stored.version),
                })?;
                Ok(())
            },
            move |result| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                match result {
                    Ok(()) => {
                        browser.clear_passphrase_fields();
                        browser.show_toast(t!("Passphrase geändert"));
                    }
                    Err(ApiError::KeyChanged | ApiError::NoKey) => browser.key_changed(),
                    Err(ApiError::KeyConflict) => browser.sync_error(t!(
                        "Ein anderes Gerät hat die Passphrase gerade geändert. Versuche es noch einmal."
                    )),
                    Err(ApiError::SignedOut) => browser.signed_out_by_server(),
                    Err(error) => browser.sync_error(&t!("Ändern fehlgeschlagen: {}", error)),
                }
            },
        );
    }

    fn show_recovery_key(&self) {
        let Some(key) = self.token_store().load_key() else {
            return;
        };
        self.sync.borrow_mut().recovery_key = Some(key.recovery_key());
        self.show_sync_everywhere();
    }

    /// Removes everything the account keeps on the server. What is on the devices stays.
    fn reset_sync(&self) {
        let Some(auth) = self.signed_in_auth() else {
            return;
        };
        let this = self.this();
        self.confirm(
            t!("Atoll Sync zurücksetzen?"),
            t!(
                "Alles, was der Server für dein Konto speichert, wird gelöscht, der Schlüssel eingeschlossen. Was auf deinen Geräten liegt, bleibt. Die anderen Geräte halten an, bis sie mit der neuen Passphrase entsperrt werden."
            ),
            t!("Zurücksetzen"),
            move || {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                browser.stop_sync_engine();
                browser.sync_working();
                let api = browser.sync_api(&auth);
                let this = browser.this();
                run_in_background(
                    "sync-reset",
                    move || api.reset(),
                    move |result| {
                        let Some(browser) = this.upgrade() else {
                            return;
                        };
                        match result {
                            Ok(()) => {
                                browser.token_store().clear_key();
                                {
                                    let mut sync = browser.sync.borrow_mut();
                                    sync.key = KeyState::Missing;
                                    sync.recovery_key = None;
                                    sync.devices.clear();
                                    sync.choice = None;
                                }
                                browser.show_sync_everywhere();
                                browser.show_toast(t!("Atoll Sync wurde zurückgesetzt"));
                            }
                            Err(ApiError::SignedOut) => browser.signed_out_by_server(),
                            Err(error) => {
                                browser.sync.borrow_mut().key = KeyState::Ready;
                                browser.start_sync_engine();
                                browser
                                    .sync_error(&t!("Zurücksetzen fehlgeschlagen: {}", error));
                            }
                        }
                    },
                );
            },
        );
    }

    /// Starts the sync thread when signed in, the key is here and sync is not off.
    fn start_sync_engine(&self) {
        self.stop_sync_engine();
        let Some(auth) = self.signed_in_auth() else {
            return;
        };
        let mode = self.preferences.borrow().sync_mode();
        if mode == Mode::Off || !matches!(self.sync.borrow().key, KeyState::Ready) {
            return;
        }
        let Some(key) = self.token_store().load_key() else {
            return;
        };
        let generation = {
            let mut sync = self.sync.borrow_mut();
            sync.generation += 1;
            sync.failure = None;
            sync.generation
        };
        if let Some(manager) = self.manager() {
            let manager = Rc::downgrade(&manager);
            LISTENER.with_borrow_mut(|listener| {
                *listener = Some(Box::new(move |generation, report| {
                    if let Some(browser) = manager
                        .upgrade()
                        .and_then(|manager: Rc<WindowManager>| manager.focused_browser())
                    {
                        browser.sync_reported(generation, report);
                    }
                }));
            });
        }
        let device_name = self.preferences.borrow().sync_device_name();
        self.sync.borrow_mut().device_name_sent = Some(device_name.clone());
        let setup = Setup {
            database: self.paths.database.clone(),
            api: self.sync_api(&auth),
            account: format!("{}#{}", auth.server(), key.id()),
            key,
            mode,
            device_name,
        };
        let started = Engine::start(setup, move |report| {
            let _ = slint::invoke_from_event_loop(move || {
                LISTENER.with_borrow(|listener| {
                    if let Some(listener) = listener {
                        listener(generation, report);
                    }
                });
            });
        });
        match started {
            Ok(engine) => self.sync.borrow_mut().engine = Some(engine),
            Err(error) => {
                tracing::error!(%error, "the sync thread could not be started");
                self.sync.borrow_mut().failure = Some(error.to_string());
            }
        }
    }

    fn stop_sync_engine(&self) {
        let engine = {
            let mut sync = self.sync.borrow_mut();
            sync.generation += 1;
            sync.engine.take()
        };
        drop(engine);
    }

    fn send_sync(&self, command: Command) -> bool {
        match &self.sync.borrow().engine {
            Some(engine) => {
                engine.send(command);
                true
            }
            None => false,
        }
    }

    /// Sends and fetches at once, or mirrors in the reading modes.
    pub(super) fn sync_now(&self) {
        if !self.send_sync(Command::Now) {
            self.show_toast(t!("Atoll Sync ist nicht eingeschaltet"));
        }
    }

    /// Replaces what is here with the state of the server, after asking.
    pub(super) fn mirror_from_server(&self) {
        if self.sync.borrow().engine.is_none() {
            self.show_toast(t!("Atoll Sync ist nicht eingeschaltet"));
            return;
        }
        let this = self.this();
        self.confirm(
            t!("Vom Server holen?"),
            t!(
                "Workspaces, Inseln, Tabs und Archiv dieses Geräts werden durch den Stand des Servers ersetzt. Was hier noch nicht gesendet wurde, geht verloren. Private Tabs bleiben."
            ),
            t!("Holen"),
            move || {
                if let Some(browser) = this.upgrade() {
                    browser.send_sync(Command::Mirror);
                }
            },
        );
    }

    /// Asks how this device and the server come together the first time.
    fn ask_sync_choice(&self) {
        let Some((local_tabs, server_tabs)) = self.sync.borrow().choice else {
            return;
        };
        self.close_settings();
        self.choose(
            t!("Atoll Sync verbinden"),
            &t!(
                "Tabs auf diesem Gerät: {}, auf dem Server: {}.\n\nZusammenführen behält beides, gleichnamige Workspaces werden eins. Server übernehmen ersetzt, was hier liegt. Eigene hochladen ersetzt, was auf dem Server liegt, auch für deine anderen Geräte.",
                local_tabs,
                server_tabs
            ),
            t!("Zusammenführen"),
            &[t!("Server übernehmen"), t!("Eigene hochladen")],
            |browser, answer| match answer {
                Some(0) => browser.link_sync(Choice::Merge),
                Some(1) => browser.confirm_link(
                    Choice::TakeServer,
                    t!("Was auf diesem Gerät liegt, wird durch den Stand des Servers ersetzt."),
                ),
                Some(2) => browser.confirm_link(
                    Choice::Upload,
                    t!(
                        "Was auf dem Server liegt, wird durch den Stand dieses Geräts ersetzt, auch auf deinen anderen Geräten."
                    ),
                ),
                _ => {}
            },
        );
    }

    fn confirm_link(&self, choice: Choice, message: &str) {
        let this = self.this();
        self.confirm(
            t!("Wirklich ersetzen?"),
            message,
            t!("Ersetzen"),
            move || {
                if let Some(browser) = this.upgrade() {
                    browser.link_sync(choice);
                }
            },
        );
    }

    fn link_sync(&self, choice: Choice) {
        if self.send_sync(Command::Link(choice)) {
            self.sync.borrow_mut().choice = None;
            self.show_sync_everywhere();
        }
    }

    /// Handles what the sync thread reported.
    fn sync_reported(&self, generation: u64, report: Report) {
        if generation != self.sync.borrow().generation {
            return;
        }
        match report {
            Report::Synced { applied, .. } => {
                {
                    let mut sync = self.sync.borrow_mut();
                    sync.last_synced = Some(Instant::now());
                    sync.failure = None;
                }
                if applied.changed > 0 {
                    self.follow_sync(&applied);
                }
                if applied.orphans > 0 {
                    tracing::info!(orphans = applied.orphans, "records without their parent");
                }
            }
            Report::NeedsChoice {
                local_tabs,
                server_tabs,
            } => {
                self.sync.borrow_mut().choice = Some((local_tabs, server_tabs));
                self.ask_sync_choice();
            }
            Report::Failed(reason) => self.sync.borrow_mut().failure = Some(reason),
            Report::SignedOut => self.signed_out_by_server(),
            Report::KeyChanged => self.key_changed(),
        }
        self.show_sync_everywhere();
    }

    /// The account was set up anew elsewhere: this device has to unlock the new key.
    fn key_changed(&self) {
        self.stop_sync_engine();
        self.token_store().clear_key();
        self.sync.borrow_mut().recovery_key = None;
        self.check_sync_key();
        self.show_toast(t!(
            "Atoll Sync wurde auf einem anderen Gerät neu eingerichtet. Entsperre es in den Einstellungen mit der neuen Passphrase."
        ));
    }

    /// Lets every window follow what another device changed.
    fn follow_sync(&self, applied: &Applied) {
        let Some(manager) = self.manager() else {
            return;
        };
        for browser in manager.browsers() {
            let workspace = browser.session.borrow().workspace;
            if applied.removed_workspaces.contains(&workspace) {
                browser.leave_removed_workspace();
            }
        }
        for browser in manager.browsers() {
            browser.follow_changes();
        }
        let gone: Vec<_> = self
            .views
            .borrow()
            .iter()
            .map(|(tab, _)| *tab)
            .filter(|tab| matches!(self.storage.tab(*tab), Err(StorageError::NotFound { .. })))
            .collect();
        if !gone.is_empty() {
            self.forget_tabs(gone);
        }
        for tab in &applied.moved_tabs {
            let Ok(record) = self.storage.tab(*tab) else {
                continue;
            };
            let views = self.views.borrow();
            let Some(entry) = views.get(*tab) else {
                continue;
            };
            if record.url.is_empty() || entry.view.url().as_deref() == Some(record.url.as_str()) {
                continue;
            }
            let busy = entry
                .view
                .since_input()
                .is_some_and(|since| since < QUIET_PAGE)
                || entry
                    .view
                    .since_load()
                    .is_some_and(|since| since < SETTLING_PAGE);
            if busy {
                continue;
            }
            entry.view.load_url(&record.url);
        }
    }

    /// Shows another workspace after the one shown here was removed on another device.
    fn leave_removed_workspace(&self) {
        let workspace = self.session.borrow().workspace;
        let alone = self
            .manager()
            .is_none_or(|manager| manager.browsers().len() <= 1);
        let next = self.free_workspace(workspace).or_else(|| {
            alone
                .then(|| self.storage.create_workspace(t!("Workspace")).ok())
                .flatten()
                .map(|created| created.id)
        });
        match (next, self.manager()) {
            (Some(next), _) => {
                if let Err(error) = self.storage.set_window_workspace(self.id, next) {
                    tracing::warn!(%error, "workspace of the window could not be stored");
                }
                {
                    let mut session = self.session.borrow_mut();
                    session.workspace = next;
                    session.active_tab = None;
                }
                self.reload_session_here();
                self.show_remembered_tab();
            }
            (None, Some(manager)) => manager.close_window(self.id),
            (None, None) => {}
        }
    }

    pub(super) fn sync_mode_edited(&self, index: i32) {
        let mode = match index {
            1 => Mode::Read,
            2 => Mode::ReadOnDemand,
            3 => Mode::Off,
            _ => Mode::ReadWrite,
        };
        self.update_preference(PreferenceKey::SyncMode, |preferences| {
            preferences.sync_mode = mode.as_str().to_owned();
        });
        let running = self.sync.borrow().engine.is_some();
        match (running, mode) {
            (true, Mode::Off) => self.stop_sync_engine(),
            (true, mode) => {
                self.sync.borrow_mut().choice = None;
                self.send_sync(Command::Mode(mode));
            }
            (false, _) => self.start_sync_engine(),
        }
        self.show_sync_everywhere();
    }

    pub(super) fn sync_device_name_edited(&self, name: &str) {
        let name = name.trim().to_owned();
        if name != self.preferences.borrow().sync_device_name {
            self.update_preference(PreferenceKey::SyncDeviceName, |preferences| {
                preferences.sync_device_name = name;
            });
        }
    }

    /// Tells the server the name of this device, when it changed since it was last told.
    pub(super) fn send_sync_device_name(&self) {
        let Some(auth) = self.signed_in_auth() else {
            return;
        };
        let name = self.preferences.borrow().sync_device_name();
        {
            let sync = self.sync.borrow();
            if !matches!(sync.key, KeyState::Ready)
                || sync.device_name_sent.as_deref() == Some(name.as_str())
            {
                return;
            }
        }
        let api = self.sync_api(&auth);
        let device_name = DeviceName {
            name: name.clone(),
            platform: Platform::Desktop,
        };
        let this = self.this();
        run_in_background(
            "sync-device-name",
            move || api.name_device(&device_name),
            move |result| {
                if let Some(browser) = this.upgrade() {
                    match result {
                        Ok(()) => {
                            browser.sync.borrow_mut().device_name_sent = Some(name);
                            browser.refresh_sync_devices();
                        }
                        Err(error) => tracing::info!(%error, "the device could not be renamed"),
                    }
                }
            },
        );
    }

    /// Asks the server again when what this device knows may be outdated: another device may
    /// have set sync up, or the server may be back.
    pub(super) fn recheck_sync_key(&self) {
        if matches!(
            self.sync.borrow().key,
            KeyState::Missing | KeyState::Unreachable(_)
        ) {
            self.check_sync_key();
        }
    }

    /// Asks the server for the devices of the account, while sync is ready.
    pub(super) fn refresh_sync_devices(&self) {
        let Some(auth) = self.signed_in_auth() else {
            return;
        };
        if !matches!(self.sync.borrow().key, KeyState::Ready) {
            return;
        }
        let api = self.sync_api(&auth);
        let this = self.this();
        run_in_background(
            "sync-devices",
            move || api.devices(),
            move |result| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                match result {
                    Ok(devices) => {
                        browser.sync.borrow_mut().devices = devices;
                        browser.show_sync_everywhere();
                    }
                    Err(error) => tracing::info!(%error, "the devices could not be listed"),
                }
            },
        );
    }

    pub fn forget_sync_device(&self, id: &str) {
        let Some(auth) = self.signed_in_auth() else {
            return;
        };
        let Ok(device) = id.parse::<SyncId>() else {
            return;
        };
        let name = self
            .sync
            .borrow()
            .devices
            .iter()
            .find(|candidate| candidate.id == device)
            .map(|candidate| candidate.name.clone())
            .unwrap_or_default();
        let this = self.this();
        self.confirm(
            t!("Gerät entfernen?"),
            &t!(
                "„{}“ verschwindet aus der Liste. Meldet es sich wieder, taucht es erneut auf; abmelden lässt es sich nur auf dem Gerät selbst.",
                name
            ),
            t!("Entfernen"),
            move || {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let api = browser.sync_api(&auth);
                let this = browser.this();
                run_in_background(
                    "sync-forget-device",
                    move || api.forget_device(device),
                    move |result| {
                        if let Some(browser) = this.upgrade() {
                            if let Err(error) = result {
                                browser.show_toast(&t!(
                                    "Gerät entfernen fehlgeschlagen: {}",
                                    error
                                ));
                            }
                            browser.refresh_sync_devices();
                        }
                    },
                );
            },
        );
    }

    fn sync_error(&self, message: &str) {
        self.sync.borrow_mut().error = message.to_owned();
        self.show_sync_everywhere();
    }

    pub(super) fn show_sync_everywhere(&self) {
        self.for_all_windows(Browser::show_sync);
    }

    /// Fills the sync part of the settings.
    pub(super) fn show_sync(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<SettingsState>();
        let preferences = self.preferences.borrow().clone();
        let mode = preferences.sync_mode();
        let sync = self.sync.borrow();
        let (phase, detail) = match &sync.key {
            KeyState::Unknown => (0, String::new()),
            KeyState::Checking => (1, String::new()),
            KeyState::Unreachable(reason) => (2, reason.clone()),
            KeyState::Missing => (3, String::new()),
            KeyState::Locked(_) => (4, String::new()),
            KeyState::Working => (5, String::new()),
            KeyState::Ready => (6, String::new()),
        };
        let status = if mode == Mode::Off {
            t!("Ausgeschaltet. Dieses Gerät gleicht nichts ab.").to_owned()
        } else if sync.choice.is_some() {
            t!("Wartet auf deine Entscheidung, wie dieses Gerät und der Server zusammenkommen.")
                .to_owned()
        } else if let Some(failure) = &sync.failure {
            t!("Der letzte Abgleich ist fehlgeschlagen: {}", failure)
        } else if let Some(at) = sync.last_synced {
            t!("Abgeglichen {}", relative_age(at.elapsed()))
        } else if mode == Mode::ReadOnDemand {
            t!("Holt den Stand des Servers nur, wenn du es sagst.").to_owned()
        } else {
            t!("Wird abgeglichen …").to_owned()
        };
        let devices: Vec<SyncDeviceItem> = sync
            .devices
            .iter()
            .map(|device| SyncDeviceItem {
                id: device.id.to_string().into(),
                name: device.name.as_str().into(),
                detail: device_detail(device).into(),
                current: device.current,
            })
            .collect();
        state.set_sync_key_phase(phase);
        state.set_sync_key_detail(detail.into());
        state.set_sync_key_error(sync.error.as_str().into());
        state.set_sync_status(status.into());
        state.set_sync_mode(match mode {
            Mode::ReadWrite => 0,
            Mode::Read => 1,
            Mode::ReadOnDemand => 2,
            Mode::Off => 3,
        });
        state.set_sync_running(sync.engine.is_some());
        state.set_sync_choice_pending(sync.choice.is_some());
        state.set_sync_recovery_key(sync.recovery_key.clone().unwrap_or_default().into());
        state.set_sync_devices(ModelRc::from(Rc::new(VecModel::from(devices))));
    }
}

/// The key of `stored`, opened with a recovery key or with the passphrase.
fn unlock(stored: &StoredKey, secret: &str) -> Result<DataKey, SyncError> {
    if let Ok(key) = DataKey::from_recovery_key(secret) {
        let expected = &stored.envelope.key_id;
        if expected.is_empty() || *expected == key.id() {
            return Ok(key);
        }
        return Err(SyncError::WrongKey);
    }
    let envelope = KeyEnvelope::try_from(&stored.envelope)?;
    envelope.unwrap(secret)
}

fn device_detail(device: &Device) -> String {
    if device.current {
        return t!("Dieses Gerät").to_owned();
    }
    let platform = match device.platform {
        Some(Platform::Android) => "Android",
        _ => "Desktop",
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default();
    let age = Duration::from_millis(now.saturating_sub(device.last_seen_at).max(0) as u64);
    t!("{}, zuletzt {}", platform, relative_age(age))
}
