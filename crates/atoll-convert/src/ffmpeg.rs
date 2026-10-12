use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{ConvertError, Kind, Recipe, kind_of};

/// The tool `ffmpeg`, looked up once when Atoll starts.
#[derive(Debug, Clone)]
pub struct Ffmpeg {
    program: PathBuf,
}

impl Ffmpeg {
    /// Finds ffmpeg on the system, or answers with nothing when it is not installed.
    pub fn find() -> Option<Self> {
        let program = PathBuf::from("ffmpeg");
        let found = Command::new(&program)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        found.then_some(Self { program })
    }

    pub(crate) fn convert(
        &self,
        source: &Path,
        target: &Path,
        recipe: &Recipe,
    ) -> Result<(), ConvertError> {
        let mut command = Command::new(&self.program);
        command
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-i")
            .arg(source);
        if recipe.mirror {
            command.arg("-vf").arg("hflip");
        }
        if recipe.format.is_some_and(|format| format.sound_only) {
            // The pictures go, the sound stays.
            command.arg("-vn");
        } else if kind_of(&target.to_string_lossy()) == Some(Kind::Picture)
            && kind_of(&source.to_string_lossy()) == Some(Kind::Video)
        {
            // A still picture out of a video is its first frame.
            command.arg("-frames:v").arg("1");
        }
        let output = command
            .arg(target)
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .output()
            .map_err(|error| ConvertError::Failed(error.to_string()))?;
        if output.status.success() {
            return Ok(());
        }
        let message = String::from_utf8_lossy(&output.stderr);
        let message = message
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("unbekannter Fehler")
            .to_owned();
        Err(ConvertError::Failed(message))
    }
}
