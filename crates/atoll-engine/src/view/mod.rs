mod client;
mod handlers;
mod security;
mod state;

use std::rc::Rc;

use cef::{
    CefString, CefStringList, ImplBrowser, ImplBrowserHost, ImplDictionaryValue, ImplFrame,
    MouseEvent, Range,
};

/// Chromium's "no range", used when input method text does not replace existing text.
const NO_RANGE: Range = Range {
    from: u32::MAX,
    to: u32::MAX,
};

pub(crate) use client::build_client;
pub use security::{CertificateInfo, SecurityInfo};
pub(crate) use state::{DragSession, ViewState};

use crate::autofill::{self, LoginFill};
use crate::input::{EditCommand, KeyInput, Modifiers, MouseButton, ZoomCommand};
use crate::render::{SurfaceFrame, SurfaceLayer};
use crate::video;

/// Identifies a browser created by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewId(pub(crate) u64);

impl ViewId {
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// A rectangle in view coordinates (device independent pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// The logical size of a view and the scale factor of the display it is shown on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewGeometry {
    pub width: i32,
    pub height: i32,
    pub scale: f32,
}

impl Default for ViewGeometry {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            scale: 1.0,
        }
    }
}

/// A handle to one off-screen browser. Dropping the handle closes the browser.
pub struct BrowserView {
    state: Rc<ViewState>,
}

impl BrowserView {
    pub(crate) fn new(state: Rc<ViewState>) -> Self {
        Self { state }
    }

    pub fn id(&self) -> ViewId {
        self.state.id
    }

    pub(crate) fn host(&self) -> Option<cef::BrowserHost> {
        self.state.host()
    }

    pub fn is_closed(&self) -> bool {
        self.state.closing.get() || self.state.browser().is_none()
    }

    pub fn load_url(&self, url: &str) {
        if let Some(frame) = self
            .state
            .browser()
            .and_then(|browser| browser.main_frame())
        {
            frame.load_url(Some(&url.into()));
        }
    }

