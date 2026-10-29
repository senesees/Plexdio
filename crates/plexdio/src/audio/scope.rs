//! Sample tap for the visualiser: the engine copies every block it hands to the output into a
//! ring of mono samples, and the UI reads the part that is being heard right now (the output
//! holds a buffer of not-yet-played audio, which is skipped).
//!
//! The engine thread (not the real-time output callback) writes it, so a mutex held for a
//! memcpy is fine.

use std::sync::{Arc, Mutex};

/// Mono samples kept (about 0.17 s at 48 kHz, enough for a 4096-point window plus the output
/// buffer lag).
pub const SCOPE_LEN: usize = 16_384;

pub struct Scope {
    ring: Vec<f32>,
    /// Mono frames written since the start.
    written: u64,
    /// Frames still waiting in the output buffer (pushed but not heard yet).
    pending: usize,
    pub rate: u32,
}

pub type SharedScope = Arc<Mutex<Scope>>;

pub fn shared(rate: u32) -> SharedScope {
    Arc::new(Mutex::new(Scope { ring: vec![0.0; SCOPE_LEN], written: 0, pending: 0, rate }))
}

impl Scope {
    /// Append interleaved stereo samples as mono.
    pub fn push_stereo(&mut self, block: &[f32]) {
        for f in block.as_chunks::<2>().0 {
            let i = (self.written % SCOPE_LEN as u64) as usize;
            self.ring[i] = (f[0] + f[1]) * 0.5;
            self.written += 1;
        }
    }

    pub fn set_pending(&mut self, frames: usize) {
        self.pending = frames.min(SCOPE_LEN / 2);
    }

    /// Copy the `n` samples that end at what is audible now into `out` (oldest first). Returns
    /// false while not enough audio has passed through yet.
    pub fn audible(&self, n: usize, out: &mut Vec<f32>) -> bool {
        let n = n.min(SCOPE_LEN / 2);
        let end = self.written.saturating_sub(self.pending as u64);
        if end < n as u64 {
            return false;
        }
        out.clear();
        out.extend((end - n as u64..end).map(|k| self.ring[(k % SCOPE_LEN as u64) as usize]));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_is_audible_not_what_is_buffered() {
        let s = shared(48_000);
        let mut sc = s.lock().unwrap();
        // Frames 0..100 with value = index, as stereo.
        let block: Vec<f32> = (0..100).flat_map(|i| [i as f32, i as f32]).collect();
        sc.push_stereo(&block);
        sc.set_pending(30);
        let mut out = Vec::new();
        assert!(sc.audible(4, &mut out));
        assert_eq!(out, vec![66.0, 67.0, 68.0, 69.0], "ends before the 30 buffered frames");
        assert!(!sc.audible(80, &mut out), "not enough audible history yet");
    }

    #[test]
    fn wraps_around() {
        let s = shared(48_000);
        let mut sc = s.lock().unwrap();
        let block: Vec<f32> = (0..SCOPE_LEN + 10).flat_map(|i| [i as f32, i as f32]).collect();
        sc.push_stereo(&block);
        let mut out = Vec::new();
        assert!(sc.audible(3, &mut out));
        let last = (SCOPE_LEN + 9) as f32;
        assert_eq!(out, vec![last - 2.0, last - 1.0, last]);
    }
}
