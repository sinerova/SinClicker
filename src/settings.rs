//! Settings: the toggle hotkey binding (validation, display and parsing) and
//! persistence of the chosen binding and the CPS value in the per-user
//! registry.
//!
//! Binding rules, formatting and startup resolution are pure and unit-tested;
//! no test here registers a hotkey or sends input.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_ROUTINE_FLAGS, REG_SZ,
    REG_VALUE_TYPE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

use crate::clicker::{DEFAULT_CPS, MAX_CPS, MIN_CPS};

/// A validated toggle hotkey: optional modifiers plus one non-modifier key.
///
/// Only [`HotkeyChoice::new`] and [`HotkeyChoice::parse`] build one (or the
/// [`DEFAULT_HOTKEY`] constant), so every value satisfies the binding rules
/// documented on [`HotkeyChoice::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyChoice {
    /// Modifiers for `RegisterHotKey`: any of Ctrl, Shift and Alt (without
    /// `MOD_NOREPEAT`, which is added at registration time).
    pub modifiers: HOT_KEY_MODIFIERS,
    pub vk: VIRTUAL_KEY,
}

const CTRL_ALT: HOT_KEY_MODIFIERS = HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0);
const NO_MODIFIERS: HOT_KEY_MODIFIERS = HOT_KEY_MODIFIERS(0);

/// The default binding: Ctrl+Alt+F6.
pub const DEFAULT_HOTKEY: HotkeyChoice = HotkeyChoice {
    modifiers: CTRL_ALT,
    vk: VK_F6,
};

/// Modifier names in canonical display order. The order matches what earlier
/// versions persisted (for example `Ctrl+Shift+Alt+F6`), so saved bindings
/// keep parsing.
const MODIFIER_NAMES: [(HOT_KEY_MODIFIERS, &str); 3] = [
    (MOD_CONTROL, "Ctrl"),
    (MOD_SHIFT, "Shift"),
    (MOD_ALT, "Alt"),
];

/// Every key a binding may use, with its display name. Punctuation keys are
/// named after their US-layout legend, since Windows identifies them by
/// virtual key rather than by the character they type.
///
/// Deliberately absent: the modifier and Windows keys, Escape (it cancels
/// recording), Tab (focus navigation) and F12 (reserved for debuggers).
#[rustfmt::skip]
const KEY_NAMES: &[(VIRTUAL_KEY, &str)] = &[
    (VK_A, "A"), (VK_B, "B"), (VK_C, "C"), (VK_D, "D"), (VK_E, "E"), (VK_F, "F"),
    (VK_G, "G"), (VK_H, "H"), (VK_I, "I"), (VK_J, "J"), (VK_K, "K"), (VK_L, "L"),
    (VK_M, "M"), (VK_N, "N"), (VK_O, "O"), (VK_P, "P"), (VK_Q, "Q"), (VK_R, "R"),
    (VK_S, "S"), (VK_T, "T"), (VK_U, "U"), (VK_V, "V"), (VK_W, "W"), (VK_X, "X"),
    (VK_Y, "Y"), (VK_Z, "Z"),
    (VK_0, "0"), (VK_1, "1"), (VK_2, "2"), (VK_3, "3"), (VK_4, "4"),
    (VK_5, "5"), (VK_6, "6"), (VK_7, "7"), (VK_8, "8"), (VK_9, "9"),
    (VK_F1, "F1"), (VK_F2, "F2"), (VK_F3, "F3"), (VK_F4, "F4"), (VK_F5, "F5"),
    (VK_F6, "F6"), (VK_F7, "F7"), (VK_F8, "F8"), (VK_F9, "F9"), (VK_F10, "F10"),
    (VK_F11, "F11"), (VK_F13, "F13"), (VK_F14, "F14"), (VK_F15, "F15"),
    (VK_F16, "F16"), (VK_F17, "F17"), (VK_F18, "F18"), (VK_F19, "F19"),
    (VK_F20, "F20"), (VK_F21, "F21"), (VK_F22, "F22"), (VK_F23, "F23"),
    (VK_F24, "F24"),
    (VK_SPACE, "Space"), (VK_BACK, "Backspace"), (VK_RETURN, "Enter"),
    (VK_INSERT, "Insert"), (VK_DELETE, "Delete"), (VK_HOME, "Home"), (VK_END, "End"),
    (VK_PRIOR, "PageUp"), (VK_NEXT, "PageDown"),
    (VK_UP, "Up"), (VK_DOWN, "Down"), (VK_LEFT, "Left"), (VK_RIGHT, "Right"),
    (VK_SNAPSHOT, "PrintScreen"), (VK_SCROLL, "ScrollLock"), (VK_PAUSE, "Pause"),
    (VK_APPS, "Menu"),
    (VK_OEM_3, "Backquote"), (VK_OEM_MINUS, "Minus"), (VK_OEM_PLUS, "Equals"),
    (VK_OEM_4, "LeftBracket"), (VK_OEM_6, "RightBracket"), (VK_OEM_5, "Backslash"),
    (VK_OEM_1, "Semicolon"), (VK_OEM_7, "Quote"), (VK_OEM_COMMA, "Comma"),
    (VK_OEM_PERIOD, "Period"), (VK_OEM_2, "Slash"),
];

