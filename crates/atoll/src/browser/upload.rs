use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use atoll_core::DownloadStatus;
use atoll_core::time::calendar_date;
use atoll_engine::{FileDialogMode, FileDialogRequest};
use atoll_storage::prelude::*;
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use super::Browser;
use crate::platform::clipboard::ClipboardImage;
use crate::ui::{OverlayState, UploadFile};
use atoll_i18n::t;

/// How many recent files are offered.
const OFFERED_FILES: usize = 6;

/// How many downloads of the history are considered.
const HISTORY_FILES: usize = 50;

/// Clipboard images handed to pages are kept this long, pages may read them only on submit.
const UPLOAD_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// Endings of downloads that are still running.
const PARTIAL_ENDINGS: &[&str] = &["crdownload", "part", "partial", "tmp"];

/// A file offered for uploading.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecentFile {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

/// A file request of a page waiting for the upload popup.
#[derive(Default)]
pub struct UploadState {
    request: Option<FileDialogRequest>,
    image: Option<ClipboardImage>,
    files: Vec<RecentFile>,
}

impl Browser {
    /// A page asks for a file. The clipboard image and recent downloads are offered first,
    /// the system dialog only opens right away if there is neither.
    pub(super) fn request_file(&self, request: FileDialogRequest) {
        if !matches!(
            request.mode,
            FileDialogMode::Open | FileDialogMode::OpenMultiple
        ) {
            self.show_system_file_dialog(request);
            return;
        }
        let extensions = super::overlays::accepted_extensions(&request);
        let image = accepts(&extensions, "png")
            .then(|| self.clipboard.image())
            .flatten();
        let history = self
            .storage
            .downloads(HISTORY_FILES)
            .unwrap_or_default()
            .into_iter()
            .filter(|download| download.status == DownloadStatus::Complete)
            .filter_map(|download| download.path.map(PathBuf::from));
        let files = recent_files(
            &self.download_directory(),
            history,
            &extensions,
            OFFERED_FILES,
        );
        if image.is_none() && files.is_empty() {
            self.show_system_file_dialog(request);
            return;
        }
        let Some(window) = self.window() else { return };
        let overlay = window.global::<OverlayState>();
        overlay.set_upload_has_image(image.is_some());
        if let Some(image) = &image {
            overlay.set_upload_image(preview(image));
            overlay.set_upload_image_size(
                t!(
                    "{} × {} Pixel, wird als PNG hochgeladen",
                    image.width,
                    image.height
                )
                .into(),
            );
        }
        let now = SystemTime::now();
        let items: Vec<UploadFile> = files
            .iter()
            .map(|file| UploadFile {
                name: file
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
                    .into(),
                detail: file_detail(file, now).into(),
            })
            .collect();
        overlay.set_upload_files(ModelRc::from(Rc::new(VecModel::from(items))));
        *self.upload.borrow_mut() = UploadState {
            request: Some(request),
            image,
            files,
        };
        self.dismiss_suggestions();
        overlay.set_upload_visible(true);
    }

    /// Upload popup: hands the clipboard image to the page as a PNG file.
    pub fn upload_clipboard_image(&self) {
        let Some(image) = self.upload.borrow_mut().image.take() else {
            return;
        };
        match save_clipboard_image(&self.paths.uploads, &image, SystemTime::now()) {
            Ok(path) => self.finish_upload(Some(vec![path])),
            Err(error) => {
                tracing::error!(%error, "clipboard image could not be saved");
                self.show_toast(t!("Das Bild konnte nicht gespeichert werden"));
                self.finish_upload(None);
            }
        }
    }

    /// Upload popup: hands one of the recent files to the page.
    pub fn upload_recent_file(&self, index: i32) {
        let path = usize::try_from(index)
            .ok()
            .and_then(|index| self.upload.borrow().files.get(index).cloned())
            .map(|file| file.path);
        if let Some(path) = path {
            self.finish_upload(Some(vec![path]));
        }
    }

