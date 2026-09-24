use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crate::config::{ProxyRoute, Session};
use crate::dev_tools::DevToolsBridge;
use crate::events::{EngineEvent, RequestId};
use crate::filter::ContentFilter;
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
    pub(crate) content_filter: Option<Arc<dyn ContentFilter>>,
    events: RefCell<VecDeque<EngineEvent>>,
    views: RefCell<HashMap<ViewId, Rc<ViewState>>>,
    pending: RefCell<HashMap<RequestId, PendingCallback>>,
    popups: RefCell<HashMap<i32, ViewId>>,
    dev_tools: RefCell<HashMap<ViewId, Rc<DevToolsBridge>>>,
    next_view: Cell<u64>,
    next_request: Cell<u64>,
    /// Where the ordinary sessions send their requests, while the user's VPN carries them.
    proxy: RefCell<Option<ProxyRoute>>,
    /// The session private views share, built when the first one asks for it.
    private: RefCell<Option<cef::RequestContext>>,
    /// The same for views that go through Tor, with the port they were built for.
    tor: RefCell<Option<(u16, cef::RequestContext)>>,
    /// How many views use the private session right now.
    private_views: Cell<usize>,
    tor_views: Cell<usize>,
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
        content_filter: Option<Arc<dyn ContentFilter>>,
    ) -> Self {
        Self {
            gpu,
            download_directory: RefCell::new(download_directory),
            ask_download_location: Cell::new(ask_download_location),
            downloads: RefCell::default(),
            frame_rate,
            accelerated: Cell::new(accelerated),
            content_filter,
            events: RefCell::default(),
            views: RefCell::default(),
            pending: RefCell::default(),
            popups: RefCell::default(),
            dev_tools: RefCell::default(),
            next_view: Cell::new(0),
            next_request: Cell::new(0),
            proxy: RefCell::default(),
            private: RefCell::default(),
            tor: RefCell::default(),
            private_views: Cell::new(0),
            tor_views: Cell::new(0),
        }
    }

    /// The request context a view of `session` runs in. The private one keeps nothing on disk,
    /// and every private view shares it, so a sign-in reaches the next private tab.
    pub(crate) fn session_context(&self, session: Session) -> Option<cef::RequestContext> {
        match session {
            Session::Normal => None,
            Session::Private => {
                self.private_views.set(self.private_views.get() + 1);
                let mut held = self.private.borrow_mut();
                if held.is_none() {
                    let built = forgetful_context();
                    if let Some(context) = built.as_ref()
                        && let Err(error) = route_requests(context, self.proxy.borrow().as_ref())
                    {
                        tracing::warn!(
                            error,
                            "the private session could not be sent through the VPN"
                        );
                    }
                    *held = built;
                }
                held.clone()
            }
            Session::Tor(port) => {
                self.tor_views.set(self.tor_views.get() + 1);
                let mut held = self.tor.borrow_mut();
                if held.as_ref().is_some_and(|(known, _)| *known != port) {
                    held.take();
                }
                if held.is_none() {
                    let context = forgetful_context()?;
                    if let Err(error) = send_through_proxy(&context, port) {
                        tracing::warn!(error, "the Tor proxy could not be set");
                        return None;
                    }
                    *held = Some((port, context));
                }
                held.as_ref().map(|(_, context)| context.clone())
            }
        }
    }

    /// A private view is gone. With the last one the session goes too, so the next private tab
    /// starts without anything the previous ones left behind.
    pub(crate) fn private_view_closed(&self, through_tor: bool) {
        let (count, held) = if through_tor {
            (&self.tor_views, None)
        } else {
            (&self.private_views, Some(&self.private))
        };
        let left = count.get().saturating_sub(1);
        count.set(left);
        if left > 0 {
            return;
        }
        match held {
            Some(private) => {
                private.borrow_mut().take();
            }
            None => {
                self.tor.borrow_mut().take();
            }
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
        let gone = self
            .views
            .borrow()
            .get(&id)
            .map(|view| (view.private.get(), view.through_tor.get()));
        if let Some((private, through_tor)) = gone
            && (private || through_tor)
        {
            self.private_view_closed(through_tor);
        }
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

    /// Sends every ordinary session through `route`, or lets them go out directly again. The
    /// sessions of the private tabs are included; the ones that go through Tor keep their own way.
    pub(crate) fn set_proxy(&self, route: Option<ProxyRoute>) -> Result<(), String> {
        *self.proxy.borrow_mut() = route.clone();
        let contexts = [
            cef::request_context_get_global_context(),
            self.private.borrow().clone(),
        ];
        for context in contexts.into_iter().flatten() {
            route_requests(&context, route.as_ref())?;
        }
        Ok(())
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

/// A session that keeps nothing on disk.
fn forgetful_context() -> Option<cef::RequestContext> {
    let settings = cef::RequestContextSettings {
        size: size_of::<cef::sys::_cef_request_context_settings_t>(),
        cache_path: cef::CefString::from(""),
        persist_session_cookies: 0,
        accept_language_list: cef::CefString::from(""),
        cookieable_schemes_list: cef::CefString::from(""),
        cookieable_schemes_exclude_defaults: 0,
    };
    cef::request_context_create_context(Some(&settings), None)
}

/// Sends one session through `route`, or lets it go out directly again.
///
/// Two things are set: where requests go, and what WebRTC may do. WebRTC can open its own
/// connections past a proxy and hand a site the real address that way, so while a route stands,
/// it is only allowed the way the rest of the browser takes.
fn route_requests(context: &cef::RequestContext, route: Option<&ProxyRoute>) -> Result<(), String> {
    match route {
        Some(route) => {
            let settings = [
                ("mode", "fixed_servers".to_owned()),
                ("server", format!("socks5://127.0.0.1:{}", route.port)),
                ("bypass_list", route.bypass.join(",")),
            ];
            write_proxy(context, &settings)?;
            set_text(
                context,
                "webrtc.ip_handling_policy",
                "disable_non_proxied_udp",
            );
        }
        None => {
            write_proxy(context, &[("mode", "system".to_owned())])?;
            set_text(context, "webrtc.ip_handling_policy", "default");
        }
    }
    Ok(())
}

/// Writes the `proxy` preference of a session as Chromium reads it.
fn write_proxy(context: &cef::RequestContext, settings: &[(&str, String)]) -> Result<(), String> {
    use cef::{ImplDictionaryValue, ImplPreferenceManager, ImplValue};

    let name = cef::CefString::from("proxy");
    if context.can_set_preference(Some(&name)) == 0 {
        return Err("the engine does not take a proxy for this session".to_owned());
    }
    let dictionary = cef::dictionary_value_create().ok_or("no settings could be built")?;
    for (key, text) in settings {
        let value = cef::value_create().ok_or("no value could be built")?;
        value.set_string(Some(&cef::CefString::from(text.as_str())));
        dictionary.set_value(Some(&cef::CefString::from(*key)), Some(&mut value.clone()));
    }
    let value = cef::value_create().ok_or("no value could be built")?;
    value.set_dictionary(Some(&mut dictionary.clone()));
    let mut problem = cef::CefString::from("");
    if context.set_preference(Some(&name), Some(&mut value.clone()), Some(&mut problem)) == 0 {
        return Err(problem.to_string());
    }
    Ok(())
}

/// Sets a preference that is a plain text, and only says so in the log if it cannot be set.
fn set_text(context: &cef::RequestContext, name: &str, text: &str) {
    use cef::{ImplPreferenceManager, ImplValue};

    let name = cef::CefString::from(name);
    if context.can_set_preference(Some(&name)) == 0 {
        tracing::debug!(preference = %name, "the engine does not take this setting");
        return;
    }
    let Some(value) = cef::value_create() else {
        return;
    };
    value.set_string(Some(&cef::CefString::from(text)));
    let mut problem = cef::CefString::from("");
    if context.set_preference(Some(&name), Some(&mut value.clone()), Some(&mut problem)) == 0 {
        tracing::debug!(preference = %name, problem = %problem, "a setting was refused");
    }
}

/// Points a session at the SOCKS proxy on `port`, which is how the engine reaches Tor.
fn send_through_proxy(context: &cef::RequestContext, port: u16) -> Result<(), String> {
    write_proxy(
        context,
        &[
            ("mode", "fixed_servers".to_owned()),
            ("server", format!("socks5://127.0.0.1:{port}")),
        ],
    )?;
    set_text(
        context,
        "webrtc.ip_handling_policy",
        "disable_non_proxied_udp",
    );
    Ok(())
}
