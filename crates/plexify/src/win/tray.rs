//! Tray icon, tray menu and global media keys.
//!
//! A hidden message window on its own thread owns the notification icon and the media-key
//! hotkeys, so none of this touches winit's window procedure. Events are forwarded to the UI
//! thread through a channel and handled on the next frame. The menu and the icon are plain
//! Win32 calls: no extra crate and nothing to deploy.

use std::sync::mpsc::{Receiver, TryRecvError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    Show,
    PlayPause,
    Next,
    Prev,
    Quit,
}

/// Handle to the tray thread. Dropping it removes the icon and stops the thread.
pub struct TrayHandle {
    rx: Receiver<TrayEvent>,
    #[cfg(windows)]
    hwnd: isize,
}

impl TrayHandle {
    /// Next event from the tray or the media keys, if any.
    pub fn try_recv(&self) -> Option<TrayEvent> {
        match self.rx.try_recv() {
            Ok(e) => Some(e),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }

    /// Update the icon tooltip (track title while playing).
    pub fn set_tip(&self, _text: &str) {
        #[cfg(windows)]
        {
            if let Ok(mut tip) = TIP.lock() {
                *tip = _text.to_string();
            }
            unsafe { PostMessageW(self.hwnd as HWND, TIP_UPDATE, 0, 0) };
        }
    }

    /// Remove the icon and stop the thread (idempotent).
    pub fn stop(&self) {
        #[cfg(windows)]
        unsafe {
            PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0);
        }
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the tray thread. Returns `None` when there is no tray (non-Windows, or Win32 failed);
/// the app then simply has no tray icon and no media-key handling.
#[cfg(not(windows))]
pub fn start() -> Option<TrayHandle> {
    None
}

#[cfg(windows)]
pub fn start() -> Option<TrayHandle> {
    use std::sync::mpsc::RecvTimeoutError;
    let (tx, rx) = channel();
    let (ready_tx, ready_rx) = channel::<isize>();
    let spawned = std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || run_tray(tx, ready_tx));
    if let Err(e) = spawned {
        log::warn!("cannot start the tray thread: {e}");
        return None;
    }
    // The window is created synchronously at thread start; a short wait keeps the caller simple.
    match ready_rx.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(hwnd) if hwnd != 0 => Some(TrayHandle { rx, hwnd }),
        Ok(_) => {
            log::warn!("tray icon could not be created");
            None
        }
        Err(RecvTimeoutError::Timeout) => {
            log::warn!("tray thread did not start in time");
            None
        }
        Err(RecvTimeoutError::Disconnected) => {
            log::warn!("tray thread failed");
            None
        }
    }
}

// ------------------------------------------------------------------ Windows implementation

#[cfg(windows)]
use std::sync::mpsc::{channel, Sender};
#[cfg(windows)]
use std::sync::Mutex;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK,
};
#[cfg(windows)]
use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW,
    LoadIconW, PostMessageW, PostQuitMessage, RegisterClassW, SetForegroundWindow, TrackPopupMenu, TranslateMessage,
    CW_USEDEFAULT, HMENU, MF_SEPARATOR, MF_STRING, MSG, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_COMMAND, WM_HOTKEY,
    WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};

#[cfg(windows)]
static TIP: Mutex<String> = Mutex::new(String::new());

#[cfg(windows)]
thread_local! {
    static EVENTS: std::cell::RefCell<Option<Sender<TrayEvent>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(windows)]
const TRAY_CALLBACK: u32 = WM_APP + 1;
#[cfg(windows)]
const TIP_UPDATE: u32 = WM_APP + 2;
#[cfg(windows)]
const HOTKEY_PLAY: i32 = 1;
#[cfg(windows)]
const HOTKEY_NEXT: i32 = 2;
#[cfg(windows)]
const HOTKEY_PREV: i32 = 3;
#[cfg(windows)]
const MENU_SHOW: usize = 1;
#[cfg(windows)]
const MENU_PLAY: usize = 2;
#[cfg(windows)]
const MENU_NEXT: usize = 3;
#[cfg(windows)]
const MENU_PREV: usize = 4;
#[cfg(windows)]
const MENU_QUIT: usize = 5;

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

#[cfg(windows)]
fn send(e: TrayEvent) {
    EVENTS.with(|s| {
        if let Some(tx) = s.borrow().as_ref() {
            let _ = tx.send(e);
        }
    });
    // The app only draws on demand, so nudge the event loop out of its wait.
    crate::ui::runner::wake();
}

