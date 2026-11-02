//! Turns an [`Analysis`] into a ride: the road (slope, speed and curves over time) and the notes
//! on it. Deterministic: the same analysis, difficulty and seed always give the same map.
//!
//! Like Audiosurf, the road climbs slowly through calm parts and drops fast through intense
//! ones. Notes sit on the beat grid (and half-beats on the harder levels) where the song has an
//! onset; the band of that onset picks the lane, and the song's intensity picks the colour.

use super::analysis::{sample_curve, smooth, Analysis, INTENSITY_HZ};
use serde::{Deserialize, Serialize};

/// Lanes across the road (steering is free; notes sit on these).
pub const LANES: u8 = 5;
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
            Difficulty::Casual => Params { bends: 1, threshold: 0.55, min_gap: 0.36, subdivisions: 1, grey_rate: 0.06, speed: 30.0 },
            Difficulty::Pro => Params { bends: 2, threshold: 0.42, min_gap: 0.24, subdivisions: 2, grey_rate: 0.12, speed: 36.0 },
            Difficulty::Elite => Params { bends: MAX_BENDS, threshold: 0.30, min_gap: 0.16, subdivisions: 4, grey_rate: 0.18, speed: 42.0 },
        }
    }
}

struct Params {
    /// Most bends per hold.
    bends: usize,
    /// Minimum onset strength for a note (lowered in intense parts).
    threshold: f32,
    /// Seconds between notes at least.
    min_gap: f32,
    /// Note slots per beat: 1 on the beat, 2 adds half-beats, 4 adds quarter-beats.
    subdivisions: u8,
    /// Chance of a grey block per beat in lively parts.
    grey_rate: f32,
    /// Road speed scale in metres a second: 0.6× this when calm, ~0.8× at medium intensity, 1.5×
    /// at full.
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
    /// Seconds to hold after catching it (0 for an ordinary note).
    #[serde(default)]
    pub len: f32,
    /// Where a hold slides into other lanes along the way (empty: straight).
    #[serde(default)]
    pub bends: Bends,
}

/// Most bends a hold can have.
pub const MAX_BENDS: usize = 4;
/// A bend slides one lane over this long, arriving at its time.
pub const BEND_TIME: f32 = 0.25;
/// Bends are at least this far apart (and from the hold's start).
const BEND_GAP: f32 = 0.35;

/// A hold's bends: (song time it arrives, lane), in order, each one lane from the last.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Bends {
    n: u8,
    at: [(f32, u8); MAX_BENDS],
}

impl Bends {
    pub fn as_slice(&self) -> &[(f32, u8)] {
        &self.at[..self.n as usize]
    }

    pub fn len(&self) -> usize {
        self.n as usize
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    fn push(&mut self, t: f32, lane: u8) {
        if (self.n as usize) < MAX_BENDS {
            self.at[self.n as usize] = (t, lane);
            self.n += 1;
        }
    }

    /// Keep every bend one lane from the one before, starting from `start` (after the start
    /// lane moved); bends left with nowhere to go are dropped.
    fn reclamp(&mut self, start: u8) {
        let old = *self;
        *self = Bends::default();
        let mut prev = start;
        for &(t, lane) in old.as_slice() {
            let to = step_towards(prev, lane);
            if to != prev {
                self.push(t, to);
                prev = to;
            }
        }
    }
}

/// Bends from a list of (time, lane), for tests elsewhere.
#[cfg(test)]
pub(crate) fn generate_bends_for_test(at: &[(f32, u8)]) -> Bends {
    let mut b = Bends::default();
    for &(t, lane) in at {
        b.push(t, lane);
    }
    b
}

/// One lane from `from` towards `to`.
fn step_towards(from: u8, to: u8) -> u8 {
    match to.cmp(&from) {
        std::cmp::Ordering::Greater => from + 1,
        std::cmp::Ordering::Less => from - 1,
        std::cmp::Ordering::Equal => from,
    }
}

impl Note {
    /// An ordinary note (or grey).
    pub fn tap(t: f32, lane: u8, kind: NoteKind) -> Self {
        Note { t, lane, kind, len: 0.0, bends: Bends::default() }
    }

    /// When the note is over: its time, or the end of its hold.
    pub fn end(&self) -> f32 {
        self.t + self.len
    }

    pub fn is_hold(&self) -> bool {
        self.len > 0.0
    }

