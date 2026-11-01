//! Blocking of ads and trackers with Brave's `adblock` engine and the EasyList filter lists.
//!
//! [`Blocker`] answers whether a request is blocked and which elements of a page are hidden.
//! [`Updater`] keeps its filter lists current on a background thread. [`judge`] weighs up a page
//! sending the user to another site by itself.

mod blocker;
mod cosmetic;
mod download;
pub mod judge;
mod lists;
mod policy;
mod updater;

pub use blocker::{Blocker, Cosmetics, domain_of, site_of};
pub use lists::{COOKIE_LISTS, FILTER_LISTS, FilterList};
pub use policy::{Effective, Policy, Protection, Protections, Scope, page_of};
pub use updater::{ListState, ListStatus, Updater};
