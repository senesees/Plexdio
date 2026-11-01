//! Turns an [`Analysis`] into a ride: the road (slope, speed and curves over time) and the notes
//! on it. Deterministic: the same analysis, difficulty and seed always give the same map.
//!
//! Like Audiosurf, the road climbs slowly through calm parts and drops fast through intense
//! ones. Notes sit on the beat grid (and half-beats on the harder levels) where the song has an
//! onset; the band of that onset picks the lane, and the song's intensity picks the colour.

use super::analysis::{sample_curve, smooth, Analysis, INTENSITY_HZ};
use serde::{Deserialize, Serialize};

/// Lanes across the road (steering is free; notes sit on these).
pub const LANES: u8 = 3;
/// Note colours, from cool (few points) to hot (many points).
pub const COLOURS: u8 = 5;
/// Samples per second of the road curves.
pub const ROAD_HZ: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Difficulty {
    Casual,
    #[default]
    Pro,
    Elite,
}

impl Difficulty {
    pub const ALL: [Difficulty; 3] = [Difficulty::Casual, Difficulty::Pro, Difficulty::Elite];

    pub fn label(self) -> &'static str {
        match self {
            Difficulty::Casual => "Casual",
            Difficulty::Pro => "Pro",
            Difficulty::Elite => "Elite",
        }
    }

    fn params(self) -> Params {
        match self {
            Difficulty::Casual => Params { threshold: 0.55, min_gap: 0.36, subdivisions: 1, grey_rate: 0.06, speed: 30.0 },
            Difficulty::Pro => Params { threshold: 0.42, min_gap: 0.24, subdivisions: 2, grey_rate: 0.12, speed: 36.0 },
            Difficulty::Elite => Params { threshold: 0.30, min_gap: 0.16, subdivisions: 4, grey_rate: 0.18, speed: 42.0 },
        }
    }
}

struct Params {
    /// Minimum onset strength for a note (lowered in intense parts).
    threshold: f32,
    /// Seconds between notes at least.
    min_gap: f32,
    /// Note slots per beat: 1 on the beat, 2 adds half-beats, 4 adds quarter-beats.
    subdivisions: u8,
    /// Chance of a grey block per beat in lively parts.
    grey_rate: f32,
    /// Road speed in metres a second at medium intensity.
    speed: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteKind {
    /// 0 (cool, few points) ..= COLOURS - 1 (hot, many points).
    Colour(u8),
    /// An obstacle: hitting it breaks the combo.
    Grey,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// Seconds from the start of the song.
    pub t: f32,
    /// 0 (left) ..= LANES - 1 (right).
    pub lane: u8,
    pub kind: NoteKind,
}

/// The road at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct RoadSample {
    /// Gradient: positive climbs, negative drops.
    pub slope: f32,
    /// Turn rate in radians per second (positive turns right).
    pub curve: f32,
    /// Metres per second.
    pub speed: f32,
    /// Smoothed intensity, 0..=1 (colours the road).
    pub intensity: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Map {
    pub difficulty: Difficulty,
    pub duration: f32,
    /// The road at [`ROAD_HZ`].
    pub road: Vec<RoadSample>,
    /// Distance travelled at each road sample, in metres.
    pub distance: Vec<f32>,
    /// Sorted by time.
    pub notes: Vec<Note>,
    /// The beat grid (the road pulses on it).
    pub beats: Vec<f32>,
    pub downbeats: Vec<f32>,
}

impl Map {
    /// The road at `t` seconds (linear between samples).
    pub fn road_at(&self, t: f32) -> RoadSample {
        let x = t * ROAD_HZ;
        let f = |g: fn(&RoadSample) -> f32| sample_curve(&self.road.iter().map(g).collect::<Vec<_>>(), x);
        RoadSample { slope: f(|r| r.slope), curve: f(|r| r.curve), speed: f(|r| r.speed), intensity: f(|r| r.intensity) }
    }

    /// Metres travelled by `t` seconds.
    pub fn distance_at(&self, t: f32) -> f32 {
        let x = t * ROAD_HZ;
        if x <= 0.0 || self.distance.is_empty() {
            return 0.0;
        }
        let last = self.distance.len() - 1;
        if x >= last as f32 {
            let end = self.road.last().map_or(0.0, |r| r.speed);
            return self.distance[last] + (x - last as f32) / ROAD_HZ * end;
        }
        sample_curve(&self.distance, x)
    }

    /// Coloured notes (the ones that score).
    pub fn colour_notes(&self) -> usize {
        self.notes.iter().filter(|n| matches!(n.kind, NoteKind::Colour(_))).count()
    }
}

/// SplitMix64: small, fast and deterministic.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// 0..1
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }
}

