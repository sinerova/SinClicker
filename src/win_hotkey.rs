//! Thin Windows API surface for the global hotkey: registration, unregistration
//! helpers and failure messaging. Everything Windows-specific stays here.

use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT,
};

use crate::settings::HotkeyChoice;

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
        // the modifiers come from the validated catalog.
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
    use crate::settings::choice_for_display;
    use windows::core::HRESULT;
    use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL};

    #[test]
    fn modifiers_for_the_default_binding() {
        let choice = choice_for_display("Ctrl+Alt+F6").expect("default binding");
        let modifiers = hotkey_modifiers(choice);
        assert_eq!(modifiers, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT);
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
