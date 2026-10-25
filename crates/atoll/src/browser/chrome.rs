use std::time::{Duration, Instant};

use slint::platform::{PointerEventButton, WindowEvent};
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::ResizeDirection;
use slint::{ComponentHandle, LogicalPosition};

use super::Browser;
use crate::ui::BrowserState;

/// Two presses on the title area within this time toggle the maximized state.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

impl Browser {
    /// A press on the free space of the toolbar: moves the window, a double click maximizes it.
    pub fn window_drag(&self) {
        let Some(window) = self.window() else { return };
        let now = Instant::now();
        let double_click = self
            .last_title_press
            .replace(Some(now))
            .is_some_and(|previous| now.duration_since(previous) < DOUBLE_CLICK);
        if double_click {
            self.last_title_press.set(None);
            self.toggle_maximized();
            return;
        }
        let started = window
            .window()
            .with_winit_window(|winit| match winit.drag_window() {
                Ok(()) => true,
                Err(error) => {
                    tracing::debug!(%error, "window could not be moved");
                    false
                }
            });
        if started == Some(true) {
            self.release_pointer_after_handover();
        }
    }

    /// A press on one of the invisible borders: resizes the window from that edge.
    pub fn window_resize(&self, edge: &str) {
        let Some(direction) = resize_direction(edge) else {
            return;
        };
        let Some(window) = self.window() else { return };
        let started =
            window
                .window()
                .with_winit_window(|winit| match winit.drag_resize_window(direction) {
                    Ok(()) => true,
                    Err(error) => {
                        tracing::debug!(%error, "window could not be resized");
                        false
                    }
                });
        if started == Some(true) {
            self.release_pointer_after_handover();
        }
    }

    /// Moving and resizing is handed over to the compositor together with the pointer, and under
    /// Wayland the release of the button never reaches the window. The user interface would keep
    /// the pressed element as the target of every later click, so the release is sent to it
    /// once the current event is handled.
    fn release_pointer_after_handover(&self) {
        let window = self.window.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(window) = window.upgrade() {
                window
                    .window()
                    .dispatch_event(WindowEvent::PointerReleased {
                        position: LogicalPosition::new(-1.0, -1.0),
                        button: PointerEventButton::Left,
                    });
                window.window().dispatch_event(WindowEvent::PointerExited);
            }
        });
    }

    /// The minimize, maximize and close buttons.
    pub fn window_control(&self, action: &str) {
        let Some(window) = self.window() else { return };
        match action {
            "minimize" => {
                let handle = window.window();
                // Only a change is passed on to the window system, and on Wayland nobody says
                // that a window was brought back, so the state is set straight first. Without
                // this the button works once and never again.
                if handle.is_minimized() {
                    handle.set_minimized(false);
                }
                handle.set_minimized(true);
            }
            "maximize" => self.toggle_maximized(),
            "close" => self.close_window(),
            other => tracing::warn!(action = other, "unknown window action"),
        }
    }

    fn toggle_maximized(&self) {
        if let Some(window) = self.window() {
            let maximized = window.window().is_maximized();
            window.window().set_maximized(!maximized);
        }
    }

    /// Mirrors the maximized state into the window controls and the resize frame.
    pub fn window_state_changed(&self) {
        if let Some(window) = self.window() {
            let maximized = window.window().is_maximized();
            window.global::<BrowserState>().set_maximized(maximized);
        }
    }

    /// Closes this window. Closing the last one quits Atoll.
    pub fn close_window(&self) {
        if let Some(manager) = self.manager() {
            manager.close_window(self.id);
        }
    }

    /// Ctrl+Shift+Q: quits Atoll, every open window comes back on the next start.
    pub fn quit(&self) {
        if let Err(error) = slint::quit_event_loop() {
            tracing::warn!(%error, "event loop could not be stopped");
        }
    }

    /// Ctrl+N: opens a new window with a new workspace.
    pub fn open_new_window(&self) {
        if let Some(manager) = self.manager() {
            manager.open_new_window();
        }
    }

    pub fn pointer_seen(&self) {
        if let Some(manager) = self.manager() {
            manager.pointer_seen(self.id);
        }
    }

    pub fn window_focused(&self) {
        if let Some(manager) = self.manager() {
            manager.window_focused(self.id);
        }
    }
}

fn resize_direction(edge: &str) -> Option<ResizeDirection> {
    Some(match edge {
        "n" => ResizeDirection::North,
        "s" => ResizeDirection::South,
        "w" => ResizeDirection::West,
        "e" => ResizeDirection::East,
        "nw" => ResizeDirection::NorthWest,
        "ne" => ResizeDirection::NorthEast,
        "sw" => ResizeDirection::SouthWest,
        "se" => ResizeDirection::SouthEast,
        _ => return None,
    })
}
