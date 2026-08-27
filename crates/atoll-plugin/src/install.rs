//! Installing extensions from a zip archive or from a git address.
//!
//! Both ways land in a staging folder next to the installed extensions first. Only once the
//! manifest of what arrived has been read and accepted does the folder move to its final place,
//! so a broken package never leaves half an extension behind.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::manifest::{MANIFEST_NAME, Manifest, ManifestError};

/// How much an archive may hold, unpacked.
const MAX_UNPACKED: u64 = 64 * 1024 * 1024;

/// How many files an archive may hold.
const MAX_FILES: usize = 2000;

/// Where an installed extension came from, as it is written into the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The user put the folder there.
    Folder,
    /// Unpacked from an archive.
    Archive,
    /// Cloned from this address.
    Git(String),
}

impl Source {
    /// Reads back what [`Source::as_str`] wrote.
    pub fn parse(text: &str) -> Self {
        match text {
            "folder" => Source::Folder,
            "archive" => Source::Archive,
            other => Source::Git(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Source::Folder => "folder",
            Source::Archive => "archive",
            Source::Git(url) => url,
        }
    }

    /// The address it was cloned from, if it came from git.
    pub fn git_url(&self) -> Option<&str> {
        match self {
            Source::Git(url) => Some(url),
            _ => None,
        }
    }
}

/// What an installation left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub manifest: Manifest,
    pub folder: PathBuf,
    /// Whether an extension of the same name was already there.
    pub replaced: bool,
}

/// Why an extension could not be installed.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("{0}")]
    Unreadable(String),
    #[error("the archive could not be read: {0}")]
    BrokenArchive(String),
    #[error("the package holds no {MANIFEST_NAME}")]
    NoManifest,
    #[error("the package holds more than one extension")]
    ManyManifests,
    #[error("the package is too big, it may hold at most {} MB", MAX_UNPACKED / 1024 / 1024)]
    TooBig,
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("\"{0}\" is not a git address")]
    BadAddress(String),
    #[error("git is not installed")]
    NoGit,
    #[error("git: {0}")]
    Git(String),
    #[error("this extension was not installed from git")]
    NotFromGit,
}

/// Unpacks `archive` into `directory` and answers with the extension that arrived.
pub fn install_archive(archive: &Path, directory: &Path) -> Result<Installed, InstallError> {
    let staging = Staging::new(directory)?;
    unpack(archive, staging.path())?;
    adopt(staging, directory)
}

/// Clones `url` into `directory` and answers with the extension that arrived.
pub fn install_git(url: &str, directory: &Path) -> Result<Installed, InstallError> {
    check_address(url)?;
    let staging = Staging::new(directory)?;
    git(
        Path::new("."),
        &[
            "clone",
            "--depth",
            "1",
            "--quiet",
            "--",
            url,
            &staging.path().display().to_string(),
        ],
    )?;
    adopt(staging, directory)
}

/// Fetches what changed for an extension that came from git, and says whether anything did.
pub fn update_git(folder: &Path) -> Result<bool, InstallError> {
    if !folder.join(".git").exists() {
        return Err(InstallError::NotFromGit);
    }
    let before = git(folder, &["rev-parse", "HEAD"])?;
    git(folder, &["pull", "--ff-only", "--quiet"])?;
    let after = git(folder, &["rev-parse", "HEAD"])?;
    Ok(before.trim() != after.trim())
}

/// Reads the manifest of the staged package and moves it to where it belongs.
fn adopt(staging: Staging, directory: &Path) -> Result<Installed, InstallError> {
    let root = find_root(staging.path())?;
    let manifest = read_manifest(&root)?;
    let folder = directory.join(&manifest.id);
    let replaced = folder.exists();
    if replaced {
        std::fs::remove_dir_all(&folder).map_err(unreadable)?;
    }
    std::fs::rename(&root, &folder).map_err(unreadable)?;
    Ok(Installed {
        manifest,
        folder,
        replaced,
    })
}

fn read_manifest(folder: &Path) -> Result<Manifest, InstallError> {
    let text = std::fs::read_to_string(folder.join(MANIFEST_NAME)).map_err(unreadable)?;
    Ok(Manifest::parse(&text)?)
}

/// The folder of the package: either what was unpacked, or the one folder inside it that holds a
/// manifest, because archives usually carry the project folder along.
fn find_root(staging: &Path) -> Result<PathBuf, InstallError> {
    if staging.join(MANIFEST_NAME).is_file() {
        return Ok(staging.to_path_buf());
    }
    let mut found = None;
    for entry in std::fs::read_dir(staging).map_err(unreadable)? {
        let path = entry.map_err(unreadable)?.path();
        if !path.join(MANIFEST_NAME).is_file() {
            continue;
        }
        if found.is_some() {
            return Err(InstallError::ManyManifests);
        }
        found = Some(path);
    }
    found.ok_or(InstallError::NoManifest)
}