fn key_name(vk: VIRTUAL_KEY) -> Option<&'static str> {
    KEY_NAMES
        .iter()
        .find(|(k, _)| *k == vk)
        .map(|(_, name)| *name)
}

fn is_function_key(vk: VIRTUAL_KEY) -> bool {
    (VK_F1.0..=VK_F24.0).contains(&vk.0)
}

impl HotkeyChoice {
    /// Validates a captured combination. `win` reports whether a Windows key
    /// was held.
    ///
    /// Rules: no Windows-key combinations (Windows reserves them); the key
    /// must be one of the supported keys (see `KEY_NAMES`); keys other than
    /// F1–F24 need Ctrl or Alt, so the binding never swallows ordinary
    /// typing; and Ctrl+Alt+Delete is refused (it is the secure attention
    /// sequence, which Windows handles before any application). On failure a
    /// short message suitable for the UI is returned.
    pub fn new(
        modifiers: HOT_KEY_MODIFIERS,
        vk: VIRTUAL_KEY,
        win: bool,
    ) -> Result<HotkeyChoice, String> {
        if win {
            return Err("Windows-key shortcuts are reserved by Windows.".to_owned());
        }
        if vk == VK_F12 {
            return Err("F12 is reserved by Windows.".to_owned());
        }
        let Some(name) = key_name(vk) else {
            return Err("That key can't be used for the hotkey.".to_owned());
        };
        let modifiers = modifiers & (MOD_CONTROL | MOD_SHIFT | MOD_ALT);
        if !is_function_key(vk) && (modifiers & CTRL_ALT) == NO_MODIFIERS {
            return Err(format!("Add Ctrl or Alt to use {name}."));
        }
        if vk == VK_DELETE && (modifiers & CTRL_ALT) == CTRL_ALT {
            return Err("Ctrl+Alt+Delete is reserved by Windows.".to_owned());
        }
        Ok(HotkeyChoice { modifiers, vk })
    }

    /// The display parts in order, for example `["Ctrl", "Alt", "F6"]`.
    pub fn parts(self) -> Vec<&'static str> {
        MODIFIER_NAMES
            .iter()
            .filter(|(m, _)| (self.modifiers & *m) != NO_MODIFIERS)
            .map(|(_, name)| *name)
            .chain(key_name(self.vk))
            .collect()
    }

    /// The canonical display string, for example `Ctrl+Alt+F6`. This is also
    /// the persisted form.
    pub fn display(self) -> String {
        self.parts().join("+")
    }

    /// Parses a canonical display string (as produced by
    /// [`HotkeyChoice::display`]) back into a validated binding, or `None`
    /// when it is not one.
    pub fn parse(display: &str) -> Option<HotkeyChoice> {
        let (modifier_names, key) = match display.rsplit_once('+') {
            Some((modifier_names, key)) => (Some(modifier_names), key),
            None => (None, display),
        };
        let vk = KEY_NAMES.iter().find(|(_, name)| *name == key)?.0;
        let mut modifiers = NO_MODIFIERS;
        for name in modifier_names.into_iter().flat_map(|m| m.split('+')) {
            let (m, _) = MODIFIER_NAMES.iter().find(|(_, n)| *n == name)?;
            modifiers |= *m;
        }
        let choice = HotkeyChoice::new(modifiers, vk, false).ok()?;
        // Only the canonical spelling is accepted (no duplicates or reordering).
        (choice.display() == display).then_some(choice)
    }
}

