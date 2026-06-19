use std::path::PathBuf;
use std::time::Duration;

/// Everything needed to start the engine.
pub struct EngineConfig {
    /// Root directory for all engine profile data.
    pub root_cache_path: PathBuf,
    /// Profile directory inside `root_cache_path` holding cookies, cache and local storage.
    pub cache_path: PathBuf,
    /// Where downloads are stored.
    pub download_directory: PathBuf,
    /// UI locale such as `de`, `None` uses the system locale.
    pub locale: Option<String>,
    /// Upper bound for frames per second produced by a page.
    pub frame_rate: u32,
    /// Receive frames as shared GPU textures instead of CPU buffers where supported.
    pub accelerated_paint: bool,
    /// Keep the Chromium sandbox enabled for child processes.
    pub sandbox: bool,
    /// Additional Chromium command line switches, written as `name` or `name=value`.
    pub extra_switches: Vec<String>,
    /// Invoked from any thread when the engine wants [`crate::Engine::do_message_loop_work`] to
    /// run on the main thread after the given delay.
    pub schedule_work: Box<dyn Fn(Duration) + Send + Sync>,
}
