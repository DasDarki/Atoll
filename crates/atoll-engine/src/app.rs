use std::sync::Arc;

use cef::{
    App, BrowserProcessHandler, CefString, CommandLine, ImplApp, ImplBrowserProcessHandler,
    ImplCommandLine, WrapApp, WrapBrowserProcessHandler, rc::Rc as _, wrap_app,
    wrap_browser_process_handler,
};

use crate::pump;

const DEFAULT_SWITCHES: &[&str] = &[
    "no-first-run",
    "no-default-browser-check",
    "disable-session-crashed-bubble",
    "hide-crash-restore-bubble",
];

wrap_app! {
    pub(crate) struct AtollApp {
        switches: Arc<Vec<String>>,
        browser_process_handler: Option<BrowserProcessHandler>,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            let is_browser_process = process_type.is_none_or(|kind| kind.to_string().is_empty());
            let Some(command_line) = command_line.filter(|_| is_browser_process) else {
                return;
            };
            for switch in DEFAULT_SWITCHES.iter().copied().chain(self.switches.iter().map(String::as_str)) {
                append_switch(command_line, switch);
            }
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            self.browser_process_handler.clone()
        }
    }
}

wrap_browser_process_handler! {
    struct PumpingBrowserProcessHandler;

    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            pump::schedule(delay_ms);
        }
    }
}

fn append_switch(command_line: &mut CommandLine, switch: &str) {
    let switch = switch.trim_start_matches('-');
    match switch.split_once('=') {
        Some((name, value)) => {
            command_line.append_switch_with_value(Some(&name.into()), Some(&value.into()));
        }
        None => command_line.append_switch(Some(&switch.into())),
    }
}

/// The application object for helper processes, which need no browser process callbacks.
pub(crate) fn helper_app() -> App {
    AtollApp::new(Arc::new(Vec::new()), None)
}

/// The application object for the browser process.
pub(crate) fn browser_app(switches: Vec<String>) -> App {
    AtollApp::new(
        Arc::new(switches),
        Some(PumpingBrowserProcessHandler::new()),
    )
}
