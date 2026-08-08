mod adblock;
mod archive;
mod bookmarks;
mod chrome;
mod clipboard;
mod detach;
mod dev_tools;
mod downloads;
mod events;
mod favicons;
mod layout;
mod lifecycle;
mod memory;
mod menu;
mod navigation;
mod organize;
mod overlays;
mod page;
mod palette;
mod passwords;
mod preferences;
mod session;
mod settings;
mod sidebar;
mod site_info;
mod startup;
mod suggestions;
mod tabs;
mod transfer;
mod upload;
mod views;
mod web_input;
mod windows;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use anyhow::Result;
use atoll_core::{TabLifecycle, Timestamp, WindowId, WorkspaceId};
use atoll_engine::{Engine, GpuContext, RequestId, ViewGeometry, ViewId};
use atoll_storage::Storage;
use atoll_storage::prelude::*;
use atoll_video::YtDlp;
use slint::ComponentHandle;

use crate::input::pointer::ClickTracker;
use crate::platform::clipboard::Clipboard;
use crate::platform::paths::Paths;
use crate::ui::AppWindow;

use adblock::Adblock;
use bookmarks::BookmarkState;
use clipboard::ClipboardSync;
use dev_tools::DevToolsPresentation;
use downloads::{Downloads, VideoSearch};
use favicons::FaviconCache;
use memory::MemoryWatcher;
use overlays::DialogQueue;
use page::PageState;
use palette::PaletteState;
use passwords::{PasswordPopup, PasswordVault, PendingFill};
use preferences::Preferences;
use session::Session;
use sidebar::SidebarModels;
use suggestions::SuggestionState;
use tabs::ClosedTab;
use transfer::TransferState;
use upload::UploadState;
use views::ViewRegistry;
use web_input::InputTarget;

pub use web_input::KeyboardFocus;
pub use windows::WindowManager;

/// Everything the windows share: the database, the engine and the engine instances of all tabs.
#[derive(Clone)]
struct Shared {
    storage: Rc<Storage>,
    gpu: Rc<GpuContext>,
    paths: Paths,
    preferences: Rc<RefCell<Preferences>>,
    engine: Rc<RefCell<Option<Engine>>>,
    lifecycle: Rc<RefCell<TabLifecycle>>,
    views: Rc<RefCell<ViewRegistry>>,
    favicons: Rc<RefCell<FaviconCache>>,
    closed_tabs: Rc<RefCell<Vec<ClosedTab>>>,
    clipboard: Rc<Clipboard>,
    clipboard_sync: Rc<RefCell<ClipboardSync>>,
    downloads: Rc<RefCell<Downloads>>,
    adblock: Rc<Adblock>,
    memory: Rc<MemoryWatcher>,
    vault: Rc<PasswordVault>,
    yt_dlp: YtDlp,
}

impl Shared {
    fn new(storage: Storage, gpu: Rc<GpuContext>, paths: Paths) -> Self {
        let preferences = Preferences::load(&storage);
        let lifecycle = TabLifecycle::new(preferences.frozen_limit);
        let adblock = Adblock::new(
            preferences.adblock_enabled,
            storage.adblock_allowed_sites().unwrap_or_default(),
            paths.adblock.clone(),
        );
        Self {
            storage: Rc::new(storage),
            gpu,
            paths,
            preferences: Rc::new(RefCell::new(preferences)),
            engine: Rc::default(),
            lifecycle: Rc::new(RefCell::new(lifecycle)),
            views: Rc::default(),
            favicons: Rc::default(),
            closed_tabs: Rc::default(),
            clipboard: Rc::default(),
            clipboard_sync: Rc::default(),
            downloads: Rc::default(),
            adblock: Rc::new(adblock),
            memory: Rc::default(),
            vault: Rc::new(PasswordVault::from_environment()),
            yt_dlp: std::env::var_os("ATOLL_YTDLP")
                .map(YtDlp::new)
                .unwrap_or_default(),
        }
    }
}

