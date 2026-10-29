//! User settings persisted as JSON in the config dir.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn cycle(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }
}

/// Streaming quality: original file (direct play) or a transcoded bitrate cap in kbps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StreamQuality {
    #[default]
    Original,
    Kbps(u32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Persistent X-Plex-Client-Identifier.
    pub client_id: String,
    /// Override for plex.tv (used with the mock server).
    pub plex_tv_url: Option<String>,
    pub last_server_id: Option<String>,
    pub last_section_key: Option<String>,
    pub last_connection_uri: Option<String>,
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub crossfade_secs: f32,
    pub normalization: bool,
    pub autoplay: bool,
    pub quality: StreamQuality,
    pub image_cache_mb: u64,
    pub audio_cache_enabled: bool,
    pub audio_cache_mb: u64,
    pub minimize_to_tray: bool,
    pub close_to_tray: bool,
    pub start_with_windows: bool,
    /// Show the playing track on Discord (Rich Presence). Off by default: it shares activity.
    pub discord_presence: bool,
    /// The user's own Discord application ID (Developer Portal); empty uses the built-in one.
    pub discord_client_id: String,
    /// Show the album cover (looked up on Deezer/iTunes) instead of the app logo.
    pub discord_covers: bool,
    /// Equaliser on/off and gain per band in dB (see `audio::eq::BANDS`).
    pub eq_enabled: bool,
    pub eq_gains: [f32; crate::audio::eq::BAND_COUNT],
    /// Output device name; `None` follows the Windows default device.
    pub output_device: Option<String>,
    pub window_size: Option<[f32; 2]>,
    /// Library tab sorts and filter chips.
    pub library: crate::ui::views::library::LibraryPrefs,
    /// Interface zoom (1.0 = 100 %), 0.8..=1.5.
    pub zoom: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client_id: uuid::Uuid::new_v4().to_string(),
            plex_tv_url: None,
            last_server_id: None,
            last_section_key: None,
            last_connection_uri: None,
            volume: 0.8,
            muted: false,
            shuffle: false,
            repeat: RepeatMode::Off,
            crossfade_secs: 0.0,
            normalization: true,
            autoplay: true,
            quality: StreamQuality::Original,
            image_cache_mb: 200,
            audio_cache_enabled: true,
            audio_cache_mb: 500,
            minimize_to_tray: false,
            close_to_tray: false,
            start_with_windows: false,
            discord_presence: false,
            discord_client_id: String::new(),
            discord_covers: true,
            eq_enabled: false,
            eq_gains: [0.0; crate::audio::eq::BAND_COUNT],
            output_device: None,
            window_size: None,
            library: Default::default(),
            zoom: 1.0,
        }
    }
}

/// Zoom limits and step (10 %).
pub const ZOOM_MIN: f32 = 0.8;
pub const ZOOM_MAX: f32 = 1.5;

/// Keep a zoom factor in range, on a 10 % step.
pub fn clamp_zoom(z: f32) -> f32 {
    if !z.is_finite() {
        return 1.0;
    }
    ((z * 10.0).round() / 10.0).clamp(ZOOM_MIN, ZOOM_MAX)
}

impl Config {
    pub fn path() -> PathBuf {
        crate::paths::config_dir().join("config.json")
    }

    /// Load config; on a missing or corrupt file, return defaults (and persist them so the
    /// client id stays stable).
    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Config>(&bytes) {
                Ok(mut c) => {
                    c.sanitize();
                    return c;
                }
                Err(e) => log::warn!("config at {} is invalid ({e}); using defaults", path.display()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("cannot read config {}: {e}", path.display()),
        }
        let c = Config::default();
        if let Err(e) = c.save() {
            log::warn!("cannot write config: {e}");
        }
        c
    }

    fn sanitize(&mut self) {
        if self.client_id.trim().is_empty() {
            self.client_id = uuid::Uuid::new_v4().to_string();
        }
        self.volume = self.volume.clamp(0.0, 1.0);
        self.crossfade_secs = self.crossfade_secs.clamp(0.0, 12.0);
        self.image_cache_mb = self.image_cache_mb.clamp(10, 10_000);
        self.audio_cache_mb = self.audio_cache_mb.clamp(50, 50_000);
        crate::audio::eq::sanitize(&mut self.eq_gains);
        self.zoom = clamp_zoom(self.zoom);
    }

    /// Atomically write the config (write temp file, then rename).
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_fills_defaults_and_sanitizes() {
        let mut c: Config = serde_json::from_str(r#"{"volume": 3.0, "crossfade_secs": 40}"#).unwrap();
        c.sanitize();
        assert_eq!(c.volume, 1.0);
        assert_eq!(c.crossfade_secs, 12.0);
        assert!(c.normalization);
        assert!(!c.client_id.is_empty());
    }

    #[test]
    fn repeat_cycles() {
        assert_eq!(RepeatMode::Off.cycle(), RepeatMode::All);
        assert_eq!(RepeatMode::All.cycle(), RepeatMode::One);
        assert_eq!(RepeatMode::One.cycle(), RepeatMode::Off);
    }
}
