use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cef::args::Args;
use cef::{
    BrowserSettings, CefString, CefStringList, ImplCookieManager, ImplDownloadItemCallback,
    ImplFileDialogCallback, ImplJsdialogCallback, ImplRunContextMenuCallback, Settings, WindowInfo,
};

use crate::app;
use crate::clipboard::ClipboardHelper;
use crate::config::{EngineConfig, ProxyRoute, Session};
use crate::dev_tools::DevToolsSession;
use crate::error::EngineError;
use crate::events::{EngineEvent, RequestId};
use crate::pump;
use crate::registry::{self, PendingCallback, Runtime};
use crate::render::GpuContext;
use crate::view::{BrowserView, ViewGeometry, ViewId, build_client};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// The running browser engine. Exists at most once per process, on the main thread.
pub struct Engine {
    runtime: Rc<Runtime>,
}

impl Engine {
    /// Starts Chromium. `gpu` must be the device the user interface renders with.
    pub fn initialize(config: EngineConfig, gpu: Rc<GpuContext>) -> Result<Self, EngineError> {
        let accelerated = config.accelerated_paint && gpu.supports_shared_textures();
        pump::install(config.schedule_work);

        let settings = Settings {
            no_sandbox: i32::from(!config.sandbox),
            windowless_rendering_enabled: 1,
            external_message_pump: 1,
            persist_session_cookies: 1,
            root_cache_path: path_string(&config.root_cache_path),
            cache_path: path_string(&config.cache_path),
            locale: config.locale.as_deref().unwrap_or_default().into(),
            ..Default::default()
        };

        let args = Args::new();
        let mut app = app::browser_app(config.extra_switches);
        let initialized = cef::initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            std::ptr::null_mut(),
        );
        if initialized != 1 {
            return Err(EngineError::Initialization);
        }

