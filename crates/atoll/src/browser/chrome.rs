use std::time::{Duration, Instant};

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::ResizeDirection;

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
        window.window().with_winit_window(|winit| {
            if let Err(error) = winit.drag_window() {
                tracing::debug!(%error, "window could not be moved");
            }
        });
    }

    /// A press on one of the invisible borders: resizes the window from that edge.
    pub fn window_resize(&self, edge: &str) {
        let Some(direction) = resize_direction(edge) else {
            return;
        };
        let Some(window) = self.window() else { return };
        window.window().with_winit_window(|winit| {
            if let Err(error) = winit.drag_resize_window(direction) {
                tracing::debug!(%error, "window could not be resized");
            }
        });
    }

    /// The minimize, maximize and close buttons.
    pub fn window_control(&self, action: &str) {
        let Some(window) = self.window() else { return };
        match action {
            "minimize" => window.window().set_minimized(true),
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

    pub fn close_window(&self) {
        if let Some(window) = self.window()
            && let Err(error) = window.hide()
        {
            tracing::warn!(%error, "window could not be closed");
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
