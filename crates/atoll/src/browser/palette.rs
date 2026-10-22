use std::collections::HashSet;
use std::rc::Rc;

use atoll_core::address::{display_url, normalize};
use atoll_core::search::Matcher;
use atoll_core::{Bookmark, TabId, TabState, Timestamp, VisitedPage, WorkspaceId};
use atoll_engine::ZoomCommand;
use atoll_storage::TabEntry;
use atoll_storage::prelude::*;
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Browser;
use super::preferences::{PreferenceKey, SEARCH_ENGINES};
use super::suggestions::frecency;
use crate::ui::{OverlayState, PaletteItem, PaletteKind, SidebarState};
use atoll_i18n::t;

const COMMAND_PREFIX: char = '>';
const MAX_TABS: usize = 30;
const MAX_BOOKMARKS: usize = 10;
const MAX_PAGES: usize = 10;
const HISTORY_SOURCE: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    NewWindow,
    CloseWindow,
    Quit,
    MoveTabToNewWindow,
    OpenWorkspaceWindow(WorkspaceId),
    NewTab,
    CloseTab,
    ReopenTab,
    NewIsland,
    NewWorkspace,
    SwitchWorkspace(WorkspaceId),
    ToggleSidebar,
    ToggleBookmarksBar,
    BookmarkPage,
    ManageBookmarks,
    NewBookmarkFolder,
    OpenDownloads,
    OpenHistory,
    NewPrivateTab,
    NewTorTab,
    OpenShield,
    SearchPages,
    OpenArchivedPage,
    ImportFromOpera,
    OpenDownloadFolder,
    ImportList,
    ExportList,
    OpenArchive,
    ArchiveIsland,
    SleepIsland,
    Find,
    Reload,
    HardReload,
    DevTools,
    Fullscreen,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ClearHistory,
    ClearCookies,
    OpenSettings,
    GoHome,
    UseCurrentPageAsStartPage,
    ToggleHomeButton,
    ToggleNewTabStartPage,
    ToggleBookmarksBarAutoHide,
    SearchEngine(usize),
    FillPassword,
    DownloadVideo,
    ToggleSiteAdblock,
    ToggleAdblock,
    UpdateFilterLists,
    /// A command an extension added: the extension and the command inside it.
    Plugin(String, String),
    /// A panel an extension brings along: the extension and the panel.
    PluginPanel(String, String),
}

#[derive(Debug, Clone)]
enum Entry {
    Tab(TabId),
    Url(String),
    Command(Command),
}

/// The command palette: a quick search over every tab, bookmark and visited page, and a list of
/// commands after a leading `>`.
#[derive(Default)]
pub struct PaletteState {
    entries: Vec<Entry>,
    selected: usize,
    tabs: Vec<TabEntry>,
    bookmarks: Vec<Bookmark>,
    pages: Vec<VisitedPage>,
}

impl Browser {
    pub fn open_palette(&self, commands: bool) {
        let Some(window) = self.window() else { return };
        self.dismiss_suggestions();
        {
            let mut state = self.palette.borrow_mut();
            state.tabs = self.storage.all_tab_entries().unwrap_or_default();
            state.bookmarks = self.storage.bookmarks().unwrap_or_default();
            state.pages = self
                .storage
                .visited_pages(HISTORY_SOURCE)
                .unwrap_or_default();
        }
        let text = if commands {
            COMMAND_PREFIX.to_string()
        } else {
            String::new()
        };
        let overlay = window.global::<OverlayState>();
        overlay.set_palette_text(text.as_str().into());
        overlay.set_palette_visible(true);
        self.palette_edited(&text);
    }

