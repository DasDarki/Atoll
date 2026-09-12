//! The plain-text URL list format used to move tabs in and out of Atoll.
//!
//! ```text
//! --- Research ---
//! https://en.wikipedia.org/wiki/Web_browser
//!
//! # comments are ignored
//! https://example.org/without-island
//! ```

mod address;
mod bookmarks;
mod export;
pub mod opera;
mod parse;
mod plan;

pub use address::{InvalidReason, validate_address};
pub use bookmarks::{BookmarkNode, parse_bookmarks_html};
pub use export::{ExportGroup, ExportOptions, export};
pub use parse::{InvalidLine, ParsedGroup, ParsedList, parse};
pub use plan::{
    DuplicatePolicy, ExistingIsland, ExistingTab, ImportOptions, ImportPlan, ImportTarget,
    IslandDestination, IslandNamePolicy, PlannedEntry, PlannedIsland, SkipReason, SkippedUrl,
    plan_import,
};
