use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use url::Url;

/// Opens `path` with the default application of the desktop.
pub fn open(path: &Path) -> io::Result<()> {
    let mut child = Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Shows `path` selected in the file manager, or opens its folder if the file manager does not
/// support selecting files. Runs in the background because the file manager may take a while to
/// answer.
pub fn show_in_folder(path: PathBuf) {
    std::thread::spawn(move || {
        if !select_in_file_manager(&path) {
            let target = path.parent().unwrap_or(&path);
            if let Err(error) = open(target) {
                tracing::warn!(%error, "folder could not be opened");
            }
        }
    });
}

fn select_in_file_manager(path: &Path) -> bool {
    let Ok(uri) = Url::from_file_path(path) else {
        return false;
    };
    Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--reply-timeout=3000",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{uri}"))
        .arg("string:")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