    /// Where a hold is across the road at song time `t`, in lanes (fractional while sliding).
    pub fn lane_at(&self, t: f32) -> f32 {
        let mut lane = self.lane as f32;
        for &(at, to) in self.bends.as_slice() {
            if t >= at {
                lane = to as f32;
                continue;
            }
            let k = ((t - (at - BEND_TIME)) / BEND_TIME).clamp(0.0, 1.0);
            lane += (to as f32 - lane) * k * k * (3.0 - 2.0 * k);
            break;
        }
        lane
    }

    /// The lane a hold ends in (its own lane if straight).
    pub fn end_lane(&self) -> u8 {
        self.bends.as_slice().last().map_or(self.lane, |b| b.1)
    }

    /// Every lane the note covers (a curved hold covers each lane on its way).
    pub fn lanes(&self) -> impl Iterator<Item = u8> + '_ {
        std::iter::once(self.lane).chain(self.bends.as_slice().iter().map(|b| b.1))
    }
}

/// Shortest and longest hold, in seconds.
pub const HOLD_MIN: f32 = 0.45;
pub const HOLD_MAX: f32 = 3.0;

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

/// Turn hold candidates into holds: the end goes on the half-beat grid (at least [`HOLD_MIN`],
/// at most [`HOLD_MAX`]); notes inside a hold are dropped, unless clearly stronger, which cut
/// the hold short instead. A hold too short to keep becomes an ordinary note.
///
/// The notes a hold drops become its bends: the ribbon slides towards each one's lane (one lane
/// at a time, [`BEND_GAP`] apart, at most `max_bends`), so a curved hold follows the music
/// played over the held sound.
fn settle_holds(mut kept: Vec<(Note, f32)>, a: &Analysis, min_gap: f32, max_bends: usize) -> Vec<Note> {
    let mut grid: Vec<f32> = a.beats.clone();
    grid.extend(a.beats.windows(2).map(|w| (w[0] + w[1]) / 2.0));
    grid.sort_by(f32::total_cmp);
    let gap = min_gap.max(0.25);
    let mut drop = vec![false; kept.len()];
    for i in 0..kept.len() {
        let (h, score) = kept[i];
        if drop[i] || !h.is_hold() {
            continue;
        }
        let raw_end = h.end().min(h.t + HOLD_MAX);
        let mut end = grid.iter().rev().find(|&&g| g <= raw_end + 0.05 && g >= h.t + HOLD_MIN).copied().unwrap_or(h.t);
        for (later, later_score) in kept[i + 1..].iter().take_while(|n| n.0.t < end + gap) {
            if *later_score > score + 0.2 {
                end = end.min(later.t - gap);
                break;
            }
        }
        if end - h.t < HOLD_MIN {
            kept[i].0.len = 0.0;
            continue;
        }
        kept[i].0.len = end - h.t;
        let inside = kept[i + 1..].iter().take_while(|n| n.0.t < end + gap).count();
        drop[i + 1..i + 1 + inside].iter_mut().for_each(|d| *d = true);
        let (mut prev, mut last) = (h.lane, h.t);
        let mut bends = Bends::default();
        for (n, _) in &kept[i + 1..i + 1 + inside] {
            if bends.len() >= max_bends || n.t > end - 0.15 {
                break;
            }
            let to = step_towards(prev, n.lane);
            if to != prev && n.t - last >= BEND_GAP {
                bends.push(n.t, to);
                (prev, last) = (to, n.t);
            }
        }
        kept[i].0.bends = bends;
    }
    kept.into_iter().zip(drop).filter(|(_, d)| !d).map(|((n, _), _)| n).collect()
}

