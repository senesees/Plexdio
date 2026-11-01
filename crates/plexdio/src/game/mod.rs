//! Ride: an Audiosurf-style game on the song that is playing (see docs/audio-game-plan.md).
//! [`analysis`] listens to the whole song once; [`mapgen`] turns that into a road and notes;
//! [`play`] runs a ride against the [`clock`] of the song being heard; [`render`] draws it.

pub mod analysis;
pub mod build;
pub mod clock;
pub mod mapgen;
pub mod play;
pub mod render;

use serde::{Deserialize, Serialize};

/// Ride settings (saved in the config).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RidePrefs {
    pub difficulty: mapgen::Difficulty,
}
