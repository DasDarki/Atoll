mod archive;
mod clipboard;
mod events;
mod favicons;
mod lifecycle;
mod menu;
mod navigation;
mod organize;
mod overlays;
mod page;
mod preferences;
mod session;
mod sidebar;
mod startup;
mod tabs;
mod transfer;
mod views;
mod web_input;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};

use anyhow::Result;
use atoll_core::{TabLifecycle, Timestamp, WorkspaceId};
use atoll_engine::{Engine, GpuContext, RequestId, ViewGeometry, ViewId};
use atoll_storage::Storage;
use atoll_storage::prelude::*;
use slint::ComponentHandle;

use crate::input::pointer::ClickTracker;
use crate::platform::clipboard::Clipboard;
use crate::platform::paths::Paths;
use crate::pump::MessagePump;
use crate::ui::AppWindow;

use clipboard::ClipboardSync;
use favicons::FaviconCache;
use overlays::DialogQueue;
use page::PageState;
use preferences::Preferences;
use session::Session;
use sidebar::SidebarModels;
use tabs::ClosedTab;
use transfer::TransferState;
use views::ViewRegistry;

pub use web_input::KeyboardFocus;

const ACTIVE_WORKSPACE_KEY: &str = "active_workspace";

/// The controller that connects the user interface, the database and the engine.
pub struct Browser {
    window: slint::Weak<AppWindow>,
    storage: Storage,
    gpu: Rc<GpuContext>,
    paths: Paths,
    preferences: Preferences,
    engine: RefCell<Option<Engine>>,
    pump: RefCell<Option<Rc<MessagePump>>>,
    session: RefCell<Session>,
    lifecycle: RefCell<TabLifecycle>,
    views: RefCell<ViewRegistry>,
    sidebar: SidebarModels,
    geometry: Cell<ViewGeometry>,
    favicons: RefCell<FaviconCache>,
    page: RefCell<PageState>,
    dialogs: RefCell<DialogQueue>,
    context_menu: Cell<Option<(ViewId, RequestId)>>,
    transfer: RefCell<TransferState>,
    closed_tabs: RefCell<Vec<ClosedTab>>,
    clicks: RefCell<ClickTracker>,
    clipboard: Clipboard,
    clipboard_sync: RefCell<ClipboardSync>,
    find_text: RefCell<String>,
    downloads: RefCell<HashSet<u32>>,
    this: Weak<Browser>,
}

impl Browser {
    pub fn new(
        window: &AppWindow,
        storage: Storage,
        gpu: Rc<GpuContext>,
        paths: Paths,
    ) -> Result<Rc<Self>> {
        let workspace = initial_workspace(&storage)?;
        let session = Session::load(&storage, workspace)?;
        let preferences = Preferences::load(&storage);
        let lifecycle = TabLifecycle::new(preferences.frozen_limit);
        let geometry = ViewGeometry {
            width: 1024,
            height: 768,
            scale: window.window().scale_factor(),
        };
        Ok(Rc::new_cyclic(|this| Self {
            window: window.as_weak(),
            storage,
            gpu,
            paths,
            preferences,
            engine: RefCell::new(None),
            pump: RefCell::new(None),
            session: RefCell::new(session),
            lifecycle: RefCell::new(lifecycle),
            views: RefCell::default(),
            sidebar: SidebarModels::attach(window),
            geometry: Cell::new(geometry),
            favicons: RefCell::default(),
            page: RefCell::default(),
            dialogs: RefCell::default(),
            context_menu: Cell::new(None),
            transfer: RefCell::default(),
            closed_tabs: RefCell::default(),
            clicks: RefCell::default(),
            clipboard: Clipboard::default(),
            clipboard_sync: RefCell::default(),
            find_text: RefCell::default(),
            downloads: RefCell::default(),
            this: this.clone(),
        }))
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

fn initial_workspace(storage: &Storage) -> Result<WorkspaceId> {
    let workspaces = storage.workspaces()?;
    if workspaces.is_empty() {
        let workspace = storage.create_workspace("Allgemein")?;
        storage.set_setting(ACTIVE_WORKSPACE_KEY, &workspace.id.0.to_string())?;
        return Ok(workspace.id);
    }
    let stored = storage
        .setting_i64(ACTIVE_WORKSPACE_KEY)?
        .map(WorkspaceId)
        .filter(|id| workspaces.iter().any(|workspace| workspace.id == *id));
    Ok(stored.unwrap_or(workspaces[0].id))
}
