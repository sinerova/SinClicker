//! Settings: the supported hotkey catalog, display formatting/parsing, and
//! persistence of the chosen binding.
//!
//! Everything besides the registry I/O is pure and unit-tested; no test
//! registers a hotkey or sends input.

/// `MOD_*` modifier bits, matching `RegisterHotKey` (without `MOD_NOREPEAT`,
/// which is added at registration time).
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_SHIFT: u32 = 0x0004;

/// Virtual-key codes (verified against `windows::...::KeyboardAndMouse`).
pub const VK_BACK: u32 = 0x08;
pub const VK_SPACE: u32 = 0x20;
pub const VK_END: u32 = 0x23;
pub const VK_HOME: u32 = 0x24;
pub const VK_PRIOR: u32 = 0x21; // Page Up
pub const VK_NEXT: u32 = 0x22; // Page Down
pub const VK_DELETE: u32 = 0x2E;
pub const VK_INSERT: u32 = 0x2D;
pub const VK_OEM_PERIOD: u32 = 0xBE; // left period / comma (en-US layout)
pub const VK_A: u32 = 0x41;
pub const VK_F1: u32 = 0x70;

/// A supported toggle hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyChoice {
    /// Canonical display string, for example `Ctrl+Alt+F6`.
    pub display: &'static str,
    /// Modifiers for `RegisterHotKey` (without `MOD_NOREPEAT`).
    pub modifiers: u32,
    /// Virtual-key code.
    pub vk: u32,
}

const fn choice(display: &'static str, modifiers: u32, vk: u32) -> HotkeyChoice {
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
/// combination appears (those are reserved for the OS), and F12 is excluded
/// (it is reserved for debuggers). This is the set `RegisterHotKey` can
/// reliably own.
pub const SUPPORTED_HOTKEYS: [HotkeyChoice; 23] = [
    choice("Ctrl+Alt+F6", MOD_CONTROL | MOD_ALT, VK_F1 + 5),
    choice("Ctrl+Alt+F7", MOD_CONTROL | MOD_ALT, VK_F1 + 6),
    choice("Ctrl+Alt+F8", MOD_CONTROL | MOD_ALT, VK_F1 + 7),
    choice("Ctrl+Alt+F9", MOD_CONTROL | MOD_ALT, VK_F1 + 8),
    choice("Ctrl+Alt+F10", MOD_CONTROL | MOD_ALT, VK_F1 + 9),
    choice("Ctrl+Alt+F11", MOD_CONTROL | MOD_ALT, VK_F1 + 10),
    choice("Ctrl+Alt+F13", MOD_CONTROL | MOD_ALT, VK_F1 + 12),
    choice("Ctrl+Alt+F14", MOD_CONTROL | MOD_ALT, VK_F1 + 13),
    choice("Ctrl+Alt+F15", MOD_CONTROL | MOD_ALT, VK_F1 + 14),
    choice("Ctrl+Alt+F16", MOD_CONTROL | MOD_ALT, VK_F1 + 15),
    choice("Ctrl+Alt+Insert", MOD_CONTROL | MOD_ALT, VK_INSERT),
    choice("Ctrl+Alt+Delete", MOD_CONTROL | MOD_ALT, VK_DELETE),
    choice("Ctrl+Alt+Home", MOD_CONTROL | MOD_ALT, VK_HOME),
    choice("Ctrl+Alt+End", MOD_CONTROL | MOD_ALT, VK_END),
    choice("Ctrl+Alt+PageUp", MOD_CONTROL | MOD_ALT, VK_PRIOR),
    choice("Ctrl+Alt+PageDown", MOD_CONTROL | MOD_ALT, VK_NEXT),
    choice(
        "Ctrl+Shift+Alt+F6",
        MOD_CONTROL | MOD_ALT | MOD_SHIFT,
        VK_F1 + 5,
    ),
    choice(
        "Ctrl+Shift+Alt+F7",
        MOD_CONTROL | MOD_ALT | MOD_SHIFT,
        VK_F1 + 6,
    ),
    choice(
        "Ctrl+Shift+Alt+F9",
        MOD_CONTROL | MOD_ALT | MOD_SHIFT,
        VK_F1 + 8,
    ),
    choice("Ctrl+Alt+Space", MOD_CONTROL | MOD_ALT, VK_SPACE),
    choice("Ctrl+Alt+Backspace", MOD_CONTROL | MOD_ALT, VK_BACK),
    choice("Ctrl+Alt+Period", MOD_CONTROL | MOD_ALT, VK_OEM_PERIOD),
    choice("Ctrl+Alt+A", MOD_CONTROL | MOD_ALT, VK_A),
];

/// The default display string; also the first (index 0) UI choice.
pub const DEFAULT_HOTKEY_DISPLAY: &str = "Ctrl+Alt+F6";

/// Canonical display strings of all supported hotkeys, in UI order.
pub fn supported_hotkey_displays() -> Vec<&'static str> {
    SUPPORTED_HOTKEYS.iter().map(|h| h.display).collect()
}

