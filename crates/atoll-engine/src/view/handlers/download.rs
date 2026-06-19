use std::os::raw::c_int;
use std::path::{Path, PathBuf};

use cef::{
    BeforeDownloadCallback, Browser, CefString, DownloadHandler, DownloadItem,
    DownloadItemCallback, ImplBeforeDownloadCallback, ImplDownloadHandler, ImplDownloadItem,
    WrapDownloadHandler, rc::Rc as _, wrap_download_handler,
};

use super::text;
use crate::events::{DownloadState, EngineEvent};
use crate::registry;
use crate::view::ViewId;

wrap_download_handler! {
    pub(crate) struct ViewDownloadHandler {
        view: ViewId,
    }

    impl DownloadHandler {
        fn can_download(&self, _browser: Option<&mut Browser>, _url: Option<&CefString>, _request_method: Option<&CefString>) -> c_int {
            1
        }

        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> c_int {
            let (Some(item), Some(callback)) = (download_item, callback) else { return 0 };
            let Some(runtime) = registry::runtime() else { return 0 };
            let name = sanitize_file_name(&text(suggested_name));
            let path = unique_path(&runtime.download_directory, &name);
            if let Err(error) = std::fs::create_dir_all(&runtime.download_directory) {
                tracing::warn!(%error, "download directory could not be created");
            }
            callback.cont(Some(&path.to_string_lossy().as_ref().into()), 0);
            let mut state = download_state(self.view, item);
            state.path = Some(path);
            runtime.emit(EngineEvent::Download(state));
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            _callback: Option<&mut DownloadItemCallback>,
        ) {
            if let Some(item) = download_item {
                registry::emit(EngineEvent::Download(download_state(self.view, item)));
            }
        }
    }
}

fn download_state(view: ViewId, item: &DownloadItem) -> DownloadState {
    let path = CefString::from(&item.full_path()).to_string();
    let path = (!path.is_empty()).then(|| PathBuf::from(path));
    let file_name = path
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| CefString::from(&item.suggested_file_name()).to_string());
    DownloadState {
        id: item.id(),
        view: Some(view),
        file_name,
        path,
        received_bytes: item.received_bytes(),
        total_bytes: item.total_bytes(),
        in_progress: item.is_in_progress() != 0,
        complete: item.is_complete() != 0,
        canceled: item.is_canceled() != 0 || item.is_interrupted() != 0,
    }
}

fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_owned();
    if cleaned.is_empty() {
        String::from("download")
    } else {
        cleaned
    }
}

fn unique_path(directory: &Path, name: &str) -> PathBuf {
    let candidate = directory.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()));
    (1..)
        .map(|index| {
            directory.join(format!(
                "{stem} ({index}){}",
                extension.as_deref().unwrap_or_default()
            ))
        })
        .find(|candidate| !candidate.exists())
        .unwrap_or(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_cannot_escape_the_download_directory() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_file_name("   "), "download");
        assert_eq!(sanitize_file_name("report.pdf"), "report.pdf");
    }

    #[test]
    fn existing_files_are_not_overwritten() {
        let directory =
            std::env::temp_dir().join(format!("atoll-download-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("a.txt"), b"x").unwrap();
        assert_eq!(
            unique_path(&directory, "a.txt"),
            directory.join("a (1).txt")
        );
        assert_eq!(unique_path(&directory, "b.txt"), directory.join("b.txt"));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
