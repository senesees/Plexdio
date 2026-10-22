//! Windows-specific integration. Every item has a no-op fallback on other platforms so the
//! app (and its tests) also build and run natively on Linux.

pub mod single_instance;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Native window handle as an integer (HWND on Windows), if available.
pub fn hwnd_of(h: &impl HasWindowHandle) -> Option<isize> {
    match h.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
        _ => None,
    }
}


/// True when running under Wine (ntdll exports `wine_get_version`).
pub fn is_wine() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let name: Vec<u16> = "ntdll.dll".encode_utf16().chain(Some(0)).collect();
        let h = GetModuleHandleW(name.as_ptr());
        !h.is_null() && GetProcAddress(h, c"wine_get_version".as_ptr().cast()).is_some()
    }
    #[cfg(not(windows))]
    false
}

/// Wine workaround: hide winit's "thread event target" helper window.
///
/// winit keeps that 0x0 layered popup `WS_VISIBLE` and calls `RedrawWindow(RDW_INTERNALPAINT)` on
/// it for every message it receives. Wine never delivers the resulting `WM_PAINT`, so the thread's
/// `QS_PAINT` bit stays set, `MsgWaitForMultipleObjectsEx` never blocks, and the idle UI thread
/// spins a core. A hidden window does not take part in paint scheduling. On real Windows the
/// window is left alone, because winit relies on it during modal resize loops.
pub fn fix_wine_event_target() {
    #[cfg(windows)]
    unsafe {
        if !is_wine() {
            return;
        }
        use windows_sys::Win32::Foundation::{HWND, LPARAM};
        use windows_sys::Win32::System::Threading::GetCurrentThreadId;
        use windows_sys::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, GetClassNameW, ShowWindow, SW_HIDE};
        unsafe extern "system" fn cb(hwnd: HWND, _: LPARAM) -> i32 {
            let mut cls = [0u16; 64];
            let n = unsafe { GetClassNameW(hwnd, cls.as_mut_ptr(), 64) };
            if String::from_utf16_lossy(&cls[..n.max(0) as usize]) == "Winit Thread Event Target" {
                unsafe { ShowWindow(hwnd, SW_HIDE) };
                log::info!("Wine detected: hid winit event-target window to avoid idle busy loop");
            }
            1
        }
        EnumThreadWindows(GetCurrentThreadId(), Some(cb), 0);
    }
}
