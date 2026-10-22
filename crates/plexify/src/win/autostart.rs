//! Start with Windows: a value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//! Per-user, so no admin rights are needed, and turning it off only removes our own value.

/// Name of the Run value (stable across versions so re-enabling replaces it).
#[cfg_attr(not(windows), allow(dead_code))]
const VALUE: &str = "Plexify";

/// True when the Run value exists (regardless of the exact path it points at).
#[cfg(windows)]
pub fn is_enabled() -> bool {
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegQueryValueExW, HKEY_CURRENT_USER, KEY_QUERY_VALUE, REG_SZ};
    let Some(key) = open(HKEY_CURRENT_USER, KEY_QUERY_VALUE) else { return false };
    let mut kind = 0u32;
    let mut len = 0u32;
    let name = wide(VALUE);
    let found = unsafe { RegQueryValueExW(key, name.as_ptr(), std::ptr::null_mut(), &mut kind, std::ptr::null_mut(), &mut len) } == 0;
    unsafe { RegCloseKey(key) };
    found && kind == REG_SZ
}

#[cfg(not(windows))]
pub fn is_enabled() -> bool {
    false
}

/// Add or remove the Run value. The command line quotes the exe so paths with spaces work.
/// Returns whether the resulting state matches the request.
#[cfg(windows)]
pub fn set_enabled(enabled: bool) -> bool {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegSetValueExW, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ,
    };
    let Some(key) = open(HKEY_CURRENT_USER, KEY_SET_VALUE) else { return false };
    let name = wide(VALUE);
    let ok = if enabled {
        let Some(exe) = std::env::current_exe().ok() else {
            unsafe { RegCloseKey(key) };
            return false;
        };
        let cmd = format!("\"{}\"", exe.display());
        let data: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
        let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2) };
        (unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, bytes.as_ptr(), bytes.len() as u32) }) == 0
    } else {
        // Deleting a missing value is success enough.
        let r = unsafe { RegDeleteValueW(key, name.as_ptr()) };
        r == 0 || r == 2 // ERROR_FILE_NOT_FOUND
    };
    unsafe { RegCloseKey(key) };
    if ok {
        log::info!("start with Windows: {}", if enabled { "enabled" } else { "disabled" });
    } else {
        log::warn!("start with Windows: could not {} the Run entry", if enabled { "add" } else { "remove" });
    }
    ok
}

#[cfg(not(windows))]
pub fn set_enabled(_enabled: bool) -> bool {
    false
}

/// Apply the saved setting at startup. Does nothing when the current state already matches.
pub fn apply(enabled: bool) {
    if is_enabled() != enabled && !set_enabled(enabled) {
        log::warn!("cannot apply the start-with-Windows setting");
    }
}

#[cfg(windows)]
fn open(root: windows_sys::Win32::System::Registry::HKEY, access: u32) -> Option<windows_sys::Win32::System::Registry::HKEY> {
    use windows_sys::Win32::System::Registry::{RegOpenKeyExW, HKEY};
    let sub = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let mut key: HKEY = std::ptr::null_mut();
    let ok = unsafe { RegOpenKeyExW(root, sub.as_ptr(), 0, access, &mut key) };
    if ok == 0 { Some(key) } else { None }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn non_windows_stubs_are_inert() {
        assert!(!is_enabled());
        assert!(!set_enabled(true));
        apply(false);
    }
}
