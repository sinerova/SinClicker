//! Settings: the supported hotkey catalog and persistence of the chosen
//! binding and the CPS value in the per-user registry.
//!
//! The catalog and startup resolution are pure and unit-tested; no test here
//! registers a hotkey or sends input.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_ROUTINE_FLAGS, REG_SZ,
    REG_VALUE_TYPE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT, VIRTUAL_KEY, VK_A, VK_BACK, VK_END, VK_F10,
    VK_F11, VK_F13, VK_F14, VK_F15, VK_F16, VK_F6, VK_F7, VK_F8, VK_F9, VK_HOME, VK_INSERT,
    VK_NEXT, VK_OEM_PERIOD, VK_PRIOR, VK_SPACE,
};

use crate::clicker::{DEFAULT_CPS, MAX_CPS, MIN_CPS};

/// A supported toggle hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyChoice {
    /// Canonical display string, for example `Ctrl+Alt+F6`. This is also the
    /// persisted form.
    pub display: &'static str,
    /// Modifiers for `RegisterHotKey` (without `MOD_NOREPEAT`, which is added
    /// at registration time).
    pub modifiers: HOT_KEY_MODIFIERS,
    pub vk: VIRTUAL_KEY,
}

const CTRL_ALT: HOT_KEY_MODIFIERS = HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0);
const CTRL_SHIFT_ALT: HOT_KEY_MODIFIERS = HOT_KEY_MODIFIERS(CTRL_ALT.0 | MOD_SHIFT.0);

const fn choice(
    display: &'static str,
    modifiers: HOT_KEY_MODIFIERS,
    vk: VIRTUAL_KEY,
) -> HotkeyChoice {
    HotkeyChoice {
        display,
        modifiers,
        vk,
    }
}

/// The bindings offered in the UI, in display order.
///
/// Every entry is Ctrl+Alt based: Ctrl is always present because Alt alone is
/// reserved for menu/IME use, Shift may be added optionally, no Windows-key
/// combination appears (those are reserved for the OS), F12 is excluded (it is
/// reserved for debuggers), and Ctrl+Alt+Delete is excluded (it is the secure
/// attention sequence, which Windows handles before any application hotkey).
pub const SUPPORTED_HOTKEYS: [HotkeyChoice; 22] = [
    choice("Ctrl+Alt+F6", CTRL_ALT, VK_F6),
    choice("Ctrl+Alt+F7", CTRL_ALT, VK_F7),
    choice("Ctrl+Alt+F8", CTRL_ALT, VK_F8),
    choice("Ctrl+Alt+F9", CTRL_ALT, VK_F9),
    choice("Ctrl+Alt+F10", CTRL_ALT, VK_F10),
    choice("Ctrl+Alt+F11", CTRL_ALT, VK_F11),
    choice("Ctrl+Alt+F13", CTRL_ALT, VK_F13),
    choice("Ctrl+Alt+F14", CTRL_ALT, VK_F14),
    choice("Ctrl+Alt+F15", CTRL_ALT, VK_F15),
    choice("Ctrl+Alt+F16", CTRL_ALT, VK_F16),
    choice("Ctrl+Alt+Insert", CTRL_ALT, VK_INSERT),
    choice("Ctrl+Alt+Home", CTRL_ALT, VK_HOME),
    choice("Ctrl+Alt+End", CTRL_ALT, VK_END),
    choice("Ctrl+Alt+PageUp", CTRL_ALT, VK_PRIOR),
    choice("Ctrl+Alt+PageDown", CTRL_ALT, VK_NEXT),
    choice("Ctrl+Shift+Alt+F6", CTRL_SHIFT_ALT, VK_F6),
    choice("Ctrl+Shift+Alt+F7", CTRL_SHIFT_ALT, VK_F7),
    choice("Ctrl+Shift+Alt+F9", CTRL_SHIFT_ALT, VK_F9),
    choice("Ctrl+Alt+Space", CTRL_ALT, VK_SPACE),
    choice("Ctrl+Alt+Backspace", CTRL_ALT, VK_BACK),
    choice("Ctrl+Alt+Period", CTRL_ALT, VK_OEM_PERIOD),
    choice("Ctrl+Alt+A", CTRL_ALT, VK_A),
];

