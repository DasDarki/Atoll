mod adblock;
mod archive;
mod bookmarks;
mod chrome;
mod clipboard;
mod detach;
mod dev_tools;
mod downloads;
mod duplicates;
mod events;
mod favicons;
mod filter;
mod history;
mod layout;
mod lifecycle;
mod memory;
mod menu;
mod navigation;
mod opera;
mod organize;
mod overlays;
mod page;
mod pages;
mod palette;
mod passwords;
mod plugins;
mod preferences;
mod private;
mod reorder;
mod session;
mod settings;
mod shield;
mod sidebar;
mod site_info;
mod startup;
mod suggestions;
mod tabs;
mod transfer;
mod upload;
mod views;
mod vpn;
mod wayback;
mod web_input;
mod windows;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use anyhow::Result;
use atoll_core::{TabLifecycle, Timestamp, WindowId, WorkspaceId};
use atoll_engine::{Engine, GpuContext, RequestId, ViewGeometry, ViewId};
use atoll_storage::Storage;
use atoll_storage::prelude::*;
use atoll_tor::Tor;
use atoll_video::YtDlp;
use atoll_vpn::Vpn;
use slint::ComponentHandle;

use crate::input::pointer::ClickTracker;
use crate::platform::clipboard::Clipboard;
use crate::platform::paths::Paths;
use crate::ui::AppWindow;

use adblock::Adblock;
use bookmarks::BookmarkState;
use clipboard::ClipboardSync;
use dev_tools::DevToolsPresentation;
use downloads::{Downloads, TorrentSession, VideoSearch};
use duplicates::OpenElsewhere;
use favicons::FaviconCache;
use history::HistoryView;
use memory::MemoryWatcher;
use opera::OperaImport;
use overlays::DialogQueue;
use page::PageState;
use pages::PageSearch;
use palette::PaletteState;
use passwords::{PasswordPopup, PasswordVault, PendingFill};
use plugins::{OpenPanel, Plugins};
pub use preferences::Preferences;
use private::PrivatePages;
use reorder::{IslandDrag, TabDrag};
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
    private: Rc<RefCell<PrivatePages>>,
    tor: Tor,
    vpn: Vpn,
    closed_tabs: Rc<RefCell<Vec<ClosedTab>>>,
    clipboard: Rc<Clipboard>,
    clipboard_sync: Rc<RefCell<ClipboardSync>>,
    downloads: Rc<RefCell<Downloads>>,
    adblock: Rc<Adblock>,
    memory: Rc<MemoryWatcher>,
    vault: Rc<PasswordVault>,
    yt_dlp: YtDlp,
    plugins: Rc<Plugins>,
}

impl Shared {
    fn new(storage: Storage, gpu: Rc<GpuContext>, paths: Paths) -> Self {
        let preferences = Preferences::load(&storage);
        let lifecycle = TabLifecycle::new(preferences.frozen_limit);
        let adblock = Adblock::new(
            storage.adblock_rules().unwrap_or_default(),
            paths.adblock.clone(),
            preferences.cookie_banners_hide,
            preferences.cookie_banners_answer,
        );
        let storage = Rc::new(storage);
        let plugins = Plugins::new(storage.clone(), paths.plugins.clone());
        Self {
            storage,
            gpu,
            paths,
            preferences: Rc::new(RefCell::new(preferences)),
            engine: Rc::default(),
            lifecycle: Rc::new(RefCell::new(lifecycle)),
            views: Rc::default(),
            favicons: Rc::default(),
            private: Rc::default(),
            tor: Tor::default(),
            vpn: Vpn::default(),
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
            plugins,
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
    private: Rc<RefCell<PrivatePages>>,
    tor: Tor,
    /// What Tor last said, to notice when it changes.
    tor_status: RefCell<Option<atoll_tor::Status>>,
    vpn: Vpn,
    /// The same for the VPN of the user.
    vpn_status: RefCell<Option<atoll_vpn::Status>>,
    torrents: RefCell<TorrentSession>,
    pages: RefCell<PageSearch>,
    /// The other tabs that show the page of the active tab.
    elsewhere: RefCell<OpenElsewhere>,
    history: RefCell<HistoryView>,
    opera: RefCell<OperaImport>,
    island_drag: RefCell<IslandDrag>,
    tab_drag: RefCell<TabDrag>,
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
    plugins: Rc<Plugins>,
    /// The panel of an extension this window shows.
    panel: RefCell<Option<OpenPanel>>,
    panel_geometry: Cell<ViewGeometry>,
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
            private: shared.private.clone(),
            tor: shared.tor.clone(),
            tor_status: RefCell::default(),
            vpn: shared.vpn.clone(),
            vpn_status: RefCell::default(),
            torrents: RefCell::default(),
            pages: RefCell::default(),
            history: RefCell::default(),
            opera: RefCell::default(),
            island_drag: RefCell::default(),
            elsewhere: RefCell::default(),
            tab_drag: RefCell::default(),
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
            plugins: shared.plugins.clone(),
            panel: RefCell::default(),
            panel_geometry: Cell::new(ViewGeometry {
                width: 360,
                height: 768,
                scale: geometry.scale,
            }),
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
