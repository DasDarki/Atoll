//! Turning a saved file into another format, and mirroring pictures and videos.
//!
//! Pictures are converted in Atoll itself with the `image` crate, which needs nothing installed.
//! Everything else, and anything with sound or moving pictures, goes through `ffmpeg`, the tool
//! that does this everywhere. Without it those formats are simply not offered, the same way the
//! video download says what it needs.

mod ffmpeg;
mod picture;

use std::path::{Path, PathBuf};

use thiserror::Error;

pub use ffmpeg::Ffmpeg;

/// What kind of file is being saved, which decides what it can become.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Picture,
    Sound,
    Video,
}

/// A format a file can be saved as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    /// What the user sees, for example `JPEG`.
    pub label: &'static str,
    pub extension: &'static str,
    pub kind: Kind,
    /// Whether ffmpeg has to be installed for it.
    pub needs_ffmpeg: bool,
    /// Whether the moving pictures are thrown away and only the sound is kept.
    pub sound_only: bool,
}

const FORMATS: &[Format] = &[
    Format {
        label: "PNG",
        extension: "png",
        kind: Kind::Picture,
        needs_ffmpeg: false,
        sound_only: false,
    },
    Format {
        label: "JPEG",
        extension: "jpg",
        kind: Kind::Picture,
        needs_ffmpeg: false,
        sound_only: false,
    },
    Format {
        label: "WebP",
        extension: "webp",
        kind: Kind::Picture,
        needs_ffmpeg: true,
        sound_only: false,
    },
    Format {
        label: "GIF",
        extension: "gif",
        kind: Kind::Picture,
        needs_ffmpeg: false,
        sound_only: false,
    },
    Format {
        label: "BMP",
        extension: "bmp",
        kind: Kind::Picture,
        needs_ffmpeg: false,
        sound_only: false,
    },
    Format {
        label: "TIFF",
        extension: "tiff",
        kind: Kind::Picture,
        needs_ffmpeg: false,
        sound_only: false,
    },
    Format {
        label: "AVIF",
        extension: "avif",
        kind: Kind::Picture,
        needs_ffmpeg: true,
        sound_only: false,
    },
    Format {
        label: "MP3",
        extension: "mp3",
        kind: Kind::Sound,
        needs_ffmpeg: true,
        sound_only: true,
    },
    Format {
        label: "M4A",
        extension: "m4a",
        kind: Kind::Sound,
        needs_ffmpeg: true,
        sound_only: true,
    },
    Format {
        label: "Opus",
        extension: "opus",
        kind: Kind::Sound,
        needs_ffmpeg: true,
        sound_only: true,
    },
    Format {
        label: "FLAC",
        extension: "flac",
        kind: Kind::Sound,
        needs_ffmpeg: true,
        sound_only: true,
    },
    Format {
        label: "WAV",
        extension: "wav",
        kind: Kind::Sound,
        needs_ffmpeg: true,
        sound_only: true,
    },
    Format {
        label: "MP4",
        extension: "mp4",
        kind: Kind::Video,
        needs_ffmpeg: true,
        sound_only: false,
    },
    Format {
        label: "WebM",
        extension: "webm",
        kind: Kind::Video,
        needs_ffmpeg: true,
        sound_only: false,
    },
    Format {
        label: "MKV",
        extension: "mkv",
        kind: Kind::Video,
        needs_ffmpeg: true,
        sound_only: false,
    },
    Format {
        label: "GIF",
        extension: "gif",
        kind: Kind::Video,
        needs_ffmpeg: true,
        sound_only: false,
    },
];

/// The extensions each kind is recognised by.
const PICTURES: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "avif", "ico", "jfif",
];
const SOUNDS: &[&str] = &[
    "mp3", "m4a", "aac", "opus", "ogg", "oga", "flac", "wav", "wma",
];
const VIDEOS: &[&str] = &[
    "mp4", "m4v", "webm", "mkv", "mov", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "ogv",
];

/// What kind of file an address or a file name points at, as far as its ending says.
pub fn kind_of(name: &str) -> Option<Kind> {
    let extension = extension_of(name)?;
    if PICTURES.contains(&extension.as_str()) {
        Some(Kind::Picture)
    } else if SOUNDS.contains(&extension.as_str()) {
        Some(Kind::Sound)
    } else if VIDEOS.contains(&extension.as_str()) {
        Some(Kind::Video)
    } else {
        None
    }
}

/// The lower-cased ending of an address or file name, without the dot and without what a server
/// appended with `?` or `#`.
pub fn extension_of(name: &str) -> Option<String> {
    let name = name.split(['?', '#']).next()?;
    let name = name.rsplit(['/', '\\']).next()?;
    let (_, extension) = name.rsplit_once('.')?;
    let extension = extension.trim().to_ascii_lowercase();
    (!extension.is_empty() && extension.len() <= 5 && extension.chars().all(char::is_alphanumeric))
        .then_some(extension)
}

/// The formats a file of this kind can be saved as, the ones it stays a kind of first. A video
/// can also become sound alone, which is why its list ends with the sound formats.
pub fn formats_for(kind: Kind) -> Vec<&'static Format> {
    let own = FORMATS.iter().filter(move |format| format.kind == kind);
    let sound_of_a_video = FORMATS
        .iter()
        .filter(move |format| kind == Kind::Video && format.kind == Kind::Sound);
    own.chain(sound_of_a_video).collect()
}

/// Whether a file of this kind can be mirrored.
pub fn can_be_mirrored(kind: Kind) -> bool {
    matches!(kind, Kind::Picture | Kind::Video)
}

