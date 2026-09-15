use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use atoll_core::{TabId, WindowId, WindowPlacement, WorkspaceId};
use atoll_engine::{Engine, EngineConfig, EngineEvent, GpuContext, ViewId};
use atoll_storage::Storage;
use atoll_storage::prelude::*;
use slint::winit_030::WinitWindowAccessor;
use slint::{CloseRequestResponse, ComponentHandle, LogicalPosition, LogicalSize};

use super::filter::FilterChain;
use super::memory::exceeds_budget;
use super::{Browser, Shared};
use crate::bridge::{bindings, window_events};
use crate::platform::paths::Paths;
use crate::pump::MessagePump;
use crate::ui::AppWindow;
use atoll_i18n::t;

/// The setting the single window of older versions remembered its workspace in.
const LEGACY_WORKSPACE_KEY: &str = "active_workspace";

/// How long after a tab was dropped outside its window another window may report the pointer
/// entering it. Only then the tab moves there, the drop location itself is unknown on Wayland.
const DROP_SETTLE: Duration = Duration::from_millis(180);

/// Pointer activity reported shortly before the drop still counts, events may arrive in any order.
const DROP_TOLERANCE: Duration = Duration::from_millis(60);

/// The share of a maximized window a window opened from it gets.
const DETACHED_SHARE: f32 = 0.7;

struct OpenWindow {
    id: WindowId,
    window: AppWindow,
    browser: Rc<Browser>,
}

/// Owns every window and what they share. Each window shows one workspace, a workspace is shown
/// in at most one window, and the open windows are remembered in the database.
pub struct WindowManager {
    shared: Shared,
    windows: RefCell<Vec<OpenWindow>>,
    focused: Cell<Option<WindowId>>,
    pointer_seen: Cell<Option<(WindowId, Instant)>>,
    pump: RefCell<Option<Rc<MessagePump>>>,
    this: Weak<WindowManager>,
}

impl WindowManager {
    pub fn new(storage: Storage, gpu: Rc<GpuContext>, paths: Paths) -> Rc<Self> {
        Rc::new_cyclic(|this| Self {
            shared: Shared::new(storage, gpu, paths),
            windows: RefCell::default(),
            focused: Cell::new(None),
            pointer_seen: Cell::new(None),
            pump: RefCell::default(),
            this: this.clone(),
        })
    }

    /// Opens the windows of the last session, or one window if there were none. Returns the
    /// first window.
    pub fn restore_windows(&self) -> Result<AppWindow> {
        let storage = &self.shared.storage;
        if let Err(error) = storage.interrupt_running_downloads(atoll_core::Timestamp::now()) {
            tracing::warn!(%error, "download list could not be updated");
        }
        let mut placements = storage.windows()?;
        if placements.is_empty() {
            placements.push(storage.create_window(initial_workspace(storage)?)?);
        }
        for placement in &placements {
            self.open_window(placement)?;
        }
        let windows = self.windows.borrow();
        let first = windows
            .first()
            .map(|open| open.window.clone_strong())
            .ok_or_else(|| anyhow::anyhow!("no window could be opened"))?;
        Ok(first)
    }

    fn open_window(&self, placement: &WindowPlacement) -> Result<Rc<Browser>> {
        let window = AppWindow::new()?;
        if let Some((width, height)) = placement.size {
            window
                .window()
                .set_size(LogicalSize::new(width as f32, height as f32));
        }
        if let Some((x, y)) = placement.position {
            window
                .window()
                .set_position(LogicalPosition::new(x as f32, y as f32));
        }
        let browser = Browser::new(
            &window,
            placement.id,
            placement.workspace_id,
            &self.shared,
            self.this.clone(),
        )?;
        bindings::connect(&window, &browser);
        window_events::install(&window, &browser);
        let manager = self.this.clone();
        let id = placement.id;
        window.window().on_close_requested(move || {
            if let Some(manager) = manager.upgrade() {
                manager.close_window(id);
            }
            CloseRequestResponse::KeepWindowShown
        });
        self.windows.borrow_mut().push(OpenWindow {
            id,
            window: window.clone_strong(),
            browser: browser.clone(),
        });
        self.focused.set(Some(id));
        if self.is_engine_running() {
            browser.protect_pinned_tabs();
        }
        browser.show_session();
        window.show()?;
        if placement.maximized {
            window.window().set_maximized(true);
        }
        for other in self.browsers() {
            if other.id != id {
                other.follow_changes();
            }
        }
        Ok(browser)
    }

