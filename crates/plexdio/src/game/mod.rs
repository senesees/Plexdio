//! Ride: an Audiosurf-style game on the song that is playing (see docs/audio-game-plan.md).
//! [`analysis`] listens to the whole song once; [`mapgen`] turns that into a road and notes;
//! [`play`] runs a ride against the [`clock`] of the song being heard; [`render`] draws it.

pub mod ai;
pub mod analysis;
pub mod build;
pub mod clock;
pub mod mapgen;
pub mod play;
pub mod render;

use serde::{Deserialize, Serialize};

/// Ride settings (saved in the config).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RidePrefs {
    pub difficulty: mapgen::Difficulty,
    pub craft: render::Craft,
    /// Game frames per second (30 or 60).
    pub fps: u32,
    /// How much later than reported the music is heard, in milliseconds (wireless headphones).
    pub delay_ms: i32,
    /// Refine each song's beat grid with the Beat This! model.
    pub ai: bool,
}

impl Default for RidePrefs {
    fn default() -> Self {
        Self { difficulty: Default::default(), craft: Default::default(), fps: 60, delay_ms: 0, ai: true }
    }
}
