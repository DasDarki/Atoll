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
            "devtools" => self.show_dev_tools(),
            other => tracing::warn!(action = other, "unknown menu action"),
        }
    }
}
