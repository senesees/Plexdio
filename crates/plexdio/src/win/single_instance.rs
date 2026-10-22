//! Single-instance guard. The first instance owns a named mutex and records its window handle
//! in the cache dir; a second instance finds the mutex taken, brings the first window to the
//! front and exits.

use std::path::PathBuf;

fn hwnd_file() -> PathBuf {
    crate::paths::cache_dir().join("instance.hwnd")
}

pub struct InstanceGuard {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: the mutex handle is only closed on drop.
unsafe impl Send for InstanceGuard {}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(hwnd_file());
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// Returns `Some(guard)` if this is the only instance, `None` if another is running.
pub fn acquire() -> Option<InstanceGuard> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let name: Vec<u16> = mutex_name().encode_utf16().chain(Some(0)).collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Some(InstanceGuard { handle });
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
            return None;
        }
        Some(InstanceGuard { handle })
    }
    #[cfg(not(windows))]
    {
        Some(InstanceGuard {})
    }
}

#[cfg(windows)]
fn mutex_name() -> String {
    // Scope the mutex by data dir so PLEXDIO_HOME test instances do not collide.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in crate::paths::cache_dir().to_string_lossy().bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    format!("Local\\Plexdio.SingleInstance.{h:016x}")
}

/// Remember our window so a later instance can activate it.
pub fn publish_window(hwnd: isize) {
    let path = hwnd_file();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(path, hwnd.to_string());
}

/// Bring the already-running instance's window to the foreground.
pub fn activate_existing() -> bool {
    let Some(hwnd) = std::fs::read_to_string(hwnd_file())
        .ok()
        .and_then(|s| s.trim().parse::<isize>().ok())
    else {
        return false;
    };
    show_window(hwnd)
}

/// Restore and focus a (possibly hidden/minimized) top-level window.
pub fn show_window(hwnd: isize) -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            IsWindow, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
        };
        let h = hwnd as windows_sys::Win32::Foundation::HWND;
        if IsWindow(h) == 0 {
            return false;
        }
        ShowWindow(h, SW_SHOW);
        ShowWindow(h, SW_RESTORE);
        SetForegroundWindow(h);
        true
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        false
    }
}