/// The most lanes a note may be from the previous one, `gap` seconds later.
fn max_jump(gap: f32) -> u8 {
    if gap < 0.2 {
        1
    } else if gap < 0.3 {
        2
    } else {
        LANES
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
            RoadSample { slope: (0.5 - l) * 0.5, curve: turn[i] + sway, speed: p.speed * (0.6 + 0.9 * l * l), intensity: l }
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

    // Each section gives the three bands a home lane on the left, in the middle and on the right
    // (shuffled), and a walking pattern: successive notes of a band step around their home lane,
    // so runs move like a melody instead of hammering one lane.
    const ORDERS: [[usize; 3]; 6] = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    const WALKS: [[i8; 4]; 4] = [[0, 1, 0, -1], [0, -1, 0, 1], [0, 0, 1, 1], [0, 1, 1, 0]];
    let homes: Vec<[u8; 3]> = a
        .sections
        .iter()
        .map(|_| {
            let order = ORDERS[rng.below(6) as usize];
            let spots = [rng.below(2) as u8, LANES / 2, LANES - 1 - rng.below(2) as u8];
            [spots[order[0]], spots[order[1]], spots[order[2]]]
        })
        .collect();
    let walks: Vec<[i8; 4]> = a.sections.iter().map(|_| WALKS[rng.below(4) as usize]).collect();
    let mut steps = [0usize; 3];

    let mut scored: Vec<(Note, f32)> = Vec::new();
    let mut oi = 0;
    for (t, bonus) in cands {
        let window = if bonus > 0.1 { 0.07 } else { 0.05 };
        while oi < a.onsets.len() && a.onsets[oi].t < t - window {
            oi += 1;
        }
        let mut best: Option<(u8, f32, f32)> = None;
        for o in a.onsets[oi..].iter().take_while(|o| o.t <= t + window) {
            if best.is_none_or(|(_, s, _)| o.strength > s) {
                best = Some((o.band, o.strength, o.t));
            }
        }
        let Some((mut band, strength, _)) = best else { continue };
        // Any onset here that starts a sustained sound makes this a hold note in that sound's band
        // (a chord held over a kick is a hold, even if the kick is louder). Its end is settled
        // below.
        let mut len = 0.0;
        for o in a.onsets[oi..].iter().take_while(|o| o.t <= t + window) {
            if let Some(x) = a.sustains.iter().find(|x| x.band == o.band && (x.start - o.t).abs() < 0.05) {
                band = o.band;
                len = (x.end - t).max(0.0);
                break;
            }
        }
        let intensity = a.intensity_at(t);
        let score = strength + bonus;
        if score < p.threshold - 0.25 * intensity {
            continue;
        }
        let section = a.section_at(t);
        let home = homes.get(section).map_or(band * 2, |h| h[band as usize]);
        let walk = walks.get(section).map_or(0, |w| w[steps[band as usize] % 4]);
        steps[band as usize] += 1;
        let lane = (home as i8 + walk).clamp(0, LANES as i8 - 1) as u8;
        // Every other phrase (four bars) mirrors the layout, so one busy band doesn't keep you
        // on one side of the road.
        let phrase = a.downbeats.partition_point(|&d| d <= t + 1e-3) / 4;
        let lane = if phrase % 2 == 1 { LANES - 1 - lane } else { lane };
        let colour = ((0.7 * intensity + 0.3 * strength) * COLOURS as f32).clamp(0.0, (COLOURS - 1) as f32) as u8;
        scored.push((Note { len, ..Note::tap(t, lane, NoteKind::Colour(colour)) }, score));
    }
    // Strongest first, skipping any candidate too close to one already taken.
    scored.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.t.total_cmp(&y.0.t)));
    let mut kept: Vec<(Note, f32)> = Vec::new(); // kept sorted by time
    for (note, score) in scored {
        let i = kept.partition_point(|n| n.0.t < note.t);
        let clear_before = i == 0 || note.t - kept[i - 1].0.t >= p.min_gap;
        let clear_after = i == kept.len() || kept[i].0.t - note.t >= p.min_gap;
        if clear_before && clear_after {
            kept.insert(i, (note, score));
        }
    }
    let mut notes = settle_holds(kept, a, p.min_gap, p.bends);
    // Quick successions move at most two lanes (one when very quick), towards where they were
    // going.
    for i in 1..notes.len() {
        let (prev, cur) = (notes[i - 1], notes[i]);
        let (from, max) = (prev.end_lane(), max_jump(cur.t - prev.end()));
        if cur.lane.abs_diff(from) > max {
            notes[i].lane = if cur.lane > from { from + max } else { from - max };
            let start = notes[i].lane;
            notes[i].bends.reclamp(start);
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
        // Lanes in use around then, including the whole length of a hold.
        let near: Vec<u8> = notes.iter().filter(|n| n.t - 0.5 < t && t < n.end() + 0.5).flat_map(|n| n.lanes()).collect();
        let free: Vec<u8> = (0..LANES).filter(|l| !near.contains(l)).collect();
        if free.is_empty() {
            continue;
        }
        let lane = free[rng.below(free.len() as u32) as usize];
        greys.push(Note::tap(t, lane, NoteKind::Grey));
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
                assert!(w[1].lane.abs_diff(w[0].end_lane()) <= max_jump(w[1].t - w[0].end()), "{d:?}: lane jump at {}", w[1].t);
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
            let used: std::collections::HashSet<u8> = colour.iter().map(|n| n.lane).collect();
            assert!(used.len() >= 4, "{d:?}: notes only on lanes {used:?}");
        }
    }

    #[test]
    fn held_sounds_become_holds_with_nothing_inside() {
        use crate::game::analysis::tests::pads;
        let (d, _) = drums(120.0, 30.0, 0.0);
        // Chords start on a beat (the drums' beats are at 0.25 + k/2 s).
        let (p, held) = pads(30.0);
        let shift = (0.25 * crate::game::analysis::RATE as f32) as usize;
        let p: Vec<f32> = std::iter::repeat_n(0.0, shift).chain(p).take(d.len()).collect();
        let held: Vec<(f32, f32)> = held.iter().map(|&(s, e)| (s + 0.25, e + 0.25)).collect();
        let mix: Vec<f32> = d.iter().zip(&p).map(|(a, b)| a * 0.6 + b * 1.6).collect();
        let a = analyse(&mix, None);
        assert!(!a.sustains.is_empty(), "the chords are sustains");
        for diff in Difficulty::ALL {
            let m = generate(&a, diff, 11);
            let holds: Vec<&Note> = m.notes.iter().filter(|n| n.is_hold()).collect();
            assert!(!holds.is_empty(), "{diff:?} has holds");
            for h in &holds {
                assert!(h.len >= HOLD_MIN - 1e-4 && h.len <= HOLD_MAX + 1e-4, "{diff:?}: hold of {} s", h.len);
                assert!(held.iter().any(|&(s, e)| h.t >= s - 0.1 && h.end() <= e + 0.3), "{diff:?}: hold {}–{} is not on a held chord", h.t, h.end());
                for n in &m.notes {
                    if n.t > h.t && n.t < h.end() {
                        assert!(n.kind == NoteKind::Grey && !h.lanes().any(|l| l == n.lane), "{diff:?}: {n:?} inside the hold at {}", h.t);
                    }
                }
                // Bends: inside the hold, in order, one lane each, apart, within the limit.
                let mut prev = (h.t, h.lane);
                for &(t, lane) in h.bends.as_slice() {
                    assert!(t > h.t && t <= h.end(), "{diff:?}: bend at {t} outside {}–{}", h.t, h.end());
                    assert!(t - prev.0 >= BEND_GAP - 1e-4 && lane.abs_diff(prev.1) == 1, "{diff:?}: bend {prev:?} → {:?}", (t, lane));
                    prev = (t, lane);
                }
                assert!(h.bends.len() <= diff.params().bends);
            }
        }
        // Some holds curve (the music goes on over the held chords), some stay straight.
        let m = generate(&a, Difficulty::Elite, 11);
        assert!(m.notes.iter().any(|n| !n.bends.is_empty()), "curved holds");
        // Pure drums: no holds.
        let plain = generate(&analyse(&d, None), Difficulty::Elite, 11);
        assert!(plain.notes.iter().all(|n| !n.is_hold()));
    }

    #[test]
    fn a_curved_hold_slides_smoothly_between_its_lanes() {
        let mut bends = Bends::default();
        bends.push(2.0, 3);
        bends.push(3.0, 4);
        let h = Note { len: 2.0, bends, ..Note::tap(1.0, 2, NoteKind::Colour(1)) };
        assert_eq!(h.lane_at(1.0), 2.0);
        assert_eq!(h.lane_at(1.7), 2.0, "still straight before the slide");
        let mid = h.lane_at(2.0 - BEND_TIME / 2.0);
        assert!((mid - 2.5).abs() < 0.01, "halfway through the slide: {mid}");
        assert_eq!(h.lane_at(2.0), 3.0);
        assert_eq!(h.lane_at(3.0), 4.0);
        assert_eq!(h.end_lane(), 4);
        assert_eq!(h.lanes().collect::<Vec<_>>(), vec![2, 3, 4]);
        // Moving the start re-clamps the bends to one lane at a time.
        let mut moved = h;
        moved.lane = 0;
        moved.bends.reclamp(0);
        assert_eq!(moved.bends.as_slice(), &[(2.0, 1), (3.0, 2)]);
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