/// The controller of one window: connects its user interface to the database and the engine.
/// Everything that is not about this window lives in [`Shared`].
pub struct Browser {
    id: WindowId,
    window: slint::Weak<AppWindow>,
    manager: Weak<WindowManager>,
    storage: Rc<Storage>,
    paths: Paths,
    preferences: Rc<RefCell<Preferences>>,
    engine: Rc<RefCell<Option<Engine>>>,
    session: RefCell<Session>,
    lifecycle: Rc<RefCell<TabLifecycle>>,
    views: Rc<RefCell<ViewRegistry>>,
    sidebar: SidebarModels,
    geometry: Cell<ViewGeometry>,
    dev_tools_geometry: Cell<ViewGeometry>,
    dev_tools_presentation: RefCell<DevToolsPresentation>,
    input_target: Cell<InputTarget>,
    favicons: Rc<RefCell<FaviconCache>>,
    page: RefCell<PageState>,
    dialogs: RefCell<DialogQueue>,
    context_menu: Cell<Option<(ViewId, RequestId)>>,
    transfer: RefCell<TransferState>,
    closed_tabs: Rc<RefCell<Vec<ClosedTab>>>,
    clicks: RefCell<ClickTracker>,
    clipboard: Rc<Clipboard>,
    clipboard_sync: Rc<RefCell<ClipboardSync>>,
    find_text: RefCell<String>,
    address_focus_requested: Cell<bool>,
    last_title_press: Cell<Option<std::time::Instant>>,
    downloads: Rc<RefCell<Downloads>>,
    adblock: Rc<Adblock>,
    memory: Rc<MemoryWatcher>,
    vault: Rc<PasswordVault>,
    passwords: RefCell<PasswordPopup>,
    pending_fill: RefCell<Option<PendingFill>>,
    yt_dlp: YtDlp,
    video_search: RefCell<Option<VideoSearch>>,
    suggestions: RefCell<SuggestionState>,
    palette: RefCell<PaletteState>,
    bookmarks: RefCell<BookmarkState>,
    upload: RefCell<UploadState>,
    this: Weak<Browser>,
}

impl Browser {
    fn new(
        window: &AppWindow,
        id: WindowId,
        workspace: WorkspaceId,
        shared: &Shared,
        manager: Weak<WindowManager>,
    ) -> Result<Rc<Self>> {
        let session = Session::load(&shared.storage, workspace)?;
        let geometry = ViewGeometry {
            width: 1024,
            height: 768,
            scale: window.window().scale_factor(),
        };
        Ok(Rc::new_cyclic(|this| Self {
            id,
            window: window.as_weak(),
            manager,
            storage: shared.storage.clone(),
            paths: shared.paths.clone(),
            preferences: shared.preferences.clone(),
            engine: shared.engine.clone(),
            session: RefCell::new(session),
            lifecycle: shared.lifecycle.clone(),
            views: shared.views.clone(),
            sidebar: SidebarModels::attach(window),
            geometry: Cell::new(geometry),
            dev_tools_geometry: Cell::new(geometry),
            dev_tools_presentation: RefCell::default(),
            input_target: Cell::default(),
            favicons: shared.favicons.clone(),
            page: RefCell::default(),
            dialogs: RefCell::default(),
            context_menu: Cell::new(None),
            transfer: RefCell::default(),
            closed_tabs: shared.closed_tabs.clone(),
            clicks: RefCell::default(),
            clipboard: shared.clipboard.clone(),
            clipboard_sync: shared.clipboard_sync.clone(),
            find_text: RefCell::default(),
            address_focus_requested: Cell::new(false),
            last_title_press: Cell::new(None),
            downloads: shared.downloads.clone(),
            adblock: shared.adblock.clone(),
            memory: shared.memory.clone(),
            vault: shared.vault.clone(),
            passwords: RefCell::default(),
            pending_fill: RefCell::default(),
            yt_dlp: shared.yt_dlp.clone(),
            video_search: RefCell::default(),
            suggestions: RefCell::default(),
            palette: RefCell::default(),
            bookmarks: RefCell::default(),
            upload: RefCell::default(),
            this: this.clone(),
        }))
    }

    fn manager(&self) -> Option<Rc<WindowManager>> {
        self.manager.upgrade()
    }

    /// Runs `action` for every other open window.
    fn for_other_windows(&self, mut action: impl FnMut(&Browser)) {
        if let Some(manager) = self.manager() {
            for browser in manager.browsers() {
                if browser.id != self.id {
                    action(&browser);
                }
            }
        }
    }

    /// Runs `action` for every open window, this one included.
    fn for_all_windows(&self, mut action: impl FnMut(&Browser)) {
        action(self);
        self.for_other_windows(action);
    }

    fn window(&self) -> Option<AppWindow> {
        self.window.upgrade()
    }

    fn this(&self) -> Weak<Browser> {
        self.this.clone()
    }

    fn is_engine_running(&self) -> bool {
        self.engine.borrow().is_some()
    }

    fn now() -> Timestamp {
        Timestamp::now()
    }

    fn report(&self, action: &str, result: Result<(), impl std::fmt::Display>) {
        if let Err(error) = result {
            tracing::error!(%error, "{action} failed");
            self.show_toast(&format!("{action} fehlgeschlagen: {error}"));
        }
    }

    fn active_workspace_key(workspace: WorkspaceId) -> String {
        format!("workspace.{}.active_tab", workspace.0)
    }
}
