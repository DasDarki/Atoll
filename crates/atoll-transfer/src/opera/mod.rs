//! Taking over what Opera has: its open tabs with their islands, its bookmarks and its history.
//!
//! Opera keeps all three in its profile folder, in the formats Chromium uses. Nothing here
//! changes anything in that folder, every file is only read, and the history is copied away
//! first so that a running Opera is not disturbed.

mod bookmarks;
mod history;
mod session;

use std::path::{Path, PathBuf};

pub use bookmarks::read_bookmarks;
pub use history::{OperaVisit, read_history};
pub use session::{OperaIsland, read_session};

/// Why something of Opera could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OperaError {
    #[error("there is no Opera profile in {0}")]
    NoProfile(String),
    #[error("Opera keeps no session of its last tabs")]
    NoSession,
    #[error("the file is not an Opera session")]
    NotASession,
    #[error("{0}")]
    Unreadable(String),
}

/// What an Opera profile holds for Atoll.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperaContents {
    pub islands: Vec<OperaIsland>,
    pub bookmarks: Vec<crate::BookmarkNode>,
    pub visits: Vec<OperaVisit>,
    /// What could not be read, in the words the user gets to see.
    pub problems: Vec<String>,
}

impl OperaContents {
    pub fn tab_count(&self) -> usize {
        self.islands.iter().map(|island| island.urls.len()).sum()
    }

    pub fn bookmark_count(&self) -> usize {
        self.bookmarks
            .iter()
            .map(crate::BookmarkNode::link_count)
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.tab_count() == 0 && self.bookmark_count() == 0 && self.visits.is_empty()
    }
}

/// The profile folder Opera uses, or the one `ATOLL_OPERA_DIR` names.
pub fn profile_directory() -> Option<PathBuf> {
    if let Some(directory) = std::env::var_os("ATOLL_OPERA_DIR") {
        return Some(PathBuf::from(directory));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [
        ".config/opera",
        ".config/opera-beta",
        ".config/opera-developer",
    ]
    .into_iter()
    .map(|path| home.join(path).join("Default"))
    .find(|directory| directory.is_dir())
}

/// Reads everything Atoll can take over. A part that cannot be read is left empty and said so
/// under `problems`, because the other parts are still worth having.
pub fn read_profile(directory: &Path) -> Result<OperaContents, OperaError> {
    if !directory.is_dir() {
        return Err(OperaError::NoProfile(directory.display().to_string()));
    }
    let mut contents = OperaContents::default();
    match session::newest_session(directory).ok_or(OperaError::NoSession) {
        Ok(path) => match read_session(&path) {
            Ok(islands) => contents.islands = islands,
            Err(error) => contents.problems.push(error.to_string()),
        },
        Err(error) => contents.problems.push(error.to_string()),
    }
    match read_bookmarks(directory) {
        Ok(bookmarks) => contents.bookmarks = bookmarks,
        Err(error) => contents.problems.push(error.to_string()),
    }
    match read_history(directory) {
        Ok(visits) => contents.visits = visits,
        Err(error) => contents.problems.push(error.to_string()),
    }
    Ok(contents)
}
