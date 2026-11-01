//! Song analysis for Ride: onsets in three bands, a steady beat grid, downbeats, a loudness
//! "intensity" curve and sections, from the whole song decoded to 22 050 Hz mono.
//!
//! Everything runs on 50 frames a second (a 441-sample hop), the same rate as the Beat This!
//! model, so its beats can be mixed in as extra evidence ([`AiBeats`]). The beat grid always
//! comes from our own dynamic-programming tracker (Ellis 2007): raw model beats flip between
//! tempo levels on busy songs, and a game needs a steady grid.

use crate::audio::decode::{DecodeError, TrackDecoder};
use crate::ui::visualiser::fft;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use symphonia::core::io::MediaSource;

/// Sample rate the analysis works at.
pub const RATE: u32 = 22_050;
/// Analysis frames per second.
pub const FPS: f32 = 50.0;
const HOP: usize = 441;
const WIN: usize = 1024;
/// Samples per second of the stored intensity curve.
pub const INTENSITY_HZ: f32 = 10.0;
/// Bump when the analysis changes, so cached results are rebuilt.
pub const VERSION: u32 = 1;
/// Frames are centred on their time, so a hit already shows in the frame before it: onsets and
/// beats are moved this much later (measured against real songs).
const LAG: f32 = 0.02;

/// Frequency bands, as FFT bin ranges at 22 050 Hz / 1024 (21.5 Hz per bin): bass (43–194 Hz),
/// mids (215–2000 Hz, snare and voice) and highs (2–11 kHz, hats and cymbals).
const BANDS: [(usize, usize); 3] = [(2, 10), (10, 94), (94, 512)];

/// A detected note start.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Onset {
    /// Seconds from the start of the song.
    pub t: f32,
    /// 0 bass, 1 mids, 2 highs.
    pub band: u8,
    /// 0..=1, relative to the song's own loud onsets.
    pub strength: f32,
}

/// A stretch of the song with a similar sound (intro, verse, chorus, drop…).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub start: f32,
    pub end: f32,
    /// Mean intensity, 0..=1.
    pub intensity: f32,
}

/// Beats from the Beat This! model, used as evidence for the beat grid.
#[derive(Debug, Clone, Default)]
pub struct AiBeats {
    pub beats: Vec<f32>,
    pub downbeats: Vec<f32>,
}

/// Everything the map generator needs from a song (small: no audio is kept).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    pub version: u32,
    /// Seconds.
    pub duration: f32,
    /// Tempo of the beat grid.
    pub bpm: f32,
    /// The steady beat grid, in seconds.
    pub beats: Vec<f32>,
    /// The beats that start a bar (a subset of `beats`).
    pub downbeats: Vec<f32>,
    /// All onsets, sorted by time.
    pub onsets: Vec<Onset>,
    /// Loudness and activity, 0..=1, at [`INTENSITY_HZ`].
    pub intensity: Vec<f32>,
    pub sections: Vec<Section>,
    /// Model beats were used.
    pub ai: bool,
}

impl Analysis {
    /// Intensity at `t` seconds (linear between samples).
    pub fn intensity_at(&self, t: f32) -> f32 {
        sample_curve(&self.intensity, t * INTENSITY_HZ)
    }

    /// Index of the section containing `t` (the last one past the end).
    pub fn section_at(&self, t: f32) -> usize {
        self.sections.iter().position(|s| t < s.end).unwrap_or(self.sections.len().saturating_sub(1))
    }
}

/// Linear interpolation into `curve` at fractional index `x` (clamped).
pub(crate) fn sample_curve(curve: &[f32], x: f32) -> f32 {
    if curve.is_empty() {
        return 0.0;
    }
    let x = x.clamp(0.0, (curve.len() - 1) as f32);
    let i = x as usize;
    let f = x - i as f32;
    let a = curve[i];
    let b = curve[(i + 1).min(curve.len() - 1)];
    a + (b - a) * f
}

