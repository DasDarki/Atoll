use std::rc::Rc;

use atoll_core::{BookmarkId, DownloadId, IslandId, TabId, WorkspaceId};
use slint::ComponentHandle;

use crate::browser::Browser;
use crate::ui::{
    AppWindow, BrowserState, DevToolsState, OverlayState, SettingsState, SidebarState,
};

/// Connects every callback of the user interface to the browser controller.
pub fn connect(window: &AppWindow, browser: &Rc<Browser>) {
    connect_sidebar(window, browser);
    connect_browser(window, browser);
    connect_overlays(window, browser);
    connect_settings(window, browser);
    connect_dev_tools(window, browser);
}

macro_rules! on {
    ($state:expr, $setter:ident, $browser:expr, |$target:ident $(, $argument:ident)*| $body:expr) => {{
        let weak = Rc::downgrade($browser);
        $state.$setter(move |$($argument),*| {
            if let Some($target) = weak.upgrade() {
                $body
            }
        });
    }};
    ($state:expr, $setter:ident, $browser:expr, |$target:ident $(, $argument:ident)*| -> $default:expr, $body:expr) => {{
        let weak = Rc::downgrade($browser);
        $state.$setter(move |$($argument),*| match weak.upgrade() {
            Some($target) => $body,
            None => $default,
        });
    }};
}

fn workspace(id: i32) -> WorkspaceId {
    WorkspaceId(i64::from(id))
}

fn island(id: i32) -> IslandId {
    IslandId(i64::from(id))
}

fn tab(id: i32) -> TabId {
    TabId(i64::from(id))
}

fn bookmark(id: i32) -> BookmarkId {
    BookmarkId(i64::from(id))
}

fn connect_sidebar(window: &AppWindow, browser: &Rc<Browser>) {
    let state = window.global::<SidebarState>();
    on!(state, on_select_workspace, browser, |target, id| target
        .select_workspace(workspace(id)));
    on!(state, on_create_workspace, browser, |target| target
        .create_workspace());
    on!(state, on_workspace_action, browser, |target, id, action| {
        target.workspace_action(workspace(id), &action)
    });
    on!(
        state,
        on_move_island_to_workspace,
        browser,
        |target, id, destination| target
            .move_island_to_workspace(island(id), workspace(destination))
    );
    on!(state, on_activate_tab, browser, |target, id| target
        .activate_tab(tab(id)));
    on!(state, on_close_tab, browser, |target, id| target
        .close_tab(tab(id)));
    on!(
        state,
        on_tab_action,
        browser,
        |target, id, action| match action.as_str() {
            "reload" => target.reload_tab(tab(id)),
            "sleep" => target.sleep_tab(tab(id)),
            "pin" => target.toggle_pin(tab(id)),
            "duplicate" => target.duplicate_tab(tab(id)),
            "copy-url" => target.copy_tab_address(tab(id)),
            "new-island" => target.move_tab_to_new_island(tab(id)),
            "bookmark" => target.bookmark_tab(tab(id)),
            "new-window" => target.move_tab_to_new_window(tab(id), None),
            other => tracing::warn!(action = other, "unknown tab action"),
        }
    );
    on!(
        state,
        on_move_tab_to_island,
        browser,
        |target, id, destination| target.move_tab_to_island(tab(id), island(destination))
    );
    on!(
        state,
        on_move_tab_to_workspace,
        browser,
        |target, id, destination| target.move_tab_to_workspace(tab(id), workspace(destination))
    );
    on!(state, on_tab_drag_released, browser, |target, id, x, y| {
        target.tab_drag_released(tab(id), x, y)
    });
    on!(state, on_toggle_island, browser, |target, id| target
        .toggle_island(island(id)));
    on!(
        state,
        on_island_action,
        browser,
        |target, id, action| match action.as_str() {
            "bookmark" => target.bookmark_island(island(id)),
            "new-window" => target.move_island_to_new_window(island(id)),
            other => target.island_action(island(id), other),
        }
    );
    on!(state, on_set_island_color, browser, |target, id, color| {
        target.set_island_color(island(id), usize::try_from(color).unwrap_or_default())
    });
    on!(state, on_create_island, browser, |target| target
        .create_island());
    on!(state, on_open_archive, browser, |target| target
        .open_archive());
    on!(state, on_open_import, browser, |target| target
        .open_import());
    on!(state, on_open_export, browser, |target| target
        .open_export(None));
}

