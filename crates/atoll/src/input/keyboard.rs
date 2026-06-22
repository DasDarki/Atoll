use atoll_engine::{KeyInput, KeyInputKind, Modifiers};
use slint::winit_030::winit::event::{ElementState, KeyEvent};
use slint::winit_030::winit::keyboard::{
    Key, KeyCode, KeyLocation, ModifiersState, NamedKey, PhysicalKey,
};
use slint::winit_030::winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use slint::winit_030::winit::platform::scancode::PhysicalKeyExtScancode;

const VK_RETURN: i32 = 0x0D;
const VK_OEM_MINUS: i32 = 0xBD;
const VK_OEM_4: i32 = 0xDB;
const VK_OEM_5: i32 = 0xDC;
const VK_OEM_6: i32 = 0xDD;
const VK_2: i32 = 0x32;
const VK_6: i32 = 0x36;

/// Key events for Chromium produced by one winit key event.
pub struct KeyTranslation {
    pub events: Vec<KeyInput>,
    /// Text that consists of more than one UTF-16 unit and is inserted as a whole.
    pub commit: Option<String>,
}

/// Translates a winit key event the way Chromium's own Linux port does: a raw key down followed
/// by a character event, or a key up.
pub fn translate(event: &KeyEvent, modifiers: ModifiersState) -> KeyTranslation {
    let windows_key_code = windows_key_code(event);
    let control = modifiers.control_key() && !modifiers.alt_key();
    let flags = Modifiers {
        shift: modifiers.shift_key(),
        control: modifiers.control_key(),
        alt: modifiers.alt_key(),
        meta: modifiers.super_key(),
        keypad: event.location == KeyLocation::Numpad,
        repeat: event.repeat,
        ..Modifiers::default()
    };

    let unmodified_character = if windows_key_code == VK_RETURN {
        u16::from(b'\r')
    } else {
        first_unit(key_text(&event.key_without_modifiers()).or(event.text.as_deref()))
    };
    let text_units: Vec<u16> = event
        .text
        .as_deref()
        .map(|text| text.encode_utf16().collect())
        .unwrap_or_default();
    let character = if control {
        control_character(windows_key_code, modifiers.shift_key())
    } else {
        text_units.first().copied().unwrap_or(unmodified_character)
    };

    let base = KeyInput {
        kind: KeyInputKind::RawKeyDown,
        windows_key_code,
        native_key_code: native_key_code(event.physical_key),
        character,
        unmodified_character,
        modifiers: flags,
        is_system_key: modifiers.alt_key(),
    };

    if event.state == ElementState::Released {
        return KeyTranslation {
            events: vec![KeyInput {
                kind: KeyInputKind::KeyUp,
                ..base
            }],
            commit: None,
        };
    }

    let mut events = vec![base];
    let mut commit = None;
    if text_units.len() > 1 && !control {
        commit = event.text.as_deref().map(str::to_owned);
    } else if character != 0 {
        events.push(KeyInput {
            kind: KeyInputKind::Char,
            ..base
        });
    }
    KeyTranslation { events, commit }
}

fn key_text(key: &Key) -> Option<&str> {
    match key {
        Key::Character(text) => Some(text.as_str()),
        _ => None,
    }
}

fn first_unit(text: Option<&str>) -> u16 {
    text.and_then(|text| text.encode_utf16().next())
        .unwrap_or(0)
}

/// The control character Chromium expects for a key pressed together with Ctrl.
fn control_character(windows_key_code: i32, shift: bool) -> u16 {
    if (0x41..=0x5A).contains(&windows_key_code) {
        return (windows_key_code - 0x41 + 1) as u16;
    }
    let character = if shift {
        match windows_key_code {
            VK_6 => 0x1E,
            VK_OEM_MINUS => 0x1F,
            VK_2 => 0x00,
            _ => 0,
        }
    } else {
        match windows_key_code {
            VK_OEM_4 => 0x1B,
            VK_OEM_5 => 0x1C,
            VK_OEM_6 => 0x1D,
            VK_RETURN => 0x0A,
            _ => 0,
        }
    };
    character as u16
}

#[cfg(target_os = "linux")]
fn native_key_code(key: PhysicalKey) -> i32 {
    key.to_scancode().map_or(0, |code| code as i32 + 8)
}

