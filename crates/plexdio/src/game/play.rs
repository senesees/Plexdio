//! A ride in progress: where the craft is across the road, which notes were caught or missed,
//! holds being ridden, score, combo and multiplier. Time always comes from the song (see [`super::clock`]); nothing
//! here can stop or slow the music.

use super::mapgen::{Map, NoteKind, COLOURS, LANES};

/// Where the lanes are, across the road (-1 left edge … 1 right edge of the steerable range).
pub fn lane_x(lane: u8) -> f32 {
    lane_xf(lane as f32)
}

/// [`lane_x`] for a fractional lane (a curved hold between two lanes).
pub fn lane_xf(lane: f32) -> f32 {
    let mid = (LANES - 1) as f32 / 2.0;
    (lane - mid) / mid * 0.8
}

/// How close (in steering units; 1 = 5 m) the craft must be to a lane to catch its note: 1.3 m,
/// about where the craft's side meets the block's (the lanes are 2 m apart).
const CATCH: f32 = 0.26;
/// Greys are judged narrower (0.9 m): grazing one shouldn't cost you.
const GREY_CATCH: f32 = 0.18;
/// A note can be caught from this long before its time to [`LATE`] after: the craft's nose
/// meets a block before the block's centre reaches the craft.
const EARLY: f32 = 0.07;
const LATE: f32 = 0.05;
/// When riding starts mid-song, notes this soon after are left out: a run-up to get the feel.
pub const RUN_UP: f32 = 2.0;
/// A ride starting this early in the song (carrying on into the next song, or a restart) is at
/// the song's start: no run-up, every note counts.
const SONG_START: f32 = 1.5;
/// Points per colour, cool to hot.
const POINTS: [u32; COLOURS as usize] = [10, 20, 40, 70, 100];
/// Points lost on a grey block (never below zero).
const GREY_PENALTY: u32 = 50;
/// While holding, the craft may drift this far from the lane (a little more than [`CATCH`]).
const HOLD_STAY: f32 = 0.3;
/// A hold pays a tenth of its note's points for every tenth of a second, and its note's points
/// again for riding it to the end.
const TICK: f32 = 0.1;

/// Where a note is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteState {
    /// Still ahead (or reached without being judged, in a run-up or a seek).
    Pending,
    Caught,
    Missed,
    /// A grey that was hit.
    GreyHit,
    /// A hold being ridden right now.
    Holding,
    /// A hold ridden to the end.
    Held,
    /// A hold left early.
    Broken,
    /// Left out (before the ride started, in its run-up, or jumped over by a seek): not judged,
    /// not drawn.
    Skipped,
}

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
    /// A hold ridden to its end; `points` is everything it paid.
    HoldDone { t: f32, lane: u8, colour: u8, points: u32 },
    HoldBroken { t: f32, lane: u8 },
}

/// The hold being ridden.
#[derive(Debug, Clone, Copy)]
pub struct Holding {
    /// Its note.
    pub i: usize,
    /// Paid up to this song time.
    ticked_to: f32,
    /// Points it has paid so far (head included).
    pub earned: u32,
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
    /// Per note: where it is in its life.
    states: Vec<NoteState>,
    /// Per note (holds): song time up to which its ribbon has been ridden away.
    consumed: Vec<f32>,
    /// The hold being ridden, if any.
    pub holding: Option<Holding>,
}