    /// Upload popup: opens the system dialog instead.
    pub fn upload_browse(&self) {
        let request = self.upload.borrow_mut().request.take();
        self.close_upload();
        if let Some(request) = request {
            self.show_system_file_dialog(request);
        }
    }

    pub fn upload_cancel(&self) {
        self.finish_upload(None);
    }

    fn finish_upload(&self, paths: Option<Vec<PathBuf>>) {
        let request = self.upload.borrow_mut().request.take();
        self.close_upload();
        if let Some(request) = request
            && let Some(engine) = self.engine.borrow().as_ref()
        {
            engine.resolve_file_dialog(request.id, paths);
        }
    }

    fn close_upload(&self) {
        *self.upload.borrow_mut() = UploadState::default();
        if let Some(window) = self.window() {
            let overlay = window.global::<OverlayState>();
            overlay.set_upload_visible(false);
            overlay.set_upload_image(Image::default());
        }
        self.focus_content();
    }
}

fn accepts(extensions: &[String], extension: &str) -> bool {
    extensions.is_empty() || extensions.iter().any(|accepted| accepted == extension)
}

fn preview(image: &ClipboardImage) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &image.rgba,
        image.width,
        image.height,
    ))
}

/// The newest files of `directory` and `history` with an accepted extension, newest first.
fn recent_files(
    directory: &Path,
    history: impl IntoIterator<Item = PathBuf>,
    extensions: &[String],
    limit: usize,
) -> Vec<RecentFile> {
    let listed = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()));
    let mut seen = HashSet::new();
    let mut files: Vec<RecentFile> = Vec::new();
    for path in listed.chain(history) {
        if !is_offered(&path, extensions) || !seen.insert(path.clone()) {
            continue;
        }
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        files.push(RecentFile {
            size: metadata.len(),
            modified: metadata.modified().unwrap_or(UNIX_EPOCH),
            path,
        });
    }
    files.sort_by_key(|file| std::cmp::Reverse(file.modified));
    files.truncate(limit);
    files
}

fn is_offered(path: &Path, extensions: &[String]) -> bool {
    let Some(name) = path.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };
    if name.starts_with('.') {
        return false;
    }
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    !PARTIAL_ENDINGS.contains(&extension.as_str()) && accepts(extensions, &extension)
}

/// Size and age of a recent file, like `1,2 MB · vor 5 Minuten`.
fn file_detail(file: &RecentFile, now: SystemTime) -> String {
    let age = now.duration_since(file.modified).unwrap_or_default();
    format!(
        "{} · {}",
        super::downloads::format_bytes(file.size as i64),
        age_text(age)
    )
}

fn age_text(age: Duration) -> String {
    let minutes = age.as_secs() / 60;
    match minutes {
        0 => "gerade eben".to_owned(),
        1 => t!("vor einer Minute").to_owned(),
        2..60 => t!("vor {} Minuten", minutes),
        60..120 => t!("vor einer Stunde").to_owned(),
        120..1440 => t!("vor {} Stunden", minutes / 60),
        1440..2880 => "gestern".to_owned(),
        _ => t!("vor {} Tagen", minutes / 1440),
    }
}

/// Writes `image` as a PNG into `directory` and removes clipboard images of earlier days.
fn save_clipboard_image(
    directory: &Path,
    image: &ClipboardImage,
    now: SystemTime,
) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(directory)?;
    for entry in fs::read_dir(directory)?.flatten() {
        let expired = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| {
                now.duration_since(modified).unwrap_or_default() > UPLOAD_LIFETIME
            });
        if expired {
            let _ = fs::remove_file(entry.path());
        }
    }
    let path = unique_path(&directory.join(clipboard_file_name(now)));
    let pixels = image::RgbaImage::from_raw(image.width, image.height, image.rgba.clone())
        .ok_or_else(|| anyhow::anyhow!("the clipboard image has an unexpected size"))?;
    pixels.save_with_format(&path, image::ImageFormat::Png)?;
    Ok(path)
}