    /// Ctrl+N: opens a new window with a new, empty workspace.
    pub fn open_new_window(&self) {
        let storage = &self.shared.storage;
        match storage.create_workspace(&next_workspace_name(storage)) {
            Ok(workspace) => {
                let like = self.focused_browser();
                self.open_window_for(workspace.id, like.as_deref(), None);
            }
            Err(error) => self.report_error(t!("Neues Fenster"), &error.into()),
        }
    }

    /// Opens `workspace` in a new window, or brings its window to the front if it has one.
    pub fn open_workspace_window(&self, workspace: WorkspaceId) -> Option<Rc<Browser>> {
        if let Some(browser) = self.window_for_workspace(workspace) {
            self.focus_window(browser.id);
            return Some(browser);
        }
        let like = self.focused_browser();
        self.open_window_for(workspace, like.as_deref(), None)
    }

    /// Opens a new window for `workspace`, as large as `like` and at `position` if given.
    pub(super) fn open_window_for(
        &self,
        workspace: WorkspaceId,
        like: Option<&Browser>,
        position: Option<(i32, i32)>,
    ) -> Option<Rc<Browser>> {
        let opened = self
            .shared
            .storage
            .create_window(workspace)
            .map_err(anyhow::Error::from)
            .and_then(|mut placement| {
                placement.size = like.and_then(detached_size);
                placement.position = position;
                self.shared.storage.update_window_placement(&placement)?;
                self.open_window(&placement)
            });
        match opened {
            Ok(browser) => {
                self.focus_window(browser.id);
                Some(browser)
            }
            Err(error) => {
                self.report_error(t!("Fenster öffnen"), &error);
                None
            }
        }
    }

    /// A tab of `source` was dropped outside of it. Once the pointer had time to enter the window
    /// it was dropped on, the tab moves there, or into a new window at `position`.
    pub(super) fn tab_dropped_outside(&self, source: WindowId, tab: TabId, position: (i32, i32)) {
        let dropped = Instant::now();
        let manager = self.this.clone();
        slint::Timer::single_shot(DROP_SETTLE, move || {
            let Some(manager) = manager.upgrade() else {
                return;
            };
            let Some(browser) = manager.browser(source) else {
                return;
            };
            let target = manager
                .pointer_seen
                .get()
                .filter(|(window, seen)| {
                    *window != source && dropped.saturating_duration_since(*seen) <= DROP_TOLERANCE
                })
                .and_then(|(window, _)| manager.browser(window));
            match target {
                Some(target) => browser.move_tab_to_window(tab, &target),
                None => browser.move_tab_to_new_window(tab, Some(position)),
            }
        });
    }

    /// The pointer entered or moved over window `id`.
    pub(super) fn pointer_seen(&self, id: WindowId) {
        self.pointer_seen.set(Some((id, Instant::now())));
    }

