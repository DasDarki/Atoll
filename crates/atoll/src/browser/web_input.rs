use atoll_engine::{Modifiers, MouseButton, ZoomCommand};
use slint::ComponentHandle;
use slint::winit_030::EventResult;
use slint::winit_030::winit::event::{ElementState, Ime, KeyEvent};
use slint::winit_030::winit::keyboard::{Key, ModifiersState, NamedKey};

use super::Browser;
use crate::input::keyboard;
use crate::input::shortcuts::{self, Shortcut};
use crate::ui::{BrowserState, PointerAction, PointerButton, WebPointer, WebScroll};

/// Where keyboard input currently goes, as reported by the user interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardFocus {
    pub page: bool,
    pub dev_tools: bool,
    pub modal: bool,
}

/// The engine view that receives keys, input method text and clipboard shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum InputTarget {
    #[default]
    Page,
    DevTools,
    /// A panel an extension brought along.
    Panel,
}

pub(super) fn modifiers_of(shift: bool, control: bool, alt: bool, meta: bool) -> Modifiers {
    Modifiers {
        shift,
        control,
        alt,
        meta,
        ..Modifiers::default()
    }
}

impl Browser {
    pub fn web_pointer(&self, pointer: WebPointer) {
        let modifiers = modifiers_of(pointer.shift, pointer.control, pointer.alt, pointer.meta);
        let (x, y) = (pointer.x.round() as i32, pointer.y.round() as i32);

        if pointer.action == PointerAction::Down {
            self.cancel_context_menu();
            match pointer.button {
                PointerButton::Back => return self.go_back(),
                PointerButton::Forward => return self.go_forward(),
                _ => {}
            }
        }
        let button = match pointer.button {
            PointerButton::Left => Some(MouseButton::Left),
            PointerButton::Middle => Some(MouseButton::Middle),
            PointerButton::Right => Some(MouseButton::Right),
            _ => None,
        };

        self.with_live_view(|view| match (pointer.action, button) {
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

    /// Runs `action` with the view that currently receives keyboard input.
    pub(super) fn with_input_view(&self, action: impl FnOnce(&atoll_engine::BrowserView)) {
        match self.input_target.get() {
            InputTarget::Page => self.with_live_view(action),
            InputTarget::DevTools => self.with_live_dev_tools(action),
            InputTarget::Panel => self.with_live_panel(action),
        }
    }

    fn has_input_view(&self) -> bool {
        let mut present = false;
        self.with_input_view(|_| present = true);
        present
    }

    pub fn web_scroll(&self, scroll: WebScroll) -> bool {
        let modifiers = modifiers_of(scroll.shift, scroll.control, scroll.alt, scroll.meta);
        let mut handled = false;
        self.with_live_view(|view| {
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

    pub fn web_hover(&self, inside: bool, x: f32, y: f32) {
        if !inside {
            self.with_live_view(|view| {
                view.mouse_leave(x.round() as i32, y.round() as i32, Modifiers::default())
            });
        }
    }

    /// Forwards keyboard focus changes of the page area to the engine. Losing focus is ignored
    /// while a context menu is open, because the engine cancels the menu when its browser loses
    /// focus, and the menu takes the focus as soon as it opens.
    pub fn web_focus_changed(&self, focused: bool) {
        if focused {
            self.input_target.set(InputTarget::Page);
        } else if self.context_menu.get().is_some() {
            return;
        }
        self.with_live_view(|view| view.set_focus(focused));
    }

    /// Handles a key event before the user interface sees it: shortcuts first, then the page.
    pub fn handle_key(
        &self,
        event: &KeyEvent,
        modifiers: ModifiersState,
        focus: KeyboardFocus,
    ) -> EventResult {
        if focus.modal {
            return EventResult::Propagate;
        }
        if let Some(shortcut) = shortcuts::detect(event, modifiers)
            && (!focus.dev_tools || shortcut.applies_in_dev_tools())
        {
            if !event.repeat || shortcut.repeats() {
                self.run_shortcut(shortcut);
            }
            return EventResult::PreventDefault;
        }
        if !event.repeat
            && let Some(keys) = shortcuts::describe(event, modifiers)
            && self.run_plugin_shortcut(&keys)
        {
            return EventResult::PreventDefault;
        }
        if !focus.page && !focus.dev_tools {
            return EventResult::Propagate;
        }
        self.input_target.set(if focus.dev_tools {
            InputTarget::DevTools
        } else {
            InputTarget::Page
        });
        if focus.page
            && event.state == ElementState::Pressed
            && event.logical_key == Key::Named(NamedKey::Escape)
            && self
                .window()
                .is_some_and(|window| window.global::<BrowserState>().get_fullscreen())
        {
            self.toggle_fullscreen();
        }
        if !self.has_input_view() {
            return EventResult::Propagate;
        }
        let translation = keyboard::translate(event, modifiers);
        self.deliver_keys(translation.events);
        if let Some(text) = &translation.commit {
            self.with_input_view(|view| view.commit_text(text));
        }
        EventResult::PreventDefault
    }

    pub fn handle_ime(&self, ime: &Ime, focus: KeyboardFocus) -> EventResult {
        if (!focus.page && !focus.dev_tools) || focus.modal {
            return EventResult::Propagate;
        }
        match ime {
            Ime::Preedit(text, _) => self.with_input_view(|view| {
                if text.is_empty() {
                    view.cancel_composition();
                } else {
                    view.set_composition(text);
                }
            }),
            Ime::Commit(text) => self.with_input_view(|view| view.commit_text(text)),
            Ime::Enabled | Ime::Disabled => {}
        }
        EventResult::PreventDefault
    }

    fn run_shortcut(&self, shortcut: Shortcut) {
        match shortcut {
            Shortcut::NewWindow => self.open_new_window(),
            Shortcut::CloseWindow => self.close_window(),
            Shortcut::Quit => self.quit(),
            Shortcut::NewTab => self.new_tab(),
            Shortcut::CloseTab => self.close_active_tab(),
            Shortcut::ReopenClosedTab => self.reopen_closed_tab(),
            Shortcut::FocusAddress => self.focus_address(),
            Shortcut::Reload => self.reload(false),
            Shortcut::ReloadIgnoringCache => self.reload(true),
            Shortcut::Back => self.go_back(),
            Shortcut::Forward => self.go_forward(),
            Shortcut::NextTab => self.cycle_tabs(1),
            Shortcut::PreviousTab => self.cycle_tabs(-1),
            Shortcut::Find => self.open_find(),
            Shortcut::ZoomIn => self.zoom(ZoomCommand::In),
            Shortcut::ZoomOut => self.zoom(ZoomCommand::Out),
            Shortcut::ZoomReset => self.zoom(ZoomCommand::Reset),
            Shortcut::DevTools => self.toggle_dev_tools(),
            Shortcut::ToggleFullscreen => self.toggle_fullscreen(),
            Shortcut::ToggleSidebar => self.toggle_sidebar(),
            Shortcut::BookmarkPage => self.bookmark_page(),
            Shortcut::ToggleBookmarksBar => self.toggle_bookmarks_bar(),
            Shortcut::ManageBookmarks => self.open_bookmark_manager(),
            Shortcut::QuickSearch => self.open_palette(false),
            Shortcut::Commands => self.open_palette(true),
            Shortcut::Settings => self.open_settings(),
            Shortcut::Home => self.go_home(),
            Shortcut::Downloads => self.open_downloads(),
            Shortcut::History => self.open_history(),
            Shortcut::NewPrivateTab => self.new_private_tab(),
            Shortcut::Passwords => self.show_passwords(),
            Shortcut::DownloadVideo => self.download_video(),
        }
    }
}
