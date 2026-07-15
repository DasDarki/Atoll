use atoll_engine::ZoomCommand;

use super::Browser;

impl Browser {
    /// Runs an entry of the toolbar menu.
    pub fn menu_action(&self, action: &str) {
        match action {
            "new-tab" => self.new_tab(),
            "reopen-tab" => self.reopen_closed_tab(),
            "find" => self.open_find(),
            "zoom-in" => self.zoom(ZoomCommand::In),
            "zoom-out" => self.zoom(ZoomCommand::Out),
            "zoom-reset" => self.zoom(ZoomCommand::Reset),
            "import" => self.open_import(),
            "export" => self.open_export(None),
            "archive" => self.open_archive(),
            "devtools" => self.toggle_dev_tools(),
            "palette" => self.open_palette(false),
            "commands" => self.open_palette(true),
            "bookmark" => self.bookmark_page(),
            "bookmarks-bar" => self.toggle_bookmarks_bar(),
            "bookmarks" => self.open_bookmark_manager(),
            "sidebar" => self.toggle_sidebar(),
            "settings" => self.open_settings(),
            "downloads" => self.open_downloads(),
            "new-bookmark-folder" => self.new_bookmark_folder(),
            other => tracing::warn!(action = other, "unknown menu action"),
        }
    }
}
