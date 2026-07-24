use atoll_core::TabId;
use atoll_core::address::host_of;
use atoll_engine::{EngineEvent, OpenDisposition, ViewId};
use atoll_storage::prelude::*;
use slint::ComponentHandle;

use super::Browser;
use super::page::web_cursor;
use super::views::TabView;
use crate::ui::BrowserState;

impl Browser {
    /// Handles the engine events that concern this window.
    pub(super) fn process_events(&self, events: Vec<EngineEvent>) {
        let mut sidebar_dirty = false;
        let mut page_dirty = false;
        for event in events {
            let effect = self.handle_event(event);
            sidebar_dirty |= effect.sidebar;
            page_dirty |= effect.page;
        }
        if page_dirty {
            self.sync_page();
        }
        if sidebar_dirty {
            self.refresh_sidebar();
        }
    }

    fn handle_event(&self, event: EngineEvent) -> Effect {
        match event {
            EngineEvent::FrameReady(view) => {
                if self.is_live_view(view) {
                    self.present_frame();
                } else if self.is_live_dev_tools(view) {
                    self.present_dev_tools_frame();
                }
                Effect::NONE
            }
            EngineEvent::PopupWidgetChanged(view) => {
                if self.is_live_view(view) {
                    self.present_popup();
                } else if self.is_live_dev_tools(view) {
                    self.present_dev_tools_popup();
                }
                Effect::NONE
            }
            EngineEvent::AddressChanged { view, url } => self.address_changed(view, url),
            EngineEvent::TitleChanged { view, title } => self.title_changed(view, title),
            EngineEvent::FaviconReady {
                view,
                page_url,
                png,
            } => self.favicon_ready(view, &page_url, &png),
            EngineEvent::LoadingStateChanged { view, .. } => {
                let live = self.is_live_view(view);
                if let Some(tab) = self.tab_of(view)
                    && let Some(entry) = self.views.borrow_mut().get_mut(tab)
                    && !entry.view.is_loading()
                {
                    entry.progress = 1.0;
                }
                Effect {
                    sidebar: true,
                    page: live,
                }
            }
            EngineEvent::LoadProgress { view, progress } => {
                if let Some(tab) = self.tab_of(view)
                    && let Some(entry) = self.views.borrow_mut().get_mut(tab)
                {
                    entry.progress = progress;
                }
                if self.is_live_view(view)
                    && let Some(window) = self.window()
                {
                    window
                        .global::<BrowserState>()
                        .set_progress(progress as f32);
                }
                Effect::NONE
            }
            EngineEvent::MainFrameLoaded { view, .. } => {
                self.restore_scroll(view);
                Effect::NONE
            }
            EngineEvent::CursorChanged { view, cursor } => {
                if let Some(owner) = self.dev_tools_owner(view) {
                    self.set_dev_tools_cursor(owner, view, cursor);
                    return Effect::NONE;
                }
                if let Some(tab) = self.tab_of(view)
                    && let Some(entry) = self.views.borrow_mut().get_mut(tab)
                {
                    entry.cursor = cursor;
                }
                if self.is_live_view(view)
                    && let Some(window) = self.window()
                {
                    window
                        .global::<BrowserState>()
                        .set_cursor(web_cursor(cursor));
                }
                Effect::NONE
            }
            EngineEvent::StatusMessage { view, text } => {
                if let Some(tab) = self.tab_of(view)
                    && let Some(entry) = self.views.borrow_mut().get_mut(tab)
                {
                    entry.status = text.clone();
                }
                if self.is_live_view(view)
                    && let Some(window) = self.window()
                {
                    window.global::<BrowserState>().set_status_text(text.into());
                }
                Effect::NONE
            }
            EngineEvent::Tooltip { view, text } => {
                if self.is_live_view(view)
                    && let Some(window) = self.window()
                {
                    window.global::<BrowserState>().set_tooltip(text.into());
                }
                Effect::NONE
            }
            EngineEvent::FullscreenChanged { view, fullscreen } => {
                if self.is_live_view(view) {
                    self.set_page_fullscreen(fullscreen);
                }
                Effect::NONE
            }
            EngineEvent::OpenUrl {
                view,
                url,
                disposition,
            } => {
                let source = self.tab_of(view);
                let activate = disposition != OpenDisposition::BackgroundTab;
                self.open_in_new_tab(source, &url, activate);
                Effect::NONE
            }
            EngineEvent::PopupOpened { view, opener, url } => {
                self.adopt_popup(view, opener, &url);
                Effect::NONE
            }
            EngineEvent::ContextMenu { view, request } => {
                self.show_context_menu(view, request);
                Effect::NONE
            }
            EngineEvent::FileDialog { request, .. } => {
                self.show_file_dialog(request);
                Effect::NONE
            }
            EngineEvent::JsDialog { view, request } => {
                self.show_js_dialog(view, request);
                Effect::NONE
            }
            EngineEvent::JsDialogDismissed { view } => {
                self.dismiss_js_dialogs(view);
                Effect::NONE
            }
            EngineEvent::Download(download) => {
                self.download_updated(&download);
                Effect::NONE
            }
            EngineEvent::FindResult {
                view,
                matches,
                active_match,
                ..
            } => {
                if self.is_live_view(view)
                    && let Some(window) = self.window()
                {
                    let text = if matches == 0 {
                        "Keine Treffer".to_owned()
                    } else {
                        format!("{active_match}/{matches}")
                    };
                    window.global::<BrowserState>().set_find_result(text.into());
                }
                Effect::NONE
            }
            EngineEvent::CopyText(text) => {
                self.copy_text(&text);
                Effect::NONE
            }
            EngineEvent::ClipboardText(text) => {
                self.clipboard_text_read(text);
                Effect::NONE
            }
            EngineEvent::ClipboardWritten => {
                self.clipboard_written();
                Effect::NONE
            }
            EngineEvent::InspectElement { view, x, y } => {
                self.inspect_element(view, x, y);
                Effect::NONE
            }
            EngineEvent::CloseRequested(view) => {
                if let Some(owner) = self.dev_tools_owner(view) {
                    self.close_dev_tools(owner);
                    return Effect::NONE;
                }
                if let Some(tab) = self.tab_of(view) {
                    self.close_tab(tab);
                }
                Effect::NONE
            }
            EngineEvent::Closed(view) => {
                if let Some(owner) = self.dev_tools_owner(view) {
                    self.dev_tools_closed(owner);
                    return Effect::NONE;
                }
                self.view_closed(view)
            }
            EngineEvent::RenderProcessGone(view) => {
                if let Some(tab) = self.tab_of(view)
                    && let Some(entry) = self.views.borrow_mut().get_mut(tab)
                {
                    entry.crashed = true;
                }
                Effect {
                    sidebar: false,
                    page: self.is_live_view(view),
                }
            }
            EngineEvent::AcceleratedPaintFailed(view) => {
                if let Some(owner) = self.dev_tools_owner(view) {
                    self.reopen_dev_tools(owner);
                    return Effect::NONE;
                }
                self.recreate_view(view);
                Effect::NONE
            }
        }
    }

