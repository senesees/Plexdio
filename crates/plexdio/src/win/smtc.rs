//! Windows "now playing" overlay through the System Media Transport Controls.
//!
//! SMTC is what fills the media flyout (volume key popup, lock screen and the hardware media
//! keys on keyboards that report through it). It is best effort: the WinRT class is missing in
//! Wine and on older Windows builds, and Windows only gives it to processes with a COM apartment.
//! When `init` returns `None` the app keeps the `RegisterHotKey` media keys from `tray.rs`, which
//! work everywhere.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A transport button pressed on the Windows media overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtcButton {
    PlayPause,
    Next,
    Prev,
}

/// Button presses raised by SMTC, shared between the COM callback (which runs on the UI
/// thread's message pump) and the next frame.
#[derive(Clone, Default)]
pub struct SmtcEvents(Arc<Mutex<VecDeque<SmtcButton>>>);

impl SmtcEvents {
    pub fn drain(&self) -> Vec<SmtcButton> {
        match self.0.lock() {
            Ok(mut q) => q.drain(..).collect(),
            Err(_) => Vec::new(),
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn push(&self, button: SmtcButton) {
        if let Ok(mut q) = self.0.lock() {
            q.push_back(button);
        }
        // The app paints on demand, so make sure the queued button is seen.
        crate::ui::runner::wake();
    }
}

/// What the overlay should show. Borrowed to avoid clones on every frame.
pub struct NowPlaying<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub playing: bool,
}

/// Live SMTC connection. Dropping it releases the overlay session.
pub struct Smtc {
    #[cfg(windows)]
    inner: WindowsSmtc,
}

impl Smtc {
    pub fn update(&mut self, info: &NowPlaying<'_>) {
        #[cfg(windows)]
        self.inner.update(info);
        #[cfg(not(windows))]
        let _ = info;
    }

    /// Whether the media overlay is actually connected (for the log and tests).
    #[cfg(windows)]
    pub fn is_live(&self) -> bool {
        true
    }
}

/// Connect the window to the Windows media overlay. `hwnd` is the winit window's native handle.
#[cfg(windows)]
pub fn init(hwnd: isize, events: SmtcEvents) -> Option<Smtc> {
    use windows::core::{factory, HSTRING};
    use windows::Foundation::TypedEventHandler;
    use windows::Media::{MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;

    if hwnd == 0 {
        return None;
    }
    let interop: ISystemMediaTransportControlsInterop =
        match factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>() {
        Ok(f) => f,
        Err(e) => {
            log::info!("media overlay unavailable: {e}");
            return None;
        }
    };
    let controls: SystemMediaTransportControls = match unsafe { interop.GetForWindow(HWND(hwnd)) } {
        Ok(c) => c,
        Err(e) => {
            log::info!("media overlay unavailable: {e}");
            return None;
        }
    };
    let _ = controls.SetIsEnabled(true);
    let _ = controls.SetIsPlayEnabled(true);
    let _ = controls.SetIsPauseEnabled(true);
    let _ = controls.SetIsNextEnabled(true);
    let _ = controls.SetIsPreviousEnabled(true);

    // The handler runs on this (UI) thread while the window pumps messages.
    let handler_events = events.clone();
    let handler = TypedEventHandler::<SystemMediaTransportControls, windows::Media::SystemMediaTransportControlsButtonPressedEventArgs>::new(move |_sender, args| {
        if let Some(args) = args {
            let _: windows::core::Result<()> = (|| {
                use windows::Media::SystemMediaTransportControlsButton as Button;
                match args.Button()? {
                    Button::Play | Button::Pause => handler_events.push(SmtcButton::PlayPause),
                    Button::Next => handler_events.push(SmtcButton::Next),
                    Button::Previous => handler_events.push(SmtcButton::Prev),
                    _ => {}
                }
                Ok(())
            })();
        }
        Ok(())
    });
    if let Err(e) = controls.ButtonPressed(&handler) {
        // Wine's SMTC stub exists but rejects event registration; the media keys still work.
        log::info!("media overlay buttons unavailable: {e}");
    }

    let timeline = windows::Media::SystemMediaTransportControlsTimelineProperties::new().ok();
    if let Ok(updater) = controls.DisplayUpdater() {
        let _ = updater.SetType(MediaPlaybackType::Music);
        let _ = updater.MusicProperties().and_then(|p| p.SetTitle(&HSTRING::from("Plexdio")));
        let _ = updater.Update();
    }
    let _ = controls.SetPlaybackStatus(MediaPlaybackStatus::Stopped);
    log::info!("media overlay connected");
    Some(Smtc {
        inner: WindowsSmtc {
            controls,
            timeline,
            last_title: String::new(),
            last_album: String::new(),
            last_artist: String::new(),
            last_playing: None,
            last_bucket: u64::MAX,
            last_duration: u64::MAX,
        },
    })
}

#[cfg(not(windows))]
pub fn init(_hwnd: isize, _events: SmtcEvents) -> Option<Smtc> {
    None
}

#[cfg(windows)]
struct WindowsSmtc {
    controls: windows::Media::SystemMediaTransportControls,
    timeline: Option<windows::Media::SystemMediaTransportControlsTimelineProperties>,
    last_title: String,
    last_artist: String,
    last_album: String,
    last_playing: Option<bool>,
    /// Position in whole seconds, to keep timeline updates cheap while the seek bar moves.
    last_bucket: u64,
    last_duration: u64,
}

#[cfg(windows)]
impl WindowsSmtc {
    fn update(&mut self, info: &NowPlaying<'_>) {
        use windows::core::HSTRING;
        use windows::Foundation::TimeSpan;
        use windows::Media::MediaPlaybackStatus;

        if info.title != self.last_title || info.artist != self.last_artist || info.album != self.last_album {
            if let Ok(updater) = self.controls.DisplayUpdater() {
                let _ = updater.SetType(windows::Media::MediaPlaybackType::Music);
                if let Ok(props) = updater.MusicProperties() {
                    let _ = props.SetTitle(&HSTRING::from(info.title));
                    let _ = props.SetArtist(&HSTRING::from(info.artist));
                    let _ = props.SetAlbumTitle(&HSTRING::from(info.album));
                }
                let _ = updater.Update();
            }
            self.last_title = info.title.to_string();
            self.last_artist = info.artist.to_string();
            self.last_album = info.album.to_string();
        }

        if self.last_playing != Some(info.playing) {
            let status = if info.playing { MediaPlaybackStatus::Playing } else { MediaPlaybackStatus::Paused };
            let _ = self.controls.SetPlaybackStatus(status);
            self.last_playing = Some(info.playing);
        }

        let bucket = info.position_ms / 1000;
        if self.timeline.is_some() && (bucket != self.last_bucket || info.duration_ms != self.last_duration) {
            let end = TimeSpan { Duration: (info.duration_ms as i64).saturating_mul(10_000) };
            let pos = TimeSpan { Duration: (info.position_ms as i64).saturating_mul(10_000) };
            if let Some(tl) = &self.timeline {
                let _ = tl.SetStartTime(TimeSpan { Duration: 0 });
                let _ = tl.SetEndTime(end);
                let _ = tl.SetMinSeekTime(TimeSpan { Duration: 0 });
                let _ = tl.SetMaxSeekTime(end);
                let _ = tl.SetPosition(pos);
                let _ = self.controls.UpdateTimelineProperties(tl);
            }
            self.last_bucket = bucket;
            self.last_duration = info.duration_ms;
        }
    }
}