        let runtime = Rc::new(Runtime::new(
            gpu,
            config.download_directory,
            config.ask_download_location,
            config.frame_rate.clamp(1, 240) as i32,
            accelerated,
            config.content_filter,
        ));
        registry::install(runtime.clone());
        tracing::info!(accelerated, "browser engine ready");
        Ok(Self { runtime })
    }

    /// Performs pending engine work. Call on the main thread whenever the scheduler asks for it.
    pub fn do_message_loop_work(&self) {
        cef::do_message_loop_work();
        self.runtime.flush_dev_tools();
    }

    /// Takes every event produced since the last call.
    pub fn drain_events(&self) -> Vec<EngineEvent> {
        self.runtime.drain_events()
    }

    /// Whether new views receive frames as shared GPU textures.
    pub fn uses_accelerated_paint(&self) -> bool {
        self.runtime.accelerated.get()
    }

    /// Creates a browser that loads `url` in the ordinary session.
    pub fn create_view(
        &self,
        url: &str,
        geometry: ViewGeometry,
        hidden: bool,
    ) -> Result<BrowserView, EngineError> {
        self.create_view_in(url, geometry, hidden, Session::Normal)
    }

    /// Creates a browser that loads `url` in the given session.
    pub fn create_view_in(
        &self,
        url: &str,
        geometry: ViewGeometry,
        hidden: bool,
        session: Session,
    ) -> Result<BrowserView, EngineError> {
        let state = self.runtime.create_view(geometry, hidden);
        let window_info = WindowInfo {
            windowless_rendering_enabled: 1,
            shared_texture_enabled: i32::from(self.runtime.accelerated.get()),
            external_begin_frame_enabled: 0,
            ..Default::default()
        };
        let browser_settings = BrowserSettings {
            windowless_frame_rate: self.runtime.frame_rate,
            ..Default::default()
        };
        state.private.set(session == Session::Private);
        state.through_tor.set(matches!(session, Session::Tor(_)));
        let mut client = build_client(state.id);
        let mut context = self.runtime.session_context(session);
        let browser = cef::browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&url.into()),
            Some(&browser_settings),
            None,
            context.as_mut(),
        );
        let Some(browser) = browser else {
            self.runtime.remove_view(state.id);
            return Err(EngineError::BrowserCreation);
        };
        if hidden && let Some(host) = cef::ImplBrowser::host(&browser) {
            cef::ImplBrowserHost::was_hidden(&host, 1);
        }
        *state.browser.borrow_mut() = Some(browser);
        Ok(BrowserView::new(state))
    }

    /// Opens the developer tools of `inspected` in a new view with `geometry`.
    pub fn open_dev_tools(
        &self,
        inspected: &BrowserView,
        geometry: ViewGeometry,
    ) -> Result<DevToolsSession, EngineError> {
        DevToolsSession::open(inspected, |url| self.create_view(url, geometry, false))
    }

    /// Creates the hidden page that moves text between Chromium's and the system clipboard.
    pub fn create_clipboard_helper(&self) -> Result<ClipboardHelper, EngineError> {
        let geometry = ViewGeometry {
            width: 64,
            height: 64,
            scale: 1.0,
        };
        let view = self.create_view(&ClipboardHelper::page_url(), geometry, true)?;
        if let Some(state) = self.runtime.view(view.id()) {
            state.clipboard_helper.set(true);
        }
        Ok(ClipboardHelper::new(view))
    }

    /// Takes over a view the engine created on its own, see [`EngineEvent::PopupOpened`].
    pub fn adopt_view(&self, view: ViewId) -> Option<BrowserView> {
        self.runtime.view(view).map(BrowserView::new)
    }

    /// Answers a context menu request with the chosen command, or `None` if it was dismissed.
    pub fn resolve_context_menu(&self, request: RequestId, command: Option<i32>) {
        if let Some(PendingCallback::ContextMenu(callback)) = self.runtime.take_pending(request) {
            match command {
                Some(command) => callback.cont(command, cef::EventFlags::default()),
                None => callback.cancel(),
            }
        }
    }

    /// Answers a file dialog request with the chosen paths, or `None` if it was canceled.
    pub fn resolve_file_dialog(&self, request: RequestId, paths: Option<Vec<PathBuf>>) {
        if let Some(PendingCallback::FileDialog(callback)) = self.runtime.take_pending(request) {
            match paths.filter(|paths| !paths.is_empty()) {
                Some(paths) => {
                    let mut list = CefStringList::new();
                    for path in paths {
                        list.append(&path.to_string_lossy());
                    }
                    callback.cont(Some(&mut list));
                }
                None => callback.cancel(),
            }
        }
    }

    /// Answers a JavaScript dialog. `input` is only used by prompts.
    pub fn resolve_js_dialog(&self, request: RequestId, accepted: bool, input: Option<&str>) {
        if let Some(PendingCallback::JsDialog { callback, .. }) = self.runtime.take_pending(request)
        {
            let input = input.map(CefString::from);
            callback.cont(i32::from(accepted), input.as_ref());
        }
    }

    /// Where new downloads go, and whether to ask for a location each time instead.
    pub fn set_download_options(&self, directory: PathBuf, ask_for_location: bool) {
        *self.runtime.download_directory.borrow_mut() = directory;
        self.runtime.ask_download_location.set(ask_for_location);
    }

    /// Cancels a running download, see [`crate::DownloadState::id`].
    pub fn cancel_download(&self, id: u32) {
        if let Some(callback) = self.download_callback(id) {
            callback.cancel();
        }
    }

    pub fn pause_download(&self, id: u32) {
        if let Some(callback) = self.download_callback(id) {
            callback.pause();
        }
    }

    pub fn resume_download(&self, id: u32) {
        if let Some(callback) = self.download_callback(id) {
            callback.resume();
        }
    }

    /// The callback is cloned out of the registry because the engine may report the change
    /// while the call is still running.
    fn download_callback(&self, id: u32) -> Option<cef::DownloadItemCallback> {
        self.runtime.downloads.borrow().get(&id).cloned()
    }

    /// Sends every ordinary session through the SOCKS server on the loopback address, or lets
    /// them go out directly again. This is how the user's VPN carries the browser.
    pub fn set_proxy(&self, route: Option<ProxyRoute>) -> Result<(), String> {
        self.runtime.set_proxy(route)
    }

    /// Deletes every cookie of the profile. Returns `false` if the cookie store is unavailable.
    pub fn delete_all_cookies(&self) -> bool {
        cef::cookie_manager_get_global_manager(None)
            .is_some_and(|manager| manager.delete_cookies(None, None, None) != 0)
    }

    /// Closes every browser, waits for them to finish and shuts Chromium down.
    pub fn shutdown(self) {
        self.runtime.clear_pending();
        for view in self.runtime.views() {
            view.closing.set(true);
            if let Some(host) = view.host() {
                cef::ImplBrowserHost::close_browser(&host, 1);
            }
        }

        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        while self.runtime.view_count() > 0 && Instant::now() < deadline {
            cef::do_message_loop_work();
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.runtime.view_count() > 0 {
            tracing::warn!(
                open = self.runtime.view_count(),
                "browsers did not close in time"
            );
        }
        for view in self.runtime.views() {
            view.browser.borrow_mut().take();
        }
        self.runtime.drain_events();
        registry::uninstall();
        drop(self.runtime);
        cef::shutdown();
    }
}

fn path_string(path: &std::path::Path) -> CefString {
    path.to_string_lossy().as_ref().into()
}