    fn tab_of(&self, view: ViewId) -> Option<TabId> {
        self.views.borrow().tab_of(view)
    }

    fn dev_tools_owner(&self, view: ViewId) -> Option<TabId> {
        self.views.borrow().dev_tools_owner(view)
    }

    fn address_changed(&self, view: ViewId, url: String) -> Effect {
        let Some(tab) = self.tab_of(view) else {
            return Effect::NONE;
        };
        if let Some(entry) = self.views.borrow_mut().get_mut(tab) {
            if entry.expected_address.as_deref() == Some(url.as_str()) {
                entry.expected_address = None;
            } else if entry.expected_address.is_none() {
                entry.replacement_history = false;
            }
        }
        let (changed, title) = {
            let mut session = self.session.borrow_mut();
            let Some(record) = session.tab_mut(tab) else {
                return Effect::NONE;
            };
            let changed = record.url != url;
            if changed {
                record.url = url.clone();
            }
            (changed, record.title.clone())
        };
        let stored = if changed {
            self.storage.update_tab_address(tab, &url)
        } else {
            Ok(())
        };
        let result = stored.and_then(|()| {
            self.storage
                .record_visit(tab, &url, title.as_deref(), Self::now())
        });
        if let Err(error) = result {
            tracing::warn!(%error, "address could not be stored");
        }
        Effect {
            sidebar: changed,
            page: self.is_live_view(view),
        }
    }

    fn title_changed(&self, view: ViewId, title: String) -> Effect {
        let Some(tab) = self.tab_of(view) else {
            return Effect::NONE;
        };
        let url = {
            let mut session = self.session.borrow_mut();
            let Some(record) = session.tab_mut(tab) else {
                return Effect::NONE;
            };
            if record.title.as_deref() == Some(title.as_str()) {
                return Effect::NONE;
            }
            record.title = Some(title.clone());
            record.url.clone()
        };
        let result = self
            .storage
            .update_tab_title(tab, Some(&title))
            .and_then(|()| self.storage.update_visit_title(tab, &url, &title));
        if let Err(error) = result {
            tracing::warn!(%error, "title could not be stored");
        }
        Effect {
            sidebar: true,
            page: self.is_live_view(view),
        }
    }