/// Decode a whole song to mono at [`RATE`]. `cancel` stops early (returns what was decoded);
/// `progress` receives 0..=1000 (per mille of the song) when the length is known.
pub fn decode_mono(source: Box<dyn MediaSource>, ext_hint: Option<&str>, cancel: &AtomicBool, progress: Option<&AtomicU32>) -> Result<Vec<f32>, DecodeError> {
    let mut dec = TrackDecoder::open(source, ext_hint, RATE)?;
    let total = dec.total_frames().unwrap_or(0) as usize;
    let mut out = Vec::with_capacity(total);
    while !cancel.load(Ordering::Relaxed) {
        let block = dec.next_block(16_384)?;
        if block.is_empty() {
            break;
        }
        out.extend(block.as_chunks::<2>().0.iter().map(|[l, r]| (l + r) * 0.5));
        if let (Some(p), true) = (progress, total > 0) {
            p.store((out.len() * 1000 / total).min(1000) as u32, Ordering::Relaxed);
        }
    }
    Ok(out)
}

/// Per-frame features of the whole song.
struct Features {
    /// Spectral flux per band (rectified increase of log magnitude).
    flux: [Vec<f32>; 3],
    /// Mean log magnitude per band.
    energy: [Vec<f32>; 3],
    /// Frame loudness in dB.
    rms_db: Vec<f32>,
}

fn features(samples: &[f32]) -> Features {
    let frames = samples.len() / HOP + 1;
    let window: Vec<f32> = (0..WIN).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / WIN as f32).cos()).collect();
    let (mut re, mut im) = (vec![0.0f32; WIN], vec![0.0f32; WIN]);
    let mut prev = vec![0.0f32; WIN / 2];
    let mut cur = vec![0.0f32; WIN / 2];
    let mut f = Features { flux: Default::default(), energy: Default::default(), rms_db: Vec::with_capacity(frames) };
    for b in 0..3 {
        f.flux[b] = Vec::with_capacity(frames);
        f.energy[b] = Vec::with_capacity(frames);
    }
    // Normalise so a full-scale sine is about 1 before the log.
    let scale = 4.0 / WIN as f32;
    for i in 0..frames {
        // Frames are centred on i * HOP.
        let start = (i * HOP) as isize - (WIN / 2) as isize;
        for k in 0..WIN {
            let j = start + k as isize;
            re[k] = if j >= 0 && (j as usize) < samples.len() { samples[j as usize] * window[k] } else { 0.0 };
            im[k] = 0.0;
        }
        fft(&mut re, &mut im);
        for k in 0..WIN / 2 {
            let mag = (re[k] * re[k] + im[k] * im[k]).sqrt() * scale;
            cur[k] = (1.0 + 100.0 * mag).ln();
        }
        for (b, &(lo, hi)) in BANDS.iter().enumerate() {
            let (mut flux, mut energy) = (0.0, 0.0);
            for k in lo..hi {
                flux += (cur[k] - prev[k]).max(0.0);
                energy += cur[k];
            }
            let n = (hi - lo) as f32;
            f.flux[b].push(if i == 0 { 0.0 } else { flux / n });
            f.energy[b].push(energy / n);
        }
        std::mem::swap(&mut prev, &mut cur);
        let s = (i * HOP).min(samples.len());
        let e = (s + HOP).min(samples.len());
        let ms = if e > s { samples[s..e].iter().map(|v| v * v).sum::<f32>() / (e - s) as f32 } else { 0.0 };
        f.rms_db.push(10.0 * (ms + 1e-10).log10());
    }
    f
}

/// Moving average over `±half` samples.
pub(crate) fn smooth(v: &[f32], half: usize) -> Vec<f32> {
    let mut prefix = Vec::with_capacity(v.len() + 1);
    prefix.push(0.0f64);
    for &x in v {
        prefix.push(prefix.last().unwrap() + x as f64);
    }
    (0..v.len())
        .map(|i| {
            let a = i.saturating_sub(half);
            let b = (i + half + 1).min(v.len());
            ((prefix[b] - prefix[a]) / (b - a) as f64) as f32
        })
        .collect()
}

