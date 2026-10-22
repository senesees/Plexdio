//! Filesystem locations. Config lives in the roaming profile (%APPDATA%\Plexdio),
//! caches and logs in the local profile (%LOCALAPPDATA%\Plexdio).
//! `PLEXDIO_HOME` overrides both (used by tests and side-by-side runs).

use std::path::PathBuf;

const APP_DIR: &str = "Plexdio";
/// Folder name used before the app was renamed from Plexify.
const LEGACY_APP_DIR: &str = "Plexify";

/// Move the config and cache folders of a pre-rename install (`Plexify`) to the new name, so
/// settings, the library cache and the saved session survive the rename. Call before anything
/// opens files under these folders (including the logger). Skipped with `PLEXDIO_HOME`.
pub fn migrate_legacy_dirs() {
    if home_override().is_some() {
        return;
    }
    for base in [dirs::config_dir(), dirs::cache_dir()].into_iter().flatten() {
        let (old, new) = (base.join(LEGACY_APP_DIR), base.join(APP_DIR));
        if old.is_dir() && !new.exists() {
            // The logger is not up yet; a failure just means starting fresh.
            let _ = std::fs::rename(&old, &new);
        }
    }
}

fn home_override() -> Option<PathBuf> {
    std::env::var_os("PLEXDIO_HOME").filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn config_dir() -> PathBuf {
    home_override()
        .map(|h| h.join("config"))
        .or_else(|| dirs::config_dir().map(|d| d.join(APP_DIR)))
        .unwrap_or_else(|| PathBuf::from(".plexdio/config"))
}

pub fn cache_dir() -> PathBuf {
    home_override()
        .map(|h| h.join("cache"))
        .or_else(|| dirs::cache_dir().map(|d| d.join(APP_DIR)))
        .unwrap_or_else(|| PathBuf::from(".plexdio/cache"))
}

pub fn log_dir() -> PathBuf {
    cache_dir().join("logs")
}

pub fn image_cache_dir() -> PathBuf {
    cache_dir().join("images")
}

pub fn audio_cache_dir() -> PathBuf {
    cache_dir().join("audio")
}

pub fn db_dir() -> PathBuf {
    cache_dir().join("db")
}

/// Total size in bytes of all regular files below `dir` (0 if missing).
pub fn dir_size(dir: &std::path::Path) -> u64 {
    let mut total = 0;
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    for entry in rd.flatten() {
        match entry.metadata() {
            Ok(m) if m.is_dir() => total += dir_size(&entry.path()),
            Ok(m) => total += m.len(),
            Err(_) => {}
        }
    }
    total
}