fn connect_browser(window: &AppWindow, browser: &Rc<Browser>) {
    let state = window.global::<BrowserState>();
    on!(state, on_navigate, browser, |target, input| target
        .navigate(&input));
    on!(state, on_toggle_sidebar, browser, |target| target
        .toggle_sidebar());
    on!(state, on_go_home, browser, |target| target.go_home());
    on!(state, on_open_downloads, browser, |target| target
        .open_downloads());
    on!(state, on_show_site_info, browser, |target| target
        .show_site_info());
    on!(state, on_show_passwords, browser, |target| target
        .show_passwords());
    on!(state, on_window_drag, browser, |target| target
        .window_drag());
    on!(state, on_window_control, browser, |target, action| target
        .window_control(&action));
    on!(state, on_window_resize, browser, |target, edge| target
        .window_resize(&edge));
    on!(state, on_address_edited, browser, |target, source, text| -> Default::default(), target.address_edited(source, &text));
    on!(
        state,
        on_address_submitted,
        browser,
        |target, _source, text| target.address_submitted(&text)
    );
    on!(state, on_suggestion_move, browser, |target, delta| -> Default::default(), target.suggestion_move(delta));
    on!(state, on_suggestion_activate, browser, |target, index| {
        target.suggestion_activate(index)
    });
    on!(state, on_suggestions_dismissed, browser, |target| target
        .dismiss_suggestions());
    on!(state, on_toggle_bookmark, browser, |target| target
        .bookmark_page());
    on!(state, on_open_bookmark, browser, |target, id, new_tab| {
        target.open_bookmark(bookmark(id), new_tab)
    });
    on!(
        state,
        on_open_bookmark_folder,
        browser,
        |target, id, x, y| target.open_bookmark_folder(bookmark(id), x, y)
    );
    on!(state, on_bookmark_action, browser, |target, id, action| {
        target.bookmark_action(bookmark(id), &action)
    });
    on!(
        state,
        on_bookmark_dropped,
        browser,
        |target, dragged, destination, placement| target.bookmark_dropped(
            bookmark(dragged),
            bookmark(destination),
            placement
        )
    );
    on!(state, on_go_back, browser, |target| target.go_back());
    on!(state, on_go_forward, browser, |target| target.go_forward());
    on!(state, on_reload, browser, |target, ignore_cache| target
        .reload(ignore_cache));
    on!(state, on_stop, browser, |target| target.stop());
    on!(state, on_menu_action, browser, |target, action| target
        .menu_action(&action));
    on!(state, on_web_pointer, browser, |target, pointer| target
        .web_pointer(pointer));
    on!(state, on_web_scroll, browser, |target, scroll| -> false, target.web_scroll(scroll));
    on!(state, on_web_hover, browser, |target, inside, x, y| target
        .web_hover(inside, x, y));
    on!(state, on_web_resized, browser, |target, width, height| {
        target.resize_pages(width, height)
    });
    on!(state, on_web_focus_changed, browser, |target, focused| {
        target.web_focus_changed(focused)
    });
    on!(state, on_find, browser, |target, text, forward| target
        .find(&text, forward));
    on!(state, on_close_find, browser, |target| target.close_find());
}