/// What should become of a saved file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Recipe {
    /// The format it should get, or `None` to keep the one it has.
    pub format: Option<Format>,
    /// Whether it is mirrored from left to right.
    pub mirror: bool,
}

impl Recipe {
    /// Whether this recipe would change the file at all.
    pub fn changes(&self, name: &str) -> bool {
        self.mirror
            || self
                .format
                .is_some_and(|format| !matches_extension(&format, name))
    }

    /// Whether ffmpeg is needed to follow it.
    pub fn needs_ffmpeg(&self, source: &Path) -> bool {
        if self.format.is_some_and(|format| format.needs_ffmpeg) {
            return true;
        }
        let name = source.to_string_lossy();
        match kind_of(&name) {
            // Moving pictures keep moving only through ffmpeg, a still picture crate would
            // turn them into their first frame.
            Some(Kind::Picture) => extension_of(&name).is_some_and(|ending| ending == "gif"),
            Some(_) => true,
            None => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConvertError {
    #[error("ffmpeg is not installed")]
    MissingFfmpeg,
    #[error("the file could not be read: {0}")]
    Unreadable(String),
    #[error("the file could not be written: {0}")]
    Unwritable(String),
    #[error("ffmpeg failed: {0}")]
    Failed(String),
}

/// Follows `recipe` for the file at `source` and answers with the file that came out of it. The
/// source stays untouched unless the result takes its place, which happens when nothing but the
/// ending changed.
pub fn convert(
    source: &Path,
    recipe: &Recipe,
    ffmpeg: Option<&Ffmpeg>,
) -> Result<PathBuf, ConvertError> {
    let target = target_path(source, recipe);
    if recipe.needs_ffmpeg(source) {
        let ffmpeg = ffmpeg.ok_or(ConvertError::MissingFfmpeg)?;
        ffmpeg.convert(source, &target, recipe)?;
    } else {
        picture::convert(source, &target, recipe)?;
    }
    if target != source {
        // What was downloaded is gone once it has become what was asked for. If it cannot be
        // removed it simply stays next to the result, which is better than failing here.
        let _ = std::fs::remove_file(source);
    }
    Ok(target)
}

/// Where the result goes: next to the source, under its name with the new ending, and with a
/// number appended when something of that name is already there.
fn target_path(source: &Path, recipe: &Recipe) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "datei".to_owned());
    let extension = recipe
        .format
        .map(|format| format.extension.to_owned())
        .or_else(|| {
            source
                .extension()
                .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        })
        .unwrap_or_else(|| "bin".to_owned());
    let directory = source.parent().unwrap_or(Path::new("."));
    let first = directory.join(format!("{stem}.{extension}"));
    if first == source || !first.exists() {
        return first;
    }
    for number in 2..1000 {
        let candidate = directory.join(format!("{stem} ({number}).{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

fn matches_extension(format: &Format, name: &str) -> bool {
    extension_of(name).is_some_and(|ending| {
        ending == format.extension || (ending == "jpeg" && format.extension == "jpg")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ending_says_what_a_file_is() {
        assert_eq!(kind_of("https://example.org/logo.png"), Some(Kind::Picture));
        assert_eq!(kind_of("https://example.org/lied.mp3"), Some(Kind::Sound));
        assert_eq!(
            kind_of("https://example.org/film.mp4?token=1"),
            Some(Kind::Video)
        );
        assert_eq!(kind_of("https://example.org/seite"), None);
        assert_eq!(kind_of("https://example.org/"), None);
    }

    #[test]
    fn a_question_mark_is_not_part_of_the_ending() {
        assert_eq!(
            extension_of("https://example.org/bild.JPG?v=3#x"),
            Some("jpg".to_owned())
        );
        assert_eq!(extension_of("https://example.org/pfad.mit.punkt/"), None);
    }

    #[test]
    fn a_video_can_also_become_sound_alone() {
        let formats = formats_for(Kind::Video);
        assert_eq!(
            formats.first().map(|format| format.extension),
            Some("mp4"),
            "what it stays a video of comes first"
        );
        assert!(formats.iter().any(|format| format.sound_only));
        let first_sound = formats
            .iter()
            .position(|format| format.sound_only)
            .expect("sound formats");
        assert!(
            formats[..first_sound]
                .iter()
                .all(|format| !format.sound_only),
            "the sound formats stand together at the end"
        );
        assert!(
            !formats_for(Kind::Picture)
                .iter()
                .any(|format| format.sound_only)
        );
    }

    #[test]
    fn a_recipe_that_keeps_everything_changes_nothing() {
        let keep = Recipe::default();
        assert!(!keep.changes("bild.png"));
        let same = Recipe {
            format: formats_for(Kind::Picture)
                .iter()
                .find(|format| format.extension == "png")
                .map(|format| **format),
            mirror: false,
        };
        assert!(!same.changes("bild.png"));
        assert!(same.changes("bild.jpg"));
        let mirrored = Recipe {
            format: None,
            mirror: true,
        };
        assert!(mirrored.changes("bild.png"));
    }

    #[test]
    fn the_result_goes_next_to_the_source() {
        let recipe = Recipe {
            format: formats_for(Kind::Picture)
                .iter()
                .find(|format| format.extension == "jpg")
                .map(|format| **format),
            mirror: false,
        };
        assert_eq!(
            target_path(Path::new("/tmp/bilder/logo.png"), &recipe),
            PathBuf::from("/tmp/bilder/logo.jpg")
        );
        assert_eq!(
            target_path(Path::new("/tmp/bilder/logo.png"), &Recipe::default()),
            PathBuf::from("/tmp/bilder/logo.png"),
            "without a format it keeps its name"
        );
    }
}