    fn favicon_ready(&self, view: ViewId, page_url: &str, png: &[u8]) -> Effect {
        let Some(tab) = self.tab_of(view) else {
            return Effect::NONE;
        };
        let Some(host) = host_of(page_url) else {
            return Effect::NONE;
        };
        let stored = self
            .storage
            .upsert_favicon(&host, png, Self::now())
            .and_then(|favicon| {
                self.storage
                    .set_tab_favicon(tab, Some(favicon))
                    .map(|()| favicon)
            });
        match stored {
            Ok(favicon) => {
                self.favicons.borrow_mut().insert(favicon, host, png);
                if let Some(record) = self.session.borrow_mut().tab_mut(tab) {
                    record.favicon_id = Some(favicon);
                }
                Effect {
                    sidebar: true,
                    page: false,
                }
            }
            Err(error) => {
                tracing::warn!(%error, "favicon could not be stored");
                Effect::NONE
            }
        }
    }

    fn restore_scroll(&self, view: ViewId) {
        let Some(tab) = self.tab_of(view) else { return };
        let mut views = self.views.borrow_mut();
        let Some(entry) = views.get_mut(tab) else {
            return;
        };
        let Some(offset) = entry.pending_scroll.take() else {
            return;
        };
        entry.view.execute_script(&format!(
            "(() => {{ const y = {offset}; const restore = () => window.scrollTo(0, y); restore(); \
             setTimeout(() => {{ if (Math.abs(window.scrollY - y) > 2) restore(); }}, 400); }})();"
        ));
    }

    fn adopt_popup(&self, view: ViewId, opener: ViewId, url: &str) {
        let adopted = self
            .engine
            .borrow()
            .as_ref()
            .and_then(|engine| engine.adopt_view(view));
        let Some(adopted) = adopted else { return };
        let source = self.tab_of(opener);
        let target = source.and_then(|tab| {
            let session = self.session.borrow();
            let island = session.tab(tab)?.island_id;
            let index = session
                .island(island)?
                .tabs
                .iter()
                .position(|candidate| candidate.id == tab)?;
            Some((island, index + 1))
        });
        let island = match target {
            Some((island, _)) => Some(island),
            None => self.ensure_island(),
        };
        let Some(island) = island else { return };
        let created = self.storage.create_tab(atoll_storage::NewTab {
            island_id: island,
            url,
            title: None,
            index: target.map(|(_, index)| index),
            created_at: Self::now(),
        });
        match created {
            Ok(tab) => {
                self.views
                    .borrow_mut()
                    .insert(tab.id, TabView::new(adopted, None));
                self.reload_session();
                self.activate_tab(tab.id);
            }
            Err(error) => self.report("Popup öffnen", Err::<(), _>(error)),
        }
    }

    fn view_closed(&self, view: ViewId) -> Effect {
        let Some(tab) = self.tab_of(view) else {
            return Effect::NONE;
        };
        let live = self.is_live_view(view);
        self.views.borrow_mut().remove(tab);
        self.lifecycle.borrow_mut().forget(tab);
        let pinned = self
            .session
            .borrow()
            .tab(tab)
            .is_some_and(|record| record.pinned);
        if pinned {
            self.lifecycle.borrow_mut().set_protected(tab, true);
        }
        Effect {
            sidebar: true,
            page: live,
        }
    }

    /// Replaces a view that failed to paint with shared textures by a CPU rendered one.
    fn recreate_view(&self, view: ViewId) {
        let Some(tab) = self.tab_of(view) else { return };
        let url = self
            .views
            .borrow()
            .get(tab)
            .and_then(|entry| entry.view.url());
        let Some(entry) = self.views.borrow_mut().remove(tab) else {
            return;
        };
        entry.view.close();
        let created = self.engine.borrow().as_ref().map(|engine| {
            let address = url.unwrap_or_default();
            engine.create_view(&address, self.geometry.get(), !self.is_live_view(view))
        });
        match created {
            Some(Ok(replacement)) => {
                self.views
                    .borrow_mut()
                    .insert(tab, TabView::new(replacement, None));
                self.sync_page();
            }
            Some(Err(error)) => tracing::error!(%error, "view could not be recreated"),
            None => {}
        }
    }
}

#[derive(Clone, Copy)]
struct Effect {
    sidebar: bool,
    page: bool,
}

impl Effect {
    const NONE: Effect = Effect {
        sidebar: false,
        page: false,
    };
}