/// The default binding; also the first UI choice.
pub const DEFAULT_HOTKEY: HotkeyChoice = SUPPORTED_HOTKEYS[0];

/// Resolves a display string (from the UI or from persisted storage) to a
/// supported choice, or `None` when it is not one.
pub fn choice_for_display(display: &str) -> Option<HotkeyChoice> {
    SUPPORTED_HOTKEYS
        .iter()
        .find(|h| h.display == display)
        .copied()
}

/// The binding to start with: the persisted one when it is still supported,
/// otherwise the default.
pub fn resolve_startup_hotkey(stored: Option<&str>) -> HotkeyChoice {
    stored
        .and_then(choice_for_display)
        .unwrap_or(DEFAULT_HOTKEY)
}

/// Resolves a persisted CPS value to the CPS the application starts with: a
/// stored value inside the supported range is used as-is; `None` (missing or
/// unreadable storage) and any out-of-range number fall back to the default.
pub fn resolve_startup_cps(stored: Option<i32>) -> i32 {
    match stored {
        Some(value) if (MIN_CPS..=MAX_CPS).contains(&value) => value,
        None | Some(_) => DEFAULT_CPS,
    }
}

// ---- Registry persistence (HKCU, so no elevation is required) -------------

const SETTINGS_SUBKEY: PCWSTR = w!("Software\\SinClicker");
const HOTKEY_VALUE: PCWSTR = w!("Hotkey");
const CPS_VALUE: PCWSTR = w!("Cps");

/// Longest display string accepted from the registry, in UTF-16 units
/// (including the terminator). Every catalog entry is far shorter; anything
/// longer is not a supported binding, so it is treated as unreadable.
const MAX_STORED_DISPLAY_UNITS: usize = 64;

/// Reads a value of the type `flags` allows into `buf`, returning the number
/// of bytes written, or `None` when the key or value is missing, has another
/// type, does not fit, or cannot be read.
fn read_value(name: PCWSTR, flags: REG_ROUTINE_FLAGS, buf: &mut [u8]) -> Option<usize> {
    let mut size = u32::try_from(buf.len()).ok()?;
    let status = unsafe {
        // SAFETY: both names are static NUL-terminated strings; `buf` is
        // writable for `size` bytes and outlives the call; `RRF_RT_*` makes
        // Windows reject any other value type instead of writing it.
        RegGetValueW(
            HKEY_CURRENT_USER,
            SETTINGS_SUBKEY,
            name,
            flags,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(size as usize)
}

/// Writes a value, creating the settings key when it does not exist yet.
fn write_value(name: PCWSTR, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<(), String> {
    let len = u32::try_from(data.len()).map_err(|_| "value too large".to_owned())?;
    let status = unsafe {
        // SAFETY: both names are static NUL-terminated strings; `data` is
        // readable for `len` bytes and outlives the call.
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            SETTINGS_SUBKEY,
            name,
            kind.0,
            Some(data.as_ptr().cast()),
            len,
        )
    };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("Windows error code {:#x}", status.0))
    }
}

/// Reads the persisted binding display string, or `None` when nothing usable
/// is stored (first launch, missing value, wrong type, or a registry error).
pub fn load_hotkey_display() -> Option<String> {
    let mut buf = [0u8; MAX_STORED_DISPLAY_UNITS * 2];
    let len = read_value(HOTKEY_VALUE, RRF_RT_REG_SZ, &mut buf)?;
    // `RegGetValueW` NUL-terminates REG_SZ data. Decode up to the terminator.
    let units: Vec<u16> = buf[..len]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16(&units).ok().filter(|s| !s.is_empty())
}

/// Persists the chosen binding display string.
pub fn save_hotkey_display(value: &str) -> Result<(), String> {
    let data: Vec<u8> = value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    write_value(HOTKEY_VALUE, REG_SZ, &data)
}

/// Reads the persisted CPS value, or `None` when nothing usable is stored.
/// Range checking is the caller's job ([`resolve_startup_cps`]).
pub fn load_cps() -> Option<i32> {
    let mut buf = [0u8; 4];
    match read_value(CPS_VALUE, RRF_RT_REG_DWORD, &mut buf)? {
        4 => Some(i32::from_le_bytes(buf)),
        _ => None,
    }
}

