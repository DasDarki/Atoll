use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::{ProjectDirs, UserDirs};

/// Where Atoll keeps its data.
#[derive(Debug, Clone)]
pub struct Paths {
    pub database: PathBuf,
    pub engine_root: PathBuf,
    pub engine_profile: PathBuf,
    pub downloads: PathBuf,
}

impl Paths {
    /// Resolves the platform directories. `ATOLL_DATA_DIR` overrides the data directory.
    pub fn resolve() -> Result<Self> {
        let data = match std::env::var_os("ATOLL_DATA_DIR") {
            Some(directory) => PathBuf::from(directory),
            None => ProjectDirs::from("dev", "DasDarki", "Atoll")
                .context("no home directory found")?
                .data_dir()
                .to_path_buf(),
        };
        let downloads = UserDirs::new()
            .and_then(|directories| {
                directories
                    .download_dir()
                    .map(PathBuf::from)
                    .or_else(|| Some(directories.home_dir().join("Downloads")))
            })
            .unwrap_or_else(|| data.join("downloads"));
        let engine_root = data.join("engine");
        Ok(Self {
            database: data.join("atoll.db"),
            engine_profile: engine_root.join("Default"),
            engine_root,
            downloads,
        })
    }
}
