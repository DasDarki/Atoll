use cef::sys::cef_event_flags_t;

/// Keyboard and mouse button modifiers of an input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub meta: bool,
    pub alt_graph: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
    pub keypad: bool,
    pub repeat: bool,
}

impl Modifiers {
    pub(crate) fn event_flags(self) -> u32 {
        let mut flags = 0;
        let mut set = |enabled: bool, flag: cef_event_flags_t| {
            if enabled {
                flags |= flag.0;
            }
        };
        set(self.shift, cef_event_flags_t::EVENTFLAG_SHIFT_DOWN);
        set(self.control, cef_event_flags_t::EVENTFLAG_CONTROL_DOWN);
        set(self.alt, cef_event_flags_t::EVENTFLAG_ALT_DOWN);
        set(self.meta, cef_event_flags_t::EVENTFLAG_COMMAND_DOWN);
        set(self.alt_graph, cef_event_flags_t::EVENTFLAG_ALTGR_DOWN);
        set(self.caps_lock, cef_event_flags_t::EVENTFLAG_CAPS_LOCK_ON);
        set(self.num_lock, cef_event_flags_t::EVENTFLAG_NUM_LOCK_ON);
        set(self.keypad, cef_event_flags_t::EVENTFLAG_IS_KEY_PAD);
        set(self.repeat, cef_event_flags_t::EVENTFLAG_IS_REPEAT);
        flags
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

impl MouseButton {
    pub(crate) fn cef_type(self) -> cef::MouseButtonType {
        match self {
            MouseButton::Left => cef::MouseButtonType::LEFT,
            MouseButton::Middle => cef::MouseButtonType::MIDDLE,
            MouseButton::Right => cef::MouseButtonType::RIGHT,
        }
    }

    pub(crate) fn event_flag(self) -> u32 {
        match self {
            MouseButton::Left => cef_event_flags_t::EVENTFLAG_LEFT_MOUSE_BUTTON.0,
            MouseButton::Middle => cef_event_flags_t::EVENTFLAG_MIDDLE_MOUSE_BUTTON.0,
            MouseButton::Right => cef_event_flags_t::EVENTFLAG_RIGHT_MOUSE_BUTTON.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyInputKind {
    /// A key was pressed. Chromium derives shortcuts and navigation from this event.
    RawKeyDown,
    KeyUp,
    /// A character was produced by a key press.
    Char,
}

/// A keyboard event in the form Chromium expects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    pub kind: KeyInputKind,
    /// The Windows virtual key code, which Chromium uses on every platform.
    pub windows_key_code: i32,
    /// The platform scan code. On Linux this is the XKB key code.
    pub native_key_code: i32,
    /// The UTF-16 unit produced by the key, for `Char` events.
    pub character: u16,
    pub unmodified_character: u16,
    pub modifiers: Modifiers,
    pub is_system_key: bool,
}

impl KeyInput {
    pub(crate) fn to_cef(self) -> cef::KeyEvent {
        let kind = match self.kind {
            KeyInputKind::RawKeyDown => cef::sys::cef_key_event_type_t::KEYEVENT_RAWKEYDOWN,
            KeyInputKind::KeyUp => cef::sys::cef_key_event_type_t::KEYEVENT_KEYUP,
            KeyInputKind::Char => cef::sys::cef_key_event_type_t::KEYEVENT_CHAR,
        };
        cef::KeyEvent {
            type_: kind.into(),
            modifiers: self.modifiers.event_flags(),
            windows_key_code: self.windows_key_code,
            native_key_code: self.native_key_code,
            is_system_key: i32::from(self.is_system_key),
            character: self.character,
            unmodified_character: self.unmodified_character,
            ..Default::default()
        }
    }
}

/// Editing commands executed on the focused frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCommand {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomCommand {
    In,
    Out,
    Reset,
}
