//! Small spectrum / waveform visualiser drawn behind the now-playing controls and along the
//! bottom of the mini-player. It reads the samples being heard from the engine's
//! [`crate::audio::scope`] tap and runs a hand-written FFT (no extra crate).

use crate::audio::scope::SharedScope;
use egui::{Color32, Painter, Pos2, Rect, Stroke};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VisMode {
    #[default]
    Off,
    /// Spectrum bars.
    Bars,
    /// Waveform line.
    Wave,
}

impl VisMode {
    pub fn label(self) -> &'static str {
        match self {
            VisMode::Off => "Off",
            VisMode::Bars => "Bars",
            VisMode::Wave => "Wave",
        }
    }
}

/// Points per FFT (about 43 ms at 48 kHz).
const N: usize = 2048;
pub const BANDS: usize = 32;
const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 16_000.0;
/// Points drawn for the waveform.
const WAVE_POINTS: usize = 160;

#[derive(Default)]
pub struct Visualiser {
    /// Smoothed band levels 0..=1.
    levels: Vec<f32>,
    /// Smoothed waveform (WAVE_POINTS samples) and its gain (fades when paused).
    wave: Vec<f32>,
    wave_gain: f32,
    samples: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    last: f64,
}

/// In-place iterative radix-2 FFT; `re.len()` must be a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

/// Band levels (0..=1) of `samples` (N points) at `rate`.
fn spectrum(samples: &[f32], rate: u32, re: &mut Vec<f32>, im: &mut Vec<f32>) -> [f32; BANDS] {
    re.clear();
    im.clear();
    // Hann window.
    re.extend(samples.iter().enumerate().map(|(i, s)| s * (0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos())));
    im.resize(N, 0.0);
    fft(re, im);
    let bin_hz = rate as f32 / N as f32;
    let ratio = HIGH_HZ / LOW_HZ;
    let mut out = [0.0; BANDS];
    for (b, level) in out.iter_mut().enumerate() {
        let f0 = LOW_HZ * ratio.powf(b as f32 / BANDS as f32);
        let f1 = LOW_HZ * ratio.powf((b + 1) as f32 / BANDS as f32);
        let k0 = ((f0 / bin_hz) as usize).max(1);
        let k1 = ((f1 / bin_hz) as usize).max(k0 + 1).min(N / 2);
        let peak = (k0..k1).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).fold(0.0f32, f32::max);
        // Amplitude of a full-scale sine is N/4 after the Hann window.
        let amp = peak / (N as f32 / 4.0);
        // Music has less energy up high; tilt +3 dB per octave above 1 kHz so bars look even.
        let tilt = 3.0 * ((f0 + f1) / 2000.0).log2();
        let db = 20.0 * (amp + 1e-9).log10() + tilt;
        *level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    }
    out
}

impl Visualiser {
    /// Read the audible samples and move the bars towards them; with `playing` false everything
    /// falls back to rest. `now` is egui time in seconds.
    pub fn update(&mut self, scope: Option<&SharedScope>, playing: bool, now: f64) {
        let dt = ((now - self.last) as f32).clamp(0.0, 0.2);
        self.last = now;
        if self.levels.len() != BANDS {
            self.levels = vec![0.0; BANDS];
            self.wave = vec![0.0; WAVE_POINTS];
        }
        let mut target = [0.0f32; BANDS];
        let mut have = false;
        if let (true, Some(scope)) = (playing, scope) {
            if let Ok(sc) = scope.lock() {
                have = sc.audible(N, &mut self.samples);
                if have {
                    target = spectrum(&self.samples, sc.rate, &mut self.re, &mut self.im);
                }
            }
        }
        for (l, t) in self.levels.iter_mut().zip(target) {
            *l = if t > *l { *l + (t - *l) * 0.6 } else { (*l - dt * 1.6).max(t) };
        }
        if have {
            // The last ~20 ms as the waveform.
            let span = (self.samples.len() / 4).max(WAVE_POINTS);
            let tail = &self.samples[self.samples.len() - span..];
            for (i, w) in self.wave.iter_mut().enumerate() {
                let s = tail[i * span / WAVE_POINTS];
                *w += (s - *w) * 0.7;
            }
            self.wave_gain = (self.wave_gain + dt * 4.0).min(1.0);
        } else {
            self.wave_gain = (self.wave_gain - dt * 2.0).max(0.0);
        }
    }