/// A name like `clipboard-2026-09-21-141320.png`, in UTC.
fn clipboard_file_name(now: SystemTime) -> String {
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let (year, month, day) = calendar_date(seconds as i64);
    let time = seconds % 86_400;
    format!(
        "clipboard-{year:04}-{month:02}-{day:02}-{:02}{:02}{:02}.png",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

/// `path`, or `path` with a number appended if it exists.
fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    (2..)
        .map(|number| path.with_file_name(format!("{stem}-{number}.png")))
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("atoll-upload-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn touch(path: &Path, age: u64) {
        fs::write(path, b"data").unwrap();
        let modified = SystemTime::now() - Duration::from_secs(age);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    fn names(files: &[RecentFile]) -> Vec<String> {
        files
            .iter()
            .map(|file| {
                file.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn recent_files_are_newest_first_and_match_the_filter() {
        let downloads = scratch("recent");
        let elsewhere = scratch("elsewhere");
        touch(&downloads.join("old.png"), 3000);
        touch(&downloads.join("new.jpg"), 10);
        touch(&downloads.join("notes.txt"), 20);
        touch(&downloads.join(".hidden.png"), 5);
        touch(&downloads.join("running.png.crdownload"), 1);
        fs::create_dir_all(downloads.join("folder.png")).unwrap();
        touch(&elsewhere.join("saved.png"), 100);

        let history = [
            elsewhere.join("saved.png"),
            elsewhere.join("gone.png"),
            downloads.join("new.jpg"),
        ];
        let all = recent_files(&downloads, history.clone(), &[], 10);
        assert_eq!(
            names(&all),
            ["new.jpg", "notes.txt", "saved.png", "old.png"]
        );

        let images = ["png".to_owned(), "jpg".to_owned()];
        let pictures = recent_files(&downloads, history.clone(), &images, 10);
        assert_eq!(names(&pictures), ["new.jpg", "saved.png", "old.png"]);
        assert_eq!(recent_files(&downloads, history, &images, 2).len(), 2);

        fs::remove_dir_all(downloads).unwrap();
        fs::remove_dir_all(elsewhere).unwrap();
    }

    #[test]
    fn clipboard_images_become_png_files() {
        let directory = scratch("clipboard");
        let image = ClipboardImage {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 0, 255, 255],
        };
        let now = SystemTime::now();
        let first = save_clipboard_image(&directory, &image, now).unwrap();
        let second = save_clipboard_image(&directory, &image, now).unwrap();
        assert_eq!(
            first.file_name().unwrap().to_string_lossy(),
            clipboard_file_name(now)
        );
        assert_eq!(
            second.file_name().unwrap().to_string_lossy(),
            clipboard_file_name(now).replace(".png", "-2.png")
        );
        let decoded = image::open(&first).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.get_pixel(1, 0).0, [0, 0, 255, 255]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn clipboard_files_are_named_after_the_time() {
        let now = UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        assert_eq!(clipboard_file_name(now), "clipboard-2026-09-21-141320.png");
    }

    #[test]
    fn old_clipboard_files_are_removed() {
        let directory = scratch("expired");
        let image = ClipboardImage {
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        };
        let old = directory.join("clipboard-old.png");
        touch(&old, 2 * 86_400);
        save_clipboard_image(&directory, &image, SystemTime::now()).unwrap();
        assert!(!old.exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn broken_clipboard_images_are_refused() {
        let directory = scratch("broken");
        let image = ClipboardImage {
            width: 4,
            height: 4,
            rgba: vec![0; 3],
        };
        assert!(save_clipboard_image(&directory, &image, SystemTime::now()).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn only_accepted_complete_files_are_offered() {
        let images = ["png".to_owned()];
        assert!(is_offered(Path::new("/tmp/Shot.PNG"), &images));
        assert!(!is_offered(Path::new("/tmp/report.pdf"), &images));
        assert!(is_offered(Path::new("/tmp/report.pdf"), &[]));
        assert!(!is_offered(Path::new("/tmp/file.part"), &[]));
        assert!(!is_offered(Path::new("/tmp/.config"), &[]));
    }
}