    /// Requests the content filter blocked since the current page started loading.
    pub fn blocked_requests(&self) -> u32 {
        self.state
            .blocked_requests
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The address of the main frame.
    pub fn url(&self) -> Option<String> {
        let frame = self.state.browser()?.main_frame()?;
        let url = CefString::from(&frame.url()).to_string();
        (!url.is_empty()).then_some(url)
    }

    pub fn go_back(&self) {
        if let Some(browser) = self.state.browser() {
            browser.go_back();
        }
    }

    pub fn go_forward(&self) {
        if let Some(browser) = self.state.browser() {
            browser.go_forward();
        }
    }

    pub fn reload(&self, ignore_cache: bool) {
        if let Some(browser) = self.state.browser() {
            if ignore_cache {
                browser.reload_ignore_cache();
            } else {
                browser.reload();
            }
        }
    }

    pub fn stop(&self) {
        if let Some(browser) = self.state.browser() {
            browser.stop_load();
        }
    }

    pub fn is_loading(&self) -> bool {
        self.state.navigation.get().loading
    }

    pub fn can_go_back(&self) -> bool {
        self.state.navigation.get().can_go_back
    }

    pub fn can_go_forward(&self) -> bool {
        self.state.navigation.get().can_go_forward
    }

    pub fn geometry(&self) -> ViewGeometry {
        self.state.geometry.get()
    }

    /// Updates the logical size and scale factor, re-rendering the page if anything changed.
    pub fn resize(&self, geometry: ViewGeometry) {
        let previous = self.state.geometry.replace(geometry);
        if previous == geometry {
            return;
        }
        if let Some(host) = self.state.host() {
            if previous.scale != geometry.scale {
                host.notify_screen_info_changed();
            }
            host.was_resized();
        }
    }

    pub fn set_hidden(&self, hidden: bool) {
        if self.state.hidden.replace(hidden) == hidden {
            return;
        }
        if let Some(host) = self.state.host() {
            host.was_hidden(i32::from(hidden));
            if !hidden {
                host.invalidate(cef::PaintElementType::VIEW);
            }
        }
    }

    pub fn is_hidden(&self) -> bool {
        self.state.hidden.get()
    }

    pub fn set_focus(&self, focused: bool) {
        if let Some(host) = self.state.host() {
            host.set_focus(i32::from(focused));
        }
    }

    pub fn mouse_move(&self, x: i32, y: i32, modifiers: Modifiers) {
        let Some(host) = self.state.host() else {
            return;
        };
        let event = self.mouse_event(x, y, modifiers);
        if let Some(drag) = self.state.drag.borrow().as_ref() {
            host.drag_target_drag_over(Some(&event), drag.allowed_operations);
            return;
        }
        host.send_mouse_move_event(Some(&event), 0);
    }

    pub fn mouse_leave(&self, x: i32, y: i32, modifiers: Modifiers) {
        if self.state.drag.borrow().is_some() {
            return;
        }
        if let Some(host) = self.state.host() {
            host.send_mouse_move_event(Some(&self.mouse_event(x, y, modifiers)), 1);
        }
    }

    pub fn mouse_button(
        &self,
        x: i32,
        y: i32,
        button: MouseButton,
        pressed: bool,
        click_count: i32,
        modifiers: Modifiers,
    ) {
        let Some(host) = self.state.host() else {
            return;
        };
        let buttons = self.state.pressed_buttons.get();
        self.state.pressed_buttons.set(if pressed {
            buttons | button.event_flag()
        } else {
            buttons & !button.event_flag()
        });
        let event = self.mouse_event(x, y, modifiers);

        if !pressed && let Some(drag) = self.state.drag.borrow_mut().take() {
            host.drag_target_drop(Some(&event));
            host.drag_source_ended_at(x, y, drag.current_operation);
            host.drag_source_system_drag_ended();
            return;
        }
        host.send_mouse_click_event(
            Some(&event),
            button.cef_type(),
            i32::from(!pressed),
            click_count.max(1),
        );
    }

    pub fn mouse_wheel(&self, x: i32, y: i32, delta_x: i32, delta_y: i32, modifiers: Modifiers) {
        if let Some(host) = self.state.host() {
            host.send_mouse_wheel_event(Some(&self.mouse_event(x, y, modifiers)), delta_x, delta_y);
        }
    }

    /// Tells the page that the pointer capture was lost, for example when a popup opened.
    pub fn capture_lost(&self) {
        self.state.pressed_buttons.set(0);
        if let Some(host) = self.state.host() {
            if let Some(drag) = self.state.drag.borrow_mut().take() {
                host.drag_target_drag_leave();
                host.drag_source_ended_at(0, 0, drag.current_operation);
                host.drag_source_system_drag_ended();
            }
            host.send_capture_lost_event();
        }
    }

    pub fn key(&self, input: KeyInput) {
        if let Some(host) = self.state.host() {
            host.send_key_event(Some(&input.to_cef()));
        }
    }

    /// Inserts text at the caret as if it had been typed, used for input methods.
    pub fn commit_text(&self, text: &str) {
        if let Some(host) = self.state.host() {
            host.ime_commit_text(Some(&text.into()), Some(&NO_RANGE), 0);
        }
    }

    pub fn set_composition(&self, text: &str) {
        if let Some(host) = self.state.host() {
            let length = text.encode_utf16().count() as u32;
            let selection = Range {
                from: length,
                to: length,
            };
            host.ime_set_composition(Some(&text.into()), None, Some(&NO_RANGE), Some(&selection));
        }
    }

    pub fn cancel_composition(&self) {
        if let Some(host) = self.state.host() {
            host.ime_cancel_composition();
        }
    }

    pub fn edit(&self, command: EditCommand) {
        let Some(frame) = self
            .state
            .browser()
            .and_then(|browser| browser.focused_frame().or_else(|| browser.main_frame()))
        else {
            return;
        };
        match command {
            EditCommand::Undo => frame.undo(),
            EditCommand::Redo => frame.redo(),
            EditCommand::Cut => frame.cut(),
            EditCommand::Copy => frame.copy(),
            EditCommand::Paste => frame.paste(),
            EditCommand::SelectAll => frame.select_all(),
        }
    }

    pub fn zoom(&self, command: ZoomCommand) {
        if let Some(host) = self.state.host() {
            host.zoom(match command {
                ZoomCommand::In => cef::ZoomCommand::IN,
                ZoomCommand::Out => cef::ZoomCommand::OUT,
                ZoomCommand::Reset => cef::ZoomCommand::RESET,
            });
        }
    }

    pub fn find(&self, text: &str, forward: bool, find_next: bool) {
        if let Some(host) = self.state.host() {
            host.find(
                Some(&text.into()),
                i32::from(forward),
                0,
                i32::from(find_next),
            );
        }
    }

    pub fn stop_finding(&self) {
        if let Some(host) = self.state.host() {
            host.stop_finding(1);
        }
    }

    /// Fills `login` into the visible login form of every frame whose address `include`
    /// accepts. Each of them answers with [`crate::EngineEvent::LoginFilled`], the number of
    /// frames asked is returned.
    pub fn fill_login(&self, login: LoginFill<'_>, include: impl Fn(&str) -> bool) -> usize {
        self.execute_in_frames(&autofill::fill_script(login), include)
    }

    /// Asks every web frame for its videos. Each of them answers with
    /// [`crate::EngineEvent::VideosFound`], the number of frames asked is returned.
    pub fn find_videos(&self) -> usize {
        self.execute_in_frames(&video::find_videos_script(), |url| {
            ["http://", "https://", "file://"]
                .iter()
                .any(|scheme| url.starts_with(scheme))
        })
    }

    /// Runs `script` in every frame whose address `include` accepts and returns how many there
    /// were.
    fn execute_in_frames(&self, script: &str, include: impl Fn(&str) -> bool) -> usize {
        let Some(browser) = self.state.browser() else {
            return 0;
        };
        let mut identifiers = CefStringList::new();
        browser.frame_identifiers(Some(&mut identifiers));
        let mut asked = 0;
        for identifier in identifiers {
            let Some(frame) = browser.frame_by_identifier(Some(&identifier.as_str().into())) else {
                continue;
            };
            let url = CefString::from(&frame.url()).to_string();
            if frame.is_valid() != 0 && include(&url) {
                frame.execute_java_script(Some(&script.into()), Some(&url.as_str().into()), 0);
                asked += 1;
            }
        }
        asked
    }

    pub fn execute_script(&self, code: &str) {
        if let Some(frame) = self
            .state
            .browser()
            .and_then(|browser| browser.main_frame())
        {
            frame.execute_java_script(Some(&code.into()), None, 0);
        }
    }

    /// The connection security of the visible page, if the engine knows it.
    pub fn security_info(&self) -> Option<SecurityInfo> {
        security::security_info(&self.state.host()?)
    }

    /// The zoom factor in percent.
    pub fn zoom_percent(&self) -> u32 {
        self.state.host().map_or(100, |host| {
            (1.2_f64.powf(host.zoom_level()) * 100.0).round() as u32
        })
    }

    /// Deletes cookies, storage, caches and service workers of `origin`, for example
    /// `https://example.org`. Returns `false` if the request could not be sent.
    pub fn clear_site_data(&self, origin: &str) -> bool {
        let (Some(host), Some(mut parameters)) =
            (self.state.host(), cef::dictionary_value_create())
        else {
            return false;
        };
        parameters.set_string(Some(&"origin".into()), Some(&origin.into()));
        parameters.set_string(Some(&"storageTypes".into()), Some(&"all".into()));
        host.execute_dev_tools_method(
            0,
            Some(&"Storage.clearDataForOrigin".into()),
            Some(&mut parameters),
        ) != 0
    }

    /// Downloads `url` without navigating.
    pub fn start_download(&self, url: &str) {
        if let Some(host) = self.state.host() {
            host.start_download(Some(&url.into()));
        }
    }

    pub fn exit_fullscreen(&self) {
        if let Some(host) = self.state.host() {
            host.exit_fullscreen(1);
        }
    }

    pub fn set_audio_muted(&self, muted: bool) {
        if let Some(host) = self.state.host() {
            host.set_audio_muted(i32::from(muted));
        }
    }

    /// The latest page frame.
    pub fn frame(&self) -> Option<SurfaceFrame> {
        self.state.surface.frame(SurfaceLayer::Page)
    }

    /// The latest frame of an open page widget such as a `<select>` dropdown and its position.
    pub fn popup_frame(&self) -> Option<(SurfaceFrame, ViewRect)> {
        let rect = self.state.popup_rect.get()?;
        Some((self.state.surface.frame(SurfaceLayer::Popup)?, rect))
    }

    /// The scroll offset of the page's root layer.
    pub fn scroll_offset(&self) -> (f64, f64) {
        self.state.scroll.get()
    }

    /// Closes the browser without running `beforeunload` handlers.
    pub fn close(&self) {
        if self.state.closing.replace(true) {
            return;
        }
        *self.state.drag.borrow_mut() = None;
        if let Some(host) = self.state.host() {
            host.close_browser(1);
        }
    }

    fn mouse_event(&self, x: i32, y: i32, modifiers: Modifiers) -> MouseEvent {
        MouseEvent {
            x,
            y,
            modifiers: modifiers.event_flags() | self.state.pressed_buttons.get(),
        }
    }
}

impl Drop for BrowserView {
    fn drop(&mut self) {
        self.close();
    }
}