#[cfg(windows)]
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            TRAY_CALLBACK => {
                match lparam as u32 {
                    WM_LBUTTONUP => send(TrayEvent::Show),
                    WM_RBUTTONUP => {
                        let mut pt = POINT { x: 0, y: 0 };
                        GetCursorPos(&mut pt);
                        // Required so the menu closes when the user clicks elsewhere.
                        SetForegroundWindow(hwnd);
                        let menu: HMENU = CreatePopupMenu();
                        if !menu.is_null() {
                            let items = [
                                (MENU_SHOW, "Show Plexify"),
                                (MENU_PLAY, "Play / Pause"),
                                (MENU_NEXT, "Next track"),
                                (MENU_PREV, "Previous track"),
                            ];
                            for (id, text) in items {
                                let t = wide(text);
                                AppendMenuW(menu, MF_STRING, id, t.as_ptr());
                            }
                            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
                            let quit = wide("Quit Plexify");
                            AppendMenuW(menu, MF_STRING, MENU_QUIT, quit.as_ptr());
                            let cmd = TrackPopupMenu(
                                menu,
                                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                                pt.x,
                                pt.y,
                                0,
                                hwnd,
                                std::ptr::null(),
                            ) as usize;
                            DestroyMenu(menu);
                            match cmd {
                                MENU_SHOW => send(TrayEvent::Show),
                                MENU_PLAY => send(TrayEvent::PlayPause),
                                MENU_NEXT => send(TrayEvent::Next),
                                MENU_PREV => send(TrayEvent::Prev),
                                MENU_QUIT => send(TrayEvent::Quit),
                                _ => {}
                            }
                            PostMessageW(hwnd, WM_NULL, 0, 0);
                        }
                    }
                    _ => {}
                }
                0
            }
            WM_HOTKEY => {
                match wparam as i32 {
                    HOTKEY_PLAY => send(TrayEvent::PlayPause),
                    HOTKEY_NEXT => send(TrayEvent::Next),
                    HOTKEY_PREV => send(TrayEvent::Prev),
                    _ => {}
                }
                0
            }
            WM_COMMAND => {
                match wparam & 0xffff {
                    MENU_SHOW => send(TrayEvent::Show),
                    MENU_PLAY => send(TrayEvent::PlayPause),
                    MENU_NEXT => send(TrayEvent::Next),
                    MENU_PREV => send(TrayEvent::Prev),
                    MENU_QUIT => send(TrayEvent::Quit),
                    _ => {}
                }
                0
            }
            TIP_UPDATE => {
                update_tip(hwnd);
                0
            }
            WM_CLOSE => {
                remove_icon(hwnd);
                UnregisterHotKey(hwnd, HOTKEY_PLAY);
                UnregisterHotKey(hwnd, HOTKEY_NEXT);
                UnregisterHotKey(hwnd, HOTKEY_PREV);
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

// MAKEINTRESOURCE: a small integer cast to a pointer is the Win32 way to name a resource id.
#[cfg(windows)]
#[allow(clippy::manual_dangling_ptr)]
fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = TRAY_CALLBACK;
    let hinst = unsafe { GetModuleHandleW(std::ptr::null()) };
    // Resource id 1 is the exe icon embedded by build.rs; fall back to the generic app icon.
    let mut icon = unsafe { LoadIconW(hinst, 1 as *const u16) };
    if icon.is_null() {
        icon = unsafe { LoadIconW(std::ptr::null_mut(), 32512 as *const u16) }; // IDI_APPLICATION
    }
    nid.hIcon = icon;
    let tip: Vec<u16> = TIP
        .lock()
        .map(|t| if t.is_empty() { "Plexify".to_string() } else { t.clone() })
        .unwrap_or_else(|_| "Plexify".into())
        .encode_utf16()
        .collect();
    let n = tip.len().min(nid.szTip.len() - 1);
    nid.szTip[..n].copy_from_slice(&tip[..n]);
    nid
}

#[cfg(windows)]
fn update_tip(hwnd: HWND) {
    let mut nid = icon_data(hwnd);
    nid.uFlags = NIF_TIP;
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

#[cfg(windows)]
fn remove_icon(hwnd: HWND) {
    let mut nid = icon_data(hwnd);
    nid.uFlags = 0;
    unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
}

#[cfg(windows)]
fn run_tray(tx: Sender<TrayEvent>, ready: Sender<isize>) {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW as Module;
    unsafe {
        EVENTS.with(|s| *s.borrow_mut() = Some(tx));
        let hinst = Module(std::ptr::null());
        let class_name = wide("PlexifyTray");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        if RegisterClassW(&wc) == 0 {
            log::warn!("tray: RegisterClass failed");
            let _ = ready.send(0);
            return;
        }
        // A normal, never-shown window: some shells ignore tray icons on message-only windows.
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            wide("Plexify").as_ptr(),
            0,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            log::warn!("tray: CreateWindow failed");
            let _ = ready.send(0);
            return;
        }
        let nid = icon_data(hwnd);
        if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
            log::warn!("tray: Shell_NotifyIcon(NIM_ADD) failed");
            let _ = ready.send(0);
            return;
        }
        if RegisterHotKey(hwnd, HOTKEY_PLAY, 0, VK_MEDIA_PLAY_PAUSE as u32) == 0 {
            log::debug!("tray: the play/pause media key is already registered by another program");
        }
        if RegisterHotKey(hwnd, HOTKEY_NEXT, 0, VK_MEDIA_NEXT_TRACK as u32) == 0 {
            log::debug!("tray: the next-track media key is already registered");
        }
        if RegisterHotKey(hwnd, HOTKEY_PREV, 0, VK_MEDIA_PREV_TRACK as u32) == 0 {
            log::debug!("tray: the previous-track media key is already registered");
        }
        log::info!("tray icon and media keys ready");
        let _ = ready.send(hwnd as isize);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
