use std::os::raw::c_int;

use cef::sys::{cef_context_menu_media_type_t, cef_menu_id_t, cef_menu_item_type_t};
use cef::{
    Browser, CefString, ContextMenuHandler, ContextMenuParams, EventFlags, Frame,
    ImplContextMenuHandler, ImplContextMenuParams, ImplMenuModel, MenuModel,
    RunContextMenuCallback, WrapContextMenuHandler, rc::Rc as _, wrap_context_menu_handler,
};

use crate::events::{ContextMenuRequest, EngineEvent, MenuEntry, MenuEntryKind, OpenDisposition};
use crate::registry::{self, PendingCallback};
use crate::view::ViewId;
use atoll_i18n::t;

const FIRST_COMMAND: i32 = cef_menu_id_t::MENU_ID_USER_FIRST as i32;
const OPEN_LINK_IN_TAB: i32 = FIRST_COMMAND;
const COPY_LINK: i32 = FIRST_COMMAND + 1;
const OPEN_IMAGE_IN_TAB: i32 = FIRST_COMMAND + 2;
const COPY_IMAGE_ADDRESS: i32 = FIRST_COMMAND + 3;
const SAVE_IMAGE: i32 = FIRST_COMMAND + 4;
const CONVERT_IMAGE: i32 = FIRST_COMMAND + 5;
const INSPECT: i32 = FIRST_COMMAND + 6;

wrap_context_menu_handler! {
    pub(crate) struct ViewContextMenuHandler {
        view: ViewId,
    }

    impl ContextMenuHandler {
        fn on_before_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            model: Option<&mut MenuModel>,
        ) {
            let (Some(params), Some(model)) = (params, model) else { return };
            let mut index = 0;
            let mut insert = |command: i32, label: &str| {
                model.insert_item_at(index, command, Some(&label.into()));
                index += 1;
            };
            if !link_url(params).is_empty() {
                insert(OPEN_LINK_IN_TAB, t!("Link in neuem Tab öffnen"));
                insert(COPY_LINK, t!("Linkadresse kopieren"));
            }
            if is_image(params) {
                insert(OPEN_IMAGE_IN_TAB, t!("Bild in neuem Tab öffnen"));
                insert(COPY_IMAGE_ADDRESS, t!("Bildadresse kopieren"));
                insert(SAVE_IMAGE, t!("Bild speichern"));
                insert(CONVERT_IMAGE, t!("Bild umwandeln und speichern"));
            }
            if index > 0 && model.count() > index {
                model.insert_separator_at(index);
            }
            let frontend = registry::with_view(self.view, |_, view| view.dev_tools_frontend.get());
            if frontend != Some(true) {
                model.add_separator();
                model.add_item(INSPECT, Some(&"Untersuchen".into()));
            }
        }

        fn run_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            model: Option<&mut MenuModel>,
            callback: Option<&mut RunContextMenuCallback>,
        ) -> c_int {
            let (Some(params), Some(model), Some(callback)) = (params, model, callback) else {
                return 0;
            };
            let Some(runtime) = registry::runtime() else { return 0 };
            let id = runtime.add_pending(PendingCallback::ContextMenu(callback.clone()));
            runtime.emit(EngineEvent::ContextMenu {
                view: self.view,
                request: ContextMenuRequest {
                    id,
                    x: params.xcoord(),
                    y: params.ycoord(),
                    entries: entries(model),
                },
            });
            1
        }

        fn on_context_menu_command(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            command_id: c_int,
            _event_flags: EventFlags,
        ) -> c_int {
            let Some(params) = params else { return 0 };
            let event = match command_id {
                OPEN_LINK_IN_TAB => open_in_tab(self.view, link_url(params)),
                COPY_LINK => EngineEvent::CopyText(link_url(params)),
                OPEN_IMAGE_IN_TAB => open_in_tab(self.view, source_url(params)),
                COPY_IMAGE_ADDRESS => EngineEvent::CopyText(source_url(params)),
                SAVE_IMAGE => EngineEvent::SaveUrl {
                    view: self.view,
                    url: source_url(params),
                    convert: false,
                },
                CONVERT_IMAGE => EngineEvent::SaveUrl {
                    view: self.view,
                    url: source_url(params),
                    convert: true,
                },
                INSPECT => EngineEvent::InspectElement {
                    view: self.view,
                    x: params.xcoord(),
                    y: params.ycoord(),
                },
                _ => return 0,
            };
            registry::emit(event);
            1
        }
    }
}

fn open_in_tab(view: ViewId, url: String) -> EngineEvent {
    EngineEvent::OpenUrl {
        view,
        url,
        disposition: OpenDisposition::BackgroundTab,
    }
}

fn link_url(params: &ContextMenuParams) -> String {
    CefString::from(&params.link_url()).to_string()
}

fn source_url(params: &ContextMenuParams) -> String {
    CefString::from(&params.source_url()).to_string()
}

fn is_image(params: &ContextMenuParams) -> bool {
    *params.media_type().as_ref() == cef_context_menu_media_type_t::CM_MEDIATYPE_IMAGE
        && !source_url(params).is_empty()
}

fn entries(model: &MenuModel) -> Vec<MenuEntry> {
    (0..model.count())
        .filter(|index| model.is_visible_at(*index) != 0)
        .map(|index| {
            let command_id = model.command_id_at(index);
            let label = clean_label(&CefString::from(&model.label_at(index)).to_string());
            let enabled = model.is_enabled_at(index) != 0;
            let kind = match *model.type_at(index).as_ref() {
                cef_menu_item_type_t::MENUITEMTYPE_SEPARATOR => MenuEntryKind::Separator,
                cef_menu_item_type_t::MENUITEMTYPE_CHECK
                | cef_menu_item_type_t::MENUITEMTYPE_RADIO => MenuEntryKind::Check {
                    checked: model.is_checked_at(index) != 0,
                },
                cef_menu_item_type_t::MENUITEMTYPE_SUBMENU => MenuEntryKind::Submenu(
                    model
                        .sub_menu_at(index)
                        .map(|menu| entries(&menu))
                        .unwrap_or_default(),
                ),
                _ => MenuEntryKind::Command,
            };
            MenuEntry {
                command_id,
                label,
                enabled,
                kind,
            }
        })
        .collect()
}

/// Removes the `&` mnemonic markers Chromium puts into menu labels.
fn clean_label(label: &str) -> String {
    let mut cleaned = String::with_capacity(label.len());
    let mut characters = label.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '&' {
            if characters.peek() == Some(&'&') {
                cleaned.push('&');
                characters.next();
            }
            continue;
        }
        cleaned.push(character);
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::clean_label;

    #[test]
    fn mnemonics_are_removed_and_escaped_ampersands_kept() {
        assert_eq!(clean_label("&Zurück"), "Zurück");
        assert_eq!(clean_label("Kopieren && Einfügen"), "Kopieren & Einfügen");
    }
}