#[cfg(not(target_os = "linux"))]
fn native_key_code(key: PhysicalKey) -> i32 {
    key.to_scancode()
        .map_or(0, |code| ((code as i32) << 16) | 1)
}

/// The layout dependent Windows virtual key code of a key event.
pub fn windows_key_code(event: &KeyEvent) -> i32 {
    let physical = match event.physical_key {
        PhysicalKey::Code(code) => Some(code),
        PhysicalKey::Unidentified(_) => None,
    };
    let from_physical = physical.and_then(physical_key_code).unwrap_or(0);
    match &event.logical_key {
        Key::Named(named) => named_key_code(*named).unwrap_or(from_physical),
        Key::Character(text) => {
            if physical.is_some_and(is_numpad) {
                return from_physical;
            }
            let Some(character) = text.chars().next() else {
                return from_physical;
            };
            if character.is_ascii_alphabetic() {
                character.to_ascii_uppercase() as i32
            } else if from_physical != 0 {
                from_physical
            } else if character.is_ascii_digit() {
                character as i32
            } else {
                0
            }
        }
        _ => from_physical,
    }
}

fn is_numpad(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Numpad0
            | KeyCode::Numpad1
            | KeyCode::Numpad2
            | KeyCode::Numpad3
            | KeyCode::Numpad4
            | KeyCode::Numpad5
            | KeyCode::Numpad6
            | KeyCode::Numpad7
            | KeyCode::Numpad8
            | KeyCode::Numpad9
            | KeyCode::NumpadAdd
            | KeyCode::NumpadSubtract
            | KeyCode::NumpadMultiply
            | KeyCode::NumpadDivide
            | KeyCode::NumpadDecimal
    )
}

fn named_key_code(key: NamedKey) -> Option<i32> {
    let code = match key {
        NamedKey::Enter => 0x0D,
        NamedKey::Tab => 0x09,
        NamedKey::Space => 0x20,
        NamedKey::Backspace => 0x08,
        NamedKey::Escape => 0x1B,
        NamedKey::ArrowLeft => 0x25,
        NamedKey::ArrowUp => 0x26,
        NamedKey::ArrowRight => 0x27,
        NamedKey::ArrowDown => 0x28,
        NamedKey::Home => 0x24,
        NamedKey::End => 0x23,
        NamedKey::PageUp => 0x21,
        NamedKey::PageDown => 0x22,
        NamedKey::Insert => 0x2D,
        NamedKey::Delete => 0x2E,
        NamedKey::Shift => 0x10,
        NamedKey::Control => 0x11,
        NamedKey::Alt => 0x12,
        NamedKey::AltGraph => 0xE1,
        NamedKey::Super | NamedKey::Meta => 0x5B,
        NamedKey::CapsLock => 0x14,
        NamedKey::NumLock => 0x90,
        NamedKey::ScrollLock => 0x91,
        NamedKey::PrintScreen => 0x2C,
        NamedKey::Pause => 0x13,
        NamedKey::ContextMenu => 0x5D,
        NamedKey::MediaPlayPause => 0xB3,
        NamedKey::MediaStop => 0xB2,
        NamedKey::MediaTrackNext => 0xB0,
        NamedKey::MediaTrackPrevious => 0xB1,
        NamedKey::AudioVolumeMute => 0xAD,
        NamedKey::AudioVolumeDown => 0xAE,
        NamedKey::AudioVolumeUp => 0xAF,
        NamedKey::BrowserBack => 0xA6,
        NamedKey::BrowserForward => 0xA7,
        NamedKey::BrowserRefresh => 0xA8,
        NamedKey::BrowserHome => 0xAC,
        NamedKey::F1 => 0x70,
        NamedKey::F2 => 0x71,
        NamedKey::F3 => 0x72,
        NamedKey::F4 => 0x73,
        NamedKey::F5 => 0x74,
        NamedKey::F6 => 0x75,
        NamedKey::F7 => 0x76,
        NamedKey::F8 => 0x77,
        NamedKey::F9 => 0x78,
        NamedKey::F10 => 0x79,
        NamedKey::F11 => 0x7A,
        NamedKey::F12 => 0x7B,
        _ => return None,
    };
    Some(code)
}

