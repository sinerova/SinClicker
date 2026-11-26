//! Thin Windows API surface for the global hotkey: registration, unregistration
//! helpers and failure messaging. Everything Windows-specific stays here.

use windows::Win32::UI::Input::KeyboardAndMouse::*;

use crate::settings::HotkeyChoice;

/// Slint's codes for the named keys a binding may use (see Slint's
/// `key_codes.rs`), mapped to their virtual keys. Everything else either is a
/// printable character (resolved by [`vk_from_key_text`] through the keyboard
/// layout) or is not a supported hotkey key.
#[rustfmt::skip]
const SLINT_NAMED_KEYS: &[(char, VIRTUAL_KEY)] = &[
    ('\u{0008}', VK_BACK), ('\u{000a}', VK_RETURN), ('\u{001b}', VK_ESCAPE),
    ('\u{007f}', VK_DELETE), ('\u{0020}', VK_SPACE), ('\u{0009}', VK_TAB),
    ('\u{F700}', VK_UP), ('\u{F701}', VK_DOWN), ('\u{F702}', VK_LEFT), ('\u{F703}', VK_RIGHT),
    ('\u{F704}', VK_F1), ('\u{F705}', VK_F2), ('\u{F706}', VK_F3), ('\u{F707}', VK_F4),
    ('\u{F708}', VK_F5), ('\u{F709}', VK_F6), ('\u{F70A}', VK_F7), ('\u{F70B}', VK_F8),
    ('\u{F70C}', VK_F9), ('\u{F70D}', VK_F10), ('\u{F70E}', VK_F11), ('\u{F70F}', VK_F12),
    ('\u{F710}', VK_F13), ('\u{F711}', VK_F14), ('\u{F712}', VK_F15), ('\u{F713}', VK_F16),
    ('\u{F714}', VK_F17), ('\u{F715}', VK_F18), ('\u{F716}', VK_F19), ('\u{F717}', VK_F20),
    ('\u{F718}', VK_F21), ('\u{F719}', VK_F22), ('\u{F71A}', VK_F23), ('\u{F71B}', VK_F24),
    ('\u{F727}', VK_INSERT), ('\u{F729}', VK_HOME), ('\u{F72B}', VK_END),
    ('\u{F72C}', VK_PRIOR), ('\u{F72D}', VK_NEXT), ('\u{F72F}', VK_SCROLL),
    ('\u{F730}', VK_PAUSE), ('\u{F731}', VK_SNAPSHOT), ('\u{F735}', VK_APPS),
];

/// Resolves the text of a Slint key event to the virtual key that produced
/// it, or `None` when it cannot be identified.
///
/// Named keys come from Slint's fixed codes. A printable character is looked
/// up in the active keyboard layout with `VkKeyScanW`, so a shifted or AltGr
/// character (`!`, `€`) resolves to its physical key (`1`, `E`), and a
/// non-US layout resolves to the key the user actually pressed.
pub fn vk_from_key_text(text: &str) -> Option<VIRTUAL_KEY> {
    let mut chars = text.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if let Some((_, vk)) = SLINT_NAMED_KEYS.iter().find(|(c, _)| *c == ch) {
        return Some(*vk);
    }
    // Characters outside the Basic Multilingual Plane have no single UTF-16
    // unit, so no key types them directly.
    let unit = u16::try_from(u32::from(ch)).ok()?;
    let scan = unsafe {
        // SAFETY: a plain query of the calling thread's keyboard layout.
        VkKeyScanW(unit)
    };
    // -1 in both bytes means no key produces the character; the low byte is
    // the virtual key, the high byte the shift state (unused here: the held
    // modifiers come from the key event itself).
    (scan != -1).then(|| VIRTUAL_KEY(u16::from(scan.to_le_bytes()[0])))
}

/// The id range `RegisterHotKey` accepts for an application (`0x0000..=0xBFFF`).
/// The hotkey thread hands out ids from this range in turn, so a `WM_HOTKEY`
/// still queued from a superseded binding carries an old id and is ignored.
pub const HOTKEY_ID_MIN: i32 = 1;
pub const HOTKEY_ID_MAX: i32 = 0xBFFF;