fn connect_overlays(window: &AppWindow, browser: &Rc<Browser>) {
    let state = window.global::<OverlayState>();
    on!(state, on_site_info_close, browser, |target| target
        .close_site_info());
    on!(state, on_download_action, browser, |target, id, action| {
        target.download_action(DownloadId(i64::from(id)), &action)
    });
    on!(state, on_downloads_clear, browser, |target| target
        .clear_downloads());
    on!(state, on_downloads_open_folder, browser, |target| target
        .open_download_folder());
    on!(state, on_downloads_close, browser, |target| target
        .close_downloads());
    on!(state, on_site_info_reset_zoom, browser, |target| target
        .site_info_reset_zoom());
    on!(state, on_site_info_clear_data, browser, |target| target
        .site_info_clear_data());
    on!(state, on_site_info_toggle_adblock, browser, |target| target
        .toggle_site_adblock());
    on!(state, on_upload_clipboard, browser, |target| target
        .upload_clipboard_image());
    on!(state, on_upload_file, browser, |target, index| target
        .upload_recent_file(index));
    on!(state, on_upload_browse, browser, |target| target
        .upload_browse());
    on!(state, on_upload_cancel, browser, |target| target
        .upload_cancel());
    on!(
        state,
        on_passwords_query_edited,
        browser,
        |target, query| target.password_query_changed(&query)
    );
    on!(state, on_passwords_choose, browser, |target, index| target
        .choose_password(index));
    on!(state, on_passwords_close, browser, |target| target
        .close_passwords());
    on!(state, on_palette_edited, browser, |target, text| target
        .palette_edited(&text));
    on!(state, on_palette_move, browser, |target, delta| target
        .palette_move(delta));
    on!(state, on_palette_activate, browser, |target, index| target
        .palette_activate(index));
    on!(state, on_palette_close, browser, |target| target
        .close_palette());
    on!(
        state,
        on_folder_popup_activate,
        browser,
        |target, id, new_tab| target.folder_popup_activate(bookmark(id), new_tab)
    );
    on!(state, on_folder_popup_up, browser, |target| target
        .folder_popup_up());
    on!(state, on_folder_popup_close, browser, |target| target
        .close_folder_popup());
    on!(state, on_bookmark_save, browser, |target| target
        .save_bookmark_editor());
    on!(state, on_bookmark_remove, browser, |target| target
        .remove_bookmark_in_editor());
    on!(state, on_bookmark_editor_new_folder, browser, |target| {
        target.bookmark_editor_new_folder()
    });
    on!(state, on_bookmark_cancel, browser, |target| target
        .cancel_bookmark_editor());
    on!(
        state,
        on_bookmark_row_action,
        browser,
        |target, id, action| target.bookmark_action(bookmark(id), &action)
    );
    on!(state, on_bookmark_new_folder, browser, |target| target
        .new_bookmark_folder());
    on!(state, on_bookmark_import, browser, |target| target
        .import_bookmarks());
    on!(state, on_bookmark_manager_close, browser, |target| target
        .close_bookmark_manager());
    on!(state, on_menu_activated, browser, |target, command| target
        .context_menu_closed(Some(command)));
    on!(state, on_menu_dismissed, browser, |target| target
        .context_menu_closed(None));
    on!(
        state,
        on_dialog_closed,
        browser,
        |target, accepted, text| target.dialog_closed(accepted, text.to_string())
    );
    on!(state, on_toast_dismissed, browser, |target| target
        .hide_toast());
    on!(state, on_archive_restore, browser, |target, id| target
        .restore_island(island(id)));
    on!(state, on_archive_delete, browser, |target, id| target
        .delete_archived_island(island(id)));
    on!(state, on_archive_keep, browser, |target, id| target
        .keep_island(island(id)));
    on!(state, on_archive_suggested, browser, |target, id| target
        .archive_suggested_island(island(id)));
    on!(state, on_archive_all_suggested, browser, |target| target
        .archive_all_suggested());
    on!(state, on_archive_close, browser, |target| target
        .close_archive());
    on!(state, on_import_text_edited, browser, |target, text| target
        .import_text_edited(&text));
    on!(state, on_import_choose_file, browser, |target| target
        .import_choose_file());
    on!(state, on_import_paste, browser, |target| target
        .import_paste());
    on!(state, on_import_confirm, browser, |target| target
        .import_confirm());
    on!(state, on_import_cancel, browser, |target| target
        .import_cancel());
    on!(state, on_report_close, browser, |target| target
        .close_report());
    on!(state, on_export_changed, browser, |target| target
        .export_changed());
    on!(state, on_export_toggle_island, browser, |target, id| target
        .export_toggle_island(id));
    on!(state, on_export_confirm, browser, |target| target
        .export_confirm());
    on!(state, on_export_cancel, browser, |target| target
        .export_cancel());
}

fn connect_settings(window: &AppWindow, browser: &Rc<Browser>) {
    let state = window.global::<SettingsState>();
    on!(state, on_changed, browser, |target, key| target
        .setting_changed(&key));
    on!(state, on_use_current_page, browser, |target| target
        .use_current_page_as_start_page());
    on!(state, on_clear_history, browser, |target| target
        .clear_history());
    on!(state, on_clear_cookies, browser, |target| target
        .clear_cookies());
    on!(state, on_choose_download_directory, browser, |target| {
        target.choose_download_directory()
    });
    on!(state, on_reset_download_directory, browser, |target| target
        .reset_download_directory());
    on!(state, on_update_filter_lists, browser, |target| target
        .update_filter_lists());
    on!(state, on_clear_adblock_exceptions, browser, |target| target
        .clear_adblock_exceptions());
    on!(state, on_close, browser, |target| target.close_settings());
}

fn connect_dev_tools(window: &AppWindow, browser: &Rc<Browser>) {
    let state = window.global::<DevToolsState>();
    on!(state, on_pointer, browser, |target, pointer| target
        .dev_tools_pointer(pointer));
    on!(state, on_scroll, browser, |target, scroll| -> false, target.dev_tools_scroll(scroll));
    on!(state, on_hover, browser, |target, inside, x, y| target
        .dev_tools_hover(inside, x, y));
    on!(state, on_resized, browser, |target, width, height| target
        .dev_tools_resized(width, height));
    on!(state, on_focus_changed, browser, |target, focused| target
        .dev_tools_focus_changed(focused));
    on!(state, on_close, browser, |target| target
        .close_active_dev_tools());
    on!(state, on_toggle_dock, browser, |target| target
        .toggle_dev_tools_dock());
    on!(state, on_size_committed, browser, |target, size| target
        .dev_tools_size_committed(size));
}
