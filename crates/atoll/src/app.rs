use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use atoll_engine::GpuContext;
use atoll_storage::Storage;
use slint::wgpu_30::WGPUConfiguration;
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, RenderingState};

use crate::bridge::{bindings, window_events};
use crate::browser::Browser;
use crate::platform::paths::Paths;
use crate::ui::AppWindow;

const DEFAULT_FRAME_RATE: u32 = 60;

/// Starts Atoll and runs until the window is closed.
pub fn run() -> Result<()> {
    let launched = Instant::now();
    let paths = Paths::resolve()?;
    let storage = Storage::open(&paths.database).with_context(|| {
        format!(
            "Datenbank {} kann nicht geöffnet werden",
            paths.database.display()
        )
    })?;
    tracing::debug!(elapsed = ?launched.elapsed(), "database open");
    let gpu = Rc::new(GpuContext::create().context("keine geeignete Grafikkarte gefunden")?);
    tracing::debug!(elapsed = ?launched.elapsed(), "graphics device ready");

    slint::BackendSelector::new()
        .backend_name("winit".into())
        .require_wgpu_30(WGPUConfiguration::Manual {
            instance: gpu.instance.clone(),
            adapter: gpu.adapter.clone(),
            device: gpu.device.clone(),
            queue: gpu.queue.clone(),
        })
        .select()
        .context("die Oberfläche kann nicht initialisiert werden")?;

    let window = AppWindow::new()?;
    tracing::debug!(elapsed = ?launched.elapsed(), "window created");
    let browser = Browser::new(&window, storage, gpu, paths)?;
    bindings::connect(&window, &browser);
    window_events::install(&window, &browser);
    browser.show_session();
    tracing::debug!(elapsed = ?launched.elapsed(), "session shown");
    start_engine_after_first_frame(&window, &browser, launched);

    window.run()?;
    browser.shutdown();
    Ok(())
}

/// The engine starts only after the first frame is visible, so the window never waits for it.
fn start_engine_after_first_frame(window: &AppWindow, browser: &Rc<Browser>, launched: Instant) {
    let browser = Rc::downgrade(browser);
    let handle = window.as_weak();
    let mut pending = true;
    let registered = window.window().set_rendering_notifier(move |state, _| {
        if !pending || !matches!(state, RenderingState::AfterRendering) {
            return;
        }
        pending = false;
        tracing::info!(elapsed = ?launched.elapsed(), "first frame on screen");
        let browser = browser.clone();
        let handle = handle.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            let frame_rate = handle
                .upgrade()
                .and_then(|window| monitor_frame_rate(&window))
                .unwrap_or(DEFAULT_FRAME_RATE);
            if let Some(browser) = browser.upgrade() {
                browser.start_engine(frame_rate);
            }
        });
    });
    if let Err(error) = registered {
        tracing::error!(%error, "rendering notifier unavailable");
    }
}

fn monitor_frame_rate(window: &AppWindow) -> Option<u32> {
    window
        .window()
        .with_winit_window(|winit| {
            winit
                .current_monitor()
                .and_then(|monitor| monitor.refresh_rate_millihertz())
        })
        .flatten()
        .map(|millihertz| millihertz.div_ceil(1000))
}