    /// Closes a window after the current event. Closing the last window quits Atoll and keeps it
    /// for the next start.
    pub fn close_window(&self, id: WindowId) {
        let manager = self.this.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(manager) = manager.upgrade() {
                manager.close_window_now(id);
            }
        });
    }

    fn close_window_now(&self, id: WindowId) {
        let open = {
            let windows = self.windows.borrow();
            windows
                .iter()
                .find(|open| open.id == id)
                .map(|open| (open.window.clone_strong(), open.browser.clone()))
        };
        let Some((window, browser)) = open else {
            return;
        };
        tracing::debug!(window = id.0, "closing window");
        self.remember_placement(id, &window);
        if self.windows.borrow().len() <= 1 {
            if let Err(error) = slint::quit_event_loop() {
                tracing::warn!(%error, "event loop could not be stopped");
            }
            return;
        }
        browser.release_tabs();
        let storage = &self.shared.storage;
        if let Err(error) = storage.delete_window(id) {
            tracing::warn!(%error, "window could not be forgotten");
        }
        browser.remove_workspace_if_empty();
        self.windows.borrow_mut().retain(|open| open.id != id);
        if let Err(error) = window.hide() {
            tracing::warn!(%error, "window could not be hidden");
        }
        if self.focused.get() == Some(id) {
            self.focused
                .set(self.windows.borrow().first().map(|open| open.id));
        }
        for other in self.browsers() {
            other.follow_changes();
        }
    }

    fn remember_placement(&self, id: WindowId, window: &AppWindow) {
        let storage = &self.shared.storage;
        let previous = storage
            .windows()
            .ok()
            .and_then(|windows| windows.into_iter().find(|placement| placement.id == id));
        let Some(previous) = previous else { return };
        let handle = window.window();
        let scale = handle.scale_factor();
        let maximized = handle.is_maximized();
        let size = handle.size().to_logical(scale);
        let position = handle.position().to_logical(scale);
        let placement = WindowPlacement {
            size: if maximized {
                previous.size
            } else {
                Some((size.width.round() as u32, size.height.round() as u32))
            },
            position: if maximized {
                previous.position
            } else {
                Some((position.x.round() as i32, position.y.round() as i32))
            },
            maximized,
            ..previous
        };
        if let Err(error) = storage.update_window_placement(&placement) {
            tracing::warn!(%error, "window placement could not be stored");
        }
    }

    fn browser(&self, id: WindowId) -> Option<Rc<Browser>> {
        self.windows
            .borrow()
            .iter()
            .find(|open| open.id == id)
            .map(|open| open.browser.clone())
    }

    pub fn browsers(&self) -> Vec<Rc<Browser>> {
        self.windows
            .borrow()
            .iter()
            .map(|open| open.browser.clone())
            .collect()
    }

    pub(super) fn window_for_workspace(&self, workspace: WorkspaceId) -> Option<Rc<Browser>> {
        self.browsers()
            .into_iter()
            .find(|browser| browser.session.borrow().workspace == workspace)
    }

    pub(super) fn window_for_tab(&self, tab: TabId) -> Option<Rc<Browser>> {
        self.browsers()
            .into_iter()
            .find(|browser| browser.session.borrow().contains_tab(tab))
    }

    /// Whether `workspace` is shown in a window other than `except`.
    pub(super) fn is_open_elsewhere(&self, workspace: WorkspaceId, except: WindowId) -> bool {
        self.window_for_workspace(workspace)
            .is_some_and(|browser| browser.id != except)
    }

    pub(super) fn focus_window(&self, id: WindowId) {
        let window = self
            .windows
            .borrow()
            .iter()
            .find(|open| open.id == id)
            .map(|open| open.window.clone_strong());
        if let Some(window) = window {
            window
                .window()
                .with_winit_window(|winit| winit.focus_window());
            self.focused.set(Some(id));
        }
    }

    pub(super) fn window_focused(&self, id: WindowId) {
        self.focused.set(Some(id));
    }

    pub(super) fn focused_browser(&self) -> Option<Rc<Browser>> {
        let browsers = self.browsers();
        let focused = self.focused.get();
        browsers
            .iter()
            .find(|browser| Some(browser.id) == focused)
            .or_else(|| browsers.first())
            .cloned()
    }

    fn report_error(&self, action: &str, error: &anyhow::Error) {
        tracing::error!(%error, "{action} failed");
        if let Some(browser) = self.focused_browser() {
            browser.show_toast(&format!("{action} fehlgeschlagen: {error}"));
        }
    }

    fn is_engine_running(&self) -> bool {
        self.shared.engine.borrow().is_some()
    }

    /// Starts the engine once the first frame is on screen and wakes the active tab of every
    /// window.
    pub fn start_engine(&self, frame_rate: u32) {
        if self.is_engine_running() {
            return;
        }
        self.shared.plugins.attach(self.this.clone());
        if let Some(browser) = self.focused_browser() {
            browser.load_plugins();
        }
        let started = Instant::now();
        let preferences = self.shared.preferences.borrow().clone();
        let manager = self.this.clone();
        let content_filter = self.shared.adblock.start(move || {
            if let Some(manager) = manager.upgrade() {
                for browser in manager.browsers() {
                    browser.adblock_status_changed();
                }
            }
        });
        let paths = &self.shared.paths;
        let config = EngineConfig {
            root_cache_path: paths.engine_root.clone(),
            cache_path: paths.engine_profile.clone(),
            download_directory: preferences
                .download_directory
                .clone()
                .unwrap_or_else(|| paths.downloads.clone()),
            ask_download_location: preferences.download_ask,
            locale: None,
            frame_rate,
            accelerated_paint: std::env::var_os("ATOLL_SOFTWARE_RENDERING").is_none(),
            sandbox: std::env::var_os("ATOLL_NO_SANDBOX").is_none(),
            extra_switches: chromium_switches(),
            schedule_work: MessagePump::scheduler(),
            content_filter: Some(Arc::new(FilterChain::new(
                content_filter,
                self.shared.plugins.rules(),
            ))),
        };
        match Engine::initialize(config, self.shared.gpu.clone()) {
            Ok(engine) => {
                *self.shared.engine.borrow_mut() = Some(engine);
                tracing::info!(elapsed = ?started.elapsed(), "engine started");
            }
            Err(error) => {
                tracing::error!(%error, "engine could not be started");
                if let Some(browser) = self.focused_browser() {
                    browser.show_toast(t!("Die Browser-Engine konnte nicht gestartet werden."));
                }
                return;
            }
        }
        if let Some(browser) = self.browsers().first() {
            browser.start_clipboard_bridge();
        }
        let this = self.this.clone();
        *self.pump.borrow_mut() = Some(MessagePump::start(move || {
            if let Some(manager) = this.upgrade() {
                manager.pump();
            }
        }));
        for browser in self.browsers() {
            browser.engine_started();
        }
        let manager = self.this.clone();
        self.shared.memory.start(move |usage| {
            if let Some(manager) = manager.upgrade() {
                manager.memory_measured(usage);
            }
        });
    }

    /// Puts the least recently used background tab to sleep while the pages use more memory than
    /// the budget, one per measurement.
    fn memory_measured(&self, usage: u64) {
        tracing::debug!(usage, "renderer memory measured");
        self.shared.memory.set_usage(usage);
        let (enabled, budget) = {
            let preferences = self.shared.preferences.borrow();
            (
                preferences.memory_limit_enabled,
                preferences.memory_limit_gb,
            )
        };
        if enabled && exceeds_budget(usage, budget) {
            let demoted = self.shared.lifecycle.borrow_mut().demote_least_recent();
            if let Some(command) = demoted
                && let Some(browser) = self.browsers().first()
            {
                tracing::info!(usage, budget, ?command, "memory budget exceeded");
                browser.apply(vec![command]);
                for browser in self.browsers() {
                    browser.refresh_sidebar();
                }
                self.shared.memory.measure_soon();
            }
        }
        for browser in self.browsers() {
            browser.memory_measured();
        }
    }

    /// Runs one engine message loop iteration and hands every event to the window it concerns.
    fn pump(&self) {
        let events = {
            let engine = self.shared.engine.borrow();
            let Some(engine) = engine.as_ref() else {
                return;
            };
            engine.do_message_loop_work();
            engine.drain_events()
        };
        self.shared.plugins.report_rule_hits();
        self.shared.plugins.poll_workers();
        for browser in self.browsers() {
            browser.watch_tor();
        }
        if events.is_empty() {
            return;
        }
        let mut batches: Vec<(Rc<Browser>, Vec<EngineEvent>)> = Vec::new();
        for event in events {
            let Some(target) = self.event_target(&event) else {
                continue;
            };
            match batches
                .iter_mut()
                .find(|(browser, _)| Rc::ptr_eq(browser, &target))
            {
                Some((_, events)) => events.push(event),
                None => batches.push((target, vec![event])),
            }
        }
        for (browser, events) in batches {
            browser.process_events(events);
        }
    }

    fn event_target(&self, event: &EngineEvent) -> Option<Rc<Browser>> {
        event
            .view()
            .and_then(|view| self.window_for_view(view))
            .or_else(|| self.focused_browser())
    }

    fn window_for_view(&self, view: ViewId) -> Option<Rc<Browser>> {
        let tab = {
            let views = self.shared.views.borrow();
            views.tab_of(view).or_else(|| views.dev_tools_owner(view))
        }?;
        self.window_for_tab(tab)
    }

    /// Remembers the open windows and shuts the engine down.
    pub fn shutdown(&self) {
        let windows: Vec<(WindowId, AppWindow)> = self
            .windows
            .borrow()
            .iter()
            .map(|open| (open.id, open.window.clone_strong()))
            .collect();
        for (id, window) in &windows {
            self.remember_placement(*id, window);
        }
        self.pump.borrow_mut().take();
        let browsers = self.browsers();
        for browser in &browsers {
            browser.cancel_context_menu();
        }
        if let Some(browser) = browsers.first() {
            browser.stop_clipboard_bridge();
            browser.store_scroll_positions();
        }
        for (_, entry) in self.shared.views.borrow_mut().drain() {
            entry.view.close();
        }
        if let Some(engine) = self.shared.engine.borrow_mut().take() {
            engine.shutdown();
        }
        self.windows.borrow_mut().clear();
    }
}