/// The value below which `q` (0..=1) of `v` lies.
fn percentile(v: &[f32], q: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    s[((s.len() - 1) as f32 * q).round() as usize]
}

/// Peak picking on one band's flux: local maxima above an adaptive threshold.
fn pick_onsets(flux: &[f32], band: u8, out: &mut Vec<Onset>) {
    let norm = percentile(flux, 0.95).max(1e-6);
    let n: Vec<f32> = flux.iter().map(|v| v / norm).collect();
    let mean = smooth(&n, 25);
    let mut last: Option<usize> = None;
    for i in 1..n.len().saturating_sub(1) {
        let v = n[i];
        if v < 0.2 || v < mean[i] + 0.12 {
            continue;
        }
        let lo = i.saturating_sub(3);
        let hi = (i + 4).min(n.len());
        if n[lo..hi].iter().any(|&w| w > v) {
            continue;
        }
        if last.is_some_and(|l| i - l < 4) {
            continue;
        }
        last = Some(i);
        out.push(Onset { t: i as f32 / FPS + LAG, band, strength: (v / 2.0).min(1.0) });
    }
}

/// The onset envelope used for tempo and beats: summed flux, high-passed, scaled to unit spread.
fn onset_envelope(f: &Features) -> Vec<f32> {
    let n = f.flux[0].len();
    let norms: Vec<f32> = f.flux.iter().map(|b| percentile(b, 0.95).max(1e-6)).collect();
    // Bass counts most: beats are usually carried by the kick, hats often sit between them.
    let raw: Vec<f32> = (0..n).map(|i| 1.5 * f.flux[0][i] / norms[0] + f.flux[1][i] / norms[1] + 0.3 * f.flux[2][i] / norms[2]).collect();
    let mean = smooth(&raw, 25);
    let env: Vec<f32> = raw.iter().zip(&mean).map(|(r, m)| (r - m).max(0.0)).collect();
    let sd = (env.iter().map(|v| v * v).sum::<f32>() / n.max(1) as f32).sqrt().max(1e-6);
    env.iter().map(|v| v / sd).collect()
}

/// How much the tracker likes a beat period of `lag` frames: a log-normal preference around
/// 0.5 s (120 bpm), one octave wide.
fn tempo_prior(lag: f32) -> f32 {
    let octaves = (lag / (0.5 * FPS)).log2();
    (-0.5 * octaves * octaves).exp()
}

/// Beat period in frames: the autocorrelation peak of the envelope, weighted by the prior. With
/// model beats, their median spacing (or its half or double) competes as well.
fn beat_period(env: &[f32], ai: Option<&AiBeats>) -> f32 {
    let (lo, hi) = ((0.25 * FPS) as usize, (2.0 * FPS) as usize);
    // Onsets are one-frame spikes and a beat is rarely a whole number of frames: smoothing
    // (±2 frames) lets hits a frame apart still correlate.
    let env = &smooth(env, 2);
    let n = env.len();
    let ac = |lag: usize| -> f32 {
        if lag >= n {
            return 0.0;
        }
        env[..n - lag].iter().zip(&env[lag..]).map(|(a, b)| a * b).sum::<f32>() / (n - lag) as f32
    };
    let acs: Vec<f32> = (0..=3 * hi + 2).map(ac).collect();
    // A comb: the right period also lines up at two and three times itself; a wrong one (say
    // 1.5 beats, which matches kick against hat) doesn't.
    let score = |lag: f32| -> f32 {
        let i = lag as usize;
        if i < lo || i >= hi {
            return 0.0;
        }
        let at = |l: f32| if (l as usize) + 1 < acs.len() { sample_curve(&acs, l) } else { 0.0 };
        (at(lag) + 0.5 * at(lag * 2.0) + 0.33 * at(lag * 3.0)) * tempo_prior(lag)
    };
    let mut best = (lo..hi).max_by(|&a, &b| score(a as f32).total_cmp(&score(b as f32))).unwrap_or(25) as f32;
    // Parabolic refinement of the peak.
    let i = best as usize;
    if i > lo && i + 1 < hi {
        let (a, b, c) = (acs[i - 1], acs[i], acs[i + 1]);
        let d = a - 2.0 * b + c;
        if d < 0.0 {
            best += (0.5 * (a - c) / d).clamp(-0.5, 0.5);
        }
    }
    if let Some(ai) = ai.filter(|a| a.beats.len() > 8) {
        let mut ibi: Vec<f32> = ai.beats.windows(2).map(|w| (w[1] - w[0]) * FPS).collect();
        ibi.sort_by(f32::total_cmp);
        // The model's tempo (or its half or double if the envelope much prefers those) wins
        // unless the envelope clearly disagrees.
        let mut model = ibi[ibi.len() / 2];
        for cand in [model * 0.5, model * 2.0] {
            if score(cand) > 1.5 * score(model) {
                model = cand;
            }
        }
        if score(model) * 1.3 > score(best) {
            best = model;
        }
    }
    best.clamp(lo as f32, hi as f32)
}

