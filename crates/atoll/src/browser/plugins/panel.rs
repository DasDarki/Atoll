//! The pages extensions bring along. A panel is an engine instance of its own that shows a file
//! of the extension folder next to the page, the way the developer tools do.

use std::path::Path;

use atoll_engine::{
    BrowserView, CursorKind, Modifiers, MouseButton, ScriptWorld, ViewGeometry, ViewId,
};
use atoll_plugin::{HostError, Panel, PluginId};
use serde_json::Value;
use slint::{ComponentHandle, Image};

use super::super::page::web_cursor;
use super::super::web_input::{InputTarget, modifiers_of};
use crate::bridge::images::frame_image;
use crate::browser::Browser;
use crate::ui::{PanelState, PointerAction, PointerButton, WebPointer, WebScroll};

/// The panel one window shows.
pub struct OpenPanel {
    pub(super) plugin: PluginId,
    pub(super) id: String,
    title: String,
    view: BrowserView,
    cursor: CursorKind,
    frame_generation: u64,
    popup_generation: u64,
}

impl Browser {
    /// Opens the panel of an extension, replacing the one that was open.
    pub(in crate::browser) fn open_panel(
        &self,
        plugin: &PluginId,
        folder: &Path,
        panel: &Panel,
    ) -> Result<(), HostError> {
        let page = folder.join(&panel.page);
        if !page.is_file() {
            return Err(HostError::Refused(format!("{} gibt es nicht", panel.page)));
        }
        let url = format!("file://{}", page.display());
        let view = {
            let engine = self.engine.borrow();
            let engine = engine
                .as_ref()
                .ok_or_else(|| HostError::Refused("der Browser startet noch".to_owned()))?;
            engine
                .create_view("about:blank", self.panel_geometry.get(), false)
                .map_err(|error| HostError::Refused(error.to_string()))?
        };
        view.start_bridge();
        view.load_url(&url);
        self.close_panel();
        self.plugins.remember_panel(view.id(), plugin, &panel.id);
        *self.panel.borrow_mut() = Some(OpenPanel {
            plugin: plugin.clone(),
            id: panel.id.clone(),
            title: panel.title.clone(),
            view,
            cursor: CursorKind::Default,
            frame_generation: 0,
            popup_generation: 0,
        });
        self.sync_panel();
        self.focus_panel();
        Ok(())
    }

    /// Closes the panel, whether the user or the extension asked for it.
    pub fn close_panel(&self) {
        let Some(panel) = self.panel.borrow_mut().take() else {
            return;
        };
        self.plugins.forget_panel(panel.view.id());
        panel.view.close();
        if self.input_target.get() == InputTarget::Panel {
            self.input_target.set(InputTarget::Page);
        }
        self.sync_panel();
        self.focus_content();
    }

    /// Closes the panel `id` of `plugin`, if that is the one on screen.
    pub(in crate::browser) fn close_panel_of(&self, plugin: &PluginId, id: &str) {
        let matches = self
            .panel
            .borrow()
            .as_ref()
            .is_some_and(|panel| panel.plugin == *plugin && panel.id == id);
        if matches {
            self.close_panel();
        }
    }

    /// Sends a message of an extension into its panel, where `window.atoll.on` picks it up.
    pub(in crate::browser) fn post_to_panel(
        &self,
        plugin: &PluginId,
        id: &str,
        channel: &str,
        payload: &Value,
    ) {
        let panel = self.panel.borrow();
        let Some(panel) = panel
            .as_ref()
            .filter(|panel| panel.plugin == *plugin && panel.id == id)
        else {
            return;
        };
        let code = format!(
            "window.atoll && window.atoll.__deliver({}, {})",
            Value::from(channel),
            payload
        );
        panel.view.evaluate(&code, ScriptWorld::Page);
    }

    pub(in crate::browser) fn is_live_panel(&self, view: ViewId) -> bool {
        self.panel
            .borrow()
            .as_ref()
            .is_some_and(|panel| panel.view.id() == view)
    }

    pub(in crate::browser) fn with_live_panel(&self, action: impl FnOnce(&BrowserView)) {
        if let Some(panel) = self.panel.borrow().as_ref() {
            action(&panel.view);
        }
    }

    pub(in crate::browser) fn set_panel_cursor(&self, cursor: CursorKind) {
        if let Some(panel) = self.panel.borrow_mut().as_mut() {
            panel.cursor = cursor;
        }
        if let Some(window) = self.window() {
            window.global::<PanelState>().set_cursor(web_cursor(cursor));
        }
    }

    /// The engine closed the panel on its own, for example because its page called `close()`.
    pub(in crate::browser) fn panel_closed(&self, view: ViewId) {
        if self.is_live_panel(view) {
            self.close_panel();
        }
    }

