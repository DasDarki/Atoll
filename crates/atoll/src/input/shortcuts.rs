use slint::winit_030::winit::event::{ElementState, KeyEvent};
use slint::winit_030::winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use slint::winit_030::winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

/// Application wide keyboard shortcuts, handled before input reaches the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    NewWindow,
    CloseWindow,
    Quit,
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
    SearchPages,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    DevTools,
    ToggleFullscreen,
    ToggleSidebar,
    BookmarkPage,
    ToggleBookmarksBar,
    ManageBookmarks,
    QuickSearch,
    Commands,
    Settings,
    Home,
    Downloads,
    History,
    NewPrivateTab,
    Passwords,
    DownloadVideo,
}

impl Shortcut {
    /// Whether the shortcut stays with Atoll while the developer tools have focus. The others
    /// belong to the developer tools, for example Ctrl+F for searching or Ctrl+L for clearing
    /// the console.
    pub fn applies_in_dev_tools(self) -> bool {
        matches!(
            self,
            Shortcut::NewWindow
                | Shortcut::CloseWindow
                | Shortcut::Quit
                | Shortcut::NewTab
                | Shortcut::CloseTab
                | Shortcut::ReopenClosedTab
                | Shortcut::NextTab
                | Shortcut::PreviousTab
                | Shortcut::Reload
                | Shortcut::ReloadIgnoringCache
                | Shortcut::DevTools
                | Shortcut::ToggleFullscreen
                | Shortcut::ToggleSidebar
                | Shortcut::ToggleBookmarksBar
                | Shortcut::QuickSearch
                | Shortcut::Settings
                | Shortcut::Home
                | Shortcut::Downloads
                | Shortcut::History
        )
    }

    /// Whether holding the keys repeats the shortcut. Opening or closing windows does not, a key
    /// held a little too long would otherwise close the next window as well.
    pub fn repeats(self) -> bool {
        !matches!(
            self,
            Shortcut::NewWindow | Shortcut::CloseWindow | Shortcut::Quit
        )
    }
}

/// The key combination of `event` written the way extensions write it in their manifest, for
/// example `Ctrl+Shift+G`. Only combinations with Control or Alt are named, plain typing is not.
pub fn describe(event: &KeyEvent, modifiers: ModifiersState) -> Option<String> {
    if event.state != ElementState::Pressed {
        return None;
    }
    let control = modifiers.control_key();
    let alt = modifiers.alt_key();
    if !control && !alt {
        return None;
    }
    let key = match &event.logical_key {
        Key::Named(named) => format!("{named:?}"),
        Key::Character(_) => event.key_without_modifiers().to_text()?.to_uppercase(),
        _ => return None,
    };
    let mut keys = Vec::new();
    if control {
        keys.push("Ctrl");
    }
    if modifiers.shift_key() {
        keys.push("Shift");
    }
    if alt {
        keys.push("Alt");
    }
    keys.push(&key);
    Some(keys.join("+"))
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
            ("n", false) => Shortcut::NewWindow,
            ("n", true) => Shortcut::NewPrivateTab,
            ("w", true) => Shortcut::CloseWindow,
            ("q", true) => Shortcut::Quit,
            ("t", false) => Shortcut::NewTab,
            ("t", true) => Shortcut::ReopenClosedTab,
            ("w", false) => Shortcut::CloseTab,
            ("l", false) => Shortcut::FocusAddress,
            ("r", false) => Shortcut::Reload,
            ("r", true) => Shortcut::ReloadIgnoringCache,
            ("f", false) => Shortcut::Find,
            ("f", true) => Shortcut::SearchPages,
            ("i", true) => Shortcut::DevTools,
            ("b", false) => Shortcut::ToggleSidebar,
            ("b", true) => Shortcut::ToggleBookmarksBar,
            ("d", false) => Shortcut::BookmarkPage,
            ("o", true) => Shortcut::ManageBookmarks,
            ("k", false) => Shortcut::QuickSearch,
            ("p", true) => Shortcut::Commands,
            (",", false) => Shortcut::Settings,
            ("j", false) => Shortcut::Downloads,
            ("h", false) => Shortcut::History,
            ("l", true) => Shortcut::Passwords,
            ("s", true) => Shortcut::DownloadVideo,
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
        NamedKey::Home if alt && !control => Shortcut::Home,
        NamedKey::BrowserHome => Shortcut::Home,
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