/// The logical size of a window opened from `browser`'s window.
fn detached_size(browser: &Browser) -> Option<(u32, u32)> {
    let window = browser.window()?;
    let handle = window.window();
    let size = handle.size().to_logical(handle.scale_factor());
    let share = if handle.is_maximized() {
        DETACHED_SHARE
    } else {
        1.0
    };
    Some((
        (size.width * share).round() as u32,
        (size.height * share).round() as u32,
    ))
}

/// The workspace of the first window: the one older versions remembered, or the first one, or a
/// new one on the very first start.
fn initial_workspace(storage: &Storage) -> Result<WorkspaceId> {
    let workspaces = storage.workspaces()?;
    if workspaces.is_empty() {
        return Ok(storage.create_workspace(t!("Allgemein"))?.id);
    }
    let remembered = storage
        .setting_i64(LEGACY_WORKSPACE_KEY)?
        .map(WorkspaceId)
        .filter(|id| workspaces.iter().any(|workspace| workspace.id == *id));
    Ok(remembered.unwrap_or(workspaces[0].id))
}

/// A name like "Workspace 3" that no workspace uses yet.
fn next_workspace_name(storage: &Storage) -> String {
    let names: Vec<String> = storage
        .workspaces()
        .unwrap_or_default()
        .into_iter()
        .map(|workspace| workspace.name)
        .collect();
    (names.len() + 1..)
        .map(|number| t!("Workspace {}", number))
        .find(|name| !names.contains(name))
        .unwrap_or_else(|| t!("Workspace").to_owned())
}

/// Additional Chromium switches: Atoll's defaults, then those from `ATOLL_CHROMIUM_FLAGS`.
///
/// Chromium keeps the key for cookies and saved passwords in the system keyring and waits for
/// it without a time limit before the first request. An unresponsive keyring service therefore
/// stops every page from loading, so Chromium's own store is used unless `ATOLL_SYSTEM_KEYRING`
/// is set.
fn chromium_switches() -> Vec<String> {
    let mut switches = Vec::new();
    if std::env::var_os("ATOLL_SYSTEM_KEYRING").is_none() {
        switches.push("password-store=basic".to_owned());
    }
    if let Ok(flags) = std::env::var("ATOLL_CHROMIUM_FLAGS") {
        switches.extend(flags.split_whitespace().map(str::to_owned));
    }
    switches
}
