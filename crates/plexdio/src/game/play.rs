//! A ride in progress: where the craft is across the road, which notes were caught or missed,
//! score, combo and multiplier. Time always comes from the song (see [`super::clock`]); nothing
//! here can stop or slow the music.

use super::mapgen::{Map, NoteKind, COLOURS, LANES};

/// Where the lanes are, across the road (-1 left edge … 1 right edge of the steerable range).
pub fn lane_x(lane: u8) -> f32 {
    let mid = (LANES - 1) as f32 / 2.0;
    (lane as f32 - mid) / mid * 0.8
}

/// How close (in steering units) the craft must be to a lane to catch its note: a little under
/// half the lane spacing, so between two lanes catches neither.
const CATCH: f32 = 0.18;
/// Notes this soon after the ride starts are left out: a run-up to get the feel.
pub const RUN_UP: f32 = 2.0;
/// Points per colour, cool to hot.
const POINTS: [u32; COLOURS as usize] = [10, 20, 40, 70, 100];
/// Points lost on a grey block (never below zero).
const GREY_PENALTY: u32 = 50;

/// Steering input for one frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct Input {
    /// The pointer's position across the view (0 left … 1 right), when it moved this frame.
    pub pointer: Option<f32>,
    pub left: bool,
    pub right: bool,
}

/// Something that just happened, for effects.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    Caught { t: f32, lane: u8, colour: u8, points: u32 },
    Missed { t: f32, lane: u8 },
    Grey { t: f32, lane: u8 },
}

pub struct Ride {
    pub map: Map,
    /// Craft position across the road, -1..=1.
    pub x: f32,
    target: f32,
    /// Steering speed (for banking).
    pub vx: f32,
    pub score: u64,
    pub combo: u32,
    pub best_combo: u32,
    pub caught: u32,
    pub missed: u32,
    pub greys: u32,
    /// Next note not yet reached.
    next: usize,
    /// Song time the ride started.
    pub start: f32,
    last_t: f32,
    /// Recent events (the renderer draws bursts for a moment).
    pub events: Vec<Event>,
    /// Per note: caught (or a grey that was hit), so it is gone from the road. Missed notes stay
    /// and fly past.
    collected: Vec<bool>,
}

impl Ride {
    /// Start riding `map` at song time `t` (mid-song is fine).
    pub fn new(map: Map, t: f32) -> Self {
        let next = map.notes.partition_point(|n| n.t < t + RUN_UP);
        let collected = vec![false; map.notes.len()];
        Ride { map, x: 0.0, target: 0.0, vx: 0.0, score: 0, combo: 0, best_combo: 0, caught: 0, missed: 0, greys: 0, next, start: t, last_t: t, events: Vec::new(), collected }
    }

    /// Score multiplier from the current combo: ×2 at 10, ×3 at 25, ×4 at 50.
    pub fn multiplier(&self) -> u32 {
        match self.combo {
            0..=9 => 1,
            10..=24 => 2,
            25..=49 => 3,
            _ => 4,
        }
    }

    /// Caught notes as a share of the notes reached so far (1.0 before any).
    pub fn accuracy(&self) -> f32 {
        let n = self.caught + self.missed;
        if n == 0 {
            1.0
        } else {
            self.caught as f32 / n as f32
        }
    }

    /// The song has played past the end of the map.
    pub fn finished(&self) -> bool {
        self.last_t >= self.map.duration - 0.05
    }

    /// Index of the first note still ahead (for drawing).
    pub fn next_note(&self) -> usize {
        self.next
    }

    /// Note `i` was caught (or hit, for a grey) and is no longer on the road.
    pub fn collected(&self, i: usize) -> bool {
        self.collected.get(i).copied().unwrap_or(false)
    }

    /// Advance to song time `t`, steering with `input` over `dt` seconds of frame time.
    pub fn update(&mut self, t: f32, input: Input, dt: f32) {
        // Steering: the pointer sets the target directly; keys move it.
        if let Some(p) = input.pointer {
            self.target = ((p - 0.5) * 2.4).clamp(-1.0, 1.0);
        }
        let keys = input.right as i32 - input.left as i32;
        if keys != 0 {
            self.target = (self.target + keys as f32 * 2.6 * dt).clamp(-1.0, 1.0);
        }
        let old = self.x;
        self.x += (self.target - self.x) * (1.0 - (-18.0 * dt).exp());
        self.vx = if dt > 0.0 { (self.x - old) / dt } else { 0.0 };

        // A seek or a long stall: skip what was jumped over without judging it.
        if t < self.last_t - 0.3 || t > self.last_t + 1.0 {
            self.next = self.map.notes.partition_point(|n| n.t < t + 0.5);
        }
        self.last_t = t;
        self.events.retain(|e| t - event_t(e) < 1.0 && event_t(e) <= t + 0.1);

        while let Some(n) = self.map.notes.get(self.next).copied() {
            if n.t > t {
                break;
            }
            let i = self.next;
            self.next += 1;
            let close = (self.x - lane_x(n.lane)).abs() < CATCH;
            match n.kind {
                NoteKind::Colour(c) if close => {
                    let points = POINTS[c as usize] * self.multiplier();
                    self.combo += 1;
                    self.best_combo = self.best_combo.max(self.combo);
                    self.caught += 1;
                    self.score += points as u64;
                    self.collected[i] = true;
                    self.events.push(Event::Caught { t: n.t, lane: n.lane, colour: c, points });
                }
                NoteKind::Colour(_) => {
                    self.combo = 0;
                    self.missed += 1;
                    self.events.push(Event::Missed { t: n.t, lane: n.lane });
                }
                NoteKind::Grey if close => {
                    self.combo = 0;
                    self.greys += 1;
                    self.score = self.score.saturating_sub(GREY_PENALTY as u64);
                    self.collected[i] = true;
                    self.events.push(Event::Grey { t: n.t, lane: n.lane });
                }
                NoteKind::Grey => {}
            }
        }
    }
}

