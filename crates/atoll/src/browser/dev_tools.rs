use atoll_core::TabId;
use atoll_engine::{BrowserView, CursorKind, Modifiers, MouseButton, ViewGeometry, ViewId};
use slint::{ComponentHandle, Image};

use super::Browser;
use super::page::web_cursor;
use super::preferences::PreferenceKey;
use super::views::DevTools;
use super::web_input::{InputTarget, modifiers_of};
use crate::bridge::images::frame_image;
use crate::ui::{DevToolsState, PointerAction, PointerButton, WebPointer, WebScroll};

/// What the developer tools panel currently displays.
#[derive(Default)]
pub struct DevToolsPresentation {
    view: Option<ViewId>,
    frame_generation: u64,
    popup_generation: u64,
}

impl Browser {
    /// Applies the stored dock side and size to the panel.
    pub(super) fn restore_dev_tools_layout(&self) {
        let Some(window) = self.window() else { return };
        let preferences = self.preferences.borrow();
        let state = window.global::<DevToolsState>();
        state.set_docked_right(preferences.dev_tools_docked_right);
        state.set_size(preferences.dev_tools_size as f32);
    }

    /// F12, Ctrl+Shift+I and the menu: opens or closes the developer tools of the active tab.
    pub fn toggle_dev_tools(&self) {
        let Some(tab) = self.session.borrow().active_tab else {
            return;
        };
        let open = self
            .views
            .borrow()
            .get(tab)
            .is_some_and(|entry| entry.dev_tools.is_some());
        if open {
            self.close_dev_tools(tab);
        } else {
            self.open_dev_tools(tab, None);
        }
    }

    /// "Inspect" in the context menu of the page.
    pub(super) fn inspect_element(&self, view: ViewId, x: i32, y: i32) {
        if !self.is_live_view(view) {
            return;
        }
        if let Some(tab) = self.session.borrow().active_tab {
            self.open_dev_tools(tab, Some((x, y)));
        }
    }

    fn open_dev_tools(&self, tab: TabId, inspect_at: Option<(i32, i32)>) {
        let opened = {
            let views = self.views.borrow();
            let Some(entry) = views.get(tab) else {
                return;
            };
            if let Some(dev_tools) = &entry.dev_tools {
                if let Some((x, y)) = inspect_at {
                    dev_tools.session.inspect(x, y);
                }
                None
            } else {
                let engine = self.engine.borrow();
                let Some(engine) = engine.as_ref() else {
                    return;
                };
                Some(engine.open_dev_tools(&entry.view, self.dev_tools_geometry.get()))
            }
        };
        match opened {
            Some(Ok(session)) => {
                if let Some((x, y)) = inspect_at {
                    session.inspect(x, y);
                }
                self.views
                    .borrow_mut()
                    .attach_dev_tools(tab, DevTools::new(session));
            }
            Some(Err(error)) => {
                self.report("Entwicklertools öffnen", Err::<(), _>(error));
                return;
            }
            None => {}
        }
        self.sync_page();
        self.focus_dev_tools();
    }

    pub(super) fn close_dev_tools(&self, tab: TabId) {
        let detached = self.views.borrow_mut().detach_dev_tools(tab);
        if let Some(dev_tools) = detached {
            dev_tools.view().close();
            self.sync_page();
            self.focus_content();
        }
    }

    /// The close button of the panel.
    pub fn close_active_dev_tools(&self) {
        if let Some(tab) = self.session.borrow().active_tab {
            self.close_dev_tools(tab);
        }
    }

    /// Called when the engine closed a developer tools view on its own.
    pub(super) fn dev_tools_closed(&self, tab: TabId) {
        self.views.borrow_mut().detach_dev_tools(tab);
        self.sync_page();
    }

    /// Replaces developer tools whose shared texture import failed by CPU rendered ones.
    pub(super) fn reopen_dev_tools(&self, tab: TabId) {
        let detached = self.views.borrow_mut().detach_dev_tools(tab);
        if let Some(dev_tools) = detached {
            dev_tools.view().close();
            self.open_dev_tools(tab, None);
        }
    }

    pub fn toggle_dev_tools_dock(&self) {
        let docked_right = !self.preferences.borrow().dev_tools_docked_right;
        self.update_preference(PreferenceKey::DevToolsDockedRight, |preferences| {
            preferences.dev_tools_docked_right = docked_right;
        });
        self.restore_dev_tools_layout();
    }

    /// The splitter was released at `size` logical pixels.
    pub fn dev_tools_size_committed(&self, size: f32) {
        let size = size.round().max(1.0) as u32;
        self.update_preference(PreferenceKey::DevToolsSize, |preferences| {
            preferences.dev_tools_size = size;
        });
    }

    pub fn dev_tools_resized(&self, width: f32, height: f32) {
        let geometry = ViewGeometry {
            width: width.round().max(1.0) as i32,
            height: height.round().max(1.0) as i32,
            scale: self.geometry.get().scale,
        };
        self.set_dev_tools_geometry(geometry);
    }

    pub(super) fn set_dev_tools_geometry(&self, geometry: ViewGeometry) {
        if self.dev_tools_geometry.replace(geometry) == geometry {
            return;
        }
        for (_, entry) in self.views.borrow().iter() {
            if let Some(dev_tools) = &entry.dev_tools {
                dev_tools.view().resize(geometry);
            }
        }
    }