    pub fn panel_resized(&self, width: f32, height: f32) {
        let geometry = ViewGeometry {
            width: width.round().max(1.0) as i32,
            height: height.round().max(1.0) as i32,
            scale: self.geometry.get().scale,
        };
        if self.panel_geometry.replace(geometry) == geometry {
            return;
        }
        self.with_live_panel(|view| view.resize(geometry));
    }

    pub fn panel_pointer(&self, pointer: WebPointer) {
        let modifiers = modifiers_of(pointer.shift, pointer.control, pointer.alt, pointer.meta);
        let (x, y) = (pointer.x.round() as i32, pointer.y.round() as i32);
        if pointer.action == PointerAction::Down {
            self.cancel_context_menu();
        }
        let button = match pointer.button {
            PointerButton::Left => Some(MouseButton::Left),
            PointerButton::Middle => Some(MouseButton::Middle),
            PointerButton::Right => Some(MouseButton::Right),
            _ => None,
        };
        self.with_live_panel(|view| match (pointer.action, button) {
            (PointerAction::Move, _) => view.mouse_move(x, y, modifiers),
            (PointerAction::Down, Some(button)) => {
                let count = self.clicks.borrow_mut().press(button, pointer.x, pointer.y);
                view.mouse_button(x, y, button, true, count, modifiers);
            }
            (PointerAction::Up, Some(button)) => {
                let count = self.clicks.borrow().current();
                view.mouse_button(x, y, button, false, count, modifiers);
            }
            (PointerAction::Cancel, _) => view.capture_lost(),
            _ => {}
        });
    }

    pub fn panel_scroll(&self, scroll: WebScroll) -> bool {
        let modifiers = modifiers_of(scroll.shift, scroll.control, scroll.alt, scroll.meta);
        let mut handled = false;
        self.with_live_panel(|view| {
            view.mouse_wheel(
                scroll.x.round() as i32,
                scroll.y.round() as i32,
                scroll.delta_x.round() as i32,
                scroll.delta_y.round() as i32,
                modifiers,
            );
            handled = true;
        });
        handled
    }

    pub fn panel_hover(&self, inside: bool, x: f32, y: f32) {
        if !inside {
            self.with_live_panel(|view| {
                view.mouse_leave(x.round() as i32, y.round() as i32, Modifiers::default());
            });
        }
    }

    /// See [`Browser::web_focus_changed`].
    pub fn panel_focus_changed(&self, focused: bool) {
        if focused {
            self.input_target.set(InputTarget::Panel);
        } else if self.context_menu.get().is_some() {
            return;
        }
        self.with_live_panel(|view| view.set_focus(focused));
    }

    /// Moves the keyboard into the panel once it is laid out.
    fn focus_panel(&self) {
        let window = self.window.clone();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(window) = window.upgrade()
                && !window.get_modal_open()
            {
                window.invoke_focus_panel();
            }
        });
    }

    /// Shows or hides the panel area and puts its title up.
    pub(in crate::browser) fn sync_panel(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<PanelState>();
        let panel = self.panel.borrow();
        match panel.as_ref() {
            Some(panel) => {
                state.set_visible(true);
                state.set_title(panel.title.as_str().into());
                state.set_cursor(web_cursor(panel.cursor));
            }
            None => {
                state.set_visible(false);
                state.set_title("".into());
                state.set_frame(Image::default());
                state.set_popup_visible(false);
            }
        }
        drop(panel);
        self.present_panel_frame();
    }

    pub(in crate::browser) fn present_panel_frame(&self) {
        let Some(window) = self.window() else { return };
        let image = {
            let mut panel = self.panel.borrow_mut();
            let Some(panel) = panel.as_mut() else { return };
            let Some(frame) = panel.view.frame() else {
                return;
            };
            let fresh = panel.frame_generation != frame.generation;
            panel.frame_generation = frame.generation;
            fresh.then(|| frame_image(frame.texture)).flatten()
        };
        if let Some(image) = image {
            window.global::<PanelState>().set_frame(image);
        }
        window.window().request_redraw();
    }

    pub(in crate::browser) fn present_panel_popup(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<PanelState>();
        let shown = {
            let mut panel = self.panel.borrow_mut();
            let Some(panel) = panel.as_mut() else { return };
            match panel.view.popup_frame() {
                Some((frame, rect)) => {
                    let fresh = panel.popup_generation != frame.generation;
                    panel.popup_generation = frame.generation;
                    Some((fresh.then(|| frame_image(frame.texture)).flatten(), rect))
                }
                None => {
                    panel.popup_generation = 0;
                    None
                }
            }
        };
        let Some((image, rect)) = shown else {
            state.set_popup_visible(false);
            return;
        };
        if let Some(image) = image {
            state.set_popup_frame(image);
        }
        state.set_popup_x(rect.x as f32);
        state.set_popup_y(rect.y as f32);
        state.set_popup_visible(true);
        window.window().request_redraw();
    }
}
