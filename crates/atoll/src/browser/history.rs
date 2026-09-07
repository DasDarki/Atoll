//! The history as a list to look through: the visited pages by day, with a search of their own,
//! and a way to forget a single one.
//!
//! The address bar and the palette search the same pages, but they only ever show a handful of
//! them. This is the place that shows all of them.

use std::rc::Rc;

use atoll_core::{Timestamp, VisitedPage, address::display_url};
use atoll_i18n::t;
use atoll_storage::prelude::*;
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Browser;
use crate::platform::clock::{LocalTime, local_time};
use crate::ui::{HistoryItem, OverlayState};

/// How many pages are read at once. More are fetched when the user asks for them.
const PAGE_SIZE: usize = 150;

/// What the open history view shows.
#[derive(Default)]
pub struct HistoryView {
    pages: Vec<VisitedPage>,
    query: String,
    /// Whether the last read filled a whole page, so there may be older entries.
    more: bool,
}

impl Browser {
    pub fn open_history(&self) {
        {
            let mut view = self.history.borrow_mut();
            view.query.clear();
            view.pages.clear();
            view.more = false;
        }
        self.read_history(None);
        if let Some(window) = self.window() {
            let overlay = window.global::<OverlayState>();
            overlay.set_history_query("".into());
            overlay.set_history_visible(true);
        }
    }

    pub fn close_history(&self) {
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_history_visible(false);
        }
        self.focus_content();
    }

    /// Searches inside the history. An empty text shows everything again.
    pub fn history_search(&self, query: &str) {
        {
            let mut view = self.history.borrow_mut();
            view.query = query.trim().to_owned();
            view.pages.clear();
        }
        self.read_history(None);
    }

    /// Reads the pages that come after the ones already shown.
    pub fn history_load_more(&self) {
        let oldest = self
            .history
            .borrow()
            .pages
            .last()
            .map(|page| page.last_visited_at);
        if oldest.is_some() {
            self.read_history(oldest);
        }
    }

    pub fn history_open(&self, index: i32, background: bool) {
        let Some(url) = self.history_url(index) else {
            return;
        };
        let active = self.session.borrow().active_tab;
        self.open_in_new_tab(active, &url, !background);
        if !background {
            self.close_history();
        }
    }

    /// Forgets one page, in the history and in the logs of the tabs.
    pub fn history_forget(&self, index: i32) {
        let Some(url) = self.history_url(index) else {
            return;
        };
        if let Err(error) = self.storage.forget_page(&url) {
            self.show_toast(&t!("Eintrag konnte nicht entfernt werden: {}", error));
            return;
        }
        self.history
            .borrow_mut()
            .pages
            .retain(|page| page.url != url);
        self.refresh_history();
    }

    /// Drops what the view shows, after the whole history was thrown away.
    pub(super) fn forget_shown_history(&self) {
        self.history.borrow_mut().pages.clear();
        self.refresh_history();
    }

    fn history_url(&self, index: i32) -> Option<String> {
        let index = usize::try_from(index).ok()?;
        self.history
            .borrow()
            .pages
            .get(index)
            .map(|page| page.url.clone())
    }

    /// Reads the next pages from the database and shows them. `before` continues an earlier read.
    fn read_history(&self, before: Option<Timestamp>) {
        let query = self.history.borrow().query.clone();
        let read = if query.is_empty() {
            self.storage.history_before(before, PAGE_SIZE)
        } else {
            self.storage
                .history_before_containing(&query, before, PAGE_SIZE)
        };
        let pages = match read {
            Ok(pages) => pages,
            Err(error) => {
                tracing::warn!(%error, "the history could not be read");
                Vec::new()
            }
        };
        {
            let mut view = self.history.borrow_mut();
            view.more = pages.len() == PAGE_SIZE;
            view.pages.extend(pages);
        }
        self.refresh_history();
    }

    pub(super) fn refresh_history(&self) {
        let Some(window) = self.window() else { return };
        let view = self.history.borrow();
        let today = local_time(Self::now().millis() / 1_000);
        let mut favicons = self.favicons.borrow_mut();
        let mut previous: Option<LocalTime> = None;
        let items: Vec<HistoryItem> = view
            .pages
            .iter()
            .map(|page| {
                let at = local_time(page.last_visited_at.millis() / 1_000);
                let day = match previous {
                    Some(before) if before.same_day(at) => String::new(),
                    _ => day_label(today, at),
                };
                previous = Some(at);
                let icon = favicons.image_for_url(&self.storage, &page.url);
                HistoryItem {
                    title: page
                        .title
                        .clone()
                        .filter(|title| !title.trim().is_empty())
                        .unwrap_or_else(|| display_url(&page.url))
                        .into(),
                    url: page.url.as_str().into(),
                    detail: display_url(&page.url).into(),
                    time: format!("{:02}:{:02}", at.hour, at.minute).into(),
                    day: day.into(),
                    has_icon: icon.is_some(),
                    icon: icon.unwrap_or_default(),
                }
            })
            .collect();
        let summary = if view.pages.is_empty() {
            String::new()
        } else {
            t!("{} Seiten", view.pages.len())
        };
        let overlay = window.global::<OverlayState>();
        overlay.set_history_items(ModelRc::from(Rc::new(VecModel::from(items))));
        overlay.set_history_summary(summary.as_str().into());
        overlay.set_history_more(view.more);
    }
}