    pub fn dev_tools_pointer(&self, pointer: WebPointer) {
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
        self.with_live_dev_tools(|view| match (pointer.action, button) {
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
        if pointer.action == PointerAction::Up && pointer.button == PointerButton::Left {
            self.schedule_clipboard_read();
        }
    }

    pub fn dev_tools_scroll(&self, scroll: WebScroll) -> bool {
        let modifiers = modifiers_of(scroll.shift, scroll.control, scroll.alt, scroll.meta);
        let mut handled = false;
        self.with_live_dev_tools(|view| {
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

    pub fn dev_tools_hover(&self, inside: bool, x: f32, y: f32) {
        if !inside {
            self.with_live_dev_tools(|view| {
                view.mouse_leave(x.round() as i32, y.round() as i32, Modifiers::default())
            });
        }
    }

    /// See [`Browser::web_focus_changed`].
    pub fn dev_tools_focus_changed(&self, focused: bool) {
        if focused {
            self.input_target.set(InputTarget::DevTools);
        } else if self.context_menu.get().is_some() {
            return;
        }
        self.with_live_dev_tools(|view| view.set_focus(focused));
    }

    pub(super) fn with_live_dev_tools(&self, action: impl FnOnce(&BrowserView)) {
        let active = self.session.borrow().active_tab;
        let views = self.views.borrow();
        if let Some(dev_tools) = active
            .and_then(|tab| views.get(tab))
            .and_then(|entry| entry.dev_tools.as_ref())
        {
            action(dev_tools.view());
        }
    }

    pub(super) fn is_live_dev_tools(&self, view: ViewId) -> bool {
        self.dev_tools_presentation.borrow().view == Some(view)
    }

    pub(super) fn set_dev_tools_cursor(&self, tab: TabId, view: ViewId, cursor: CursorKind) {
        if let Some(dev_tools) = self
            .views
            .borrow_mut()
            .get_mut(tab)
            .and_then(|entry| entry.dev_tools.as_mut())
        {
            dev_tools.cursor = cursor;
        }
        if self.is_live_dev_tools(view)
            && let Some(window) = self.window()
        {
            window
                .global::<DevToolsState>()
                .set_cursor(web_cursor(cursor));
        }
    }

    /// Moves keyboard focus into the developer tools once the panel is laid out.
    fn focus_dev_tools(&self) {
        let window = self.window.clone();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(window) = window.upgrade()
                && !window.get_modal_open()
            {
                window.invoke_focus_dev_tools();
            }
        });
    }

    /// Shows the developer tools of the active tab, or hides the panel if it has none.
    pub(super) fn sync_dev_tools(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<DevToolsState>();
        let (view, cursor) = {
            let session = self.session.borrow();
            let views = self.views.borrow();
            let dev_tools = session
                .active_tab
                .and_then(|tab| views.get(tab))
                .and_then(|entry| entry.dev_tools.as_ref());
            (
                dev_tools.map(|dev_tools| dev_tools.view().id()),
                dev_tools.map_or(CursorKind::Default, |dev_tools| dev_tools.cursor),
            )
        };
        state.set_visible(view.is_some());
        state.set_cursor(web_cursor(cursor));
        let changed = {
            let mut presentation = self.dev_tools_presentation.borrow_mut();
            let changed = presentation.view != view;
            if changed {
                *presentation = DevToolsPresentation {
                    view,
                    ..DevToolsPresentation::default()
                };
            }
            changed
        };
        if changed {
            state.set_frame(Image::default());
            state.set_popup_visible(false);
            if self.input_target.get() == InputTarget::DevTools {
                self.input_target.set(InputTarget::Page);
            }
        }
        self.present_dev_tools_frame();
        self.present_dev_tools_popup();
    }

    pub(super) fn present_dev_tools_frame(&self) {
        let Some(window) = self.window() else { return };
        let mut frame = None;
        self.with_live_dev_tools(|view| frame = view.frame());
        let Some(frame) = frame else { return };
        let fresh = {
            let mut presentation = self.dev_tools_presentation.borrow_mut();
            let fresh = presentation.frame_generation != frame.generation;
            presentation.frame_generation = frame.generation;
            fresh
        };
        if fresh && let Some(image) = frame_image(frame.texture) {
            window.global::<DevToolsState>().set_frame(image);
        }
        window.window().request_redraw();
    }

    pub(super) fn present_dev_tools_popup(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<DevToolsState>();
        let mut popup = None;
        self.with_live_dev_tools(|view| popup = view.popup_frame());
        let Some((frame, rect)) = popup else {
            state.set_popup_visible(false);
            self.dev_tools_presentation.borrow_mut().popup_generation = 0;
            return;
        };
        let fresh = {
            let mut presentation = self.dev_tools_presentation.borrow_mut();
            let fresh = presentation.popup_generation != frame.generation;
            presentation.popup_generation = frame.generation;
            fresh
        };
        if fresh && let Some(image) = frame_image(frame.texture) {
            state.set_popup_frame(image);
        }
        state.set_popup_x(rect.x as f32);
        state.set_popup_y(rect.y as f32);
        state.set_popup_visible(true);
        window.window().request_redraw();
    }
}
