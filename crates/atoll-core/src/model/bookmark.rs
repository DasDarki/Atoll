use crate::ids::BookmarkId;

/// A bookmark or a bookmark folder. Folders have no address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub id: BookmarkId,
    /// The containing folder, `None` for entries of the bookmarks bar.
    pub parent_id: Option<BookmarkId>,
    pub title: String,
    pub url: Option<String>,
    pub position: i64,
}

impl Bookmark {
    pub fn is_folder(&self) -> bool {
        self.url.is_none()
    }
}