/// The binding to start with: the persisted one when it is still valid,
/// otherwise the default.
pub fn resolve_startup_hotkey(stored: Option<&str>) -> HotkeyChoice {
    stored
        .and_then(HotkeyChoice::parse)
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
/// (including the terminator). Every valid binding is far shorter; anything
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
            resolve_startup_hotkey(Some("Ctrl+Alt+F9")).display(),
            "Ctrl+Alt+F9"
        );
        // Invalid or reserved bindings fall back.
        assert_eq!(
            resolve_startup_hotkey(Some("Ctrl+Alt+Delete")),
            DEFAULT_HOTKEY
        );
        assert_eq!(resolve_startup_hotkey(Some("garbage")), DEFAULT_HOTKEY);
    }

    #[test]
    fn default_is_ctrl_alt_f6() {
        assert_eq!(DEFAULT_HOTKEY.display(), "Ctrl+Alt+F6");
        assert_eq!(DEFAULT_HOTKEY.parts(), ["Ctrl", "Alt", "F6"]);
        assert_eq!(
            HotkeyChoice::new(CTRL_ALT, VK_F6, false),
            Ok(DEFAULT_HOTKEY)
        );
    }

    #[test]
    fn bindings_saved_by_earlier_versions_still_parse() {
        for display in [
            "Ctrl+Alt+F6",
            "Ctrl+Alt+F16",
            "Ctrl+Alt+Insert",
            "Ctrl+Alt+PageUp",
            "Ctrl+Alt+PageDown",
            "Ctrl+Shift+Alt+F9",
            "Ctrl+Alt+Space",
            "Ctrl+Alt+Backspace",
            "Ctrl+Alt+Period",
            "Ctrl+Alt+A",
        ] {
            let choice = HotkeyChoice::parse(display).expect(display);
            assert_eq!(choice.display(), display);
        }
    }

    #[test]
    fn every_key_round_trips() {
        for (vk, name) in KEY_NAMES {
            let choice = HotkeyChoice::new(MOD_CONTROL | MOD_SHIFT, *vk, false).expect(name);
            assert_eq!(HotkeyChoice::parse(&choice.display()), Some(choice));
        }
    }

    #[test]
    fn key_names_are_unique() {
        for (i, (vk_a, a)) in KEY_NAMES.iter().enumerate() {
            for (vk_b, b) in &KEY_NAMES[i + 1..] {
                assert_ne!(vk_a, vk_b, "{a} vs {b}");
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn new_enforces_the_binding_rules() {
        assert!(HotkeyChoice::new(CTRL_ALT, VK_F6, true).is_err()); // Win key
        assert!(HotkeyChoice::new(CTRL_ALT, VK_F12, false).is_err());
        assert!(HotkeyChoice::new(CTRL_ALT, VK_DELETE, false).is_err());
        assert!(HotkeyChoice::new(CTRL_ALT, VK_ESCAPE, false).is_err());
        assert!(HotkeyChoice::new(CTRL_ALT, VK_SHIFT, false).is_err());
        assert!(HotkeyChoice::new(MOD_SHIFT, VK_A, false).is_err());
        assert!(HotkeyChoice::new(NO_MODIFIERS, VK_A, false).is_err());
        // Function keys may stand alone; other keys need Ctrl or Alt.
        assert!(HotkeyChoice::new(NO_MODIFIERS, VK_F8, false).is_ok());
        assert!(HotkeyChoice::new(MOD_SHIFT, VK_F8, false).is_ok());
        assert!(HotkeyChoice::new(MOD_CONTROL, VK_A, false).is_ok());
        assert!(HotkeyChoice::new(MOD_ALT, VK_DELETE, false).is_ok());
        // Stray flags such as MOD_NOREPEAT are dropped.
        assert_eq!(
            HotkeyChoice::new(CTRL_ALT | MOD_NOREPEAT, VK_F6, false),
            Ok(DEFAULT_HOTKEY)
        );
    }

    #[test]
    fn parse_rejects_non_canonical_or_invalid_strings() {
        assert_eq!(
            HotkeyChoice::parse("F6"),
            HotkeyChoice::new(NO_MODIFIERS, VK_F6, false).ok()
        );
        assert_eq!(HotkeyChoice::parse("Ctrl+Alt+F12"), None);
        assert_eq!(HotkeyChoice::parse("A"), None);
        assert_eq!(HotkeyChoice::parse("Alt+Ctrl+F6"), None); // order
        assert_eq!(HotkeyChoice::parse("Ctrl+Ctrl+F6"), None); // duplicate
        assert_eq!(HotkeyChoice::parse("Ctrl+Alt+Win+F6"), None);
        assert_eq!(HotkeyChoice::parse(""), None);
        assert_eq!(HotkeyChoice::parse("+F6"), None);
        assert_eq!(HotkeyChoice::parse("Ctrl+Alt+F6 "), None); // trailing space
    }
}
