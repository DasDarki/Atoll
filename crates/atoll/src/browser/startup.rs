use std::time::Instant;

use atoll_engine::{Engine, EngineConfig};

use super::Browser;
use crate::pump::MessagePump;

impl Browser {
    /// Shows the stored session. Only the database is read, no engine instance is created yet.
    pub fn show_session(&self) {
        self.restore_layout();
        self.restore_downloads();
        self.refresh_bookmarks();
        self.show_remembered_tab();
    }

    /// Starts the engine and wakes the active tab. Called once the first frame is on screen.
    pub fn start_engine(&self, frame_rate: u32) {
        if self.is_engine_running() {
            return;
        }
        let started = Instant::now();
        let config = EngineConfig {
            root_cache_path: self.paths.engine_root.clone(),
            cache_path: self.paths.engine_profile.clone(),
            download_directory: self.download_directory(),
            ask_download_location: self.preferences.borrow().download_ask,
            locale: None,
            frame_rate,
            accelerated_paint: std::env::var_os("ATOLL_SOFTWARE_RENDERING").is_none(),
            sandbox: std::env::var_os("ATOLL_NO_SANDBOX").is_none(),
            extra_switches: std::env::var("ATOLL_CHROMIUM_FLAGS")
                .map(|flags| flags.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default(),
            schedule_work: MessagePump::scheduler(),
        };
        match Engine::initialize(config, self.gpu.clone()) {
            Ok(engine) => {
                *self.engine.borrow_mut() = Some(engine);
                tracing::info!(elapsed = ?started.elapsed(), "engine started");
            }
            Err(error) => {
                tracing::error!(%error, "engine could not be started");
                self.show_toast("Die Browser-Engine konnte nicht gestartet werden.");
                return;
            }
        }

        self.start_clipboard_bridge();

        let this = self.this();
        *self.pump.borrow_mut() = Some(MessagePump::start(move || {
            if let Some(browser) = this.upgrade() {
                browser.pump();
            }
        }));

        let pinned: Vec<_> = self
            .session
            .borrow()
            .tabs()
            .filter(|tab| tab.pinned)
            .map(|tab| tab.id)
            .collect();
        for tab in pinned {
            self.lifecycle.borrow_mut().set_protected(tab, true);
        }

        let active = self.session.borrow().active_tab;
        if let Some(tab) = active {
            self.activate_tab(tab);
        }
    }

    /// Stores what is only known at runtime and shuts the engine down.
    pub fn shutdown(&self) {
        self.pump.borrow_mut().take();
        self.cancel_context_menu();
        self.stop_clipboard_bridge();
        self.store_scroll_positions();
        for (_, entry) in self.views.borrow_mut().drain() {
            entry.view.close();
        }
        if let Some(engine) = self.engine.borrow_mut().take() {
            engine.shutdown();
        }
    }
}
