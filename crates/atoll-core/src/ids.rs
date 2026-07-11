use std::fmt;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub i64);

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}", self.0)
            }
        }
    };
}

define_id!(
    /// Identifies a workspace, the top level of the organisation hierarchy.
    WorkspaceId
);
define_id!(
    /// Identifies an island, a named group of tabs inside a workspace.
    IslandId
);
define_id!(
    /// Identifies a tab record.
    TabId
);
define_id!(
    /// Identifies a cached favicon.
    FaviconId
);
define_id!(
    /// Identifies a bookmark or a bookmark folder.
    BookmarkId
);
define_id!(
    /// Identifies an entry of the download list.
    DownloadId
);
