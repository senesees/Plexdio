//! 10-band graphic equaliser: one peaking biquad per band (RBJ Audio EQ Cookbook), run on the
//! interleaved stereo mix just before it goes to the output device.
//!
//! With every band at 0 dB the equaliser is bypassed entirely, so "flat" is bit-exact.

use super::gain::{db_to_linear, limit};

/// Band centre frequencies in Hz (ISO octave bands).
pub const BANDS: [f32; 10] = [31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0];
pub const BAND_COUNT: usize = BANDS.len();
/// Gain range of each band in dB.
pub const MAX_DB: f32 = 12.0;
/// Bandwidth of each peaking filter (one octave, matching the band spacing).
const Q: f32 = 1.41;

/// Built-in presets: (name, gain per band in dB).
pub const PRESETS: &[(&str, [f32; BAND_COUNT])] = &[
    ("Flat", [0.0; BAND_COUNT]),
    ("Bass boost", [6.0, 5.0, 4.0, 2.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Bass reducer", [-6.0, -5.0, -4.0, -2.5, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Treble boost", [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.5, 4.0, 5.0, 6.0]),
    ("Vocal", [-2.0, -2.0, -1.0, 1.0, 3.0, 3.5, 3.0, 1.5, 0.0, -1.0]),
    ("Rock", [4.5, 3.5, 2.0, 0.5, -1.0, -0.5, 1.5, 3.0, 3.5, 4.0]),
    ("Pop", [-1.0, 0.0, 1.5, 3.0, 3.5, 2.5, 1.0, 0.0, -0.5, -1.0]),
    ("Jazz", [3.0, 2.0, 1.0, 2.0, -1.5, -1.5, 0.0, 1.5, 2.5, 3.0]),
    ("Classical", [4.0, 3.0, 2.5, 1.5, -1.0, -1.0, 0.0, 2.0, 3.0, 3.5]),
    ("Electronic", [4.5, 4.0, 1.5, 0.0, -1.5, 1.5, 0.5, 1.5, 4.0, 4.5]),
    ("Loudness", [5.0, 3.5, 0.0, 0.0, -1.5, 0.0, -0.5, -4.0, 4.0, 1.0]),
];

/// Name of the preset matching `gains`, if any.
pub fn preset_name(gains: &[f32; BAND_COUNT]) -> Option<&'static str> {
    PRESETS
        .iter()
        .find(|(_, p)| p.iter().zip(gains).all(|(a, b)| (a - b).abs() < 0.05))
        .map(|(name, _)| *name)
}

/// Combined response of all bands in dB at `freq` (for drawing the curve; no pre-cut).
pub fn response_db(gains: &[f32; BAND_COUNT], freq: f32) -> f32 {
    const RATE: f32 = 48_000.0;
    let w = 2.0 * std::f32::consts::PI * freq / RATE;
    // e^{-jw} and e^{-2jw}
    let (c1, s1) = (w.cos(), -w.sin());
    let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
    BANDS
        .iter()
        .zip(gains)
        .filter(|(_, g)| g.abs() >= 0.01)
        .map(|(&f, &g)| {
            let c = Coeffs::peaking(f, g, RATE);
            let (nr, ni) = (c.b0 + c.b1 * c1 + c.b2 * c2, c.b1 * s1 + c.b2 * s2);
            let (dr, di) = (1.0 + c.a1 * c1 + c.a2 * c2, c.a1 * s1 + c.a2 * s2);
            10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()
        })
        .sum()
}

/// Clamp gains to the supported range (and NaN to 0).
pub fn sanitize(gains: &mut [f32; BAND_COUNT]) {
    for g in gains.iter_mut() {
        *g = if g.is_finite() { g.clamp(-MAX_DB, MAX_DB) } else { 0.0 };
    }
}

#[derive(Clone, Copy, Default)]
struct Coeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coeffs {
    /// Peaking EQ: boost or cut `db` around `freq`.
    fn peaking(freq: f32, db: f32, rate: f32) -> Self {
        let a = 10f32.powf(db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * Q);
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }
}

/// Transposed direct form II state for one channel of one band.
#[derive(Clone, Copy, Default)]
struct State {
    z1: f32,
    z2: f32,
}

impl State {
    #[inline]
    fn run(&mut self, c: &Coeffs, x: f32) -> f32 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }
}

pub struct Equalizer {
    rate: f32,
    gains: [f32; BAND_COUNT],
    enabled: bool,
    /// Active filters: (band index, coefficients, left state, right state). Empty when bypassed.
    filters: Vec<(usize, Coeffs, State, State)>,
    /// Headroom so boosts do not clip straight away.
    preamp: f32,
}