/// Resolves a display string (from the UI or from persisted storage) to a
/// supported choice, or `None` when it is not a supported form.
pub fn choice_for_display(display: &str) -> Option<HotkeyChoice> {
    SUPPORTED_HOTKEYS
        .iter()
        .find(|h| h.display == display)
        .copied()
}

/// Parses a canonical display string such as `Ctrl+Alt+F6` into
/// `(ctrl, alt, shift, key)`. Returns `None` when the string is not a
/// supported display form: both Ctrl and Alt must be present, any other
/// modifier token or malformed shape is rejected, and the key token must be a
/// short alphanumeric run.
pub fn parse_display(display: &str) -> Option<(bool, bool, bool, &str)> {
    let parts: Vec<&str> = display.split('+').collect();
    if parts.len() < 2 {
        return None;
    }
    let key = *parts.last()?;
    if !is_valid_key(key) {
        return None;
    }
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    for part in &parts[..parts.len() - 1] {
        match *part {
            "Ctrl" => ctrl = true,
            "Alt" => alt = true,
            "Shift" => shift = true,
            _ => return None,
        }
    }
    if !(ctrl && alt) {
        return None;
    }
    Some((ctrl, alt, shift, key))
}

/// A key token is valid when it is a short, non-empty alphanumeric run
/// (e.g. `F6`, `PageUp`, `Period`).
fn is_valid_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    !bytes.is_empty() && bytes.len() <= 10 && bytes.iter().all(|b| b.is_ascii_alphanumeric())
}

/// Formats a canonical display string from its parts, in `Ctrl+[Shift+]Alt+key`
/// order — the same modifier order the catalog's display strings use.
pub fn format_display(ctrl: bool, alt: bool, shift: bool, key: &str) -> String {
    let mut s = String::new();
    if ctrl {
        s.push_str("Ctrl");
    }
    if shift {
        s.push('+');
        s.push_str("Shift");
    }
    if alt {
        s.push('+');
        s.push_str("Alt");
    }
    s.push('+');
    s.push_str(key);
    s
}

/// Registry location (per-user, so no elevation is required) of the persisted
/// hotkey value.
#[cfg(target_os = "windows")]
pub const SETTINGS_SUBKEY: &str = "Software\\SinClicker";
#[cfg(target_os = "windows")]
const HOTKEY_VALUE: &str = "Hotkey";

/// A NUL-terminated UTF-16 string on a `Vec` that stays alive for a call.
#[cfg(target_os = "windows")]
struct WideString(Vec<u16>);

#[cfg(target_os = "windows")]
impl WideString {
    fn new(s: &str) -> Self {
        let mut v: Vec<u16> = s.encode_utf16().collect();
        v.push(0);
        WideString(v)
    }
    fn ptr(&mut self) -> windows::core::PWSTR {
        windows::core::PWSTR(self.0.as_mut_ptr())
    }
}

/// Reads the persisted binding display string, or `None` when nothing usable
/// is stored or reading fails (first launch, missing value, wrong type, or a
/// registry error).
#[cfg(target_os = "windows")]
pub fn load_hotkey_display() -> Option<String> {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_SZ,
        REG_VALUE_TYPE,
    };

    let mut subkey = WideString::new(SETTINGS_SUBKEY);
    let mut value = WideString::new(HOTKEY_VALUE);
    let mut hkey: HKEY = unsafe { core::mem::zeroed() };

    // SAFETY: `RegOpenKeyExW` writes the opened key handle into `hkey`; the
    // subkey is a live NUL-terminated UTF-16 string. `WIN32_ERROR(0)` means
    // success.
    if unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.ptr(),
            Some(0),
            KEY_READ,
            &mut hkey,
        )
    }
    .0 != 0
    {
        return None;
    }

    let result = (|| -> Option<String> {
        let mut size: u32 = 0;
        // SAFETY: a null data buffer with a size out-param asks only for the
        // stored value's size.
        if unsafe { RegQueryValueExW(hkey, value.ptr(), None, None, None, Some(&mut size)) }.0 != 0
            || size == 0
        {
            return None;
        }
        let mut data_type: REG_VALUE_TYPE = REG_VALUE_TYPE(0);
        let mut buf = vec![0u8; size as usize];
        // SAFETY: `buf` is exactly `size` bytes and outlives the query;
        // `data_type` outlives the query too.
        if unsafe {
            RegQueryValueExW(
                hkey,
                value.ptr(),
                None,
                Some(&mut data_type),
                Some(buf.as_mut_ptr()),
                Some(&mut size),
            )
        }
        .0 != 0
            || data_type != REG_SZ
        {
            return None;
        }
        // `size` is the byte count of the UTF-16 value (including its
        // terminator). Decode up to the first NUL.
        let wide_len = (size as usize).min(buf.len()).div_ceil(2);
        let wide: &[u16] =
            unsafe { core::slice::from_raw_parts(buf.as_ptr() as *const u16, wide_len) };
        let s = String::from_utf16_lossy(wide);
        let s = s.split('\0').next()?.to_owned();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    })();

    // SAFETY: `hkey` was opened above; this is its last use.
    let _ = unsafe { RegCloseKey(hkey) };
    result
}

