//! Building a song's analysis in the background while it plays: open a second reader on the
//! song (sharing the player's download), decode, analyse. The music is never touched.

use super::analysis::{analyse, decode_mono, Analysis};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use symphonia::core::io::MediaSource;

/// Opens the song: its bytes and a file-extension hint for the decoder.
pub type Opener = Box<dyn FnOnce() -> Result<(Box<dyn MediaSource>, Option<String>), String> + Send>;

/// A build in progress. Dropping it cancels the work.
pub struct Build {
    pub track: i64,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
    result: Arc<Mutex<Option<Result<Analysis, String>>>>,
}

impl Build {
    /// Start analysing `track` on a worker thread; `wake` is called when it is done.
    pub fn start(track: i64, open: Opener, wake: impl Fn() + Send + 'static) -> Self {
        let b = Build { track, cancel: Arc::default(), progress: Arc::default(), result: Arc::default() };
        let (cancel, progress, result) = (b.cancel.clone(), b.progress.clone(), b.result.clone());
        let spawned = std::thread::Builder::new().name("ride-build".into()).spawn(move || {
            below_normal_priority();
            let started = std::time::Instant::now();
            let out = open().and_then(|(source, ext)| {
                let samples = decode_mono(source, ext.as_deref(), &cancel, Some(&progress)).map_err(|e| e.to_string())?;
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let decoded = started.elapsed();
                let a = analyse(&samples, None);
                log::info!(
                    "ride: track {track} analysed ({:.0} s of audio; decoded in {:.2} s, analysed in {:.2} s): {:.1} bpm, {} onsets, {} sections",
                    a.duration,
                    decoded.as_secs_f64(),
                    (started.elapsed() - decoded).as_secs_f64(),
                    a.bpm,
                    a.onsets.len(),
                    a.sections.len()
                );
                Ok(a)
            });
            if let Err(e) = &out {
                log::warn!("ride: cannot analyse track {track}: {e}");
            }
            if let Ok(mut r) = result.lock() {
                *r = Some(out);
            }
            wake();
        });
        if let Err(e) = spawned {
            if let Ok(mut r) = b.result.lock() {
                *r = Some(Err(format!("cannot start the analysis: {e}")));
            }
        }
        b
    }

    /// 0..=1 (decoding is nearly all of the time).
    pub fn progress(&self) -> f32 {
        self.progress.load(Ordering::Relaxed) as f32 / 1000.0
    }

    /// The result, once (None while still working).
    pub fn take(&self) -> Option<Result<Analysis, String>> {
        self.result.lock().ok().and_then(|mut r| r.take())
    }
}

impl Drop for Build {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Keep the analysis out of the way of the UI and audio threads.
fn below_normal_priority() {
    #[cfg(windows)]
    // SAFETY: changes only the calling thread's priority, through its pseudo-handle.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A tiny 16-bit mono WAV of a drum pattern.
    fn wav(samples: &[f32], rate: u32) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(&data);
        b
    }

    #[test]
    fn builds_in_the_background_and_reports_once() {
        let (song, _) = crate::game::analysis::tests::drums(120.0, 12.0, 0.0);
        let bytes = wav(&song, 22_050);
        let (tx, rx) = std::sync::mpsc::channel();
        let b = Build::start(7, Box::new(move || Ok((Box::new(Cursor::new(bytes)) as Box<dyn MediaSource>, Some("wav".into())))), move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(20)).expect("finished");
        let a = b.take().expect("result").expect("analysis");
        assert!((a.bpm - 120.0).abs() < 2.0 && (a.duration - 12.0).abs() < 0.1, "{} bpm, {} s", a.bpm, a.duration);
        assert!(b.take().is_none());
        assert!((b.progress() - 1.0).abs() < 0.01);
    }

    #[test]
    fn failures_are_reported() {
        let (tx, rx) = std::sync::mpsc::channel();
        let b = Build::start(1, Box::new(|| Err("offline".into())), move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(5)).expect("finished");
        assert_eq!(b.take(), Some(Err("offline".into())));
    }
}
