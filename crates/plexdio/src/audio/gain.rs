//! ReplayGain helpers. Plex reports track/album loudness as `gain` (dB) in the track metadata;
//! we apply it as a linear scale with a safety clamp so a bogus value cannot blow the mix up.

/// Linear multiplier for a ReplayGain value in dB.
pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Linear gain for a track, with the user's normalization preference applied.
pub fn track_gain(track_gain_db: Option<f32>, album_gain_db: Option<f32>, normalization: bool) -> f32 {
    if !normalization {
        return 1.0;
    }
    // Prefer the album gain so a whole album keeps its internal balance.
    let db = album_gain_db.or(track_gain_db).unwrap_or(0.0);
    if !db.is_finite() {
        return 1.0;
    }
    db_to_linear(db).clamp(0.05, 8.0)
}

/// Soft-limit one sample to [-1, 1]. ReplayGain in Plex is already conservative, so a hard
/// clamp only catches pathological tags.
#[inline]
pub fn limit(s: f32) -> f32 {
    s.clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_conversion_matches_reference_values() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(-6.0) - 0.501_187).abs() < 1e-4);
        assert!((db_to_linear(6.0) - 1.995_262).abs() < 1e-4);
    }

    #[test]
    fn album_gain_wins_and_normalization_off_is_unity() {
        assert!((track_gain(Some(-3.0), Some(-6.0), true) - db_to_linear(-6.0)).abs() < 1e-6);
        assert!((track_gain(Some(-3.0), None, true) - db_to_linear(-3.0)).abs() < 1e-6);
        assert_eq!(track_gain(Some(-3.0), Some(-6.0), false), 1.0);
        assert_eq!(track_gain(Some(400.0), None, true), 8.0);
        assert_eq!(track_gain(None, None, true), 1.0);
        // Track gain is the fallback when the server has no album value.
        assert_eq!(track_gain(None, Some(0.0), true), 1.0);
    }
}
