use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;

use crate::dev_tools::DevToolsBridge;
use crate::events::{EngineEvent, RequestId};
use crate::render::GpuContext;
use crate::view::{ViewGeometry, ViewId, ViewState};

/// A callback that waits for a decision of the user interface.
pub(crate) enum PendingCallback {
    ContextMenu(cef::RunContextMenuCallback),
    FileDialog(cef::FileDialogCallback),
    JsDialog {
        view: ViewId,
        callback: cef::JsdialogCallback,
    },
}

/// State shared by all browsers. It lives on the browser process UI thread only, which is the
/// thread every handler used by Atoll is called on.
pub(crate) struct Runtime {
    pub(crate) gpu: Rc<GpuContext>,
    pub(crate) download_directory: RefCell<PathBuf>,
    pub(crate) ask_download_location: Cell<bool>,
    pub(crate) downloads: RefCell<HashMap<u32, cef::DownloadItemCallback>>,
    pub(crate) frame_rate: i32,
    pub(crate) accelerated: Cell<bool>,
    events: RefCell<VecDeque<EngineEvent>>,
    views: RefCell<HashMap<ViewId, Rc<ViewState>>>,
    pending: RefCell<HashMap<RequestId, PendingCallback>>,
    popups: RefCell<HashMap<i32, ViewId>>,
    dev_tools: RefCell<HashMap<ViewId, Rc<DevToolsBridge>>>,
    next_view: Cell<u64>,
    next_request: Cell<u64>,
}

thread_local! {
    static RUNTIME: RefCell<Option<Rc<Runtime>>> = const { RefCell::new(None) };
}

impl Runtime {
    pub(crate) fn new(
        gpu: Rc<GpuContext>,
        download_directory: PathBuf,
        ask_download_location: bool,
        frame_rate: i32,
        accelerated: bool,
    ) -> Self {
        Self {
            gpu,
            download_directory: RefCell::new(download_directory),
            ask_download_location: Cell::new(ask_download_location),
            downloads: RefCell::default(),
            frame_rate,
            accelerated: Cell::new(accelerated),
            events: RefCell::default(),
            views: RefCell::default(),
            pending: RefCell::default(),
            popups: RefCell::default(),
            dev_tools: RefCell::default(),
            next_view: Cell::new(0),
            next_request: Cell::new(0),
        }
    }

    pub(crate) fn emit(&self, event: EngineEvent) {
        self.events.borrow_mut().push_back(event);
    }

    pub(crate) fn drain_events(&self) -> Vec<EngineEvent> {
        self.events.borrow_mut().drain(..).collect()
    }

    pub(crate) fn create_view(&self, geometry: ViewGeometry, hidden: bool) -> Rc<ViewState> {
        let id = ViewId(self.next_view.get() + 1);
        self.next_view.set(id.0);
        let view = Rc::new(ViewState::new(id, geometry, hidden));
        self.views.borrow_mut().insert(id, view.clone());
        view
    }

    pub(crate) fn view(&self, id: ViewId) -> Option<Rc<ViewState>> {
        self.views.borrow().get(&id).cloned()
    }

    pub(crate) fn remove_view(&self, id: ViewId) -> Option<Rc<ViewState>> {
        self.popups.borrow_mut().retain(|_, view| *view != id);
        self.pending.borrow_mut().retain(
            |_, pending| !matches!(pending, PendingCallback::JsDialog { view, .. } if *view == id),
        );
        self.views.borrow_mut().remove(&id)
    }

    pub(crate) fn views(&self) -> Vec<Rc<ViewState>> {
        self.views.borrow().values().cloned().collect()
    }

    pub(crate) fn view_count(&self) -> usize {
        self.views.borrow().len()
    }

    pub(crate) fn remember_popup(&self, popup_id: i32, view: ViewId) {
        self.popups.borrow_mut().insert(popup_id, view);
    }

    pub(crate) fn take_popup(&self, popup_id: i32) -> Option<ViewId> {
        self.popups.borrow_mut().remove(&popup_id)
    }

    pub(crate) fn add_pending(&self, callback: PendingCallback) -> RequestId {
        let id = RequestId(self.next_request.get() + 1);
        self.next_request.set(id.0);
        self.pending.borrow_mut().insert(id, callback);
        id
    }

    pub(crate) fn take_pending(&self, id: RequestId) -> Option<PendingCallback> {
        self.pending.borrow_mut().remove(&id)
    }

    pub(crate) fn drop_js_dialogs(&self, view: ViewId) {
        self.pending.borrow_mut().retain(|_, pending| {
            !matches!(pending, PendingCallback::JsDialog { view: owner, .. } if *owner == view)
        });
    }

    pub(crate) fn clear_pending(&self) {
        self.pending.borrow_mut().clear();
        self.downloads.borrow_mut().clear();
        self.dev_tools.borrow_mut().clear();
    }

    pub(crate) fn add_dev_tools_bridge(&self, frontend: ViewId, bridge: Rc<DevToolsBridge>) {
        self.dev_tools.borrow_mut().insert(frontend, bridge);
    }

    pub(crate) fn remove_dev_tools_bridge(&self, frontend: ViewId) {
        self.dev_tools.borrow_mut().remove(&frontend);
    }

    /// Passes messages of every DevTools frontend on to the inspected pages.
    pub(crate) fn flush_dev_tools(&self) {
        let bridges: Vec<Rc<DevToolsBridge>> = self.dev_tools.borrow().values().cloned().collect();
        for bridge in bridges {
            bridge.flush();
        }
    }
}

pub(crate) fn install(runtime: Rc<Runtime>) {
    RUNTIME.with_borrow_mut(|slot| *slot = Some(runtime));
}

pub(crate) fn uninstall() {
    RUNTIME.with_borrow_mut(|slot| *slot = None);
}

pub(crate) fn runtime() -> Option<Rc<Runtime>> {
    RUNTIME.with_borrow(Clone::clone)
}

/// Runs `action` with the runtime and the state of `view` if both exist on this thread.
pub(crate) fn with_view<R>(
    view: ViewId,
    action: impl FnOnce(&Runtime, &ViewState) -> R,
) -> Option<R> {
    let runtime = runtime()?;
    let state = runtime.view(view)?;
    Some(action(&runtime, &state))
}

pub(crate) fn emit(event: EngineEvent) {
    if let Some(runtime) = runtime() {
        runtime.emit(event);
    }
}
