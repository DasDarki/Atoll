use std::collections::HashMap;

use atoll_core::TabId;
use atoll_engine::{BrowserView, CursorKind, DevToolsSession, ViewId};

/// Runtime information about a tab that currently owns an engine instance.
pub struct TabView {
    pub view: BrowserView,
    pub pending_scroll: Option<i32>,
    pub progress: f64,
    pub crashed: bool,
    pub cursor: CursorKind,
    pub status: String,
    /// Set while back navigation walks the replacement history instead of the engine history.
    pub replacement_history: bool,
    pub expected_address: Option<String>,
    /// The developer tools docked to this tab, if open.
    pub dev_tools: Option<DevTools>,
    /// Whether the page shows a password field.
    pub login_form: bool,
    /// Whether the page shows a video of a noticeable size.
    pub video: bool,
}

impl TabView {
    pub fn new(view: BrowserView, pending_scroll: Option<i32>) -> Self {
        Self {
            view,
            pending_scroll,
            progress: 0.0,
            crashed: false,
            cursor: CursorKind::Default,
            status: String::new(),
            replacement_history: false,
            expected_address: None,
            dev_tools: None,
            login_form: false,
            video: false,
        }
    }
}

/// The developer tools of a tab, rendered by their own engine view.
pub struct DevTools {
    pub session: DevToolsSession,
    pub cursor: CursorKind,
}

impl DevTools {
    pub fn new(session: DevToolsSession) -> Self {
        Self {
            session,
            cursor: CursorKind::Default,
        }
    }

    pub fn view(&self) -> &BrowserView {
        self.session.view()
    }
}

/// Maps tabs to their engine views and back.
#[derive(Default)]
pub struct ViewRegistry {
    views: HashMap<TabId, TabView>,
    tabs: HashMap<ViewId, TabId>,
    dev_tools: HashMap<ViewId, TabId>,
}

impl ViewRegistry {
    pub fn insert(&mut self, tab: TabId, view: TabView) {
        self.tabs.insert(view.view.id(), tab);
        if let Some(previous) = self.views.insert(tab, view) {
            self.forget(&previous);
        }
    }

    pub fn remove(&mut self, tab: TabId) -> Option<TabView> {
        let view = self.views.remove(&tab)?;
        self.forget(&view);
        Some(view)
    }

    pub fn get(&self, tab: TabId) -> Option<&TabView> {
        self.views.get(&tab)
    }

    pub fn get_mut(&mut self, tab: TabId) -> Option<&mut TabView> {
        self.views.get_mut(&tab)
    }

    /// Every engine instance of this window, awake or not.
    pub fn entries(&self) -> impl Iterator<Item = &TabView> {
        self.views.values()
    }

    pub fn contains(&self, tab: TabId) -> bool {
        self.views.contains_key(&tab)
    }

    /// The tab whose page `view` shows.
    pub fn tab_of(&self, view: ViewId) -> Option<TabId> {
        self.tabs.get(&view).copied()
    }

    /// The tab whose developer tools `view` shows.
    pub fn dev_tools_owner(&self, view: ViewId) -> Option<TabId> {
        self.dev_tools.get(&view).copied()
    }

    pub fn attach_dev_tools(&mut self, tab: TabId, dev_tools: DevTools) {
        let Some(entry) = self.views.get_mut(&tab) else {
            return;
        };
        self.dev_tools.insert(dev_tools.view().id(), tab);
        if let Some(previous) = entry.dev_tools.replace(dev_tools) {
            self.dev_tools.remove(&previous.view().id());
        }
    }

    pub fn detach_dev_tools(&mut self, tab: TabId) -> Option<DevTools> {
        let dev_tools = self.views.get_mut(&tab)?.dev_tools.take()?;
        self.dev_tools.remove(&dev_tools.view().id());
        Some(dev_tools)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&TabId, &TabView)> {
        self.views.iter()
    }

    pub fn drain(&mut self) -> Vec<(TabId, TabView)> {
        self.tabs.clear();
        self.dev_tools.clear();
        self.views.drain().collect()
    }

    fn forget(&mut self, view: &TabView) {
        self.tabs.remove(&view.view.id());
        if let Some(dev_tools) = &view.dev_tools {
            self.dev_tools.remove(&dev_tools.view().id());
        }
    }
}