/// A stable seed from a text key (e.g. the track's key), so maps don't change between runs.
pub fn seed_for(key: &str) -> u64 {
    // FNV-1a.
    key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Build the map for one difficulty.
pub fn generate(a: &Analysis, difficulty: Difficulty, seed: u64) -> Map {
    let p = difficulty.params();
    let mut rng = Rng(seed ^ (difficulty as u64 * 0x5851_f42d_4c95_7f2d));
    let road = build_road(a, &p, &mut rng);
    let mut distance = Vec::with_capacity(road.len());
    let mut d = 0.0;
    for r in &road {
        distance.push(d);
        d += r.speed / ROAD_HZ;
    }
    let notes = place_notes(a, &p, &mut rng);
    Map { difficulty, duration: a.duration, road, distance, notes, beats: a.beats.clone(), downbeats: a.downbeats.clone() }
}

fn build_road(a: &Analysis, p: &Params, rng: &mut Rng) -> Vec<RoadSample> {
    let n = (a.duration * ROAD_HZ).ceil() as usize + 1;
    let raw: Vec<f32> = (0..n).map(|i| sample_curve(&a.intensity, i as f32 / ROAD_HZ * INTENSITY_HZ)).collect();
    // ±1.5 s: the road reacts to the music but doesn't jitter.
    let level = smooth(&raw, (1.5 * ROAD_HZ) as usize);
    // Each section turns one way (or runs straight), eased in and out.
    let turns: Vec<f32> = a.sections.iter().map(|_| [-0.11, -0.06, 0.0, 0.06, 0.11][rng.below(5) as usize]).collect();
    let turn: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / ROAD_HZ;
            let s = a.section_at(t);
            turns.get(s).copied().unwrap_or(0.0)
        })
        .collect();
    let turn = smooth(&turn, (2.0 * ROAD_HZ) as usize);
    (0..n)
        .map(|i| {
            let t = i as f32 / ROAD_HZ;
            let l = level[i];
            // A slow sway on top, so straight sections aren't dead straight.
            let sway = 0.03 * (t * 0.37).sin();
            RoadSample { slope: (0.5 - l) * 0.5, curve: turn[i] + sway, speed: p.speed * (0.7 + 0.6 * l), intensity: l }
        })
        .collect()
}

