//! Saving what a page shows: the file in front of the user, a picture from its context menu, or
//! the video that plays, each of them as it is or turned into another format.
//!
//! Atoll loads the file the way it loads every other download, through the engine, so the page's
//! sign-in counts and nothing is fetched twice. What should become of it is remembered until the
//! download is through, and the turning itself happens afterwards, away from the window.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use atoll_convert::{Ffmpeg, Format, Kind, Recipe};
use atoll_core::DownloadId;
use atoll_i18n::t;
use atoll_storage::prelude::*;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::browser::Browser;
use crate::platform::tasks::run_in_background;
use crate::ui::OverlayState;

/// ffmpeg is looked for once, the first time a save dialog needs to know about it.
static FFMPEG: OnceLock<Option<Ffmpeg>> = OnceLock::new();

fn ffmpeg() -> Option<&'static Ffmpeg> {
    FFMPEG.get_or_init(Ffmpeg::find).as_ref()
}

/// What the save dialog is about.
#[derive(Debug, Clone)]
pub enum SaveTarget {
    /// A file the engine downloads.
    File { url: String },
    /// A video yt-dlp puts together.
    Video {
        url: String,
        name: Option<String>,
        referer: Option<String>,
    },
}

impl SaveTarget {
    fn url(&self) -> &str {
        match self {
            SaveTarget::File { url } | SaveTarget::Video { url, .. } => url,
        }
    }
}

/// The open save dialog, and what every download still owes a conversion.
#[derive(Default)]
pub struct SaveState {
    dialog: Option<Dialog>,
    /// Addresses whose download is still to be turned into something else.
    waiting: HashMap<String, Recipe>,
    /// The same for downloads that are already known by their number, which is how a video
    /// download is followed.
    running: HashMap<DownloadId, Recipe>,
}

struct Dialog {
    target: SaveTarget,
    kind: Kind,
    /// The formats offered, in the order of the list. The first entry keeps the format.
    formats: Vec<Option<Format>>,
}

impl Browser {
    /// Ctrl+S: saves what the page shows, as it is.
    pub fn save_page(&self) {
        let Some(url) = self.savable_address() else {
            self.show_toast(t!("Diese Seite lässt sich nicht als Datei speichern"));
            return;
        };
        self.save_url(&url, Recipe::default());
    }

    /// Ctrl+Shift+S: asks what should become of the file before it is saved. A page that is no
    /// file at all is searched for a video instead, which is what there is to save on it.
    pub fn save_page_as(&self) {
        let file = self
            .savable_address()
            .filter(|url| atoll_convert::kind_of(url).is_some());
        match file {
            Some(url) => self.open_save_dialog(SaveTarget::File { url }),
            None => self.download_video_asking(),
        }
    }

    /// The context menu of a picture: saves it, or asks first.
    pub(in crate::browser) fn save_from_page(&self, url: &str, convert: bool) {
        if url.trim().is_empty() {
            return;
        }
        if convert {
            self.open_save_dialog(SaveTarget::File {
                url: url.to_owned(),
            });
        } else {
            self.save_url(url, Recipe::default());
        }
    }

    /// The address of the page, if it is one that can be fetched again.
    fn savable_address(&self) -> Option<String> {
        self.current_page_url()
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
    }

    /// Starts the download and remembers what should become of the file.
    pub(in crate::browser) fn save_url(&self, url: &str, recipe: Recipe) {
        let name = file_name_of(url);
        if recipe.changes(&name) {
            self.save
                .borrow_mut()
                .waiting
                .insert(url.to_owned(), recipe);
        }
        let mut started = false;
        self.with_live_view(|view| {
            view.start_download(url);
            started = true;
        });
        if !started {
            self.save.borrow_mut().waiting.remove(url);
            self.show_toast(t!("Zum Speichern muss die Seite geladen sein"));
        }
    }