    pub fn close_palette(&self) {
        *self.palette.borrow_mut() = PaletteState::default();
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_palette_visible(false);
        }
        self.focus_content();
    }

    pub fn palette_edited(&self, text: &str) {
        let (items, entries, hint) = match text.strip_prefix(COMMAND_PREFIX) {
            Some(query) => {
                let (items, entries) = self.command_results(query.trim());
                (items, entries, t!("Kein passender Befehl"))
            }
            None => {
                let (items, entries) = self.search_results(text.trim());
                (items, entries, t!("Nichts gefunden"))
            }
        };
        {
            let mut state = self.palette.borrow_mut();
            state.entries = entries;
            state.selected = 0;
        }
        if let Some(window) = self.window() {
            let overlay = window.global::<OverlayState>();
            overlay.set_palette_items(ModelRc::from(Rc::new(VecModel::from(items))));
            overlay.set_palette_index(0);
            overlay.set_palette_hint(hint.into());
        }
    }

    pub fn palette_move(&self, delta: i32) {
        let selected = {
            let mut state = self.palette.borrow_mut();
            let last = state.entries.len().saturating_sub(1) as i64;
            state.selected = (state.selected as i64 + i64::from(delta)).clamp(0, last) as usize;
            state.selected
        };
        if let Some(window) = self.window() {
            window
                .global::<OverlayState>()
                .set_palette_index(selected as i32);
        }
    }

    pub fn palette_activate(&self, index: i32) {
        let entry = usize::try_from(index)
            .ok()
            .and_then(|index| self.palette.borrow().entries.get(index).cloned());
        let Some(entry) = entry else { return };
        self.close_palette();
        match entry {
            Entry::Tab(tab) => self.reveal_tab(tab),
            Entry::Url(url) => self.open_address(&url),
            Entry::Command(command) => self.run_command(command),
        }
    }

    /// Opens `url` in the active tab if that is blank, otherwise in a new tab.
    pub(super) fn open_address(&self, url: &str) {
        let active_is_blank = {
            let session = self.session.borrow();
            session
                .active_tab
                .and_then(|tab| session.tab(tab))
                .is_none_or(|tab| tab.is_blank())
        };
        if active_is_blank {
            self.navigate(url);
        } else {
            let active = self.session.borrow().active_tab;
            self.open_in_new_tab(active, url, true);
        }
    }

    fn search_results(&self, query: &str) -> (Vec<PaletteItem>, Vec<Entry>) {
        let matcher = Matcher::new(query);
        let state = self.palette.borrow();
        let session = self.session.borrow();
        let lifecycle = self.lifecycle.borrow();
        let mut favicons = self.favicons.borrow_mut();
        let now = Timestamp::now();
        let mut results: Vec<(PaletteItem, Entry)> = Vec::new();
        let mut seen = HashSet::new();

        // Opening a tab is what the palette is asked for most often, so it stands above the
        // search as long as the search is not about something else.
        let new_tab = t!("Neuer Tab");
        if matcher.is_empty()
            || matcher
                .score(&[new_tab, t!("tab öffnen neu new blank leer")])
                .is_some()
        {
            results.push((
                PaletteItem {
                    kind: PaletteKind::Command,
                    title: new_tab.into(),
                    shortcut: t!("Strg+T").into(),
                    ..PaletteItem::default()
                },
                Entry::Command(Command::NewTab),
            ));
        }

        let mut tabs: Vec<(i64, &TabEntry)> = state
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(recency, entry)| {
                let title = entry.tab.display_title();
                let location = format!("{} {}", entry.workspace_name, entry.island.name);
                let score = if matcher.is_empty() {
                    -(recency as i64)
                } else {
                    matcher.score(&[&title, &display_url(&entry.tab.url), &location])?
                        + if entry.island.workspace_id == session.workspace {
                            8
                        } else {
                            0
                        }
                        - if entry.island.is_archived() { 6 } else { 0 }
                };
                Some((score, entry))
            })
            .collect();
        tabs.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        for (_, entry) in tabs.into_iter().take(MAX_TABS) {
            let state = lifecycle.state_of(entry.tab.id, entry.island.is_archived());
            let badge = if session.active_tab == Some(entry.tab.id) {
                t!("Aktiv")
            } else {
                match state {
                    TabState::Live => t!("Aktiv"),
                    TabState::Frozen => t!("Eingefroren"),
                    TabState::Sleeping => "",
                    TabState::Archived => t!("Archiviert"),
                }
            };
            let icon = favicons.image_for_tab(&self.storage, &entry.tab);
            seen.insert(normalize(&entry.tab.url));
            results.push((
                PaletteItem {
                    kind: PaletteKind::Tab,
                    title: entry.tab.display_title().into(),
                    detail: format!(
                        "{} › {} · {}",
                        entry.workspace_name,
                        entry.island.name,
                        display_url(&entry.tab.url)
                    )
                    .into(),
                    badge: badge.into(),
                    has_icon: icon.is_some(),
                    icon: icon.unwrap_or_default(),
                    ..PaletteItem::default()
                },
                Entry::Tab(entry.tab.id),
            ));
        }

        if !matcher.is_empty() {
            let mut bookmarks: Vec<(i64, &Bookmark, &str)> = state
                .bookmarks
                .iter()
                .filter_map(|bookmark| {
                    let url = bookmark.url.as_deref()?;
                    let score = matcher.score(&[&bookmark.title, &display_url(url)])?;
                    Some((score, bookmark, url))
                })
                .collect();
            bookmarks.sort_by_key(|entry| std::cmp::Reverse(entry.0));
            for (_, bookmark, url) in bookmarks.into_iter().take(MAX_BOOKMARKS) {
                if !seen.insert(normalize(url)) {
                    continue;
                }
                let icon = favicons.image_for_url(&self.storage, url);
                results.push((
                    PaletteItem {
                        kind: PaletteKind::Bookmark,
                        title: bookmark.title.as_str().into(),
                        detail: display_url(url).into(),
                        badge: t!("Lesezeichen").into(),
                        has_icon: icon.is_some(),
                        icon: icon.unwrap_or_default(),
                        ..PaletteItem::default()
                    },
                    Entry::Url(url.to_owned()),
                ));
            }

            let mut pages: Vec<(i64, &VisitedPage)> = state
                .pages
                .iter()
                .filter_map(|page| {
                    let title = page.title.clone().unwrap_or_default();
                    let score =
                        matcher.score(&[&title, &display_url(&page.url)])? + frecency(page, now);
                    Some((score, page))
                })
                .collect();
            pages.sort_by_key(|entry| std::cmp::Reverse(entry.0));
            let mut added = 0;
            for (_, page) in pages {
                if added == MAX_PAGES {
                    break;
                }
                if !seen.insert(normalize(&page.url)) {
                    continue;
                }
                added += 1;
                let icon = favicons.image_for_url(&self.storage, &page.url);
                results.push((
                    PaletteItem {
                        kind: PaletteKind::History,
                        title: page
                            .title
                            .clone()
                            .unwrap_or_else(|| display_url(&page.url))
                            .into(),
                        detail: display_url(&page.url).into(),
                        badge: t!("Verlauf").into(),
                        has_icon: icon.is_some(),
                        icon: icon.unwrap_or_default(),
                        ..PaletteItem::default()
                    },
                    Entry::Url(page.url.clone()),
                ));
            }
        }
        results.into_iter().unzip()
    }

    fn command_results(&self, query: &str) -> (Vec<PaletteItem>, Vec<Entry>) {
        let matcher = Matcher::new(query);
        let entries: Vec<(Command, String, String, String)> = self
            .commands()
            .into_iter()
            .map(|(command, title, shortcut, keywords)| {
                (command, title, shortcut.to_owned(), keywords.to_owned())
            })
            .chain(self.plugin_commands())
            .collect();
        let mut commands: Vec<(i64, PaletteItem, Entry)> = entries
            .into_iter()
            .enumerate()
            .filter_map(|(index, (command, title, shortcut, keywords))| {
                let score = if matcher.is_empty() {
                    -(index as i64)
                } else {
                    matcher.score(&[&title, &keywords])?
                };
                Some((
                    score,
                    PaletteItem {
                        kind: PaletteKind::Command,
                        title: title.into(),
                        shortcut: shortcut.into(),
                        ..PaletteItem::default()
                    },
                    Entry::Command(command),
                ))
            })
            .collect();
        commands.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        // Ein neuer Tab ist der haeufigste Wunsch an die Palette und steht deshalb vor allem
        // anderen, sobald er ueberhaupt gemeint sein kann.
        if let Some(at) = commands
            .iter()
            .position(|(_, _, entry)| matches!(entry, Entry::Command(Command::NewTab)))
        {
            let favourite = commands.remove(at);
            commands.insert(0, favourite);
        }
        commands
            .into_iter()
            .map(|(_, item, entry)| (item, entry))
            .unzip()
    }

    /// The commands and panels extensions added, in the order they registered them.
    fn plugin_commands(&self) -> Vec<(Command, String, String, String)> {
        let commands = self
            .plugins
            .commands()
            .into_iter()
            .map(|(plugin, command)| {
                (
                    Command::Plugin(plugin, command.id),
                    command.title,
                    command.shortcut.unwrap_or_default(),
                    t!("erweiterung plugin").to_owned(),
                )
            });
        let panels = self.plugins.panels().into_iter().map(|(plugin, panel)| {
            (
                Command::PluginPanel(plugin, panel.id),
                panel.title,
                String::new(),
                t!("erweiterung plugin panel fenster").to_owned(),
            )
        });
        commands.chain(panels).collect()
    }

    fn commands(&self) -> Vec<(Command, String, &'static str, &'static str)> {
        let Some(window) = self.window() else {
            return Vec::new();
        };
        let sidebar_visible = window.global::<SidebarState>().get_visible();
        let preferences = self.preferences.borrow().clone();
        let bar_visible = preferences.bookmarks_bar_visible;
        let mut commands = vec![
            (
                Command::NewWindow,
                t!("Neues Fenster").to_owned(),
                t!("Strg+N"),
                "window workspace",
            ),
            (
                Command::CloseWindow,
                t!("Fenster schließen").to_owned(),
                t!("Strg+Umschalt+W"),
                "window",
            ),
            (
                Command::Quit,
                t!("Atoll beenden").to_owned(),
                t!("Strg+Umschalt+Q"),
                t!("exit schließen alle fenster"),
            ),
            (
                Command::NewTab,
                t!("Neuer Tab").to_owned(),
                t!("Strg+T"),
                t!("tab öffnen"),
            ),
            (
                Command::CloseTab,
                t!("Tab schließen").to_owned(),
                t!("Strg+W"),
                "tab",
            ),
            (
                Command::ReopenTab,
                t!("Geschlossenen Tab wiederherstellen").to_owned(),
                t!("Strg+Umschalt+T"),
                t!("rückgängig"),
            ),
            (
                Command::NewIsland,
                t!("Neue Insel").to_owned(),
                "",
                "gruppe",
            ),
            (
                Command::NewWorkspace,
                t!("Neuer Workspace").to_owned(),
                "",
                "kontext",
            ),
            (
                Command::ToggleSidebar,
                if sidebar_visible {
                    t!("Sidebar ausblenden")
                } else {
                    t!("Sidebar einblenden")
                }
                .to_owned(),
                t!("Strg+B"),
                "seitenleiste einklappen",
            ),
            (
                Command::ToggleBookmarksBar,
                if bar_visible {
                    t!("Lesezeichenleiste ausblenden")
                } else {
                    t!("Lesezeichenleiste einblenden")
                }
                .to_owned(),
                t!("Strg+Umschalt+B"),
                "bookmarks",
            ),
            (
                Command::BookmarkPage,
                t!("Seite als Lesezeichen speichern").to_owned(),
                t!("Strg+D"),
                "bookmark stern",
            ),
            (
                Command::ManageBookmarks,
                t!("Lesezeichen verwalten").to_owned(),
                t!("Strg+Umschalt+O"),
                "bookmarks import",
            ),
            (
                Command::NewBookmarkFolder,
                t!("Neuer Lesezeichenordner").to_owned(),
                "",
                t!("bookmarks ordner folder"),
            ),
            (
                Command::ImportFromOpera,
                t!("Aus Opera übernehmen").to_owned(),
                "",
                t!("opera import tabs inseln lesezeichen verlauf"),
            ),
            (
                Command::NewPrivateTab,
                t!("Neuer privater Tab").to_owned(),
                t!("Strg+Umschalt+N"),
                t!("privat inkognito tab ohne spuren"),
            ),
            (
                Command::NewTorTab,
                t!("Neuer Tab über Tor").to_owned(),
                "",
                t!("tor privat anonym zwiebel"),
            ),
            (
                Command::OpenArchivedPage,
                t!("Archivfassung dieser Seite öffnen").to_owned(),
                "",
                t!("archiv wayback kopie alte fassung"),
            ),
            (
                Command::SearchPages,
                t!("Seiten dieses Arbeitsbereichs durchsuchen").to_owned(),
                t!("Strg+Umschalt+F"),
                t!("suchen seiten tabs inhalt text finden"),
            ),
            (
                Command::OpenHistory,
                t!("Verlauf").to_owned(),
                t!("Strg+H"),
                t!("history verlauf besuchte seiten"),
            ),
            (
                Command::OpenDownloads,
                t!("Downloads").to_owned(),
                t!("Strg+J"),
                t!("herunterladen dateien"),
            ),
            (
                Command::OpenDownloadFolder,
                t!("Download-Ordner öffnen").to_owned(),
                "",
                t!("herunterladen dateien"),
            ),
            (
                Command::ImportList,
                t!("URL-Liste importieren").to_owned(),
                "",
                "opera",
            ),
            (
                Command::ExportList,
                t!("URL-Liste exportieren").to_owned(),
                "",
                "backup",
            ),
            (
                Command::OpenArchive,
                t!("Archiv öffnen").to_owned(),
                "",
                t!("inseln"),
            ),
            (
                Command::ArchiveIsland,
                t!("Aktuelle Insel archivieren").to_owned(),
                "",
                "archiv",
            ),
            (
                Command::SleepIsland,
                t!("Tabs der aktuellen Insel schlafen legen").to_owned(),
                "",
                "speicher entladen",
            ),
            (
                Command::Find,
                t!("Auf Seite suchen").to_owned(),
                t!("Strg+F"),
                "finden",
            ),
            (
                Command::Reload,
                t!("Neu laden").to_owned(),
                "F5",
                "aktualisieren",
            ),
            (
                Command::HardReload,
                t!("Vollständig neu laden, ohne Cache").to_owned(),
                t!("Strg+F5"),
                "aktualisieren cache hard reload",
            ),
            (
                Command::Fullscreen,
                t!("Vollbild").to_owned(),
                "F11",
                "fullscreen",
            ),
            (
                Command::ZoomIn,
                t!("Vergrößern").to_owned(),
                t!("Strg++"),
                "zoom",
            ),
            (
                Command::ZoomOut,
                t!("Verkleinern").to_owned(),
                t!("Strg+-"),
                "zoom",
            ),
            (
                Command::ZoomReset,
                t!("Zoom zurücksetzen").to_owned(),
                t!("Strg+0"),
                "zoom",
            ),
            (
                Command::DevTools,
                t!("Entwicklertools").to_owned(),
                "F12",
                "devtools inspect",
            ),
            (
                Command::ClearHistory,
                t!("Verlauf löschen").to_owned(),
                "",
                "history datenschutz",
            ),
            (
                Command::ClearCookies,
                t!("Cookies löschen").to_owned(),
                "",
                "datenschutz abmelden",
            ),
            (
                Command::OpenSettings,
                t!("Einstellungen").to_owned(),
                t!("Strg+,"),
                "optionen preferences",
            ),
            (
                Command::GoHome,
                t!("Startseite öffnen").to_owned(),
                t!("Alt+Pos1"),
                "home",
            ),
            (
                Command::ToggleHomeButton,
                if preferences.home_button {
                    t!("Startseiten-Button ausblenden")
                } else {
                    t!("Startseiten-Button einblenden")
                }
                .to_owned(),
                "",
                "home symbolleiste einstellung",
            ),
            (
                Command::ToggleBookmarksBarAutoHide,
                if preferences.bookmarks_bar_auto_hide {
                    t!("Lesezeichenleiste immer anzeigen")
                } else {
                    t!("Lesezeichenleiste nur mit Lesezeichen anzeigen")
                }
                .to_owned(),
                "",
                "bookmarks leer ausblenden einstellung",
            ),
        ];
        if self.session.borrow().active_tab.is_some() {
            commands.push((
                Command::MoveTabToNewWindow,
                t!("Tab in neues Fenster verschieben").to_owned(),
                "",
                "window abtrennen",
            ));
        }
        if self.current_page_url().is_some() {
            commands.push((
                Command::FillPassword,
                t!("Mit 1Password anmelden").to_owned(),
                t!("Strg+Umschalt+L"),
                t!("passwort login zugangsdaten ausfüllen"),
            ));
            commands.push((
                Command::DownloadVideo,
                t!("Video herunterladen").to_owned(),
                t!("Strg+Umschalt+S"),
                t!("video speichern download film clip"),
            ));
        }
        commands.push((
            Command::ToggleAdblock,
            if self.protections_on("").banners {
                t!("Werbeblocker ausschalten")
            } else {
                t!("Werbeblocker einschalten")
            }
            .to_owned(),
            "",
            "adblock werbung tracker einstellung",
        ));
        commands.push((
            Command::UpdateFilterLists,
            t!("Filterlisten aktualisieren").to_owned(),
            "",
            "adblock werbeblocker easylist",
        ));
        if let Some(url) = self
            .current_page_url()
            .filter(|url| url.starts_with("http"))
            && let Some(site) = atoll_adblock::site_of(&url)
        {
            commands.push((
                Command::ToggleSiteAdblock,
                if self.protections_on(&url).active {
                    t!("Schutz für {} ausschalten", site)
                } else {
                    t!("Schutz für {} einschalten", site)
                },
                "",
                t!("adblock werbung schutz seite ausnahme"),
            ));
            commands.push((
                Command::OpenShield,
                t!("Schutz für diese Seite einstellen").to_owned(),
                "",
                t!("schutz adblock popups weiterleitungen tabs"),
            ));
        }
        if self.current_page_url().is_some() {
            commands.push((
                Command::UseCurrentPageAsStartPage,
                t!("Aktuelle Seite als Startseite festlegen").to_owned(),
                "",
                "home einstellung",
            ));
        }
        if preferences.has_start_page() {
            commands.push((
                Command::ToggleNewTabStartPage,
                if preferences.new_tab_start_page {
                    t!("Neue Tabs mit der Atoll-Startseite öffnen")
                } else {
                    t!("Neue Tabs mit der Startseite öffnen")
                }
                .to_owned(),
                "",
                "home einstellung",
            ));
        }
        let current_engine = preferences.search_engine_index();
        for (index, engine) in SEARCH_ENGINES.iter().enumerate() {
            if index != current_engine {
                commands.push((
                    Command::SearchEngine(index),
                    t!("Suchmaschine: {}", engine.name),
                    "",
                    t!("suche einstellung"),
                ));
            }
        }
        let shown_elsewhere = self.workspaces_shown_elsewhere();
        let session = self.session.borrow();
        for workspace in session
            .workspaces
            .iter()
            .filter(|workspace| workspace.id != session.workspace)
        {
            if shown_elsewhere.contains(&workspace.id) {
                commands.push((
                    Command::SwitchWorkspace(workspace.id),
                    t!("Zum Fenster wechseln: {}", workspace.name),
                    "",
                    "workspace window",
                ));
                continue;
            }
            commands.push((
                Command::SwitchWorkspace(workspace.id),
                t!("Zu Workspace wechseln: {}", workspace.name),
                "",
                "workspace",
            ));
            commands.push((
                Command::OpenWorkspaceWindow(workspace.id),
                t!("In neuem Fenster öffnen: {}", workspace.name),
                "",
                "workspace window",
            ));
        }
        commands
    }

    fn run_command(&self, command: Command) {
        match command {
            Command::NewWindow => self.open_new_window(),
            Command::CloseWindow => self.close_window(),
            Command::Quit => self.quit(),
            Command::MoveTabToNewWindow => {
                let active = self.session.borrow().active_tab;
                if let Some(tab) = active {
                    self.move_tab_to_new_window(tab, None);
                }
            }
            Command::OpenWorkspaceWindow(workspace) => self.open_workspace_in_window(workspace),
            Command::NewTab => self.new_tab(),
            Command::CloseTab => self.close_active_tab(),
            Command::ReopenTab => self.reopen_closed_tab(),
            Command::NewIsland => self.create_island(),
            Command::NewWorkspace => self.create_workspace(),
            Command::SwitchWorkspace(workspace) => self.select_workspace(workspace),
            Command::ToggleSidebar => self.toggle_sidebar(),
            Command::ToggleBookmarksBar => self.toggle_bookmarks_bar(),
            Command::BookmarkPage => self.bookmark_page(),
            Command::ManageBookmarks => self.open_bookmark_manager(),
            Command::NewBookmarkFolder => self.new_bookmark_folder(),
            Command::OpenDownloads => self.open_downloads(),
            Command::OpenHistory => self.open_history(),
            Command::NewPrivateTab => self.new_private_tab(),
            Command::NewTorTab => self.new_tor_tab(),
            Command::OpenArchivedPage => self.open_archived_page(),
            Command::ImportFromOpera => self.import_from_opera(),
            Command::OpenDownloadFolder => self.open_download_folder(),
            Command::ImportList => self.open_import(),
            Command::ExportList => self.open_export(None),
            Command::OpenArchive => self.open_archive(),
            Command::ArchiveIsland => {
                if let Some(island) = self.active_island() {
                    self.island_action(island, "archive");
                }
            }
            Command::SleepIsland => {
                if let Some(island) = self.active_island() {
                    self.island_action(island, "sleep");
                }
            }
            Command::Find => self.open_find(),
            Command::Reload => self.reload(false),
            Command::HardReload => self.reload(true),
            Command::Fullscreen => self.toggle_fullscreen(),
            Command::ZoomIn => self.zoom(ZoomCommand::In),
            Command::ZoomOut => self.zoom(ZoomCommand::Out),
            Command::ZoomReset => self.zoom(ZoomCommand::Reset),
            Command::DevTools => self.toggle_dev_tools(),
            Command::ClearHistory => self.clear_history(),
            Command::ClearCookies => self.clear_cookies(),
            Command::OpenSettings => self.open_settings(),
            Command::GoHome => self.go_home(),
            Command::UseCurrentPageAsStartPage => self.use_current_page_as_start_page(),
            Command::ToggleHomeButton => {
                let visible = !self.preferences.borrow().home_button;
                self.set_home_button(visible);
            }
            Command::ToggleNewTabStartPage => {
                let enabled = !self.preferences.borrow().new_tab_start_page;
                self.update_preference(PreferenceKey::NewTabStartPage, |preferences| {
                    preferences.new_tab_start_page = enabled;
                });
            }
            Command::ToggleBookmarksBarAutoHide => {
                let auto_hide = !self.preferences.borrow().bookmarks_bar_auto_hide;
                self.set_bookmarks_bar_auto_hide(auto_hide);
            }
            Command::FillPassword => self.show_passwords(),
            Command::DownloadVideo => self.download_video(),
            Command::ToggleSiteAdblock => self.toggle_site_adblock(),
            Command::ToggleAdblock => {
                let enabled = !self.protections_on("").banners;
                self.set_adblock_enabled(enabled);
            }
            Command::OpenShield => self.show_shield(),
            Command::SearchPages => self.open_page_search(),
            Command::UpdateFilterLists => self.update_filter_lists(),
            Command::Plugin(plugin, command) => self.plugins.run_command(&plugin, &command),
            Command::PluginPanel(plugin, panel) => self.plugins.show_panel(&plugin, &panel),
            Command::SearchEngine(index) => {
                if let Some(engine) = SEARCH_ENGINES.get(index) {
                    self.set_search_url(engine.url);
                    self.show_toast(&t!("Suchmaschine: {}", engine.name));
                }
            }
        }
    }

    fn active_island(&self) -> Option<atoll_core::IslandId> {
        let session = self.session.borrow();
        session
            .active_tab
            .and_then(|tab| session.tab(tab))
            .map(|tab| tab.island_id)
    }
}
