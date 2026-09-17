//! Blocking of ads and trackers with Brave's `adblock` engine and the EasyList filter lists.
//!
//! [`Blocker`] answers whether a request is blocked and which elements of a page are hidden.
//! [`Updater`] keeps its filter lists current on a background thread.

mod blocker;
mod cosmetic;
mod download;
mod lists;
mod updater;

pub use blocker::{Blocker, Cosmetics, site_of};
pub use lists::{COOKIE_LISTS, FILTER_LISTS, FilterList};
pub use updater::{ListState, ListStatus, Updater};