    pub(in crate::browser) fn open_save_dialog(&self, target: SaveTarget) {
        let Some(window) = self.window() else { return };
        let name = file_name_of(target.url());
        let kind = match &target {
            SaveTarget::Video { .. } => Kind::Video,
            SaveTarget::File { url } => atoll_convert::kind_of(url).unwrap_or(Kind::Picture),
        };
        let has_ffmpeg = ffmpeg().is_some();
        let mut formats: Vec<Option<Format>> = vec![None];
        let mut labels: Vec<SharedString> = vec![t!("Unverändert lassen").into()];
        for format in atoll_convert::formats_for(kind) {
            if format.needs_ffmpeg && !has_ffmpeg {
                continue;
            }
            labels.push(
                if format.sound_only && kind == Kind::Video {
                    t!("Nur Ton, als {}", format.label)
                } else {
                    format.label.to_owned()
                }
                .into(),
            );
            formats.push(Some(*format));
        }
        let state = window.global::<OverlayState>();
        state.set_save_name(name.as_str().into());
        state.set_save_source(atoll_core::address::display_url(target.url()).into());
        state.set_save_formats(ModelRc::from(std::rc::Rc::new(VecModel::from(labels))));
        state.set_save_format(0);
        state.set_save_can_mirror(atoll_convert::can_be_mirrored(kind) && has_ffmpeg_for(kind));
        state.set_save_mirror(false);
        state.set_save_hint(
            if has_ffmpeg {
                String::new()
            } else {
                t!("Ohne ffmpeg lassen sich nur Bilder umwandeln. Installiere ffmpeg für Ton, Videos und WebP.").to_owned()
            }
            .into(),
        );
        state.set_save_visible(true);
        self.save.borrow_mut().dialog = Some(Dialog {
            target,
            kind,
            formats,
        });
    }

    pub fn save_dialog_cancelled(&self) {
        self.save.borrow_mut().dialog = None;
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_save_visible(false);
        }
        self.focus_content();
    }

    pub fn save_dialog_confirmed(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<OverlayState>();
        let chosen = state.get_save_format().max(0) as usize;
        let mirror = state.get_save_mirror();
        let Some(dialog) = self.save.borrow_mut().dialog.take() else {
            return;
        };
        state.set_save_visible(false);
        self.focus_content();
        let recipe = Recipe {
            format: dialog.formats.get(chosen).copied().flatten(),
            mirror: mirror && atoll_convert::can_be_mirrored(dialog.kind),
        };
        match dialog.target {
            SaveTarget::File { url } => self.save_url(&url, recipe),
            SaveTarget::Video { url, name, referer } => {
                self.download_stream_converting(&url, name.as_deref(), referer.as_deref(), recipe);
            }
        }
    }

    /// Remembers what should become of a video that is being downloaded.
    pub(super) fn convert_when_done(&self, id: DownloadId, recipe: Recipe) {
        if recipe.format.is_some() || recipe.mirror {
            self.save.borrow_mut().running.insert(id, recipe);
        }
    }

    /// A download is through: turns the file into what was asked for, if anything.
    pub(super) fn convert_downloaded(&self, id: DownloadId, url: &str, path: Option<&str>) {
        let recipe = {
            let mut save = self.save.borrow_mut();
            save.running
                .remove(&id)
                .or_else(|| save.waiting.remove(url))
        };
        let (Some(recipe), Some(path)) = (recipe, path) else {
            return;
        };
        let source = PathBuf::from(path);
        if !recipe.changes(&source.to_string_lossy()) {
            return;
        }
        let this = self.this();
        run_in_background(
            "convert",
            move || atoll_convert::convert(&source, &recipe, ffmpeg()),
            move |result| {
                if let Some(browser) = this.upgrade() {
                    browser.conversion_finished(id, result);
                }
            },
        );
    }

    fn conversion_finished(
        &self,
        id: DownloadId,
        result: Result<PathBuf, atoll_convert::ConvertError>,
    ) {
        let path = match result {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(%error, "a file could not be converted");
                self.show_toast(&t!("Umwandeln fehlgeschlagen: {}", error));
                return;
            }
        };
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        {
            let mut downloads = self.downloads.borrow_mut();
            if let Some(entry) = downloads.entries.iter_mut().find(|entry| entry.id == id) {
                entry.path = Some(path.to_string_lossy().into_owned());
                entry.file_name = name.clone();
            }
        }
        let stored = self
            .storage
            .rename_download(id, &name, path.to_string_lossy().as_ref());
        if let Err(error) = stored {
            tracing::warn!(%error, "the converted file could not be stored");
        }
        self.show_toast(&t!("Gespeichert als {}", name));
        self.refresh_downloads();
    }
}

/// Whether the kind can be mirrored with what is installed: a picture needs nothing, everything
/// else needs ffmpeg.
fn has_ffmpeg_for(kind: Kind) -> bool {
    kind == Kind::Picture || ffmpeg().is_some()
}

/// The name a file saved from `url` would get, as far as the address says.
fn file_name_of(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let last = without_query
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    if last.is_empty() || !last.contains('.') {
        return t!("Datei").to_owned();
    }
    last.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_name_comes_from_the_address() {
        assert_eq!(
            file_name_of("https://example.org/bilder/logo.png"),
            "logo.png"
        );
        assert_eq!(
            file_name_of("https://example.org/logo.png?v=2#x"),
            "logo.png"
        );
        assert_eq!(file_name_of("https://example.org/seite/"), t!("Datei"));
    }
}