/// Unpacks an archive, after looking at how much it would become. The zip reader refuses entries
/// that would leave the folder, so nothing can be written past `into`.
fn unpack(archive: &Path, into: &Path) -> Result<(), InstallError> {
    let file = std::fs::File::open(archive).map_err(unreadable)?;
    let mut zip = zip::ZipArchive::new(file).map_err(broken)?;
    if zip.len() > MAX_FILES {
        return Err(InstallError::TooBig);
    }
    let mut unpacked = 0u64;
    for index in 0..zip.len() {
        let entry = zip.by_index_raw(index).map_err(broken)?;
        if entry.enclosed_name().is_none() {
            return Err(InstallError::BrokenArchive(format!(
                "\"{}\" would leave the folder",
                entry.name()
            )));
        }
        unpacked = unpacked.saturating_add(entry.size());
        if unpacked > MAX_UNPACKED {
            return Err(InstallError::TooBig);
        }
    }
    zip.extract(into).map_err(broken)
}

/// Runs git and answers with what it printed.
fn git(folder: &Path, arguments: &[&str]) -> Result<String, InstallError> {
    let output = Command::new("git")
        .current_dir(folder)
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("GCM_INTERACTIVE", "never")
        .output()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => InstallError::NoGit,
            _ => InstallError::Unreadable(error.to_string()),
        })?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        let message = message.trim();
        let message = message.lines().last().unwrap_or("").trim();
        return Err(InstallError::Git(if message.is_empty() {
            "the command failed".to_owned()
        } else {
            message.to_owned()
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Only addresses git can fetch from, and never one that git would read as an option.
fn check_address(url: &str) -> Result<(), InstallError> {
    let refused = || InstallError::BadAddress(url.to_owned());
    if url.starts_with('-') || url.contains(char::is_whitespace) {
        return Err(refused());
    }
    let schemes = ["https://", "http://", "ssh://", "git://", "file://"];
    if schemes.iter().any(|scheme| url.starts_with(scheme)) {
        return Ok(());
    }
    let scp = url
        .split_once(':')
        .is_some_and(|(host, path)| host.contains('@') && !path.is_empty());
    if scp { Ok(()) } else { Err(refused()) }
}

fn unreadable(error: io::Error) -> InstallError {
    InstallError::Unreadable(error.to_string())
}

fn broken(error: zip::result::ZipError) -> InstallError {
    InstallError::BrokenArchive(error.to_string())
}

/// A folder for a package that is being installed. It is removed again unless the install took it.
struct Staging(PathBuf);

impl Staging {
    fn new(directory: &Path) -> Result<Self, InstallError> {
        std::fs::create_dir_all(directory).map_err(unreadable)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default();
        let path = directory.join(format!(".incoming-{stamp}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).map_err(unreadable)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("atoll-install-{name}-{stamp}"));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const MANIFEST: &str = "id = \"demo\"\nname = \"Demo\"\nversion = \"1.0\"\napi = 1\n";

    fn archive(path: &Path, entries: &[(&str, &str)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        for (name, content) in entries {
            zip.start_file::<_, ()>(*name, zip::write::FileOptions::default())
                .unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn an_archive_becomes_a_folder_named_after_the_extension() {
        let scratch = Scratch::new("archive");
        let package = scratch.path().join("demo.zip");
        archive(
            &package,
            &[
                ("demo-main/plugin.toml", MANIFEST),
                ("demo-main/main.luau", "atoll.log(\"da\")"),
            ],
        );
        let plugins = scratch.path().join("plugins");
        let installed = install_archive(&package, &plugins).unwrap();
        assert_eq!(installed.manifest.id, "demo");
        assert_eq!(installed.folder, plugins.join("demo"));
        assert!(!installed.replaced);
        assert!(plugins.join("demo/main.luau").is_file());
        assert_eq!(
            std::fs::read_dir(&plugins).unwrap().count(),
            1,
            "nothing is left over from unpacking"
        );
    }

    #[test]
    fn installing_again_replaces_what_was_there() {
        let scratch = Scratch::new("again");
        let plugins = scratch.path().join("plugins");
        let first = scratch.path().join("first.zip");
        archive(
            &first,
            &[("plugin.toml", MANIFEST), ("old.luau", "veraltet")],
        );
        install_archive(&first, &plugins).unwrap();

        let second = scratch.path().join("second.zip");
        archive(
            &second,
            &[(
                "plugin.toml",
                "id = \"demo\"\nname = \"Demo\"\nversion = \"2.0\"\napi = 1\n",
            )],
        );
        let installed = install_archive(&second, &plugins).unwrap();
        assert!(installed.replaced);
        assert_eq!(installed.manifest.version, "2.0");
        assert!(
            !plugins.join("demo/old.luau").exists(),
            "the old files are gone"
        );
    }

    #[test]
    fn a_package_without_a_manifest_is_refused() {
        let scratch = Scratch::new("empty");
        let package = scratch.path().join("leer.zip");
        archive(&package, &[("liesmich.txt", "nichts")]);
        let plugins = scratch.path().join("plugins");
        let error = install_archive(&package, &plugins).unwrap_err();
        assert!(matches!(error, InstallError::NoManifest));
        assert_eq!(
            std::fs::read_dir(&plugins).unwrap().count(),
            0,
            "a refused package leaves nothing behind"
        );
    }

    #[test]
    fn a_package_with_two_extensions_is_refused() {
        let scratch = Scratch::new("two");
        let package = scratch.path().join("zwei.zip");
        archive(
            &package,
            &[
                ("eins/plugin.toml", MANIFEST),
                (
                    "zwei/plugin.toml",
                    "id = \"other\"\nname = \"Other\"\nversion = \"1\"\napi = 1\n",
                ),
            ],
        );
        let error = install_archive(&package, &scratch.path().join("plugins")).unwrap_err();
        assert!(matches!(error, InstallError::ManyManifests));
    }

    #[test]
    fn a_broken_manifest_stops_the_installation() {
        let scratch = Scratch::new("broken");
        let package = scratch.path().join("kaputt.zip");
        archive(
            &package,
            &[("plugin.toml", "id = \"Demo Extension\"\napi = 1\n")],
        );
        let error = install_archive(&package, &scratch.path().join("plugins")).unwrap_err();
        assert!(matches!(error, InstallError::Manifest(_)), "{error}");
    }

    #[test]
    fn only_addresses_git_can_fetch_are_taken() {
        assert!(check_address("https://example.org/demo.git").is_ok());
        assert!(check_address("git@example.org:user/demo.git").is_ok());
        assert!(check_address("file:///tmp/demo").is_ok());
        assert!(check_address("--upload-pack=touch").is_err());
        assert!(check_address("/etc/passwd").is_err());
        assert!(check_address("https://example.org/a b").is_err());
    }

    #[test]
    fn what_a_source_is_survives_the_database() {
        assert_eq!(Source::parse("folder"), Source::Folder);
        assert_eq!(Source::parse("archive"), Source::Archive);
        let git = Source::parse("https://example.org/demo.git");
        assert_eq!(git.git_url(), Some("https://example.org/demo.git"));
        assert_eq!(Source::parse(git.as_str()), git);
        assert_eq!(Source::Folder.git_url(), None);
    }

    /// Clones from a repository on this machine, so the test needs no network.
    #[test]
    fn an_extension_is_cloned_and_updated_from_git() {
        let scratch = Scratch::new("git");
        let repository = scratch.path().join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        let run = |arguments: &[&str]| {
            let output = Command::new("git")
                .current_dir(&repository)
                .args(arguments)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.org")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.org")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {arguments:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "--quiet", "--initial-branch", "main"]);
        std::fs::write(repository.join(MANIFEST_NAME), MANIFEST).unwrap();
        std::fs::write(repository.join("main.luau"), "atoll.log(\"eins\")").unwrap();
        run(&["add", "."]);
        run(&["commit", "--quiet", "-m", "start"]);

        let plugins = scratch.path().join("plugins");
        let url = format!("file://{}", repository.display());
        let installed = install_git(&url, &plugins).unwrap();
        assert_eq!(installed.folder, plugins.join("demo"));
        assert!(
            installed.folder.join(".git").exists(),
            "updates stay possible"
        );

        assert!(
            !update_git(&installed.folder).unwrap(),
            "nothing changed yet"
        );
        std::fs::write(repository.join("main.luau"), "atoll.log(\"zwei\")").unwrap();
        run(&["commit", "--quiet", "-am", "more"]);
        assert!(update_git(&installed.folder).unwrap());
        assert_eq!(
            std::fs::read_to_string(installed.folder.join("main.luau")).unwrap(),
            "atoll.log(\"zwei\")"
        );
    }

    #[test]
    fn a_folder_without_git_cannot_be_updated() {
        let scratch = Scratch::new("nogit");
        let error = update_git(scratch.path()).unwrap_err();
        assert!(matches!(error, InstallError::NotFromGit));
    }
}