/// Ellis' dynamic-programming beat tracker: beats on strong envelope frames, spaced close to
/// `period`. `tightness` punishes tempo drift.
fn track_beats(local: &[f32], period: f32, tightness: f32) -> Vec<usize> {
    let n = local.len();
    if n == 0 {
        return Vec::new();
    }
    let mut cum = vec![0.0f32; n];
    let mut back = vec![usize::MAX; n];
    let (min_back, max_back) = ((period * 0.5).round() as usize, (period * 2.0).round() as usize);
    for i in 0..n {
        let mut best = 0.0f32;
        let mut arg = usize::MAX;
        if i >= min_back {
            let first = i.saturating_sub(max_back);
            for (j, &c) in cum.iter().enumerate().take(i - min_back + 1).skip(first) {
                let r = ((i - j) as f32 / period).ln();
                let v = c - tightness * r * r;
                if arg == usize::MAX || v > best {
                    best = v;
                    arg = j;
                }
            }
        }
        if arg != usize::MAX && best > 0.0 {
            cum[i] = local[i] + best;
            back[i] = arg;
        } else {
            cum[i] = local[i];
        }
    }
    // End on the best-scoring frame within the last period.
    let tail = n.saturating_sub(period.round() as usize);
    let mut i = (tail..n).max_by(|&a, &b| cum[a].total_cmp(&cum[b])).unwrap_or(n - 1);
    let mut beats = vec![i];
    while back[i] != usize::MAX {
        i = back[i];
        beats.push(i);
    }
    beats.reverse();
    beats
}

