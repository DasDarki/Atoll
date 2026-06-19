use std::cell::{Cell, RefCell};

use cef::{Browser, BrowserHost, ImplBrowser};

use super::{ViewGeometry, ViewId, ViewRect};
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
        }
    }

    pub(crate) fn browser(&self) -> Option<Browser> {
        self.browser.borrow().clone()
    }

    pub(crate) fn host(&self) -> Option<BrowserHost> {
        self.browser().and_then(|browser| browser.host())
    }
}
