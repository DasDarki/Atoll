use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use cef::{Browser, BrowserHost, ImplBrowser};

use super::{ViewGeometry, ViewId, ViewRect};
use crate::activity::PageActivity;
use crate::page_bridge::PageBridge;
use crate::render::ViewSurface;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NavigationState {
    pub(crate) loading: bool,
    pub(crate) can_go_back: bool,
    pub(crate) can_go_forward: bool,
}

/// An in-page drag started by the page itself, fed with mouse events until the button is released.
pub(crate) struct DragSession {
    pub(crate) allowed_operations: cef::DragOperationsMask,
    pub(crate) current_operation: cef::DragOperationsMask,
}

/// Everything the engine knows about one browser.
pub(crate) struct ViewState {
    pub(crate) id: ViewId,
    pub(crate) browser: RefCell<Option<Browser>>,
    pub(crate) geometry: Cell<ViewGeometry>,
    pub(crate) hidden: Cell<bool>,
    pub(crate) surface: ViewSurface,
    pub(crate) popup_rect: Cell<Option<ViewRect>>,
    pub(crate) scroll: Cell<(f64, f64)>,
    pub(crate) navigation: Cell<NavigationState>,
    pub(crate) pressed_buttons: Cell<u32>,
    pub(crate) drag: RefCell<Option<DragSession>>,
    pub(crate) closing: Cell<bool>,
    pub(crate) paint_failed: Cell<bool>,
    pub(crate) clipboard_helper: Cell<bool>,
    /// Whether the view runs in the private session, which is given up with the last of them.
    pub(crate) private: Cell<bool>,
    /// The same for the session that goes through Tor.
    pub(crate) through_tor: Cell<bool>,
    /// The view shows a DevTools frontend, see [`crate::DevToolsSession`].
    pub(crate) dev_tools_frontend: Cell<bool>,
    /// Requests blocked by the content filter since the page started loading. Counted on the
    /// network thread.
    pub(crate) blocked_requests: Arc<AtomicU32>,
    pub(crate) activity: RefCell<PageActivity>,
    /// Talking to the page through Chromium's protocol.
    pub(crate) bridge: RefCell<PageBridge>,
    pub(crate) bridge_registration: RefCell<Option<cef::Registration>>,
}

impl ViewState {
    pub(crate) fn new(id: ViewId, geometry: ViewGeometry, hidden: bool) -> Self {
        Self {
            id,
            browser: RefCell::new(None),
            geometry: Cell::new(geometry),
            hidden: Cell::new(hidden),
            surface: ViewSurface::default(),
            popup_rect: Cell::new(None),
            scroll: Cell::new((0.0, 0.0)),
            navigation: Cell::new(NavigationState::default()),
            pressed_buttons: Cell::new(0),
            drag: RefCell::new(None),
            closing: Cell::new(false),
            paint_failed: Cell::new(false),
            clipboard_helper: Cell::new(false),
            private: Cell::new(false),
            through_tor: Cell::new(false),
            dev_tools_frontend: Cell::new(false),
            blocked_requests: Arc::default(),
            activity: RefCell::default(),
            bridge: RefCell::default(),
            bridge_registration: RefCell::default(),
        }
    }

    pub(crate) fn browser(&self) -> Option<Browser> {
        self.browser.borrow().clone()
    }

    pub(crate) fn host(&self) -> Option<BrowserHost> {
        self.browser().and_then(|browser| browser.host())
    }
}
