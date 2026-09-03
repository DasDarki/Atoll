use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use atoll_engine::GpuContext;
use atoll_storage::Storage;
use slint::wgpu_30::WGPUConfiguration;
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, RenderingState};

use crate::browser::{Preferences, WindowManager};
use crate::platform::paths::Paths;
use crate::ui::AppWindow;
use atoll_i18n::t;

const DEFAULT_FRAME_RATE: u32 = 60;

/// Tells the user interface which of the bundled translations to show. It only works once a
/// window exists, and German needs none because it is the language of the source.
pub fn select_interface_language(language: atoll_i18n::Language) {
    if let Err(error) = slint::select_bundled_translation(language.bundle()) {
        tracing::warn!(%error, language = language.tag(), "the language could not be selected");
    }
    slint::update_all_translations();
}

/// Starts Atoll and runs until the last window is closed.
pub fn run() -> Result<()> {
    let launched = Instant::now();
    let paths = Paths::resolve()?;
    let storage = Storage::open(&paths.database).with_context(|| {
        t!(
            "Datenbank {} kann nicht geöffnet werden",
            paths.database.display()
        )
    })?;
    tracing::debug!(elapsed = ?launched.elapsed(), "database open");
    atoll_i18n::set_language(Preferences::load(&storage).language());
    let gpu = Rc::new(GpuContext::create().context(t!("keine geeignete Grafikkarte gefunden"))?);
    let gpu_handle = gpu.clone();
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
        .context(t!("die Oberfläche kann nicht initialisiert werden"))?;

    let manager = WindowManager::new(storage, gpu, paths);
    let first = manager.restore_windows()?;
    select_interface_language(atoll_i18n::language());
    tracing::debug!(elapsed = ?launched.elapsed(), "windows shown");
    start_engine_after_first_frame(&first, &manager, launched);

    slint::run_event_loop()?;
    manager.shutdown();

    // Slint releases its graphics device only while thread locals are destroyed, and dropping the
    // last queue reference at that point panics inside wgpu. Keeping one reference until the
    // process ends avoids that release entirely.
    std::mem::forget(gpu_handle);
    Ok(())
}

/// The engine starts only after the first frame is visible, so the window never waits for it.
fn start_engine_after_first_frame(
    window: &AppWindow,
    manager: &Rc<WindowManager>,
    launched: Instant,
) {
    let manager = Rc::downgrade(manager);
    let handle = window.as_weak();
    let mut pending = true;
    let registered = window.window().set_rendering_notifier(move |state, _| {
        if !pending || !matches!(state, RenderingState::AfterRendering) {
            return;
        }
        pending = false;
        tracing::info!(elapsed = ?launched.elapsed(), "first frame on screen");
        let manager = manager.clone();
        let handle = handle.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            let frame_rate = handle
                .upgrade()
                .and_then(|window| monitor_frame_rate(&window))
                .unwrap_or(DEFAULT_FRAME_RATE);
            if let Some(manager) = manager.upgrade() {
                manager.start_engine(frame_rate);
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