fn event_t(e: &Event) -> f32 {
    match *e {
        Event::Caught { t, .. } | Event::Missed { t, .. } | Event::Grey { t, .. } => t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::mapgen::{Difficulty, Note};

    fn map(notes: Vec<Note>) -> Map {
        Map { difficulty: Difficulty::Pro, duration: 60.0, road: Vec::new(), distance: Vec::new(), notes, beats: Vec::new(), downbeats: Vec::new() }
    }

    fn note(t: f32, lane: u8, kind: NoteKind) -> Note {
        Note { t, lane, kind }
    }

    /// Ride from 0 to `until` at 60 fps, pointing at `lane` the whole time.
    fn ride_on_lane(r: &mut Ride, from: f32, until: f32, lane: u8) {
        let p = Some(lane_x(lane) / 2.4 + 0.5);
        let mut t = from;
        while t < until {
            t += 1.0 / 60.0;
            r.update(t, Input { pointer: p, ..Default::default() }, 1.0 / 60.0);
        }
    }

    #[test]
    fn catching_scores_and_builds_the_combo() {
        let notes = (0..30).map(|k| note(3.0 + k as f32 * 0.5, 0, NoteKind::Colour(4))).collect();
        let mut r = Ride::new(map(notes), 0.0);
        ride_on_lane(&mut r, 0.0, 20.0, 0);
        assert_eq!((r.caught, r.missed, r.combo), (30, 0, 30));
        // 10 at ×1, 15 at ×2, 5 at ×3 (100 points each).
        assert_eq!(r.score, 100 * (10 + 15 * 2 + 5 * 3));
        assert_eq!(r.multiplier(), 3);
        assert!((r.accuracy() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn misses_and_greys_break_the_combo() {
        let notes = vec![note(3.0, 4, NoteKind::Colour(0)), note(4.0, 4, NoteKind::Colour(0)), note(5.0, 3, NoteKind::Colour(0)), note(6.0, 4, NoteKind::Grey)];
        let mut r = Ride::new(map(notes), 0.0);
        ride_on_lane(&mut r, 0.0, 6.5, 4);
        // The neighbouring lane is missed: the lanes don't overlap.
        assert_eq!((r.caught, r.missed, r.greys, r.combo), (2, 1, 1, 0));
        assert!(r.collected(0) && r.collected(1) && !r.collected(2) && r.collected(3), "caught and hit notes leave the road; missed ones stay");
        assert_eq!(r.score, 0, "20 points minus the grey's 50 stays at zero");
        assert!(r.events.iter().any(|e| matches!(e, Event::Grey { .. })));
    }

    #[test]
    fn a_mid_song_start_has_a_run_up_and_seeks_skip_notes() {
        let notes = (0..100).map(|k| note(k as f32, 1, NoteKind::Colour(1))).collect();
        let mut r = Ride::new(map(notes), 30.2);
        ride_on_lane(&mut r, 30.2, 34.5, 1);
        // Notes at 30, 31 and 32 are in the run-up; 33 and 34 count.
        assert_eq!(r.caught, 2);
        // Jump ahead 10 s: nothing in between is judged.
        r.update(44.6, Input::default(), 1.0 / 60.0);
        assert_eq!(r.caught + r.missed, 2);
        // Judging resumes half a second past the jump: 46 and 47 count.
        ride_on_lane(&mut r, 44.6, 47.5, 1);
        assert_eq!(r.caught, 4);
    }

    #[test]
    fn keys_steer_smoothly_and_stay_on_the_road() {
        let mut r = Ride::new(map(Vec::new()), 0.0);
        for k in 1..=120 {
            r.update(k as f32 / 60.0, Input { right: true, ..Default::default() }, 1.0 / 60.0);
        }
        assert!(r.x > 0.95 && r.x <= 1.0);
        assert!(lane_x(0) < lane_x(1) && lane_x(1) < 0.0 && lane_x(2) == 0.0 && lane_x(4) > lane_x(3));
        assert!((lane_x(4) - 0.8).abs() < 1e-6);
    }
}
