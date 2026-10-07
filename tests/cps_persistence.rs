//! End-to-end persistence of the CPS value against the real per-user registry
//! (HKCU\Software\SinClicker\Cps). One sequential test function, because the
//! cases share that value: every mutation is followed by a read through the
//! real `save_cps`/`load_cps`/`resolve_startup_cps` code paths, and a
//! drop-guard restores the user's prior value (type + bytes) or removes it.
//!
//! Wrong-typed entries (REG_SZ/REG_QWORD) and malformed values are exercised
//! against the built application via UIA (see PROGRESS.md), not here: the
//! type check inside `load_cps` is internal and such values cannot be
//! produced through the public API.

use sinclicker::clicker::{DEFAULT_CPS, MAX_CPS, MIN_CPS};
use sinclicker::settings::{load_cps, resolve_startup_cps, save_cps};

use windows::core::HSTRING;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE,
};

const SUBKEY: &str = "Software\\SinClicker";
const CPS_VALUE: &str = "Cps";

struct WideString(Vec<u16>);

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

/// Opens (creating) the settings key for writing.
fn open_key_for_write() -> HKEY {
    let mut subkey = WideString::new(SUBKEY);
    let mut hkey: HKEY = unsafe { core::mem::zeroed() };
    // SAFETY: `subkey` is a live NUL-terminated UTF-16 string; `RegCreateKeyExW`
    // writes its result into `hkey`; null class/security attributes are valid
    // per the Win32 contract. `WIN32_ERROR(0)` means success.
    let code = unsafe {
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
    assert_eq!(code.0, 0, "could not open the settings key");
    hkey
}

/// Queries the raw stored value (type + payload bytes), or `None` when the
/// value is absent.
fn query_raw(hkey: HKEY) -> Option<(REG_VALUE_TYPE, Vec<u8>)> {
    let mut value = WideString::new(CPS_VALUE);
    let mut size: u32 = 0;
    // SAFETY: a null data buffer with a size out-param asks only for the
    // stored value's size.
    if unsafe { RegQueryValueExW(hkey, value.ptr(), None, None, None, Some(&mut size)) }.0 != 0 {
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
    {
        return None;
    }
    buf.truncate(size as usize);
    Some((data_type, buf))
}

fn delete_value(hkey: HKEY) {
    let mut value = WideString::new(CPS_VALUE);
    // SAFETY: `value` is a live NUL-terminated UTF-16 string; the removal is
    // best-effort (the value may already be absent).
    let _ = unsafe { RegDeleteValueW(hkey, value.ptr()) };
}

/// Rewrites the user's prior value verbatim, or removes the value when the
/// user had none. Called from the drop guard at test end; opens its own
/// write handle because the snapshot's read handle cannot write.
fn restore_prior(prior: &Option<(u32, Vec<u8>)>) {
    let value_name = HSTRING::from(CPS_VALUE);
    let mut subkey = WideString::new(SUBKEY);
    if let Some((typ, bytes)) = prior {
        let mut hkey: HKEY = unsafe { core::mem::zeroed() };
        // SAFETY: `subkey` is a live NUL-terminated UTF-16 string;
        // `RegCreateKeyExW` writes its result into `hkey`. Creating on demand
        // is correct: a missing key means the user had no settings at all,
        // and restoring a value requires its key.
        let code = unsafe {
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
        assert_eq!(code.0, 0, "could not open the settings key for restore");
        // SAFETY: `(typ, bytes)` came straight back from `RegQueryValueExW`,
        // so it is a consistent registry payload that outlives the call.
        let code =
            unsafe { RegSetValueExW(hkey, &value_name, None, REG_VALUE_TYPE(*typ), Some(bytes)) };
        // SAFETY: `hkey` was opened above; this is its last use.
        let _ = unsafe { RegCloseKey(hkey) };
        assert_eq!(code.0, 0, "could not restore the prior settings value");
    } else {
        // Best-effort removal; a missing key or value is the correct state.
        let mut hkey: HKEY = unsafe { core::mem::zeroed() };
        // SAFETY: `subkey` is a live NUL-terminated UTF-16 string;
        // `RegOpenKeyExW` writes its result into `hkey`; the key may be absent.
        let code = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.ptr(),
                Some(0),
                KEY_SET_VALUE,
                &mut hkey,
            )
        };
        if code.0 == 0 {
            // SAFETY: `value_name` is a valid value name; the value may
            // already be absent, which is the correct restored state.
            let _ = unsafe { RegDeleteValueW(hkey, &value_name) };
            // SAFETY: `hkey` was opened above; this is its last use.
            let _ = unsafe { RegCloseKey(hkey) };
        }
    }
}

#[cfg(target_os = "windows")]
#[test]
fn cps_persists_and_falls_back() {
    // ---- snapshot the user's prior value; restore it when the guard drops
    let mut subkey = WideString::new(SUBKEY);
    let mut hkey: HKEY = unsafe { core::mem::zeroed() };
    // SAFETY: `subkey` is a live NUL-terminated UTF-16 string; `RegOpenKeyExW`
    // writes its result into `hkey`. The key may be absent (first launch),
    // in which case there is nothing to restore.
    let open = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.ptr(),
            Some(0),
            KEY_READ,
            &mut hkey,
        )
    };
    assert!(open.0 == 0 || open.0 == 2, "RegOpenKeyExW: {}", open.0);

    struct Restorer {
        hkey: HKEY,
        key_open: bool,
        prior: Option<(u32, Vec<u8>)>,
        restored: bool,
    }
    impl Drop for Restorer {
        fn drop(&mut self) {
            if self.key_open {
                // SAFETY: `hkey` was opened before the guard was created;
                // this is its last use here (restore_prior opens its own
                // write handle, so the read handle must be closed first).
                let _ = unsafe { RegCloseKey(self.hkey) };
            }
            if !self.restored {
                self.restored = true;
                restore_prior(&self.prior);
            }
        }
    }

    let _guard = Restorer {
        hkey,
        key_open: open.0 == 0,
        prior: if open.0 == 0 {
            // SAFETY: `hkey` is open; `query_raw` only reads.
            query_raw(hkey).map(|(dt, bytes)| (dt.0, bytes))
        } else {
            None
        },
        restored: false,
    };

    // The guard keeps its read handle open until the end of the test (or a
    // panic), so the test's own mutations need a separate write handle.
    let write_key = open_key_for_write();

    // ---- in-range round-trips: save -> read -> resolve
    for cps in [MIN_CPS, DEFAULT_CPS, MAX_CPS, 250] {
        save_cps(cps).expect("save_cps must succeed");
        assert_eq!(load_cps(), Some(cps), "load_cps after save_cps({cps})");
        assert_eq!(
            resolve_startup_cps(load_cps()),
            cps,
            "resolve_startup_cps after save_cps({cps})"
        );
    }

    // ---- absent value: reads back None and resolves to the default
    delete_value(write_key);
    assert_eq!(load_cps(), None, "absent value must read as None");
    assert_eq!(
        resolve_startup_cps(load_cps()),
        DEFAULT_CPS,
        "absent value must resolve to the default"
    );

    // ---- out-of-range stored values: stored verbatim by the public API, but
    // resolved to the default
    for bad in [0i32, 501, 999] {
        save_cps(bad).expect("save_cps accepts any i32");
        assert_eq!(load_cps(), Some(bad), "store of {bad} must be readable");
        assert_eq!(
            resolve_startup_cps(load_cps()),
            DEFAULT_CPS,
            "stored {bad} must resolve to the default"
        );
    }

    // Leave an in-range value behind; the guard rewrites the user's prior
    // state afterwards, so this never outlives the test.
    save_cps(DEFAULT_CPS).expect("final save must succeed");

    // SAFETY: `write_key` was opened above; this is its last use.
    let _ = unsafe { RegCloseKey(write_key) };
}