/// Persists the CPS value as a REG_DWORD.
pub fn save_cps(cps: i32) -> Result<(), String> {
    write_value(CPS_VALUE, REG_DWORD, &cps.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_startup_cps_uses_a_stored_in_range_value() {
        assert_eq!(resolve_startup_cps(Some(MIN_CPS)), MIN_CPS);
        assert_eq!(resolve_startup_cps(Some(DEFAULT_CPS)), DEFAULT_CPS);
        assert_eq!(resolve_startup_cps(Some(MAX_CPS)), MAX_CPS);
        assert_eq!(resolve_startup_cps(Some(250)), 250);
        assert_eq!(resolve_startup_cps(Some(499)), 499);
    }

    #[test]
    fn resolve_startup_cps_falls_back_to_the_default() {
        assert_eq!(resolve_startup_cps(None), DEFAULT_CPS);
        assert_eq!(resolve_startup_cps(Some(MIN_CPS - 1)), DEFAULT_CPS);
        assert_eq!(resolve_startup_cps(Some(0)), DEFAULT_CPS);
        assert_eq!(resolve_startup_cps(Some(MAX_CPS + 1)), DEFAULT_CPS);
        assert_eq!(resolve_startup_cps(Some(i32::MIN)), DEFAULT_CPS);
        assert_eq!(resolve_startup_cps(Some(i32::MAX)), DEFAULT_CPS);
    }

    #[test]
    fn resolve_startup_hotkey_falls_back_to_the_default() {
        assert_eq!(resolve_startup_hotkey(None), DEFAULT_HOTKEY);
        assert_eq!(
            resolve_startup_hotkey(Some("Ctrl+Alt+F9")).display,
            "Ctrl+Alt+F9"
        );
        // Unsupported or retired bindings fall back.
        assert_eq!(
            resolve_startup_hotkey(Some("Ctrl+Alt+Delete")),
            DEFAULT_HOTKEY
        );
        assert_eq!(resolve_startup_hotkey(Some("garbage")), DEFAULT_HOTKEY);
    }

    #[test]
    fn catalog_has_expected_default_and_excludes_reserved_keys() {
        assert_eq!(DEFAULT_HOTKEY.display, "Ctrl+Alt+F6");
        for h in SUPPORTED_HOTKEYS {
            assert!(!h.display.contains("F12"), "F12 is reserved: {}", h.display);
            assert!(
                !h.display.contains("Win"),
                "Win is OS-reserved: {}",
                h.display
            );
            assert!(
                !h.display.contains("Delete"),
                "Ctrl+Alt+Delete is the SAS: {}",
                h.display
            );
            assert!(
                h.modifiers.0 & CTRL_ALT.0 == CTRL_ALT.0,
                "every binding needs Ctrl+Alt: {}",
                h.display
            );
        }
    }

    #[test]
    fn catalog_entries_are_unique() {
        for (i, a) in SUPPORTED_HOTKEYS.iter().enumerate() {
            for b in &SUPPORTED_HOTKEYS[i + 1..] {
                assert_ne!(a.display, b.display);
                assert_ne!(
                    (a.modifiers, a.vk),
                    (b.modifiers, b.vk),
                    "{} vs {}",
                    a.display,
                    b.display
                );
            }
        }
    }

    #[test]
    fn choice_for_display_round_trips_catalog() {
        for h in SUPPORTED_HOTKEYS {
            assert_eq!(choice_for_display(h.display), Some(h), "for {}", h.display);
        }
    }

    #[test]
    fn choice_for_display_rejects_unsupported() {
        assert_eq!(choice_for_display("Ctrl+Alt+F12"), None);
        assert_eq!(choice_for_display("Ctrl+Alt+G"), None);
        assert_eq!(choice_for_display("Alt+F6"), None);
        assert_eq!(choice_for_display("Ctrl+F6"), None);
        assert_eq!(choice_for_display("Ctrl+Alt+Win+F6"), None);
        assert_eq!(choice_for_display(""), None);
        assert_eq!(choice_for_display("Ctrl+Alt+F6 "), None); // trailing space
    }
}
