use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use thiserror::Error;

use crate::output::{
    FILE_MARK, Line, NAME_MARK, PROGRESS_MARK, Progress, error_message, output_template, parse_line,
};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VideoError {
    #[error("yt-dlp is not installed")]
    Missing,
    #[error("the download was cancelled")]
    Cancelled,
    #[error("yt-dlp failed: {0}")]
    Failed(String),
}

/// What to download and where to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoRequest {
    /// A page, player or stream address yt-dlp understands.
    pub url: String,
    pub directory: PathBuf,
    /// The file name without extension, `None` to name the file after the video's title.
    pub name: Option<String>,
    /// The page the video was found on, which some servers insist on.
    pub referer: Option<String>,
}

/// What a running download reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoEvent {
    /// The path the video will be saved to, known once yt-dlp has read its details.
    Named(PathBuf),
    /// How far the current file is. Videos with separate sound are downloaded as two files and
    /// merged afterwards, so the progress starts over once.
    Progress(Progress),
}

/// The video downloader yt-dlp.
#[derive(Debug, Clone)]
pub struct YtDlp {
    program: PathBuf,
}

impl YtDlp {
    /// Uses the program at `program`, usually just `yt-dlp` from the search path.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// The version of yt-dlp, which also tells whether it is installed.
    pub fn version(&self) -> Result<String, VideoError> {
        let output = Command::new(&self.program)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .map_err(start_error)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(VideoError::Failed(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ))
        }
    }

    /// Prepares downloading `request`. Run it with [`VideoDownload::run`] off the user interface
    /// thread.
    pub fn download(&self, request: VideoRequest) -> VideoDownload {
        VideoDownload {
            program: self.program.clone(),
            request,
            process: Arc::default(),
        }
    }
}

impl Default for YtDlp {
    fn default() -> Self {
        Self::new("yt-dlp")
    }
}

#[derive(Debug, Default)]
struct Process {
    child: Option<Child>,
    cancelled: bool,
}

/// A download that has not run yet.
#[derive(Debug)]
pub struct VideoDownload {
    program: PathBuf,
    request: VideoRequest,
    process: Arc<Mutex<Process>>,
}

/// Stops a [`VideoDownload`] from any thread.
#[derive(Debug, Clone)]
pub struct Canceller {
    process: Arc<Mutex<Process>>,
}

impl Canceller {
    pub fn cancel(&self) {
        let mut process = lock(&self.process);
        process.cancelled = true;
        if let Some(child) = process.child.as_mut() {
            stop(child);
        }
    }
}

impl VideoDownload {
    pub fn canceller(&self) -> Canceller {
        Canceller {
            process: self.process.clone(),
        }
    }

    /// Downloads the video, reporting to `on_event` while it runs, and returns where it was
    /// saved. Blocks until yt-dlp has finished.
    pub fn run(self, mut on_event: impl FnMut(VideoEvent)) -> Result<PathBuf, VideoError> {
        let workspace = self.request.directory.join(workspace_name());
        let result = self.run_in(&workspace, &mut on_event);
        let _ = fs::remove_dir_all(&workspace);
        result
    }

