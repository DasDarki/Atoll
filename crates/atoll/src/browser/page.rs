use atoll_engine::{CursorKind, ViewId};
use slint::{ComponentHandle, Image};

use super::Browser;
use crate::bridge::images::frame_image;
use crate::ui::{BrowserState, WebCursor};

/// What the page area currently displays, to avoid re-wrapping textures on every frame.
#[derive(Default)]
pub struct PageState {
    view: Option<ViewId>,
    frame_generation: u64,
    popup_generation: u64,
}

impl Browser {
    /// Shows the active tab in the toolbar and the page area.
    pub(super) fn sync_page(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<BrowserState>();
        let session = self.session.borrow();
        let active = session.active_tab.and_then(|id| session.tab(id));

        let Some(tab) = active else {
            state.set_has_page(false);
            state.set_title("".into());
            state.set_address("".into());
            state.set_secure(false);
            state.set_can_go_back(false);
            state.set_can_go_forward(false);
            state.set_loading(false);
            state.set_crashed(false);
            state.set_status_text("".into());
            state.set_find_visible(false);
            state.set_page_frame(Image::default());
            state.set_popup_visible(false);
            *self.page.borrow_mut() = PageState::default();
            return;
        };

        let views = self.views.borrow();
        let entry = views.get(tab.id);
        let address = entry
            .and_then(|entry| entry.view.url())
            .unwrap_or_else(|| tab.url.clone());
        state.set_has_page(!tab.is_blank());
        state.set_title(tab.title.clone().unwrap_or_default().into());
        state.set_secure(address.starts_with("https://"));
        state.set_address(address.into());
        state.set_can_go_back(
            entry.is_some_and(|entry| entry.view.can_go_back())
                || self.has_replacement_history(tab.id),
        );
        state.set_can_go_forward(entry.is_some_and(|entry| entry.view.can_go_forward()));
        state.set_loading(entry.is_some_and(|entry| entry.view.is_loading()));
        state.set_progress(entry.map_or(0.0, |entry| entry.progress as f32));
        state.set_crashed(entry.is_some_and(|entry| entry.crashed));
        state.set_status_text(entry.map_or("", |entry| entry.status.as_str()).into());
        state.set_cursor(web_cursor(
            entry.map_or(CursorKind::Default, |entry| entry.cursor),
        ));

        let view = entry.map(|entry| entry.view.id());
        drop(views);
        drop(session);
        let changed_view = {
            let mut page = self.page.borrow_mut();
            let changed = page.view != view;
            if changed {
                *page = PageState {
                    view,
                    ..PageState::default()
                };
            }
            changed
        };
        if changed_view {
            state.set_page_frame(Image::default());
            state.set_popup_visible(false);
        }
        self.present_frame();
        self.present_popup();
    }

    /// Shows the newest frame of the live view.
    pub(super) fn present_frame(&self) {
        let Some(window) = self.window() else { return };
        let frame = {
            let session = self.session.borrow();
            let views = self.views.borrow();
            session
                .active_tab
                .and_then(|tab| views.get(tab))
                .and_then(|entry| entry.view.frame())
        };
        let Some(frame) = frame else { return };
        let fresh = {
            let mut page = self.page.borrow_mut();
            let fresh = page.frame_generation != frame.generation;
            page.frame_generation = frame.generation;
            fresh
        };
        if fresh && let Some(image) = frame_image(frame.texture) {
            window.global::<BrowserState>().set_page_frame(image);
        }
        window.window().request_redraw();
    }

    /// Shows or hides page widgets such as an open `<select>` of the live view.
    pub(super) fn present_popup(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<BrowserState>();
        let popup = {
            let session = self.session.borrow();
            let views = self.views.borrow();
            session
                .active_tab
                .and_then(|tab| views.get(tab))
                .and_then(|entry| entry.view.popup_frame())
        };
        let Some((frame, rect)) = popup else {
            state.set_popup_visible(false);
            self.page.borrow_mut().popup_generation = 0;
            return;
        };
        let fresh = {
            let mut page = self.page.borrow_mut();
            let fresh = page.popup_generation != frame.generation;
            page.popup_generation = frame.generation;
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

    pub(super) fn is_live_view(&self, view: ViewId) -> bool {
        self.page.borrow().view == Some(view)
    }
}

pub(super) fn web_cursor(cursor: CursorKind) -> WebCursor {
    match cursor {
        CursorKind::Default => WebCursor::Default,
        CursorKind::Pointer => WebCursor::Pointer,
        CursorKind::Text => WebCursor::Text,
        CursorKind::Wait => WebCursor::Wait,
        CursorKind::Progress => WebCursor::Progress,
        CursorKind::Help => WebCursor::Help,
        CursorKind::Crosshair => WebCursor::Crosshair,
        CursorKind::Move => WebCursor::Move,
        CursorKind::NotAllowed => WebCursor::NotAllowed,
        CursorKind::NoDrop => WebCursor::NoDrop,
        CursorKind::Grab => WebCursor::Grab,
        CursorKind::Grabbing => WebCursor::Grabbing,
        CursorKind::Copy => WebCursor::Copy,
        CursorKind::Alias => WebCursor::Alias,
        CursorKind::ResizeEastWest => WebCursor::ResizeEw,
        CursorKind::ResizeNorthSouth => WebCursor::ResizeNs,
        CursorKind::ResizeNorthEastSouthWest => WebCursor::ResizeNesw,
        CursorKind::ResizeNorthWestSouthEast => WebCursor::ResizeNwse,
        CursorKind::ResizeColumn => WebCursor::ResizeColumn,
        CursorKind::ResizeRow => WebCursor::ResizeRow,
        CursorKind::None => WebCursor::None,
    }
}
