use cef::args::Args;

use crate::app;

/// Runs a Chromium helper process if the current process is one.
///
/// Must be the first call in `main`. Returns the exit code for helper processes and `None` for
/// the browser process, which then continues with the normal application start.
pub fn run_subprocess() -> Option<i32> {
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let args = Args::new();
    let mut app = app::helper_app();
    let code = cef::execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    (code >= 0).then_some(code)
}