/// The heading a day gets in the list.
fn day_label(today: LocalTime, at: LocalTime) -> String {
    if today.same_day(at) {
        return t!("Heute").to_owned();
    }
    if yesterday(today).is_some_and(|before| before.same_day(at)) {
        return t!("Gestern").to_owned();
    }
    t!(
        "{}, {}",
        weekday(at.weekday),
        format!("{:02}.{:02}.{}", at.day, at.month, at.year)
    )
}

/// The day before `today`, which the labels compare against.
fn yesterday(today: LocalTime) -> Option<LocalTime> {
    let seconds = days_since_epoch(today)? * 86_400 - 43_200;
    Some(local_time(seconds))
}

/// Days between 1970 and the date, for stepping one day back.
fn days_since_epoch(date: LocalTime) -> Option<i64> {
    let (year, month, day) = (date.year, i64::from(date.month), i64::from(date.day));
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

fn weekday(day: u32) -> &'static str {
    match day {
        0 => t!("Sonntag"),
        1 => t!("Montag"),
        2 => t!("Dienstag"),
        3 => t!("Mittwoch"),
        4 => t!("Donnerstag"),
        5 => t!("Freitag"),
        _ => t!("Samstag"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i64, month: u32, day: u32, weekday: u32) -> LocalTime {
        LocalTime {
            year,
            month,
            day,
            hour: 10,
            minute: 0,
            weekday,
        }
    }

    #[test]
    fn days_are_counted_from_the_epoch() {
        assert_eq!(days_since_epoch(date(1970, 1, 1, 4)), Some(0));
        assert_eq!(days_since_epoch(date(2025, 9, 18, 4)), Some(20_349));
        assert_eq!(
            days_since_epoch(date(2024, 3, 1, 5)).unwrap()
                - days_since_epoch(date(2024, 2, 29, 4)).unwrap(),
            1,
            "a leap day is a day like any other"
        );
    }

    #[test]
    fn the_newest_days_are_named_instead_of_dated() {
        // SAFETY: the tests of this module are the only ones that touch the time zone.
        unsafe { std::env::set_var("TZ", "UTC") };
        let today = date(2025, 9, 18, 4);
        assert_eq!(day_label(today, today), "Heute");
        assert_eq!(day_label(today, date(2025, 9, 17, 3)), "Gestern");
        assert_eq!(
            day_label(today, date(2025, 9, 15, 1)),
            "Montag, 15.09.2025",
            "older days carry their date"
        );
    }
}
