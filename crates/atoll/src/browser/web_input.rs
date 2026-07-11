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
    pub modal: bool,
}

impl Browser {
    pub fn web_pointer(&self, pointer: WebPointer) {
        let modifiers = Modifiers {
            shift: pointer.shift,
            control: pointer.control,
            alt: pointer.alt,
            meta: pointer.meta,
            ..Modifiers::default()
        };
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

    fn has_live_view(&self) -> bool {
        let mut present = false;
        self.with_live_view(|_| present = true);
        present
    }

    pub fn web_scroll(&self, scroll: WebScroll) -> bool {
        let modifiers = Modifiers {
            shift: scroll.shift,
            control: scroll.control,
            alt: scroll.alt,
            meta: scroll.meta,
            ..Modifiers::default()
        };
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
        if !focused && self.context_menu.get().is_some() {
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
        if let Some(shortcut) = shortcuts::detect(event, modifiers) {
            self.run_shortcut(shortcut);
            return EventResult::PreventDefault;
        }
        if !focus.page {
            return EventResult::Propagate;
        }
        if event.state == ElementState::Pressed
            && event.logical_key == Key::Named(NamedKey::Escape)
            && self
                .window()
                .is_some_and(|window| window.global::<BrowserState>().get_fullscreen())
        {
            self.toggle_fullscreen();
        }
        if !self.has_live_view() {
            return EventResult::Propagate;
        }
        let translation = keyboard::translate(event, modifiers);
        self.deliver_keys(translation.events);
        if let Some(text) = &translation.commit {
            self.with_live_view(|view| view.commit_text(text));
        }
        EventResult::PreventDefault
    }

    pub fn handle_ime(&self, ime: &Ime, focus: KeyboardFocus) -> EventResult {
        if !focus.page || focus.modal {
            return EventResult::Propagate;
        }
        match ime {
            Ime::Preedit(text, _) => self.with_live_view(|view| {
                if text.is_empty() {
                    view.cancel_composition();
                } else {
                    view.set_composition(text);
                }
            }),
            Ime::Commit(text) => self.with_live_view(|view| view.commit_text(text)),
            Ime::Enabled | Ime::Disabled => {}
        }
        EventResult::PreventDefault
    }

    fn run_shortcut(&self, shortcut: Shortcut) {
        match shortcut {
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
            Shortcut::DevTools => self.show_dev_tools(),
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
        }
    }
}
