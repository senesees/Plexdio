//! The AI tier of the analysis: Beat This! (Foscarin, Schlüter & Widmer, ISMIR 2024), small
//! model, through the `beat-this` crate on the pure-Rust rten runtime. Its beats and downbeats
//! feed [`super::analysis::analyse`] as evidence; the steady grid still comes from our tracker.
//!
//! The models are embedded in the exe (about 11 MB, see assets/models/README.md) and unpacked
//! once into the cache folder, because the crate loads models from files.

use super::analysis::{AiBeats, RATE};
use beat_this::{BeatThis, RtenRuntime};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static MEL: &[u8] = include_bytes!("../../../../assets/models/mel_spectrogram.onnx");
static BEAT: &[u8] = include_bytes!("../../../../assets/models/beat_this_small.onnx");

/// Samples fed per step (10 s), so a cancel is noticed quickly.
const CHUNK: usize = RATE as usize * 10;

/// Write `bytes` to `dir/name` unless an identical-size copy is already there.
fn unpack(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let path = dir.join(name);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) {
        return Ok(path);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("model folder: {e}"))?;
    // Write under a temporary name first, so a half-written file is never loaded.
    let tmp = dir.join(format!("{name}.part"));
    std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, &path)).map_err(|e| format!("unpack {name}: {e}"))?;
    Ok(path)
}

/// The runtime's work goes to one thread of its own at below-normal priority. Its default is a
/// pool with a thread per core at normal priority, which made the UI stutter while a song was
/// being analysed (the model is effectively single-threaded anyway).
fn pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .thread_name(|_| "ride-ai".into())
            .start_handler(|_| super::build::below_normal_priority())
            .build()
            .map_err(|e| log::warn!("ride: no thread pool for the beat model ({e}); using the default"))
            .ok()
    })
    .as_ref()
}

/// Beats and downbeats of a whole song (mono at [`RATE`]) from the model. `models` is the folder
/// the embedded models are unpacked into.
pub fn beats(samples: &[f32], models: &Path, cancel: &AtomicBool) -> Result<AiBeats, String> {
    match pool() {
        Some(p) => p.install(|| run(samples, models, cancel)),
        None => run(samples, models, cancel),
    }
}

fn run(samples: &[f32], models: &Path, cancel: &AtomicBool) -> Result<AiBeats, String> {
    let mel = unpack(models, "mel_spectrogram.onnx", MEL)?;
    let beat = unpack(models, "beat_this_small.onnx", BEAT)?;
    let mut bt = BeatThis::new(&RtenRuntime, &mel, &beat).map_err(|e| format!("load the beat model: {e}"))?;
    let mut stream = bt.stream(RATE).map_err(|e| e.to_string())?;
    for chunk in samples.chunks(CHUNK) {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        stream.push(chunk).map_err(|e| e.to_string())?;
    }
    let a = stream.finish().map_err(|e| e.to_string())?;
    Ok(AiBeats { beats: a.beats, downbeats: a.downbeats })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::analysis::{analyse, tests::drums};

    #[test]
    fn the_model_finds_the_beat_and_steadies_the_grid() {
        let dir = std::env::temp_dir().join(format!("plexdio-models-{}", std::process::id()));
        let (song, kicks) = drums(120.0, 20.0, 0.0);
        let ai = beats(&song, &dir, &AtomicBool::new(false)).expect("model runs");
        assert!(ai.beats.len() > 20, "{} beats", ai.beats.len());
        // Model beats land on the kicks (or the hats between them, at double tempo).
        let near = ai.beats.iter().filter(|b| kicks.iter().any(|k| (*b - k).abs() < 0.05 || (*b - k - 0.25).abs() < 0.05)).count();
        assert!(near as f32 > 0.8 * ai.beats.len() as f32, "{near} of {} model beats on the pattern", ai.beats.len());
        let a = analyse(&song, Some(&ai));
        assert!(a.ai && (a.bpm - 120.0).abs() < 2.0 || (a.bpm - 240.0).abs() < 4.0, "{} bpm", a.bpm);
        // Unpacked once: a second run reuses the files.
        let modified = std::fs::metadata(dir.join("beat_this_small.onnx")).and_then(|m| m.modified()).unwrap();
        beats(&song[..RATE as usize * 3], &dir, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::metadata(dir.join("beat_this_small.onnx")).and_then(|m| m.modified()).unwrap(), modified);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancel_stops_it() {
        let dir = std::env::temp_dir().join(format!("plexdio-models-c-{}", std::process::id()));
        let (song, _) = drums(120.0, 30.0, 0.0);
        assert_eq!(beats(&song, &dir, &AtomicBool::new(true)), Err("cancelled".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