    fn run_in(
        &self,
        workspace: &Path,
        on_event: &mut impl FnMut(VideoEvent),
    ) -> Result<PathBuf, VideoError> {
        let mut command = self.command(workspace);
        let (stdout, stderr) = {
            let mut process = lock(&self.process);
            if process.cancelled {
                return Err(VideoError::Cancelled);
            }
            let mut child = command.spawn().map_err(start_error)?;
            let streams = (child.stdout.take(), child.stderr.take());
            process.child = Some(child);
            streams
        };
        let errors = thread::spawn(move || {
            stderr
                .map(|stderr| {
                    BufReader::new(stderr)
                        .lines()
                        .map_while(Result::ok)
                        .filter_map(|line| error_message(&line))
                        .last()
                })
                .unwrap_or_default()
        });
        let mut file = None;
        if let Some(stdout) = stdout {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                match parse_line(&line) {
                    Some(Line::Progress(progress)) => on_event(VideoEvent::Progress(progress)),
                    Some(Line::Name(path)) => on_event(VideoEvent::Named(path)),
                    Some(Line::File(path)) => file = Some(path),
                    None => {}
                }
            }
        }
        let error = errors.join().unwrap_or_default();
        let child = lock(&self.process).child.take();
        let status = match child {
            Some(mut child) => child.wait().ok(),
            None => None,
        };
        if lock(&self.process).cancelled {
            return Err(VideoError::Cancelled);
        }
        match (status, file) {
            (Some(status), Some(file)) if status.success() => Ok(file),
            (Some(status), _) => {
                Err(VideoError::Failed(error.unwrap_or_else(|| {
                    format!("yt-dlp stopped without saving a file ({status})")
                })))
            }
            (None, _) => Err(VideoError::Failed(
                "yt-dlp could not be waited for".to_owned(),
            )),
        }
    }

    fn command(&self, workspace: &Path) -> Command {
        let request = &self.request;
        let template = request
            .name
            .as_deref()
            .map_or_else(|| "%(title).150B.%(ext)s".to_owned(), output_template);
        let mut command = Command::new(&self.program);
        command
            .args([
                "--newline",
                "--no-colors",
                "--no-playlist",
                "--no-mtime",
                "--progress",
            ])
            .arg("--progress-template")
            .arg(format!(
                "download:{PROGRESS_MARK} %(progress.downloaded_bytes)s \
                 %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s"
            ))
            .arg("--print")
            .arg(format!("before_dl:{NAME_MARK} %(filename)s"))
            .arg("--print")
            .arg(format!("after_move:{FILE_MARK} %(filepath)s"))
            .args(["--merge-output-format", "mp4"])
            .arg("-P")
            .arg(format!("home:{}", request.directory.display()))
            .arg("-P")
            .arg(format!("temp:{}", workspace.display()))
            .arg("-o")
            .arg(template);
        if let Some(referer) = &request.referer {
            command.arg("--referer").arg(referer);
        }
        command
            .arg("--")
            .arg(&request.url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        command
    }
}

fn lock(process: &Mutex<Process>) -> MutexGuard<'_, Process> {
    process.lock().unwrap_or_else(PoisonError::into_inner)
}

fn start_error(error: io::Error) -> VideoError {
    match error.kind() {
        io::ErrorKind::NotFound => VideoError::Missing,
        _ => VideoError::Failed(error.to_string()),
    }
}

/// A folder of its own for the parts of one download, removed afterwards.
fn workspace_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        ".atoll-video-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Stops yt-dlp together with the ffmpeg it may have started, which shares its process group.
#[cfg(unix)]
fn stop(child: &mut Child) {
    let Ok(group) = libc::pid_t::try_from(child.id()) else {
        let _ = child.kill();
        return;
    };
    // SAFETY: `kill` only sends a signal. The group was created for this child with
    // `process_group(0)` and is still alive, because the child has not been waited for.
    unsafe {
        libc::kill(-group, libc::SIGTERM);
    }
}