/// Analyse a whole song (mono at [`RATE`]). `ai` adds model beats as evidence when available.
pub fn analyse(samples: &[f32], ai: Option<&AiBeats>) -> Analysis {
    let duration = samples.len() as f32 / RATE as f32;
    let f = features(samples);
    let n = f.rms_db.len();

    let mut onsets = Vec::new();
    for b in 0..3 {
        pick_onsets(&f.flux[b], b as u8, &mut onsets);
    }
    onsets.sort_by(|a, b| a.t.total_cmp(&b.t));

    // Beat grid.
    let env = onset_envelope(&f);
    let period = beat_period(&env, ai);
    let mut local = env.clone();
    if let Some(ai) = ai {
        // Gaussian bumps (σ = 2 frames) at the model's beats pull the grid onto them.
        for &b in &ai.beats {
            let c = (b - LAG) * FPS;
            let lo = (c - 6.0).max(0.0) as usize;
            let hi = ((c + 7.0) as usize).min(n);
            for (i, l) in local.iter_mut().enumerate().take(hi).skip(lo) {
                let d = (i as f32 - c) / 2.0;
                *l += 3.0 * (-0.5 * d * d).exp();
            }
        }
    }
    let mut beat_frames = track_beats(&local, period, 100.0);
    if ai.is_none() {
        // The kick is normally on the beat: if the grid half a beat later hits clearly more bass,
        // the tracker locked onto the off-beats.
        let bass_at = |i: usize| f.flux[0][i.saturating_sub(2)..(i + 3).min(n)].iter().cloned().fold(0.0, f32::max);
        let half = (period * 0.5).round() as usize;
        let on: f32 = beat_frames.iter().map(|&i| bass_at(i)).sum();
        let off: f32 = beat_frames.iter().filter(|&&i| i + half < n).map(|&i| bass_at(i + half)).sum();
        if off > 1.25 * on {
            beat_frames = beat_frames.iter().map(|&i| i + half).filter(|&i| i < n).collect();
        }
    }
    // Drop grid beats in leading and trailing silence (all of them in a silent song).
    match (onsets.first(), onsets.last()) {
        (Some(first), Some(last)) => {
            let (a, b) = ((first.t - LAG) * FPS - period, (last.t - LAG) * FPS + period);
            beat_frames.retain(|&i| (i as f32) >= a && (i as f32) <= b);
        }
        _ => beat_frames.clear(),
    }
    let beats: Vec<f32> = beat_frames.iter().map(|&i| i as f32 / FPS + LAG).collect();
    let bpm = if beats.len() > 1 { 60.0 * (beats.len() - 1) as f32 / (beats[beats.len() - 1] - beats[0]) } else { 60.0 * FPS / period };

    // Downbeats: every 4th beat, at the phase the model (or the bass) likes best.
    let mut votes = [0.0f32; 4];
    match ai.filter(|a| !a.downbeats.is_empty()) {
        Some(ai) => {
            for &d in &ai.downbeats {
                if let Some((j, &b)) = beats.iter().enumerate().min_by(|a, b| (a.1 - d).abs().total_cmp(&(b.1 - d).abs())) {
                    if (b - d).abs() < 0.1 {
                        votes[j % 4] += 1.0;
                    }
                }
            }
        }
        None => {
            for (j, &i) in beat_frames.iter().enumerate() {
                let lo = i.saturating_sub(2);
                let hi = (i + 3).min(n);
                votes[j % 4] += f.flux[0][lo..hi].iter().cloned().fold(0.0, f32::max);
            }
        }
    }
    let phase = (0..4).max_by(|&a, &b| votes[a].total_cmp(&votes[b])).unwrap_or(0);
    let downbeats: Vec<f32> = beats.iter().enumerate().filter(|(j, _)| j % 4 == phase).map(|(_, &b)| b).collect();

    // Intensity: smoothed loudness plus onset activity, spread over the song's own range.
    let loud = smooth(&f.rms_db, 25);
    let activity = smooth(&env, 50);
    let (l5, l95) = (percentile(&loud, 0.05), percentile(&loud, 0.95));
    let (a5, a95) = (percentile(&activity, 0.05), percentile(&activity, 0.95));
    let norm = |v: f32, lo: f32, hi: f32| if hi - lo > 1e-6 { ((v - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.5 };
    let per_frame: Vec<f32> = (0..n).map(|i| 0.7 * norm(loud[i], l5, l95) + 0.3 * norm(activity[i], a5, a95)).collect();
    let step = (FPS / INTENSITY_HZ) as usize;
    let intensity: Vec<f32> = per_frame.iter().step_by(step).copied().collect();

    let sections = find_sections(&f, &intensity, &downbeats, duration);

    Analysis { version: VERSION, duration, bpm, beats, downbeats, onsets, intensity, sections, ai: ai.is_some() }
}

/// Section boundaries where the sound changes: the distance between the mean features of the 6 s
/// before and after each point, peak-picked, at least 6 s apart and moved onto a downbeat.
fn find_sections(f: &Features, intensity: &[f32], downbeats: &[f32], duration: f32) -> Vec<Section> {
    const BLOCK: usize = 25; // half a second of frames
    const K: usize = 12; // 6 s of blocks
    let n = f.rms_db.len() / BLOCK;
    let mut feats: Vec<[f32; 4]> = (0..n)
        .map(|b| {
            let r = b * BLOCK..(b + 1) * BLOCK;
            let m = |v: &[f32]| v[r.clone()].iter().sum::<f32>() / BLOCK as f32;
            [m(&f.energy[0]), m(&f.energy[1]), m(&f.energy[2]), m(&f.rms_db) / 10.0]
        })
        .collect();
    // Standardise each feature over the song.
    for d in 0..4 {
        let mean = feats.iter().map(|x| x[d]).sum::<f32>() / n.max(1) as f32;
        let sd = (feats.iter().map(|x| (x[d] - mean).powi(2)).sum::<f32>() / n.max(1) as f32).sqrt().max(1e-6);
        for x in &mut feats {
            x[d] = (x[d] - mean) / sd;
        }
    }
    let mut novelty = vec![0.0f32; n];
    for c in K..n.saturating_sub(K) {
        let mut d2 = 0.0;
        for d in 0..4 {
            let before = feats[c - K..c].iter().map(|x| x[d]).sum::<f32>() / K as f32;
            let after = feats[c..c + K].iter().map(|x| x[d]).sum::<f32>() / K as f32;
            d2 += (after - before).powi(2);
        }
        novelty[c] = d2.sqrt();
    }
    let mean = novelty.iter().sum::<f32>() / n.max(1) as f32;
    let sd = (novelty.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n.max(1) as f32).sqrt();
    let mut cuts: Vec<f32> = Vec::new();
    for c in K..n.saturating_sub(K) {
        let v = novelty[c];
        let lo = c.saturating_sub(K / 2);
        let hi = (c + K / 2 + 1).min(n);
        if v > mean + 0.5 * sd && v > 0.8 && novelty[lo..hi].iter().all(|&w| w <= v) {
            let t = c as f32 * BLOCK as f32 / FPS;
            // Onto the nearest downbeat within 2 s.
            let t = downbeats.iter().copied().filter(|d| (d - t).abs() < 2.0).min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs())).unwrap_or(t);
            if cuts.last().is_none_or(|&l| t - l >= 6.0) && t >= 4.0 && duration - t >= 4.0 {
                cuts.push(t);
            }
        }
    }
    let mut bounds = vec![0.0];
    bounds.extend(cuts);
    bounds.push(duration);
    bounds
        .windows(2)
        .map(|w| {
            let (a, b) = ((w[0] * INTENSITY_HZ) as usize, ((w[1] * INTENSITY_HZ) as usize).min(intensity.len()));
            let mean = if b > a { intensity[a..b].iter().sum::<f32>() / (b - a) as f32 } else { 0.0 };
            Section { start: w[0], end: w[1], intensity: mean }
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A synthetic drum track: kicks on every beat, hats on the off-beats, a bass note per bar,
    /// optionally quiet (hats only) for the first `quiet` seconds.
    pub(crate) fn drums(bpm: f32, seconds: f32, quiet: f32) -> (Vec<f32>, Vec<f32>) {
        let n = (seconds * RATE as f32) as usize;
        let beat = 60.0 / bpm;
        let mut out = vec![0.0f32; n];
        let mut kicks = Vec::new();
        let mut noise = 0x2545_f491u32;
        let mut last_white = 0.0f32;
        let mut t = 0.25;
        let mut k = 0;
        while t < seconds - 0.5 {
            let loud = t >= quiet;
            if loud {
                kicks.push(t);
            }
            let s0 = (t * RATE as f32) as usize;
            for i in 0..(0.25 * RATE as f32) as usize {
                let (tt, j) = (i as f32 / RATE as f32, s0 + i);
                if j >= n {
                    break;
                }
                if loud {
                    out[j] += (std::f32::consts::TAU * (55.0 + 90.0 * (-tt * 25.0).exp()) * tt).sin() * (-tt * 12.0).exp() * 0.8;
                }
                // Hat half a beat later.
                let h = j + (beat * 0.5 * RATE as f32) as usize;
                if h < n && tt < 0.05 {
                    noise ^= noise << 13;
                    noise ^= noise >> 17;
                    noise ^= noise << 5;
                    // High-passed noise (a first difference), like a real hat: little bass.
                    let white = (noise >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
                    out[h] += (white - last_white) * 0.5 * (-tt * 80.0).exp() * if loud { 0.25 } else { 0.08 };
                    last_white = white;
                }
            }
            k += 1;
            t = 0.25 + k as f32 * beat;
        }
        (out, kicks)
    }

    #[test]
    fn finds_the_tempo_and_lands_on_the_kicks() {
        for bpm in [90.0, 100.0, 128.0, 150.0] {
            let (song, kicks) = drums(bpm, 30.0, 0.0);
            let a = analyse(&song, None);
            assert!((a.bpm - bpm).abs() / bpm < 0.01, "{bpm} bpm detected as {}", a.bpm);
            let near = a.beats.iter().filter(|b| kicks.iter().any(|k| (*b - k).abs() < 0.03)).count();
            assert!(near as f32 >= 0.9 * a.beats.len() as f32, "{bpm}: {near} of {} beats on kicks", a.beats.len());
            assert!(a.downbeats.len().abs_diff(a.beats.len() / 4) <= 1, "{} downbeats for {} beats", a.downbeats.len(), a.beats.len());
        }
    }

    #[test]
    fn fast_songs_may_be_counted_at_half_tempo() {
        let (song, _) = drums(174.0, 30.0, 0.0);
        let a = analyse(&song, None);
        let ok = |target: f32| (a.bpm - target).abs() / target < 0.01;
        assert!(ok(174.0) || ok(87.0), "174 bpm detected as {}", a.bpm);
    }

    #[test]
    fn model_beats_choose_the_tempo_level() {
        let (song, _) = drums(174.0, 30.0, 0.0);
        let beats: Vec<f32> = (0..80).map(|k| 0.25 + k as f32 * 60.0 / 174.0).collect();
        let a = analyse(&song, Some(&AiBeats { beats, downbeats: Vec::new() }));
        assert!((a.bpm - 174.0).abs() < 2.0, "{}", a.bpm);
        assert!(a.ai);
    }

    #[test]
    fn onsets_follow_the_hits() {
        let (song, kicks) = drums(120.0, 20.0, 0.0);
        let a = analyse(&song, None);
        let bass: Vec<f32> = a.onsets.iter().filter(|o| o.band == 0).map(|o| o.t).collect();
        let found = kicks.iter().filter(|k| bass.iter().any(|b| (b - *k).abs() < 0.04)).count();
        assert!(found as f32 >= 0.9 * kicks.len() as f32, "{found} of {} kicks", kicks.len());
        assert!(a.onsets.windows(2).all(|w| w[0].t <= w[1].t));
    }

    #[test]
    fn a_quiet_intro_is_its_own_low_intensity_section() {
        let (song, _) = drums(120.0, 48.0, 20.0);
        let a = analyse(&song, None);
        assert!(a.sections.len() >= 2, "{:?}", a.sections);
        let cut = a.sections[1].start;
        assert!((cut - 20.0).abs() < 2.5, "section starts at {cut}");
        assert!(a.sections[0].intensity < a.sections[1].intensity);
        assert!(a.intensity_at(10.0) < a.intensity_at(35.0));
        assert_eq!(a.section_at(30.0), 1);
    }

    #[test]
    fn silence_gives_an_empty_but_valid_analysis() {
        let a = analyse(&vec![0.0; RATE as usize * 5], None);
        assert!(a.beats.is_empty() && a.onsets.is_empty());
        assert_eq!(a.sections.len(), 1);
        assert!((a.duration - 5.0).abs() < 0.01);
    }
}