    /// Anything left to animate (bars still falling)?
    pub fn moving(&self) -> bool {
        self.levels.iter().any(|l| *l > 0.002) || self.wave_gain > 0.0
    }

    pub fn paint(&self, painter: &Painter, rect: Rect, mode: VisMode, color: Color32) {
        match mode {
            VisMode::Off => {}
            VisMode::Bars => self.paint_bars(painter, rect, color),
            VisMode::Wave => self.paint_wave(painter, rect, color),
        }
    }

    fn paint_bars(&self, painter: &Painter, rect: Rect, color: Color32) {
        if self.levels.is_empty() {
            return;
        }
        let n = self.levels.len();
        let slot = rect.width() / n as f32;
        let w = (slot * 0.62).max(1.0);
        for (i, l) in self.levels.iter().enumerate() {
            // Silent bands draw nothing (no row of dots along the edge).
            if *l < 0.02 {
                continue;
            }
            let h = l * rect.height();
            let x = rect.left() + i as f32 * slot + (slot - w) / 2.0;
            let r = Rect::from_min_max(egui::pos2(x, rect.bottom() - h), egui::pos2(x + w, rect.bottom()));
            painter.rect_filled(r, (w / 2.0).min(2.0), color);
        }
    }

    fn paint_wave(&self, painter: &Painter, rect: Rect, color: Color32) {
        if self.wave.is_empty() {
            return;
        }
        let mid = rect.center().y;
        let amp = rect.height() / 2.0 * self.wave_gain;
        let pts: Vec<Pos2> = self
            .wave
            .iter()
            .enumerate()
            .map(|(i, s)| egui::pos2(rect.left() + rect.width() * i as f32 / (self.wave.len() - 1) as f32, mid - (s * 1.6).clamp(-1.0, 1.0) * amp))
            .collect();
        painter.add(egui::Shape::line(pts, Stroke::new(1.5, color)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: u32, amp: f32) -> Vec<f32> {
        (0..N).map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin()).collect()
    }

    #[test]
    fn fft_finds_a_sine() {
        let (mut re, mut im) = (sine(1000.0, 48_000, 1.0), vec![0.0; N]);
        fft(&mut re, &mut im);
        let mags: Vec<f32> = (0..N / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).collect();
        let peak = mags.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(peak, (1000.0 * N as f32 / 48_000.0).round() as usize);
    }

    #[test]
    fn a_tone_lights_its_band_and_silence_none() {
        let (mut re, mut im) = (Vec::new(), Vec::new());
        let bands = spectrum(&sine(1000.0, 48_000, 0.5), 48_000, &mut re, &mut im);
        let loudest = bands.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        let f0 = LOW_HZ * (HIGH_HZ / LOW_HZ).powf(loudest as f32 / BANDS as f32);
        let f1 = LOW_HZ * (HIGH_HZ / LOW_HZ).powf((loudest + 1) as f32 / BANDS as f32);
        assert!(f0 <= 1050.0 && f1 >= 950.0, "1 kHz lands in band {loudest} ({f0}-{f1} Hz)");
        assert!(bands[loudest] > 0.8);
        let quiet = spectrum(&vec![0.0; N], 48_000, &mut re, &mut im);
        assert!(quiet.iter().all(|l| *l == 0.0));
    }

    #[test]
    fn bars_fall_when_paused() {
        let mut v = Visualiser::default();
        v.update(None, false, 0.0);
        v.levels.iter_mut().for_each(|l| *l = 1.0);
        v.update(None, false, 0.1);
        assert!(v.levels.iter().all(|l| *l < 1.0 && *l > 0.0));
        for i in 2..20 {
            v.update(None, false, i as f64 * 0.1);
        }
        assert!(!v.moving());
    }
}