/// Persists the chosen binding display string. The returned error carries the
/// failing Win32 code so the caller can surface it; the app keeps working
/// either way.
#[cfg(target_os = "windows")]
pub fn save_hotkey_display(value: &str) -> Result<(), String> {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let mut subkey = WideString::new(SETTINGS_SUBKEY);
    let mut value_name = WideString::new(HOTKEY_VALUE);
    let data: Vec<u8> = value
        .encode_utf16()
        .chain(core::iter::once(0u16))
        .flat_map(|c| c.to_le_bytes())
        .collect();

    let mut hkey: HKEY = unsafe { core::mem::zeroed() };
    // SAFETY: `RegCreateKeyExW` writes the created/opened key into `hkey`; the
    // subkey is a live NUL-terminated UTF-16 string; a null class name and
    // null security attributes are acceptable per the Win32 contract.
    // `WIN32_ERROR(0)` means success.
    let create = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            subkey.ptr(),
            Some(0),
            windows::core::PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        )
    };
    if create.0 != 0 {
        return Err(format!(
            "could not open the settings key (Windows error code {:#x})",
            create.0
        ));
    }

    // SAFETY: `data` is NUL-terminated UTF-16 (REG_SZ) and outlives the call.
    let set =
        unsafe { RegSetValueExW(hkey, value_name.ptr(), None, REG_SZ, Some(data.as_slice())) };
    // SAFETY: `hkey` was opened above; this is its last use.
    let _ = unsafe { RegCloseKey(hkey) };
    if set.0 != 0 {
        return Err(format!(
            "could not save the hotkey (Windows error code {:#x})",
            set.0
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_expected_size_and_order() {
        let displays = supported_hotkey_displays();
        assert_eq!(displays.len(), 23);
        assert_eq!(displays.first().copied(), Some("Ctrl+Alt+F6"));
        assert!(displays.contains(&"Ctrl+Shift+Alt+F6"));
        assert!(displays.contains(&"Ctrl+Alt+A"));
        // Reserved keys must never appear.
        for d in &displays {
            assert!(!d.contains("F12"), "F12 is reserved: {d}");
            assert!(!d.contains("Win"), "Win combinations are OS-reserved: {d}");
        }
    }

    #[test]
    fn catalog_entries_are_unique_by_vk_and_modifiers() {
        for a in SUPPORTED_HOTKEYS {
            for b in SUPPORTED_HOTKEYS {
                if (a.modifiers, a.vk) == (b.modifiers, b.vk) {
                    assert_eq!(a.display, b.display);
                }
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

    #[test]
    fn parse_display_accepts_supported_forms() {
        assert_eq!(
            parse_display("Ctrl+Alt+F6"),
            Some((true, true, false, "F6"))
        );
        assert_eq!(
            parse_display("Ctrl+Shift+Alt+F6"),
            Some((true, true, true, "F6"))
        );
        assert_eq!(
            parse_display("Ctrl+Alt+PageUp"),
            Some((true, true, false, "PageUp"))
        );
        assert_eq!(parse_display("Ctrl+Alt+A"), Some((true, true, false, "A")));
    }

    #[test]
    fn parse_display_rejects_unsupported_forms() {
        for bad in [
            "Ctrl+F6",                   // Alt missing
            "Alt+F6",                    // Ctrl missing
            "Win+F6",                    // unknown modifier
            "Ctrl+Alt",                  // no key
            "Ctrl+Alt+",                 // dangling empty key
            "",                          // empty
            "Ctrl++Alt+F6",              // empty middle token
            "ctrl+Alt+F6",               // case is significant
            "Ctrl+Alt+averylongkeyname", // key too long
            "Ctrl+Alt+PageUp Up",        // space in key
            "Ctrl+Alt+F-6",              // non-alphanumeric in key
        ] {
            assert_eq!(parse_display(bad), None, "for {bad:?}");
        }
    }

    #[test]
    fn format_display_is_canonical() {
        assert_eq!(format_display(true, true, false, "F6"), "Ctrl+Alt+F6");
        assert_eq!(format_display(true, true, true, "F7"), "Ctrl+Shift+Alt+F7");
        assert_eq!(format_display(true, true, false, "A"), "Ctrl+Alt+A");
    }

    #[test]
    fn catalog_displays_parse_and_reformat_to_themselves() {
        for h in SUPPORTED_HOTKEYS {
            let Some((c, a, s, k)) = parse_display(h.display) else {
                panic!("does not parse: {}", h.display);
            };
            assert_eq!(format_display(c, a, s, k), h.display, "for {}", h.display);
        }
    }
}
