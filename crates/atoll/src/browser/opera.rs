//! Taking over what Opera has.
//!
//! Reading the profile takes a moment, so it happens on a thread of its own. What comes back is
//! shown with its numbers, and the user picks the parts to take over. Bookmarks and history go
//! straight in, the tabs land in the import window, where the workspace and the way duplicates
//! are handled are chosen as for any other list.

use atoll_core::{Timestamp, VisitedPage};
use atoll_i18n::t;
use atoll_storage::prelude::*;
use atoll_transfer::opera::{self, OperaContents};
use atoll_transfer::{ExportGroup, ExportOptions, export};
use slint::ComponentHandle;

use super::Browser;
use crate::platform::tasks::run_in_background;
use crate::ui::OverlayState;

/// What was read out of the Opera profile, until the user says what to do with it.
#[derive(Default)]
pub struct OperaImport {
    contents: Option<OperaContents>,
}

impl Browser {
    /// Reads the Opera profile and asks what to take over.
    pub fn import_from_opera(&self) {
        let Some(directory) = opera::profile_directory() else {
            self.show_toast(t!("Kein Opera-Profil gefunden"));
            return;
        };
        let this = self.this();
        self.show_toast(t!("Opera wird gelesen …"));
        run_in_background(
            "opera-import",
            move || opera::read_profile(&directory).map(|contents| (directory, contents)),
            move |result| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                match result {
                    Ok((directory, contents)) => browser.show_opera(&directory, contents),
                    Err(error) => browser.show_toast(&t!("Opera: {}", error)),
                }
            },
        );
    }

    /// Shows what Opera holds and lets the user pick the parts.
    fn show_opera(&self, directory: &std::path::Path, contents: OperaContents) {
        let Some(window) = self.window() else { return };
        if contents.is_empty() {
            self.show_toast(t!("In Opera ist nichts zum Übernehmen"));
            return;
        }
        let state = window.global::<OverlayState>();
        let tabs = contents.tab_count();
        let bookmarks = contents.bookmark_count();
        let visits = contents.visits.len();
        state.set_opera_profile(directory.display().to_string().as_str().into());
        state.set_opera_tabs_text(
            t!("{} Tabs in {} Inseln", tabs, contents.islands.len())
                .as_str()
                .into(),
        );
        state.set_opera_bookmarks_text(t!("{} Lesezeichen", bookmarks).as_str().into());
        state.set_opera_history_text(t!("{} Verlaufseinträge", visits).as_str().into());
        state.set_opera_tabs_available(tabs > 0);
        state.set_opera_bookmarks_available(bookmarks > 0);
        state.set_opera_history_available(visits > 0);
        state.set_opera_take_tabs(tabs > 0);
        state.set_opera_take_bookmarks(bookmarks > 0);
        state.set_opera_take_history(visits > 0);
        state.set_opera_problems(contents.problems.join("\n").as_str().into());
        state.set_opera_visible(true);
        *self.opera.borrow_mut() = OperaImport {
            contents: Some(contents),
        };
    }

    pub fn opera_cancel(&self) {
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_opera_visible(false);
        }
        self.opera.borrow_mut().contents = None;
        self.focus_content();
    }

    /// Takes over the parts the user left switched on.
    pub fn opera_confirm(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<OverlayState>();
        let (tabs, bookmarks, history) = (
            state.get_opera_take_tabs(),
            state.get_opera_take_bookmarks(),
            state.get_opera_take_history(),
        );
        state.set_opera_visible(false);
        let Some(contents) = self.opera.borrow_mut().contents.take() else {
            return;
        };
        let mut done: Vec<String> = Vec::new();
        if bookmarks {
            let imported = self.import_bookmark_nodes(&contents.bookmarks);
            done.push(t!("{} Lesezeichen", imported));
        }
        if history {
            let pages: Vec<VisitedPage> = contents
                .visits
                .iter()
                .map(|visit| VisitedPage {
                    url: visit.url.clone(),
                    title: visit.title.clone(),
                    visit_count: visit.visit_count,
                    last_visited_at: Timestamp(visit.last_visited_at),
                })
                .collect();
            match self.storage.import_visited_pages(&pages) {
                Ok(_) => done.push(t!("{} Verlaufseinträge", pages.len())),
                Err(error) => self.report(t!("Verlauf übernehmen"), Err::<(), _>(error)),
            }
        }
        if !done.is_empty() {
            self.show_toast(&t!("Aus Opera übernommen: {}", done.join(", ")));
        }
        if tabs && !contents.islands.is_empty() {
            self.open_import_with(&tab_list(&contents));
        } else {
            self.focus_content();
        }
    }
}

/// The islands of Opera as the URL list the import window reads.
fn tab_list(contents: &OperaContents) -> String {
    let groups: Vec<ExportGroup> = contents
        .islands
        .iter()
        .filter(|island| !island.urls.is_empty())
        .map(|island| ExportGroup {
            name: island.island_name(),
            urls: island.urls.clone(),
        })
        .collect();
    export(
        &groups,
        ExportOptions {
            island_markers: true,
        },
    )
}
