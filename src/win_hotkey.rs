//! Thin Windows API surface for the global hotkey: registration, unregistration
//! helpers and failure messaging. Everything Windows-specific stays here.

use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, HOT_KEY_MODIFIERS};

use crate::settings::HotkeyChoice;

/// First id of the allocation pool. `RegisterHotKey` requires ids in
/// `0x0000..=0xBFFF`; starting at 1 and bumping by 1 per registration stays
/// far inside that range (the pool needs `u32::MAX` registrations to wrap).
pub const HOTKEY_ID_START: i32 = 1;

/// Allocates a fresh hotkey id from a monotonically increasing pool.
///
/// Registering each binding under its own id means a `WM_HOTKEY` message
/// queued from a *superseded* binding (pressed just before a re-register)
/// carries the old id and is ignored instead of acting as a stray toggle.
pub fn next_hotkey_id() -> i32 {
    use std::sync::atomic::{AtomicI32, Ordering};
    static NEXT: AtomicI32 = AtomicI32::new(HOTKEY_ID_START);
    // `fetch_add(1)` cannot panic here: the start value is positive and the
    // pool would need u32::MAX re-registrations to overflow i32.
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// `HRESULT` for a conflict: Win32 1409, `ERROR_HOTKEY_ALREADY_REGISTERED`.
pub const HOTKEY_ALREADY_REGISTERED: i32 = windows::core::HRESULT::from_win32(1409).0;

/// `MOD_NOREPEAT` stops keyboard auto-repeat from producing repeated toggles.
const MOD_NOREPEAT_BITS: u32 = 16384;

/// The `fsModifiers` value for a binding: the binding's modifiers plus
/// no-rePEAT.
pub fn hotkey_modifiers(choice: HotkeyChoice) -> HOT_KEY_MODIFIERS {
    HOT_KEY_MODIFIERS(choice.modifiers | MOD_NOREPEAT_BITS)
}

/// Registers `choice` under `id` as a process-global hotkey **on the calling
/// thread**.
///
/// On failure the raw Win32 error code (as carried in the `HRESULT`) is
/// returned so the caller can map it to a message without guessing a cause.
pub fn register_hotkey(choice: HotkeyChoice, id: i32) -> Result<(), i32> {
    unsafe {
        // SAFETY: `None` makes the hotkey global (posted to the calling
        // thread's message queue); `id` comes from the allocation pool, which
        // stays in the required 0x0000..=0xBFFF range; `vk` and the modifiers
        // come from the validated catalog.
        match RegisterHotKey(None, id, hotkey_modifiers(choice), choice.vk) {
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
    use crate::settings::{choice_for_display, MOD_ALT, MOD_CONTROL};
    use windows::core::HRESULT;

    #[test]
    fn modifiers_for_the_default_binding() {
        let choice = choice_for_display("Ctrl+Alt+F6").expect("default binding");
        let modifiers = hotkey_modifiers(choice);
        assert_eq!(modifiers.0, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT_BITS);
    }

    #[test]
    fn hotkey_ids_are_fresh_and_in_range() {
        let a = next_hotkey_id();
        let b = next_hotkey_id();
        assert_ne!(a, b);
        assert!(a >= HOTKEY_ID_START);
        assert!(b >= HOTKEY_ID_START);
        // `RegisterHotKey` accepts ids in 0x0000..=0xBFFF.
        assert!(a <= 0xBFFF && b <= 0xBFFF);
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