#[cfg(not(unix))]
fn stop(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    use super::*;

    /// Tests that write a program and tests that start one take turns: a program another thread
    /// still has open for writing while a process is forked cannot be started ("Text file busy").
    static PROGRAMS: Mutex<()> = Mutex::new(());

    struct FakeYtDlp {
        cli: YtDlp,
        directory: PathBuf,
        _turn: MutexGuard<'static, ()>,
    }

    impl Drop for FakeYtDlp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    /// A stand-in for yt-dlp running `script`, with the arguments in `"$@"`.
    fn fake(name: &str, script: &str) -> FakeYtDlp {
        let turn = PROGRAMS.lock().unwrap_or_else(PoisonError::into_inner);
        let directory =
            std::env::temp_dir().join(format!("atoll-yt-dlp-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("downloads")).unwrap();
        let program = directory.join("yt-dlp");
        fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        FakeYtDlp {
            cli: YtDlp::new(&program),
            directory,
            _turn: turn,
        }
    }

    fn request(fake: &FakeYtDlp) -> VideoRequest {
        VideoRequest {
            url: "https://videos.example/watch?v=1".to_owned(),
            directory: fake.directory.join("downloads"),
            name: Some("Katze".to_owned()),
            referer: Some("https://videos.example/".to_owned()),
        }
    }

    #[test]
    fn progress_name_and_file_are_reported() {
        let fake = fake(
            "run",
            r#"echo "$@" > "$(dirname "$0")/arguments"
echo "ATOLL-NAME /downloads/Katze.mp4"
echo "[download] something else"
echo "ATOLL-PROGRESS 10 100 NA 5"
echo "ATOLL-PROGRESS 100 100 NA 5"
echo "ATOLL-FILE /downloads/Katze.mp4""#,
        );
        let mut events = Vec::new();
        let file = fake
            .cli
            .download(request(&fake))
            .run(|event| events.push(event))
            .unwrap();
        assert_eq!(file, PathBuf::from("/downloads/Katze.mp4"));
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0],
            VideoEvent::Named(PathBuf::from("/downloads/Katze.mp4"))
        );
        assert_eq!(
            events[2],
            VideoEvent::Progress(Progress {
                received_bytes: 100,
                total_bytes: 100,
                speed: 5,
            })
        );
        let arguments = fs::read_to_string(fake.directory.join("arguments")).unwrap();
        assert!(arguments.contains("-o Katze.%(ext)s"));
        assert!(arguments.contains("--referer https://videos.example/"));
        assert!(
            arguments
                .trim_end()
                .ends_with("-- https://videos.example/watch?v=1")
        );
        let leftovers: Vec<_> = fs::read_dir(fake.directory.join("downloads"))
            .unwrap()
            .collect();
        assert!(leftovers.is_empty(), "the workspace is removed");
    }

    #[test]
    fn the_last_error_line_explains_a_failure() {
        let fake = fake(
            "fail",
            "echo 'WARNING: [generic] Falling back' >&2\n\
             echo 'ERROR: [generic] x: HTTP Error 404: Not Found' >&2\n\
             exit 1",
        );
        assert_eq!(
            fake.cli.download(request(&fake)).run(|_| {}),
            Err(VideoError::Failed(
                "x: HTTP Error 404: Not Found".to_owned()
            ))
        );
    }

    #[test]
    fn success_without_a_file_is_a_failure() {
        let fake = fake("nothing", "exit 0");
        assert!(matches!(
            fake.cli.download(request(&fake)).run(|_| {}),
            Err(VideoError::Failed(_))
        ));
    }

    #[test]
    fn cancelling_stops_the_process_and_its_children() {
        let fake = fake(
            "cancel",
            "echo 'ATOLL-PROGRESS 1 NA NA NA'\nsleep 30 &\nsleep 30\necho 'ATOLL-FILE /x.mp4'",
        );
        let download = fake.cli.download(request(&fake));
        let canceller = download.canceller();
        let started = Instant::now();
        let result = download.run(|_| canceller.cancel());
        assert_eq!(result, Err(VideoError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_missing_program_is_reported() {
        let _turn = PROGRAMS.lock().unwrap_or_else(PoisonError::into_inner);
        let cli = YtDlp::new("/nonexistent/atoll/yt-dlp");
        assert_eq!(cli.version(), Err(VideoError::Missing));
        let request = VideoRequest {
            url: "https://videos.example/".to_owned(),
            directory: std::env::temp_dir(),
            name: None,
            referer: None,
        };
        assert_eq!(cli.download(request).run(|_| {}), Err(VideoError::Missing));
    }
}
