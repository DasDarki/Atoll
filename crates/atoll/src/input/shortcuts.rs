use slint::winit_030::winit::event::{ElementState, KeyEvent};
use slint::winit_030::winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use slint::winit_030::winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

/// Application wide keyboard shortcuts, handled before input reaches the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    NewTab,
    CloseTab,
    ReopenClosedTab,
    FocusAddress,
    Reload,
    ReloadIgnoringCache,
    Back,
    Forward,
    NextTab,
    PreviousTab,
    Find,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    DevTools,
    ToggleFullscreen,
}

pub fn detect(event: &KeyEvent, modifiers: ModifiersState) -> Option<Shortcut> {
    if event.state != ElementState::Pressed {
        return None;
    }
    let control = modifiers.control_key();
    let shift = modifiers.shift_key();
    let alt = modifiers.alt_key();

    if let Key::Named(named) = &event.logical_key {
        return named_shortcut(*named, control, shift, alt);
    }

    let base = match event.key_without_modifiers() {
        Key::Character(text) => text.to_lowercase(),
        _ => String::new(),
    };
    let physical = match event.physical_key {
        PhysicalKey::Code(code) => Some(code),
        PhysicalKey::Unidentified(_) => None,
    };

    if control && !alt {
        let shortcut = match (base.as_str(), shift) {
            ("t", false) => Shortcut::NewTab,
            ("t", true) => Shortcut::ReopenClosedTab,
            ("w", false) => Shortcut::CloseTab,
            ("l", false) => Shortcut::FocusAddress,
            ("r", false) => Shortcut::Reload,
            ("r", true) => Shortcut::ReloadIgnoringCache,
            ("f", false) => Shortcut::Find,
            ("i", true) => Shortcut::DevTools,
            ("+" | "=", _) => Shortcut::ZoomIn,
            ("-", _) => Shortcut::ZoomOut,
            ("0", false) => Shortcut::ZoomReset,
            _ => match physical {
                Some(KeyCode::NumpadAdd) => Shortcut::ZoomIn,
                Some(KeyCode::NumpadSubtract) => Shortcut::ZoomOut,
                Some(KeyCode::Digit0 | KeyCode::Numpad0) => Shortcut::ZoomReset,
                _ => return None,
            },
        };
        return Some(shortcut);
    }

    if alt && !control && base == "d" {
        return Some(Shortcut::FocusAddress);
    }
    None
}

fn named_shortcut(key: NamedKey, control: bool, shift: bool, alt: bool) -> Option<Shortcut> {
    let shortcut = match key {
        NamedKey::Tab if control => {
            if shift {
                Shortcut::PreviousTab
            } else {
                Shortcut::NextTab
            }
        }
        NamedKey::PageDown if control => Shortcut::NextTab,
        NamedKey::PageUp if control => Shortcut::PreviousTab,
        NamedKey::ArrowLeft if alt && !control => Shortcut::Back,
        NamedKey::ArrowRight if alt && !control => Shortcut::Forward,
        NamedKey::BrowserBack => Shortcut::Back,
        NamedKey::BrowserForward => Shortcut::Forward,
        NamedKey::BrowserRefresh => Shortcut::Reload,
        NamedKey::F4 if control => Shortcut::CloseTab,
        NamedKey::F5 if control || shift => Shortcut::ReloadIgnoringCache,
        NamedKey::F5 => Shortcut::Reload,
        NamedKey::F6 => Shortcut::FocusAddress,
        NamedKey::F11 => Shortcut::ToggleFullscreen,
        NamedKey::F12 => Shortcut::DevTools,
        _ => return None,
    };
    Some(shortcut)
}
