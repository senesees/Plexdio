//! The saved listening session: queue track ids, current slot and position. Written next to the
//! config so a restart can restore the queue paused where the user left it.

use crate::config::RepeatMode;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// Queue in its logical order, as track ids (library rating keys).
    pub queue: Vec<i64>,
    /// Slot within `queue` that was playing.
    pub index: usize,
    pub position_ms: u64,
    pub shuffle: bool,
    pub repeat: RepeatMode,
}

pub fn path() -> PathBuf {
    crate::paths::config_dir().join("session.json")
}

impl Session {
    /// Load the session; a missing or corrupt file just means "start fresh".
    pub fn load() -> Option<Self> {
        let path = path();
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice::<Session>(&bytes) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("ignoring invalid session file {}: {e}", path.display());
                None
            }
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, &path)
    }

    /// Remove the saved session (sign-out, cache clear).
    pub fn clear() {
        let _ = std::fs::remove_file(path());
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let s = Session { queue: vec![1, 2, 3], index: 1, position_ms: 42_000, shuffle: true, repeat: RepeatMode::One };
        let json = serde_json::to_vec(&s).unwrap();
        let back: Session = serde_json::from_slice(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn old_or_partial_files_get_defaults() {
        let s: Session = serde_json::from_str(r#"{"queue":[9],"index":0}"#).unwrap();
        assert_eq!(s.queue, vec![9]);
        assert_eq!(s.repeat, RepeatMode::Off);
        assert_eq!(s.position_ms, 0);
    }
}