/// `HRESULT` for a conflict: Win32 1409, `ERROR_HOTKEY_ALREADY_REGISTERED`.
pub const HOTKEY_ALREADY_REGISTERED: i32 = windows::core::HRESULT::from_win32(1409).0;

/// The `fsModifiers` value for a binding: its modifiers plus `MOD_NOREPEAT`,
/// so keyboard auto-repeat does not produce repeated toggles.
pub fn hotkey_modifiers(choice: HotkeyChoice) -> HOT_KEY_MODIFIERS {
    choice.modifiers | MOD_NOREPEAT
}

/// Registers `choice` under `id` as a process-global hotkey **on the calling
/// thread**.
///
/// On failure the raw Win32 error code (as carried in the `HRESULT`) is
/// returned so the caller can map it to a message without guessing a cause.
pub fn register_hotkey(choice: HotkeyChoice, id: i32) -> Result<(), i32> {
    unsafe {
        // SAFETY: `None` makes the hotkey global (posted to the calling
        // thread's message queue); `id` is in the required range; `vk` and
        // the modifiers come from a validated `HotkeyChoice`.
        match RegisterHotKey(None, id, hotkey_modifiers(choice), u32::from(choice.vk.0)) {
            Ok(()) => Ok(()),
            Err(error) => Err(error.code().0),
        }
    }
}

/// Maps a registration failure to a short, actionable message for the UI.
/// Only what the API establishes is reported: 1409 means the combination is
/// already taken; anything else is reported with its code, and no cause is
/// asserted. `previous_active` states whether a (working) binding stays
/// active after the failure, so the message never claims the opposite.
pub fn describe_registration_failure(display: &str, code: i32, previous_active: bool) -> String {
    let suffix = if previous_active {
        " The previous binding stays active."
    } else {
        " No global hotkey is active."
    };
    if code == HOTKEY_ALREADY_REGISTERED {
        format!(
            "{display} is already in use by another application. \
             The new binding is unavailable; clicking stays as it is.{suffix}"
        )
    } else if code == 0 {
        format!("{display} could not be registered (no Windows error code).{suffix}")
    } else {
        format!(
            "{display} could not be registered (Windows error code {:#010x}).{suffix}",
            code as u32
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DEFAULT_HOTKEY;
    use windows::core::HRESULT;

    #[test]
    fn modifiers_for_the_default_binding() {
        let modifiers = hotkey_modifiers(DEFAULT_HOTKEY);
        assert_eq!(modifiers, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT);
    }

    #[test]
    fn named_slint_keys_resolve_without_the_keyboard_layout() {
        assert_eq!(vk_from_key_text("\u{F709}"), Some(VK_F6));
        assert_eq!(vk_from_key_text("\u{F71B}"), Some(VK_F24));
        assert_eq!(vk_from_key_text("\u{F72C}"), Some(VK_PRIOR));
        assert_eq!(vk_from_key_text("\u{001b}"), Some(VK_ESCAPE));
        assert_eq!(vk_from_key_text(" "), Some(VK_SPACE));
        assert_eq!(vk_from_key_text(""), None);
        assert_eq!(vk_from_key_text("ab"), None);
    }

    #[test]
    fn failure_messages_are_actionable() {
        let conflict =
            describe_registration_failure("Ctrl+Alt+F9", HOTKEY_ALREADY_REGISTERED, true);
        assert!(conflict.contains("Ctrl+Alt+F9"), "{conflict}");
        assert!(conflict.contains("already in use"), "{conflict}");
        assert!(
            conflict.contains("previous binding stays active"),
            "{conflict}"
        );

        let conflict_bare =
            describe_registration_failure("Ctrl+Alt+F9", HOTKEY_ALREADY_REGISTERED, false);
        assert!(
            conflict_bare.contains("No global hotkey is active"),
            "{conflict_bare}"
        );

        let other = describe_registration_failure("Ctrl+Alt+F9", HRESULT::from_win32(5).0, false);
        assert!(other.contains("80070005"), "{other}");
        assert!(other.contains("No global hotkey is active"), "{other}");

        let no_code = describe_registration_failure("Ctrl+Alt+F9", 0, false);
        assert!(no_code.contains("no Windows error code"), "{no_code}");
    }
}
