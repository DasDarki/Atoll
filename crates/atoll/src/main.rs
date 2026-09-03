mod app;
mod bridge;
mod browser;
mod input;
mod platform;
mod pump;

mod ui {
    #![allow(unused_qualifications, clippy::all)]
    slint::include_modules!();
}

use atoll_i18n::t;
use std::process::ExitCode;

fn main() -> ExitCode {
    if let Some(code) = atoll_engine::run_subprocess() {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }

    // SAFETY: nothing but the Chromium helper check has run so far, no other thread exists.
    unsafe { atoll_engine::prune_unusable_vulkan_drivers() };
    platform::logging::init();
    match app::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error:#}");
            eprintln!(
                "{}",
                t!(
                    "Atoll konnte nicht gestartet werden: {}",
                    format!("{error:#}")
                )
            );
            ExitCode::FAILURE
        }
    }
}