impl Equalizer {
    pub fn new(rate: u32) -> Self {
        Self { rate: rate as f32, gains: [0.0; BAND_COUNT], enabled: false, filters: Vec::new(), preamp: 1.0 }
    }

    /// Change the settings. Filter state is kept for bands that stay active, so moving a slider
    /// while music plays does not click.
    pub fn set(&mut self, enabled: bool, gains: [f32; BAND_COUNT]) {
        let mut gains = gains;
        sanitize(&mut gains);
        self.enabled = enabled;
        self.gains = gains;
        let old: Vec<(usize, Coeffs, State, State)> = std::mem::take(&mut self.filters);
        if !enabled || gains.iter().all(|g| g.abs() < 0.01) {
            self.preamp = 1.0;
            return;
        }
        let nyquist_guard = self.rate * 0.45;
        for (i, (&freq, &db)) in BANDS.iter().zip(&gains).enumerate() {
            if db.abs() < 0.01 || freq >= nyquist_guard {
                continue;
            }
            // Reuse the previous state of this band when it was active before.
            let prev = old.iter().find(|f| f.0 == i).map(|f| (f.2, f.3)).unwrap_or_default();
            self.filters.push((i, Coeffs::peaking(freq, db, self.rate), prev.0, prev.1));
        }
        // Half of the biggest boost as pre-cut: keeps most of the effect, avoids most clipping.
        let max_boost = gains.iter().copied().fold(0.0f32, f32::max);
        self.preamp = db_to_linear(-max_boost * 0.5);
    }

    /// Rebuild the filters for a new output sample rate, keeping the settings.
    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate as f32;
        self.filters.clear();
        self.set(self.enabled, self.gains);
    }

    pub fn is_active(&self) -> bool {
        !self.filters.is_empty()
    }

    /// Process interleaved stereo in place.
    pub fn process(&mut self, samples: &mut [f32]) {
        if self.filters.is_empty() {
            return;
        }
        for frame in samples.as_chunks_mut::<2>().0 {
            let (mut l, mut r) = (frame[0] * self.preamp, frame[1] * self.preamp);
            for (_, c, sl, sr) in self.filters.iter_mut() {
                l = sl.run(c, l);
                r = sr.run(c, r);
            }
            frame[0] = limit(l);
            frame[1] = limit(r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let s = 0.1 * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin();
                [s, s]
            })
            .collect()
    }

    /// Peak of the second half (after the filter settled).
    fn peak(samples: &[f32]) -> f32 {
        samples[samples.len() / 2..].iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn flat_is_bypassed_and_bit_exact() {
        let mut eq = Equalizer::new(44_100);
        eq.set(true, [0.0; BAND_COUNT]);
        assert!(!eq.is_active());
        let input = sine(1_000.0, 44_100, 0.1);
        let mut out = input.clone();
        eq.process(&mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn a_band_boosts_its_frequency_and_leaves_others_alone() {
        let mut gains = [0.0; BAND_COUNT];
        gains[5] = 12.0; // 1 kHz
        let mut eq = Equalizer::new(44_100);
        eq.set(true, gains);
        let mut at_band = sine(1_000.0, 44_100, 0.5);
        eq.process(&mut at_band);
        let mut eq = Equalizer::new(44_100);
        eq.set(true, gains);
        let mut far = sine(60.0, 44_100, 0.5);
        eq.process(&mut far);
        // +12 dB with a -6 dB pre-cut: about +6 dB (x2) at the band, -6 dB (x0.5) far away.
        assert!((peak(&at_band) / 0.1 - 2.0).abs() < 0.15, "band gain {}", peak(&at_band) / 0.1);
        assert!((peak(&far) / 0.1 - 0.5).abs() < 0.05, "off-band gain {}", peak(&far) / 0.1);
    }

    #[test]
    fn drawn_response_matches_the_band_gain() {
        let mut gains = [0.0; BAND_COUNT];
        gains[3] = 6.0; // 250 Hz
        assert!((response_db(&gains, 250.0) - 6.0).abs() < 0.1);
        assert!(response_db(&gains, 8_000.0).abs() < 0.2);
        assert_eq!(response_db(&[0.0; BAND_COUNT], 1_000.0), 0.0);
    }

    #[test]
    fn disabled_is_bypassed_and_bad_values_are_clamped() {
        let mut eq = Equalizer::new(48_000);
        eq.set(false, [6.0; BAND_COUNT]);
        assert!(!eq.is_active());
        let mut g = [f32::NAN, 40.0, -40.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        sanitize(&mut g);
        assert_eq!(&g[..4], &[0.0, MAX_DB, -MAX_DB, 1.0]);
        assert_eq!(preset_name(&PRESETS[1].1), Some("Bass boost"));
        assert_eq!(preset_name(&[0.5; BAND_COUNT]), None);
    }
}
