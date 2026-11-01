//! What the presets react to, worked out once per frame from the audio being heard: spectrum
//! bands, bass/mid/treble, a beat signal and a short waveform. Levels are normalised against a
//! slowly falling peak, so quiet and loud recordings both move the picture.

use crate::ui::visualiser::{spectrum, BANDS, N};

/// Waveform samples handed to presets.
pub const WAVE: usize = 512;

#[derive(Clone)]
pub struct Analysis {
    /// Spectrum bands 0..=1, low to high.
    pub bands: [f32; BANDS],
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    /// Overall loudness 0..=1.
    pub level: f32,
    /// A beat started this frame.
    pub beat: bool,
    /// 1 at a beat, falling towards 0.
    pub pulse: f32,
    /// Waveform, -1..=1 (gain-normalised).
    pub wave: Vec<f32>,
    /// No audio (paused or stopped).
    pub silent: bool,
    /// Seconds since the visualiser started.
    pub time: f32,
}

impl Default for Analysis {
    fn default() -> Self {
        Self { bands: [0.0; BANDS], bass: 0.0, mid: 0.0, treble: 0.0, level: 0.0, beat: false, pulse: 0.0, wave: vec![0.0; WAVE], silent: true, time: 0.0 }
    }
}

#[derive(Default)]
pub struct Analyzer {
    a: Analysis,
    bass_avg: f32,
    peak: f32,
    wave_peak: f32,
    cooldown: f32,
    re: Vec<f32>,
    im: Vec<f32>,
}

fn mean(s: &[f32]) -> f32 {
    s.iter().sum::<f32>() / s.len().max(1) as f32
}

impl Analyzer {
    /// Update from the latest `N` samples (oldest first) at `rate`, or silence.
    pub fn update(&mut self, samples: Option<(&[f32], u32)>, dt: f32) -> &Analysis {
        let dt = dt.clamp(0.0, 0.25);
        self.a.time += dt;
        self.cooldown -= dt;
        self.a.beat = false;
        self.a.pulse = (self.a.pulse - dt * 3.0).max(0.0);
        let Some((s, rate)) = samples.filter(|(s, _)| s.len() >= N) else {
            // Everything settles towards rest.
            let k = (1.0 - dt * 3.0).max(0.0);
            self.a.bands.iter_mut().for_each(|b| *b *= k);
            self.a.wave.iter_mut().for_each(|w| *w *= k);
            self.a.bass *= k;
            self.a.mid *= k;
            self.a.treble *= k;
            self.a.level *= k;
            self.a.silent = true;
            return &self.a;
        };
        let s = &s[s.len() - N..];
        let raw = spectrum(s, rate, &mut self.re, &mut self.im);
        let loud = mean(&raw);
        // Auto-gain: a peak that jumps up and drifts down over ~10 s.
        self.peak = (self.peak - dt * 0.05).max(loud).max(0.15);
        let gain = 0.75 / self.peak;
        for (b, r) in self.a.bands.iter_mut().zip(raw) {
            let t = (r * gain).min(1.0);
            *b = if t > *b { *b + (t - *b) * 0.7 } else { (*b - dt * 2.5).max(t) };
        }
        self.a.bass = mean(&self.a.bands[0..6]);
        self.a.mid = mean(&self.a.bands[6..18]);
        self.a.treble = mean(&self.a.bands[18..]);
        self.a.level = mean(&self.a.bands);
        // Beat: bass jumping clearly above its recent average.
        let bass_raw = mean(&raw[0..6]) * gain;
        self.bass_avg += (bass_raw - self.bass_avg) * (dt * 2.0).min(1.0);
        if bass_raw > self.bass_avg + 0.08 && bass_raw > 0.25 && self.cooldown <= 0.0 {
            self.a.beat = true;
            self.a.pulse = 1.0;
            self.cooldown = 0.28;
        }
        // Waveform: the newest WAVE samples, normalised.
        let tail = &s[s.len() - WAVE..];
        let pk = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        self.wave_peak = (self.wave_peak - dt * 0.2).max(pk).max(0.05);
        for (w, v) in self.a.wave.iter_mut().zip(tail) {
            *w = v / self.wave_peak;
        }
        self.a.silent = false;
        &self.a
    }

    pub fn last(&self) -> &Analysis {
        &self.a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 120 bpm kick drum over a quiet tone, at 48 kHz.
    fn kicks(seconds: f32) -> Vec<f32> {
        let rate = 48_000.0;
        (0..(seconds * rate) as usize)
            .map(|i| {
                let t = i as f32 / rate;
                let beat_t = t % 0.5;
                let kick = (std::f32::consts::TAU * 55.0 * t).sin() * (-beat_t * 18.0).exp();
                kick * 0.9 + 0.05 * (std::f32::consts::TAU * 880.0 * t).sin()
            })
            .collect()
    }

    #[test]
    fn beats_are_found_at_the_tempo() {
        let audio = kicks(6.0);
        let mut an = Analyzer::default();
        let dt = 1.0 / 30.0;
        let hop = (48_000.0 * dt) as usize;
        let mut beats = 0;
        let mut i = N;
        while i < audio.len() {
            if an.update(Some((&audio[i - N..i], 48_000)), dt).beat {
                beats += 1;
            }
            i += hop;
        }
        // 12 kicks in 6 s; allow the first second for the averages to settle.
        assert!((8..=13).contains(&beats), "{beats} beats");
        assert!(an.last().bass > an.last().treble);
    }

    #[test]
    fn silence_settles_to_rest() {
        let audio = kicks(1.0);
        let mut an = Analyzer::default();
        an.update(Some((&audio[..N], 48_000)), 0.03);
        for _ in 0..60 {
            an.update(None, 0.03);
        }
        assert!(an.last().silent && an.last().level < 0.01);
    }
}
