use std::os::raw::c_int;
use std::path::PathBuf;

use cef::sys::cef_file_dialog_mode_t;
use cef::{
    Browser, CefString, CefStringList, DialogHandler, FileDialogCallback, ImplDialogHandler,
    WrapDialogHandler, rc::Rc as _, wrap_dialog_handler,
};

use super::{list, text};
use crate::events::{EngineEvent, FileDialogMode, FileDialogRequest};
use crate::registry::{self, PendingCallback};
use crate::view::ViewId;

wrap_dialog_handler! {
    pub(crate) struct ViewDialogHandler {
        view: ViewId,
    }

    impl DialogHandler {
        fn on_file_dialog(
            &self,
            _browser: Option<&mut Browser>,
            mode: cef::FileDialogMode,
            title: Option<&CefString>,
            default_file_path: Option<&CefString>,
            accept_filters: Option<&mut CefStringList>,
            accept_extensions: Option<&mut CefStringList>,
            _accept_descriptions: Option<&mut CefStringList>,
            callback: Option<&mut FileDialogCallback>,
        ) -> c_int {
            let Some(callback) = callback else { return 0 };
            let Some(runtime) = registry::runtime() else { return 0 };
            let default_path = Some(text(default_file_path))
                .filter(|path| !path.is_empty())
                .map(PathBuf::from);
            let id = runtime.add_pending(PendingCallback::FileDialog(callback.clone()));
            runtime.emit(EngineEvent::FileDialog {
                view: self.view,
                request: FileDialogRequest {
                    id,
                    mode: dialog_mode(*mode.as_ref()),
                    title: text(title),
                    default_path,
                    accept_types: list(accept_filters),
                    accept_extensions: extensions(list(accept_extensions)),
                },
            });
            1
        }
    }
}

fn dialog_mode(mode: cef_file_dialog_mode_t) -> FileDialogMode {
    match mode {
        cef_file_dialog_mode_t::FILE_DIALOG_OPEN_MULTIPLE => FileDialogMode::OpenMultiple,
        cef_file_dialog_mode_t::FILE_DIALOG_OPEN_FOLDER => FileDialogMode::OpenFolder,
        cef_file_dialog_mode_t::FILE_DIALOG_SAVE => FileDialogMode::Save,
        _ => FileDialogMode::Open,
    }
}

fn extensions(groups: Vec<String>) -> Vec<String> {
    let mut extensions: Vec<String> = groups
        .iter()
        .flat_map(|group| group.split(';'))
        .map(|extension| {
            extension
                .trim()
                .trim_start_matches('.')
                .to_ascii_lowercase()
        })
        .filter(|extension| !extension.is_empty())
        .collect();
    extensions.dedup();
    extensions
}
