use atoll_core::{LifecycleCommand, TabId};
use atoll_engine::ViewGeometry;
use atoll_storage::prelude::*;

use super::Browser;
use super::views::TabView;

impl Browser {
    /// Carries out the commands of a lifecycle transition on the engine.
    pub(super) fn apply(&self, commands: Vec<LifecycleCommand>) {
        let mut unloaded_elsewhere = Vec::new();
        for command in commands {
            match command {
                LifecycleCommand::Wake(tab) => self.wake(tab),
                LifecycleCommand::Show(tab) => {
                    if let Some(entry) = self.views.borrow().get(tab) {
                        entry.view.resize(self.geometry.get());
                        entry.view.set_hidden(false);
                        entry.view.set_focus(true);
                        if let Some(dev_tools) = &entry.dev_tools {
                            dev_tools.view().resize(self.dev_tools_geometry.get());
                            dev_tools.view().set_hidden(false);
                        }
                    }
                }
                LifecycleCommand::Hide(tab) => {
                    if let Some(entry) = self.views.borrow().get(tab) {
                        entry.view.capture_lost();
                        entry.view.set_focus(false);
                        entry.view.set_hidden(true);
                        if let Some(dev_tools) = &entry.dev_tools {
                            dev_tools.view().capture_lost();
                            dev_tools.view().set_focus(false);
                            dev_tools.view().set_hidden(true);
                        }
                    }
                }
                LifecycleCommand::Unload(tab) => {
                    self.unload(tab);
                    if !self.session.borrow().contains_tab(tab) {
                        unloaded_elsewhere.push(tab);
                    }
                }
            }
        }
        if unloaded_elsewhere.is_empty() {
            return;
        }
        self.for_other_windows(|other| {
            let affected = {
                let session = other.session.borrow();
                unloaded_elsewhere
                    .iter()
                    .any(|tab| session.contains_tab(*tab))
            };
            if affected {
                other.refresh_sidebar();
            }
        });
    }

    fn wake(&self, tab: TabId) {
        if self.views.borrow().contains(tab) {
            return;
        }
        let record = self
            .session
            .borrow()
            .tab(tab)
            .cloned()
            .or_else(|| self.storage.tab(tab).ok());
        let Some(record) = record else { return };
        let engine = self.engine.borrow();
        let Some(engine) = engine.as_ref() else {
            return;
        };
        match engine.create_view(&record.url, self.geometry.get(), false) {
            Ok(view) => {
                let pending_scroll = record.scroll_y.filter(|offset| *offset > 0);
                self.views
                    .borrow_mut()
                    .insert(tab, TabView::new(view, pending_scroll));
            }
            Err(error) => {
                tracing::error!(%error, tab = %tab, "engine instance could not be created");
                self.lifecycle.borrow_mut().forget(tab);
            }
        }
    }

    fn unload(&self, tab: TabId) {
        let Some(entry) = self.views.borrow_mut().remove(tab) else {
            return;
        };
        let (_, offset) = entry.view.scroll_offset();
        let offset = (offset > 0.5).then_some(offset.round() as i32);
        if let Err(error) = self.storage.set_tab_scroll(tab, offset) {
            tracing::warn!(%error, "scroll position could not be stored");
        }
        if let Some(dev_tools) = &entry.dev_tools {
            dev_tools.view().close();
        }
        entry.view.close();
    }

    /// Updates the size of the page area, applied to every engine instance.
    pub fn resize_pages(&self, width: f32, height: f32) {
        let geometry = ViewGeometry {
            width: width.round().max(1.0) as i32,
            height: height.round().max(1.0) as i32,
            scale: self.geometry.get().scale,
        };
        self.set_geometry(geometry);
    }

    pub fn set_scale_factor(&self, scale: f32) {
        let geometry = ViewGeometry {
            scale,
            ..self.geometry.get()
        };
        self.set_geometry(geometry);
        self.set_dev_tools_geometry(ViewGeometry {
            scale,
            ..self.dev_tools_geometry.get()
        });
    }

    fn set_geometry(&self, geometry: ViewGeometry) {
        if self.geometry.replace(geometry) == geometry {
            return;
        }
        let session = self.session.borrow();
        for (tab, entry) in self.views.borrow().iter() {
            if session.contains_tab(*tab) {
                entry.view.resize(geometry);
            }
        }
    }

    /// Frees the engine instance of every tab in `tabs`.
    pub(super) fn forget_tabs(&self, tabs: impl IntoIterator<Item = TabId>) {
        let mut commands = Vec::new();
        {
            let mut lifecycle = self.lifecycle.borrow_mut();
            for tab in tabs {
                commands.extend(lifecycle.forget(tab));
            }
        }
        self.apply(commands);
    }

    /// Persists the scroll position of every tab with an engine instance.
    pub(super) fn store_scroll_positions(&self) {
        for (tab, entry) in self.views.borrow().iter() {
            let (_, offset) = entry.view.scroll_offset();
            let offset = (offset > 0.5).then_some(offset.round() as i32);
            if let Err(error) = self.storage.set_tab_scroll(*tab, offset) {
                tracing::warn!(%error, "scroll position could not be stored");
            }
        }
    }
}