fn physical_key_code(code: KeyCode) -> Option<i32> {
    let letter = |offset: i32| 0x41 + offset;
    let key = match code {
        KeyCode::KeyA => letter(0),
        KeyCode::KeyB => letter(1),
        KeyCode::KeyC => letter(2),
        KeyCode::KeyD => letter(3),
        KeyCode::KeyE => letter(4),
        KeyCode::KeyF => letter(5),
        KeyCode::KeyG => letter(6),
        KeyCode::KeyH => letter(7),
        KeyCode::KeyI => letter(8),
        KeyCode::KeyJ => letter(9),
        KeyCode::KeyK => letter(10),
        KeyCode::KeyL => letter(11),
        KeyCode::KeyM => letter(12),
        KeyCode::KeyN => letter(13),
        KeyCode::KeyO => letter(14),
        KeyCode::KeyP => letter(15),
        KeyCode::KeyQ => letter(16),
        KeyCode::KeyR => letter(17),
        KeyCode::KeyS => letter(18),
        KeyCode::KeyT => letter(19),
        KeyCode::KeyU => letter(20),
        KeyCode::KeyV => letter(21),
        KeyCode::KeyW => letter(22),
        KeyCode::KeyX => letter(23),
        KeyCode::KeyY => letter(24),
        KeyCode::KeyZ => letter(25),
        KeyCode::Digit0 => 0x30,
        KeyCode::Digit1 => 0x31,
        KeyCode::Digit2 => 0x32,
        KeyCode::Digit3 => 0x33,
        KeyCode::Digit4 => 0x34,
        KeyCode::Digit5 => 0x35,
        KeyCode::Digit6 => 0x36,
        KeyCode::Digit7 => 0x37,
        KeyCode::Digit8 => 0x38,
        KeyCode::Digit9 => 0x39,
        KeyCode::Numpad0 => 0x60,
        KeyCode::Numpad1 => 0x61,
        KeyCode::Numpad2 => 0x62,
        KeyCode::Numpad3 => 0x63,
        KeyCode::Numpad4 => 0x64,
        KeyCode::Numpad5 => 0x65,
        KeyCode::Numpad6 => 0x66,
        KeyCode::Numpad7 => 0x67,
        KeyCode::Numpad8 => 0x68,
        KeyCode::Numpad9 => 0x69,
        KeyCode::NumpadMultiply => 0x6A,
        KeyCode::NumpadAdd => 0x6B,
        KeyCode::NumpadSubtract => 0x6D,
        KeyCode::NumpadDecimal => 0x6E,
        KeyCode::NumpadDivide => 0x6F,
        KeyCode::NumpadEnter | KeyCode::Enter => 0x0D,
        KeyCode::Semicolon => 0xBA,
        KeyCode::Equal => 0xBB,
        KeyCode::Comma => 0xBC,
        KeyCode::Minus => 0xBD,
        KeyCode::Period => 0xBE,
        KeyCode::Slash => 0xBF,
        KeyCode::Backquote => 0xC0,
        KeyCode::BracketLeft => 0xDB,
        KeyCode::Backslash => 0xDC,
        KeyCode::BracketRight => 0xDD,
        KeyCode::Quote => 0xDE,
        KeyCode::IntlBackslash => 0xE2,
        KeyCode::Tab => 0x09,
        KeyCode::Space => 0x20,
        KeyCode::Backspace => 0x08,
        KeyCode::Escape => 0x1B,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => 0x10,
        KeyCode::ControlLeft | KeyCode::ControlRight => 0x11,
        KeyCode::AltLeft | KeyCode::AltRight => 0x12,
        KeyCode::SuperLeft => 0x5B,
        KeyCode::SuperRight => 0x5C,
        _ => return None,
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_letters_map_to_ascii_control_codes() {
        assert_eq!(control_character(0x41, false), 1);
        assert_eq!(control_character(0x5A, true), 26);
        assert_eq!(control_character(VK_RETURN, false), 0x0A);
        assert_eq!(control_character(0x31, false), 0);
    }

    #[test]
    fn punctuation_uses_the_physical_position() {
        assert_eq!(physical_key_code(KeyCode::Minus), Some(0xBD));
        assert_eq!(physical_key_code(KeyCode::KeyZ), Some(0x5A));
    }
}