fn place_notes(a: &Analysis, p: &Params, rng: &mut Rng) -> Vec<Note> {
    // Candidate times: the beats, plus half- and quarter-beats on harder levels.
    let mut cands: Vec<(f32, f32)> = Vec::new(); // (time, bonus)
    for (j, &b) in a.beats.iter().enumerate() {
        let down = a.downbeats.iter().any(|d| (d - b).abs() < 1e-3);
        cands.push((b, if down { 0.25 } else { 0.15 }));
        if let Some(&next) = a.beats.get(j + 1) {
            // The finest subdivision whose slots are still the minimum gap apart.
            let mut sub = p.subdivisions;
            while sub > 1 && (next - b) / (sub as f32) < p.min_gap {
                sub /= 2;
            }
            for k in 1..sub {
                // Half-beats rank above the quarter-beats around them.
                let bonus = if 2 * k == sub { 0.05 } else { 0.0 };
                cands.push((b + (next - b) * k as f32 / sub as f32, bonus));
            }
        }
    }
    cands.sort_by(|x, y| x.0.total_cmp(&y.0));

    // Lane per band, reshuffled each section so the patterns vary.
    let perms: Vec<[u8; 3]> = a.sections.iter().map(|_| [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]][rng.below(6) as usize]).collect();

    let mut scored: Vec<(Note, f32)> = Vec::new();
    let mut oi = 0;
    for (t, bonus) in cands {
        let window = if bonus > 0.1 { 0.07 } else { 0.05 };
        while oi < a.onsets.len() && a.onsets[oi].t < t - window {
            oi += 1;
        }
        let mut best: Option<(u8, f32)> = None;
        for o in a.onsets[oi..].iter().take_while(|o| o.t <= t + window) {
            if best.is_none_or(|(_, s)| o.strength > s) {
                best = Some((o.band, o.strength));
            }
        }
        let Some((band, strength)) = best else { continue };
        let intensity = a.intensity_at(t);
        let score = strength + bonus;
        if score < p.threshold - 0.25 * intensity {
            continue;
        }
        let section = a.section_at(t);
        let lane = perms.get(section).map_or(band, |perm| perm[band as usize]);
        let colour = ((0.7 * intensity + 0.3 * strength) * COLOURS as f32).clamp(0.0, (COLOURS - 1) as f32) as u8;
        scored.push((Note { t, lane, kind: NoteKind::Colour(colour) }, score));
    }
    // Strongest first, skipping any candidate too close to one already taken.
    scored.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.t.total_cmp(&y.0.t)));
    let mut notes: Vec<Note> = Vec::new(); // kept sorted by time
    for (note, _) in scored {
        let i = notes.partition_point(|n| n.t < note.t);
        let clear_before = i == 0 || note.t - notes[i - 1].t >= p.min_gap;
        let clear_after = i == notes.len() || notes[i].t - note.t >= p.min_gap;
        if clear_before && clear_after {
            notes.insert(i, note);
        }
    }
    // Quick successions only move one lane at a time.
    for i in 1..notes.len() {
        let (prev, cur) = (notes[i - 1], notes[i]);
        if cur.t - prev.t < 0.3 && cur.lane.abs_diff(prev.lane) > 1 {
            notes[i].lane = 1;
        }
    }

    // Grey blocks in lively parts, in a lane no nearby note uses.
    let mut greys: Vec<Note> = Vec::new();
    for &b in &a.beats {
        if b < 2.0 || a.intensity_at(b) < 0.35 || rng.unit() >= p.grey_rate {
            continue;
        }
        let t = b;
        if greys.last().is_some_and(|g| t - g.t < 0.4) {
            continue;
        }
        let near: Vec<u8> = notes.iter().filter(|n| (n.t - t).abs() < 0.5).map(|n| n.lane).collect();
        let free: Vec<u8> = (0..LANES).filter(|l| !near.contains(l)).collect();
        if free.is_empty() {
            continue;
        }
        let lane = free[rng.below(free.len() as u32) as usize];
        greys.push(Note { t, lane, kind: NoteKind::Grey });
    }
    notes.extend(greys);
    notes.sort_by(|x, y| x.t.total_cmp(&y.t));
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::analysis::{analyse, tests::drums};

    fn song() -> Analysis {
        let (s, _) = drums(124.0, 60.0, 16.0);
        analyse(&s, None)
    }

    #[test]
    fn maps_are_deterministic_and_seeded() {
        let a = song();
        let m1 = generate(&a, Difficulty::Pro, seed_for("track-1"));
        let m2 = generate(&a, Difficulty::Pro, seed_for("track-1"));
        assert_eq!(m1, m2);
        assert_ne!(seed_for("track-1"), seed_for("track-2"));
    }

    #[test]
    fn maps_are_playable() {
        let a = song();
        for d in Difficulty::ALL {
            let m = generate(&a, d, 7);
            let gap = d.params().min_gap;
            let colour: Vec<&Note> = m.notes.iter().filter(|n| matches!(n.kind, NoteKind::Colour(_))).collect();
            assert!(!colour.is_empty(), "{d:?} has notes");
            for w in colour.windows(2) {
                assert!(w[1].t - w[0].t >= gap - 1e-4, "{d:?}: notes {:.3}s apart", w[1].t - w[0].t);
                if w[1].t - w[0].t < 0.3 {
                    assert!(w[1].lane.abs_diff(w[0].lane) <= 1, "{d:?}: lane jump at {}", w[1].t);
                }
            }
            for n in &m.notes {
                assert!(n.lane < LANES && n.t >= 0.0 && n.t <= m.duration);
                if let NoteKind::Colour(c) = n.kind {
                    assert!(c < COLOURS);
                }
            }
            // A grey never shares a lane with a nearby note.
            for g in m.notes.iter().filter(|n| n.kind == NoteKind::Grey) {
                assert!(!colour.iter().any(|n| n.lane == g.lane && (n.t - g.t).abs() < 0.5), "{d:?}: grey on a note's lane at {}", g.t);
            }
            assert!(m.notes.windows(2).all(|w| w[0].t <= w[1].t));
        }
    }

    #[test]
    fn harder_levels_have_more_notes_and_speed() {
        let a = song();
        let [c, p, e] = Difficulty::ALL.map(|d| generate(&a, d, 3));
        assert!(c.colour_notes() < p.colour_notes() && p.colour_notes() <= e.colour_notes(), "{} {} {}", c.colour_notes(), p.colour_notes(), e.colour_notes());
        assert!(c.distance_at(50.0) < e.distance_at(50.0));
    }

    #[test]
    fn the_road_climbs_when_calm_and_drops_when_intense() {
        let a = song();
        let m = generate(&a, Difficulty::Pro, 1);
        let (calm, loud) = (m.road_at(6.0), m.road_at(40.0));
        assert!(calm.slope > 0.0 && loud.slope < 0.0, "calm {calm:?}, loud {loud:?}");
        assert!(loud.speed > calm.speed);
        // Distance only grows.
        assert!(m.distance.windows(2).all(|w| w[1] > w[0]));
        assert!(m.distance_at(m.duration + 5.0) > m.distance_at(m.duration));
    }
}