impl Ride {
    /// Start riding `map` at song time `t` (mid-song is fine).
    pub fn new(map: Map, t: f32) -> Self {
        let run_up = if t < SONG_START { 0.0 } else { RUN_UP };
        let next = map.notes.partition_point(|n| n.t < t + run_up);
        let n = map.notes.len();
        let mut states = vec![NoteState::Pending; n];
        states[..next].iter_mut().for_each(|s| *s = NoteState::Skipped);
        let consumed = map.notes.iter().map(|n| n.t).collect();
        Ride {
            map,
            x: 0.0,
            target: 0.0,
            vx: 0.0,
            score: 0,
            combo: 0,
            best_combo: 0,
            caught: 0,
            missed: 0,
            greys: 0,
            next,
            start: t,
            last_t: t,
            events: Vec::new(),
            states,
            consumed,
            holding: None,
        }
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

    pub fn state(&self, i: usize) -> NoteState {
        self.states.get(i).copied().unwrap_or(NoteState::Pending)
    }

    /// Song time up to which hold `i`'s ribbon has been ridden away (its start if not).
    pub fn consumed(&self, i: usize) -> f32 {
        self.consumed.get(i).copied().unwrap_or(0.0)
    }

    /// Pay out the hold being ridden up to `t`, or end it (done, or left).
    fn ride_hold(&mut self, t: f32) {
        let Some(mut h) = self.holding else { return };
        let n = self.map.notes[h.i];
        let NoteKind::Colour(c) = n.kind else {
            self.holding = None;
            return;
        };
        let upto = t.min(n.end());
        // A curved hold must be followed where it is now.
        let along = n.lane_at(upto);
        if (self.x - lane_xf(along)).abs() > HOLD_STAY {
            self.states[h.i] = NoteState::Broken;
            self.consumed[h.i] = upto;
            self.events.push(Event::HoldBroken { t: upto, lane: along.round() as u8 });
            self.holding = None;
            return;
        }
        let tick = (POINTS[c as usize] / 10).max(1) * self.multiplier();
        while h.ticked_to + TICK <= upto + 1e-4 {
            h.ticked_to += TICK;
            h.earned += tick;
            self.score += tick as u64;
        }
        self.consumed[h.i] = upto;
        if t >= n.end() {
            let bonus = POINTS[c as usize] * self.multiplier();
            h.earned += bonus;
            self.score += bonus as u64;
            self.combo += 1;
            self.best_combo = self.best_combo.max(self.combo);
            self.states[h.i] = NoteState::Held;
            self.events.push(Event::HoldDone { t: n.end(), lane: n.end_lane(), colour: c, points: h.earned });
            self.holding = None;
        } else {
            self.holding = Some(h);
        }
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

        // A seek or a long stall: skip what was jumped over without judging it (a hold being
        // ridden ends there, unpaid).
        if t < self.last_t - 0.3 || t > self.last_t + 1.0 {
            let to = self.map.notes.partition_point(|n| n.t < t + 0.5);
            for i in self.next.min(to)..to {
                if self.states[i] == NoteState::Pending {
                    self.states[i] = NoteState::Skipped;
                }
            }
            self.next = to;
            if let Some(h) = self.holding.take() {
                self.states[h.i] = NoteState::Broken;
            }
        }
        self.last_t = t;
        self.events.retain(|e| t - event_t(e) < 1.0 && event_t(e) <= t + 0.1);
        self.ride_hold(t);

        // Judge every note in its window (see EARLY and LATE). Greys are judged right at their
        // time; a dodged grey counts as passed.
        let mut i = self.next;
        while let Some(n) = self.map.notes.get(i).copied() {
            if n.t > t + EARLY {
                break;
            }
            if self.states[i] == NoteState::Pending {
                let dx = (self.x - lane_x(n.lane)).abs();
                match n.kind {
                    NoteKind::Colour(c) if dx < CATCH => self.catch(i, c, t),
                    NoteKind::Colour(_) if t > n.t + LATE => {
                        self.combo = 0;
                        self.missed += 1;
                        self.states[i] = NoteState::Missed;
                        self.events.push(Event::Missed { t: n.t, lane: n.lane });
                    }
                    NoteKind::Grey if n.t <= t && dx < GREY_CATCH => {
                        self.combo = 0;
                        self.greys += 1;
                        self.score = self.score.saturating_sub(GREY_PENALTY as u64);
                        self.states[i] = NoteState::GreyHit;
                        self.events.push(Event::Grey { t: n.t, lane: n.lane });
                    }
                    NoteKind::Grey if n.t <= t => self.states[i] = NoteState::Missed,
                    _ => {}
                }
            }
            i += 1;
        }
        while self.next < self.states.len() && self.states[self.next] != NoteState::Pending {
            self.next += 1;
        }
    }

    /// Note `i` (colour `c`) was caught at song time `t`.
    fn catch(&mut self, i: usize, c: u8, t: f32) {
        let n = self.map.notes[i];
        let points = POINTS[c as usize] * self.multiplier();
        self.combo += 1;
        self.best_combo = self.best_combo.max(self.combo);
        self.caught += 1;
        self.score += points as u64;
        self.events.push(Event::Caught { t: n.t, lane: n.lane, colour: c, points });
        if n.is_hold() {
            self.states[i] = NoteState::Holding;
            // Pay from the note's own time, even when caught a moment early.
            self.holding = Some(Holding { i, ticked_to: n.t, earned: points });
            self.ride_hold(t);
        } else {
            self.states[i] = NoteState::Caught;
        }
    }
}

fn event_t(e: &Event) -> f32 {
    match *e {
        Event::Caught { t, .. } | Event::Missed { t, .. } | Event::Grey { t, .. } | Event::HoldDone { t, .. } | Event::HoldBroken { t, .. } => t,
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
        Note::tap(t, lane, kind)
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
        assert_eq!([r.state(0), r.state(1), r.state(2), r.state(3)], [NoteState::Caught, NoteState::Caught, NoteState::Missed, NoteState::GreyHit]);
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

    fn hold(t: f32, lane: u8, len: f32) -> Note {
        Note { len, ..Note::tap(t, lane, NoteKind::Colour(4)) }
    }

    #[test]
    fn a_hold_ridden_to_the_end_pays_as_it_goes_and_a_bonus() {
        let mut r = Ride::new(map(vec![hold(3.0, 2, 1.0)]), 0.0);
        ride_on_lane(&mut r, 0.0, 3.5, 2);
        assert_eq!(r.state(0), NoteState::Holding);
        assert!(r.holding.is_some() && (r.consumed(0) - 3.5).abs() < 0.02, "ribbon ridden to {}", r.consumed(0));
        let mid = r.score;
        assert!(mid > 100, "head plus ticks so far: {mid}");
        ride_on_lane(&mut r, 3.5, 4.5, 2);
        assert_eq!(r.state(0), NoteState::Held);
        assert!(r.holding.is_none());
        // Head 100, ten ticks of 10, bonus 100.
        assert_eq!(r.score, 300);
        assert_eq!((r.caught, r.combo), (1, 2), "the head and the finished hold both count for the combo");
        assert!(r.events.iter().any(|e| matches!(e, Event::HoldDone { points: 300, .. })));
    }

    /// Ride at 60 fps from `from` to `until`, steering wherever `lane(t)` says.
    fn ride_along(r: &mut Ride, from: f32, until: f32, lane: impl Fn(f32) -> f32) {
        let mut t = from;
        while t < until {
            t += 1.0 / 60.0;
            // Aim a little ahead, as a player would.
            let p = Some(lane_xf(lane(t + 0.08)) / 2.4 + 0.5);
            r.update(t, Input { pointer: p, ..Default::default() }, 1.0 / 60.0);
        }
    }

    #[test]
    fn a_curved_hold_is_held_by_following_it() {
        let curved = || {
            let mut n = hold(3.0, 1, 2.0);
            let m = crate::game::mapgen::generate_bends_for_test(&[(3.8, 2), (4.5, 3)]);
            n.bends = m;
            n
        };
        let mut r = Ride::new(map(vec![curved()]), 0.0);
        let path = curved();
        ride_along(&mut r, 0.0, 5.5, |t| path.lane_at(t));
        assert_eq!(r.state(0), NoteState::Held, "followed the curve to the end");
        assert!(r.events.iter().any(|e| matches!(e, Event::HoldDone { lane: 3, .. })), "it ends in the last lane");
        // Staying in the starting lane breaks it at the first bend.
        let mut s = Ride::new(map(vec![curved()]), 0.0);
        ride_on_lane(&mut s, 0.0, 5.5, 1);
        assert_eq!(s.state(0), NoteState::Broken);
        assert!(s.consumed(0) > 3.5 && s.consumed(0) < 3.85, "broken where the slide starts: {}", s.consumed(0));
    }

    #[test]
    fn leaving_a_hold_breaks_it_without_breaking_the_combo() {
        let mut r = Ride::new(map(vec![hold(3.0, 2, 2.0), note(6.0, 2, NoteKind::Colour(4))]), 0.0);
        ride_on_lane(&mut r, 0.0, 3.6, 2);
        let before = r.score;
        ride_on_lane(&mut r, 3.6, 4.0, 0);
        assert_eq!(r.state(0), NoteState::Broken);
        assert!(r.score < before + 60, "no more pay once off the lane");
        assert!(r.consumed(0) > 3.5 && r.consumed(0) < 4.0);
        assert_eq!(r.combo, 1);
        ride_on_lane(&mut r, 4.0, 6.5, 2);
        assert_eq!(r.combo, 2);
        // Missing a hold's head is an ordinary miss; nothing is paid.
        let mut m = Ride::new(map(vec![hold(3.0, 0, 1.0)]), 0.0);
        ride_on_lane(&mut m, 0.0, 4.5, 4);
        assert_eq!((m.state(0), m.score, m.missed), (NoteState::Missed, 0, 1));
    }

    #[test]
    fn a_ride_from_the_start_of_a_song_judges_every_note() {
        // Carrying on into the next song: the ride starts a fraction of a second in.
        let notes = (0..8).map(|k| note(0.4 + k as f32 * 0.4, 2, NoteKind::Colour(1))).collect();
        let mut r = Ride::new(map(notes), 0.15);
        ride_on_lane(&mut r, 0.15, 3.6, 2);
        assert_eq!((r.caught, r.missed), (8, 0), "no run-up at a song's start");
        // Started mid-song, the run-up's notes are skipped: not judged and not drawn.
        let notes = (0..40).map(|k| note(k as f32 * 0.5, 2, NoteKind::Colour(1))).collect();
        let r = Ride::new(map(notes), 10.1);
        assert!((0..25).all(|i| r.state(i) == NoteState::Skipped), "before the start and in the run-up");
        assert_eq!(r.state(25), NoteState::Pending);
    }

    #[test]
    fn a_block_is_caught_when_the_craft_meets_it_not_only_at_its_centre() {
        // Leaving the lane just before the note's time: the nose already went through it.
        let mut r = Ride::new(map(vec![note(3.0, 2, NoteKind::Colour(2))]), 0.0);
        ride_on_lane(&mut r, 0.0, 2.95, 2);
        ride_on_lane(&mut r, 2.95, 3.5, 4);
        assert_eq!((r.caught, r.missed), (1, 0));
        // A wing over the block (1.2 m off its centre) catches it; half a lane further doesn't.
        let at = |dx: f32| {
            let mut r = Ride::new(map(vec![note(3.0, 2, NoteKind::Colour(2))]), 0.0);
            let p = Some((lane_x(2) + dx) / 2.4 + 0.5);
            let mut t = 0.0;
            while t < 3.5 {
                t += 1.0 / 60.0;
                r.update(t, Input { pointer: p, ..Default::default() }, 1.0 / 60.0);
            }
            r.caught
        };
        assert_eq!(at(0.24), 1);
        assert_eq!(at(-0.24), 1);
        assert_eq!(at(0.4), 0, "the next lane over");
        // A grey grazed at the same distance is not a hit.
        let mut g = Ride::new(map(vec![note(3.0, 2, NoteKind::Grey)]), 0.0);
        let p = Some((lane_x(2) + 0.22) / 2.4 + 0.5);
        let mut t = 0.0;
        while t < 3.5 {
            t += 1.0 / 60.0;
            g.update(t, Input { pointer: p, ..Default::default() }, 1.0 / 60.0);
        }
        assert_eq!((g.greys, g.state(0)), (0, NoteState::Missed));
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
