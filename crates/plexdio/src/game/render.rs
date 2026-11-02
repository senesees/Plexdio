//! Draws a ride into a retrovis [`Canvas`]: the visualiser (dimmed) as the background, then the
//! road in 3D with a chase camera, the notes as glowing blocks, the craft and hit bursts. Flat
//! shading and painter's order, PS1 style, all on the CPU.

use super::mapgen::{Map, Note, NoteKind, COLOURS, HOLD_MAX, LANES, ROAD_HZ};
use super::play::{lane_x, lane_xf, Event, NoteState, Ride};
use crate::ui::retrovis::canvas::{Canvas, Rgb};
use crate::ui::retrovis::math3d::{v3, Camera, V3};
use serde::{Deserialize, Serialize};

/// Note colours, cool to hot.
pub const PALETTE: [Rgb; COLOURS as usize] = [[0.25, 0.4, 1.0], [0.7, 0.3, 1.0], [0.15, 0.95, 0.5], [1.0, 0.8, 0.15], [1.0, 0.25, 0.15]];
const GREY: Rgb = [0.45, 0.45, 0.5];

/// Road half-width in metres, and metres per unit of steering (the five lanes sit 2 m apart,
/// out to ±0.8 of it).
const HALF_WIDTH: f32 = 5.6;
const STEER_M: f32 = 5.0;
/// How opaque the road is over the visualiser.
const ROAD_ALPHA: f32 = 0.96;
/// How long hit effects last, in seconds.
const BURST: f32 = 0.6;
/// How far ahead and behind the craft the road is drawn, and the segment length.
const AHEAD: f32 = 230.0;
const BEHIND: f32 = 12.0;
const SEG: f32 = 3.0;
/// Camera: metres behind and above the craft, and how far it looks down.
const CAM_BACK: f32 = 7.0;
const CAM_UP: f32 = 2.8;
const CAM_TILT: f32 = 0.12;

/// The road's centre line in the world, built once per map by integrating turns and climbs.
pub struct RoadGeom {
    /// Points at the map's road samples.
    pts: Vec<V3>,
    heading: Vec<f32>,
    dist: Vec<f32>,
}

/// A place on the road: position, heading (radians, 0 = +Z) and pitch (radians, up positive).
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub pos: V3,
    pub heading: f32,
    pub pitch: f32,
}

impl Frame {
    fn right(&self) -> V3 {
        v3(self.heading.cos(), 0.0, -self.heading.sin())
    }
}

impl RoadGeom {
    pub fn new(map: &Map) -> Self {
        let n = map.road.len().max(2);
        let (mut pts, mut heading, mut dist) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
        let (mut p, mut h, mut d) = (v3(0.0, 0.0, 0.0), 0.0f32, 0.0f32);
        for i in 0..n {
            let r = map.road.get(i).copied().unwrap_or_default();
            pts.push(p);
            heading.push(h);
            dist.push(d);
            let ds = r.speed.max(1.0) / ROAD_HZ;
            h += r.curve / ROAD_HZ;
            p = p.add(v3(h.sin() * ds, r.slope * ds, h.cos() * ds));
            d += ds;
        }
        RoadGeom { pts, heading, dist }
    }

    /// Length of the road in metres.
    pub fn length(&self) -> f32 {
        self.dist.last().copied().unwrap_or(0.0)
    }

    /// The road at distance `d` metres (extended straight past either end).
    pub fn at(&self, d: f32) -> Frame {
        let n = self.dist.len();
        let i = self.dist.partition_point(|&x| x <= d).clamp(1, n - 1);
        let (d0, d1) = (self.dist[i - 1], self.dist[i]);
        let f = if d1 > d0 { (d - d0) / (d1 - d0) } else { 0.0 };
        let pos = self.pts[i - 1].lerp(self.pts[i], f);
        let heading = self.heading[i - 1] + (self.heading[i] - self.heading[i - 1]) * f;
        let seg = self.pts[i].sub(self.pts[i - 1]);
        let flat = (seg.x * seg.x + seg.z * seg.z).sqrt().max(1e-3);
        Frame { pos, heading, pitch: (seg.y / flat).atan() }
    }
}

/// Everything needed to turn world points into canvas points.
struct View {
    cam: Camera,
    pos: V3,
    yaw: f32,
    pitch: f32,
}

impl View {
    fn project(&self, p: V3) -> Option<(f32, f32, f32)> {
        let q = p.sub(self.pos).rot(0.0, -self.yaw, 0.0).rot(self.pitch, 0.0, 0.0);
        self.cam.project(q)
    }
}

/// Draw one frame of `ride` at song time `t` into `out`, over `background` (the visualiser).
pub fn draw(out: &mut Canvas, background: &Canvas, ride: &Ride, geom: &RoadGeom, t: f32, craft: Craft) {
    copy_dimmed(out, background, 0.45);
    let map = &ride.map;
    let d_ship = map.distance_at(t);
    let ship = geom.at(d_ship);
    let road = map.road_at(t);
    // Loud, fast parts rush: a wider view from lower and closer, a fine shake, streaks.
    let rush = smoothstep(0.5, 0.95, road.intensity);
    // The camera sits behind the craft, follows its steering a little and looks along the road.
    let d_cam = d_ship - CAM_BACK + 1.2 * rush;
    let behind = geom.at(d_cam);
    // Nothing nearer than this along the road is drawn: it would be behind the camera.
    let near = d_cam + 0.5;
    let lat = ride.x * STEER_M;
    // Hitting a grey shakes the camera for a moment.
    let shake = ride
        .events
        .iter()
        .filter_map(|e| if let Event::Grey { t: te, .. } = *e { Some(t - te) } else { None })
        .filter(|age| (0.0..0.35).contains(age))
        .map(|age| (1.0 - age / 0.35) * 0.25 * (age * 70.0).sin())
        .sum::<f32>();
    let buzz = rush * rush * 0.035 * ((t * 53.0).sin() + 0.6 * (t * 37.0).sin());
    let cam_pos = behind.pos.add(v3(0.0, CAM_UP - 0.6 * rush + shake * 0.5 + buzz, 0.0)).add(behind.right().mul(lat * 0.55 + shake));
    // The view widens with the music: a little from medium intensity, a lot when it rushes
    // (focal 1.25 → 0.82: about 77° → 100° vertically).
    let focal = 1.25 - 0.1 * smoothstep(0.3, 0.6, road.intensity) - 0.33 * rush;
    let view = View { cam: Camera { focal, ..Camera::new(out.w, out.h) }, pos: cam_pos, yaw: ship.heading, pitch: ship.pitch - CAM_TILT + 0.03 * rush };

    let tint = intensity_colour(road.intensity);
    // The edges flash on every beat, and brighter on every catch.
    let since_beat = map.beats.iter().rev().find(|&&b| b <= t).map_or(9.0, |b| t - b);
    let since_catch = ride.events.iter().rev().find_map(|e| if let Event::Caught { t: te, .. } = *e { Some(t - te) } else { None }).filter(|a| *a >= 0.0).unwrap_or(9.0);
    let pulse = (-since_beat * 7.0).exp() + 1.5 * (-since_catch * 9.0).exp();

    // Road, far to near.
    let first = ((d_ship - BEHIND) / SEG).floor() as i64;
    // The road ends with the song.
    let last = ((d_ship + AHEAD).min(geom.length()) / SEG).ceil() as i64;
    let mut edges_l: Vec<(f32, f32)> = Vec::new();
    let mut edges_r: Vec<(f32, f32)> = Vec::new();
    for k in (first..last).rev() {
        // The segment the camera is over is clipped at the camera, not dropped: dropping it left
        // a gap at the bottom of the picture when the view is low and wide at speed.
        let (d0, d1) = ((k as f32 * SEG).max(near), (k + 1) as f32 * SEG);
        if d1 <= d0 {
            continue;
        }
        let (f0, f1) = (geom.at(d0), geom.at(d1));
        let corners = [
            f0.pos.add(f0.right().mul(-HALF_WIDTH)),
            f0.pos.add(f0.right().mul(HALF_WIDTH)),
            f1.pos.add(f1.right().mul(HALF_WIDTH)),
            f1.pos.add(f1.right().mul(-HALF_WIDTH)),
        ];
        let Some(p): Option<Vec<(f32, f32, f32)>> = corners.iter().map(|&c| view.project(c)).collect() else { continue };
        let far = ((d0 - d_ship) / AHEAD).clamp(0.0, 1.0);
        let fog = 1.0 - far * far;
        // Alternate shades give a sense of speed.
        let shade = if k.rem_euclid(2) == 0 { 1.0 } else { 0.7 };
        let c = [(0.03 + tint[0] * 0.12) * shade * fog, (0.03 + tint[1] * 0.12) * shade * fog, (0.06 + tint[2] * 0.14) * shade * fog];
        out.tri_flat([(p[0].0, p[0].1), (p[1].0, p[1].1), (p[2].0, p[2].1)], c, ROAD_ALPHA);
        out.tri_flat([(p[0].0, p[0].1), (p[2].0, p[2].1), (p[3].0, p[3].1)], c, ROAD_ALPHA);
        edges_l.push((p[0].0, p[0].1));
        edges_r.push((p[1].0, p[1].1));
        // Lane dividers (halfway between lanes), dashed.
        if k.rem_euclid(3) != 0 {
            for i in 0..LANES - 1 {
                let m = (lane_x(i) + lane_x(i + 1)) / 2.0 * STEER_M;
                if let (Some(a), Some(b)) = (view.project(f0.pos.add(f0.right().mul(m))), view.project(f1.pos.add(f1.right().mul(m)))) {
                    out.line(a.0, a.1, b.0, b.1, tint, 0.5 * fog, 1.0);
                }
            }
        }
    }
    let edge = 0.6 + 1.2 * pulse;
    out.polyline(&edges_l, tint, edge, 2.0, false);
    out.polyline(&edges_r, tint, edge, 2.0, false);
    posts_and_streaks(out, &view, geom, d_ship, rush, tint, pulse);

    // Notes, far to near: everything ahead, plus missed notes until they are behind the camera
    // (caught ones are gone). A hold whose head has passed still shows while its tail is ahead.
    let from = ride.next_note().min(map.notes.partition_point(|n| n.t < t - HOLD_MAX - 1.0));
    let visible: Vec<usize> = (from..map.notes.len())
        .take_while(|&i| map.distance_at(map.notes[i].t) < d_ship + AHEAD)
        .filter(|&i| map.distance_at(map.notes[i].end()) >= near)
        .collect();
    // Hold ribbons lie on the road, under every block.
    for &i in visible.iter().rev() {
        let n = map.notes[i];
        let NoteKind::Colour(c) = n.kind else { continue };
        if !n.is_hold() {
            continue;
        }
        let (from_t, light) = match ride.state(i) {
            NoteState::Pending => (n.t, 1.0),
            NoteState::Holding => (ride.consumed(i), 1.5 + 0.5 * (t * 18.0).sin()),
            NoteState::Missed => (n.t, 0.35),
            NoteState::Broken => (ride.consumed(i), 0.35),
            _ => continue,
        };
        ribbon(out, &view, geom, map, &n, from_t, near, PALETTE[c as usize], light, d_ship);
    }
    for &i in visible.iter().rev() {
        let n = map.notes[i];
        let state = ride.state(i);
        let (c, grey) = match n.kind {
            NoteKind::Colour(c) => (PALETTE[c as usize], false),
            NoteKind::Grey => (GREY, true),
        };
        // The head, unless caught (or hit).
        let d = map.distance_at(n.t);
        if matches!(state, NoteState::Pending | NoteState::Missed) && d >= near {
            let f = geom.at(d);
            let centre = f.pos.add(f.right().mul(lane_x(n.lane) * STEER_M));
            let far = ((d - d_ship) / AHEAD).clamp(0.0, 1.0);
            // Missed notes dim as they go past.
            let light = if d < d_ship || state == NoteState::Missed { 0.45 } else { 1.0 - far * 0.6 };
            draw_block(out, &view, centre, f, c, light, grey, 1.0);
        }
        // A hold's tail: a low cap where it ends.
        if n.is_hold() && matches!(state, NoteState::Pending | NoteState::Holding | NoteState::Missed | NoteState::Broken) {
            let d = map.distance_at(n.end());
            if d >= near && d < d_ship + AHEAD {
                let f = geom.at(d);
                let centre = f.pos.add(f.right().mul(lane_x(n.end_lane()) * STEER_M));
                let light = if matches!(state, NoteState::Missed | NoteState::Broken) { 0.4 } else { 1.0 - ((d - d_ship) / AHEAD).clamp(0.0, 1.0) * 0.6 };
                draw_block(out, &view, centre, f, c, light, false, 0.45);
            }
        }
    }

    // Hit effects travel with the craft (a burst left where the note was would fall behind the
    // camera within a tenth of a second).
    let ship_pos = ship.pos.add(ship.right().mul(lat));
    let fx = geom.at(d_ship + 1.5);
    for e in &ride.events {
        match *e {
            Event::Caught { t: te, lane, colour, .. } => {
                let age = t - te;
                if (0.0..BURST).contains(&age) {
                    let at = fx.pos.add(fx.right().mul(lane_x(lane) * STEER_M)).add(v3(0.0, 0.6, 0.0));
                    burst(out, &view, at, fx, age, PALETTE[colour as usize], te, 1.0);
                }
            }
            Event::Grey { t: te, lane } => {
                let age = t - te;
                if (0.0..BURST).contains(&age) {
                    let at = fx.pos.add(fx.right().mul(lane_x(lane) * STEER_M)).add(v3(0.0, 0.8, 0.0));
                    burst(out, &view, at, fx, age, [1.0, 0.2, 0.15], te, 0.7);
                    // A red flash over the whole picture.
                    let k = (1.0 - age / BURST).powi(2) * 0.18;
                    out.wash([1.0, 0.1, 0.05], k);
                }
            }
            Event::HoldDone { t: te, lane, colour, .. } => {
                let age = t - te;
                if (0.0..BURST).contains(&age) {
                    let at = fx.pos.add(fx.right().mul(lane_x(lane) * STEER_M)).add(v3(0.0, 0.6, 0.0));
                    burst(out, &view, at, fx, age, PALETTE[colour as usize], te + 0.5, 1.5);
                }
            }
            Event::Missed { .. } | Event::HoldBroken { .. } => {}
        }
    }
    // While holding: sparks stream back from the craft along the ribbon.
    if let Some(h) = ride.holding {
        if let NoteKind::Colour(c) = map.notes[h.i].kind {
            let col = PALETTE[c as usize];
            let fwd = v3(ship.heading.sin(), 0.0, ship.heading.cos());
            for k in 0..10 {
                let phase = (t * 3.0 + k as f32 * 0.137).fract();
                let side = ((k as f32 * 2.39).sin()) * 0.7;
                let p = ship_pos.add(fwd.mul(-0.5 - phase * 5.0)).add(ship.right().mul(side * (0.3 + phase))).add(v3(0.0, 0.3 + phase * 1.4, 0.0));
                if let Some((x, y, _)) = view.project(p) {
                    out.glow(x, y, 2.2 * (1.0 - phase) + 0.8, col, 1.3 * (1.0 - phase));
                }
            }
            if let Some((x, y, z)) = view.project(ship_pos.add(v3(0.0, 0.2, 0.0))) {
                out.glow(x, y, 26.0 / z.max(1.0), col, 0.6);
            }
        }
    }

    draw_craft(out, &view, ship, ship_pos, ride.vx, tint, craft, t);
}

/// 0 below `a`, 1 above `b`, smooth in between.
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let k = ((x - a) / (b - a)).clamp(0.0, 1.0);
    k * k * (3.0 - 2.0 * k)
}

/// Light posts along both road edges (always: they give the speed its scale), and, as the song
/// gets loud, streaks of light rushing past beside and above the road. Both are fixed in the
/// world, so they stream by at the road's speed.
fn posts_and_streaks(out: &mut Canvas, view: &View, geom: &RoadGeom, d_ship: f32, rush: f32, tint: Rgb, pulse: f32) {
    const POST: f32 = 12.0;
    let up = v3(0.0, 1.0, 0.0);
    let first = ((d_ship - CAM_BACK) / POST).ceil() as i64;
    let last = ((d_ship + AHEAD * 0.6).min(geom.length()) / POST) as i64;
    for k in (first..=last).rev() {
        let d = k as f32 * POST;
        let f = geom.at(d);
        let fog = 1.0 - ((d - d_ship) / (AHEAD * 0.6)).clamp(0.0, 1.0);
        for side in [-1.0f32, 1.0] {
            let base = f.pos.add(f.right().mul(side * (HALF_WIDTH + 0.5)));
            if let (Some(a), Some(b)) = (view.project(base), view.project(base.add(up.mul(2.2)))) {
                out.line(a.0, a.1, b.0, b.1, tint, (0.35 + 0.4 * rush) * fog, 1.0);
                out.glow(b.0, b.1, 3.0 + 6.0 * pulse * fog, tint, (0.4 + 0.6 * pulse) * fog);
            }
        }
    }
    if rush < 0.05 {
        return;
    }
    // Streaks: one candidate per 3 m of road, placed by a hash of its index.
    const CELL: f32 = 3.0;
    let length = 3.0 + 14.0 * rush;
    let first = ((d_ship - CAM_BACK) / CELL) as i64;
    let last = ((d_ship + 90.0).min(geom.length()) / CELL) as i64;
    for k in (first..=last).rev() {
        let mut h = (k as u32).wrapping_mul(0x9e37_79b9) ^ 0x5bd1_e995;
        let mut rnd = || {
            h ^= h << 13;
            h ^= h >> 17;
            h ^= h << 5;
            (h >> 8) as f32 / (1u32 << 24) as f32
        };
        if rnd() > 0.35 + 0.5 * rush {
            continue;
        }
        let d = k as f32 * CELL + rnd() * CELL;
        let side = if rnd() < 0.5 { -1.0 } else { 1.0 };
        let off = side * (HALF_WIDTH + 1.0 + rnd() * 5.0);
        let height = 0.2 + rnd() * 3.8;
        let (f0, f1) = (geom.at(d), geom.at(d - length));
        let a = f0.pos.add(f0.right().mul(off)).add(up.mul(height));
        let b = f1.pos.add(f1.right().mul(off)).add(up.mul(height));
        if let (Some(p), Some(q)) = (view.project(a), view.project(b)) {
            let fog = 1.0 - ((d - d_ship) / 90.0).clamp(0.0, 1.0);
            let c = [0.6 + 0.4 * tint[0], 0.6 + 0.4 * tint[1], 0.6 + 0.4 * tint[2]];
            out.line(p.0, p.1, q.0, q.1, c, 0.7 * rush * fog, 1.0);
        }
    }
}

/// A hold's ribbon from song time `from` to its end, following its bends: a glowing strip in its
/// note's colour, far parts first. Nothing nearer than `near` metres is drawn.
#[allow(clippy::too_many_arguments)]
fn ribbon(out: &mut Canvas, view: &View, geom: &RoadGeom, map: &Map, n: &Note, from: f32, near: f32, c: Rgb, light: f32, d_ship: f32) {
    let (t0, t1) = (from, n.end());
    if t1 <= t0 {
        return;
    }
    // Sample by time, finely enough for road segments and for the slides between lanes.
    let span = map.distance_at(t1) - map.distance_at(t0);
    let steps = ((span / SEG).ceil() as usize).max(((t1 - t0) / 0.04).ceil() as usize).clamp(1, 400);
    let pts: Vec<(f32, f32)> = (0..=steps)
        .map(|k| {
            let t = t0 + (t1 - t0) * k as f32 / steps as f32;
            (map.distance_at(t), lane_xf(n.lane_at(t)) * STEER_M)
        })
        .collect();
    let half = 0.55;
    let lift = v3(0.0, 0.06, 0.0);
    let edge = |d: f32, off: f32| {
        let f = geom.at(d);
        view.project(f.pos.add(f.right().mul(off)).add(lift))
    };
    for w in pts.windows(2).rev() {
        let ((a, oa), (b, ob)) = (w[0], w[1]);
        let a = a.max(near);
        if b <= a {
            continue;
        }
        let (Some(p0), Some(p1), Some(p2), Some(p3)) = (edge(a, oa - half), edge(a, oa + half), edge(b, ob + half), edge(b, ob - half)) else { continue };
        let fog = 1.0 - (((a - d_ship) / AHEAD).clamp(0.0, 1.0)).powi(2);
        let k = light * fog;
        // Bright enough to read the path (and its bends) well ahead.
        let fill = [c[0] * 0.75 * k, c[1] * 0.75 * k, c[2] * 0.75 * k];
        out.tri_flat([(p0.0, p0.1), (p1.0, p1.1), (p2.0, p2.1)], fill, 0.9);
        out.tri_flat([(p0.0, p0.1), (p2.0, p2.1), (p3.0, p3.1)], fill, 0.9);
        out.line(p0.0, p0.1, p3.0, p3.1, c, 1.2 * k, 1.0);
        out.line(p1.0, p1.1, p2.0, p2.1, c, 1.2 * k, 1.0);
    }
}

/// A hit: a flash, sparks flying out and falling, and a shockwave ring on the road. `seed`
/// (the note's time) makes each burst different but the same on every frame.
#[allow(clippy::too_many_arguments)]
fn burst(out: &mut Canvas, view: &View, at: V3, f: Frame, age: f32, c: Rgb, seed: f32, strength: f32) {
    let k = 1.0 - age / BURST;
    let Some((x, y, z)) = view.project(at) else { return };
    let scale = 8.0 / z.max(1.0);
    // Flash.
    if age < 0.15 {
        out.glow(x, y, (14.0 + age * 120.0) * scale, [1.0, 1.0, 1.0], 1.6 * (1.0 - age / 0.15) * strength);
    }
    out.glow(x, y, (10.0 + age * 40.0) * scale, c, 1.1 * k * strength);
    // Sparks: 18 per hit, out and up, then falling.
    let mut h = (seed * 1000.0) as u32 ^ 0x9e37_79b9;
    let mut rnd = || {
        h ^= h << 13;
        h ^= h >> 17;
        h ^= h << 5;
        (h >> 8) as f32 / (1u32 << 24) as f32
    };
    let (r, fwd) = (f.right(), v3(f.heading.sin(), 0.0, f.heading.cos()));
    for _ in 0..18 {
        let (a, up, speed) = (rnd() * std::f32::consts::TAU, 2.0 + rnd() * 5.0, 4.0 + rnd() * 8.0);
        let v = r.mul(a.cos() * speed).add(fwd.mul(a.sin() * speed * 0.6));
        let p = at.add(v.mul(age)).add(v3(0.0, up * age - 9.0 * age * age, 0.0));
        if let Some((sx, sy, _)) = view.project(p) {
            out.glow(sx, sy, 1.6 + 2.0 * k, c, 1.4 * k * strength);
        }
    }
    // Shockwave ring on the road surface.
    let radius = 0.5 + age * 9.0;
    let ring: Vec<(f32, f32)> = (0..24)
        .filter_map(|i| {
            let a = i as f32 / 24.0 * std::f32::consts::TAU;
            view.project(at.add(r.mul(a.cos() * radius)).add(fwd.mul(a.sin() * radius)).add(v3(0.0, -0.5, 0.0))).map(|q| (q.0, q.1))
        })
        .collect();
    if ring.len() == 24 {
        out.polyline(&ring, c, 0.9 * k * strength, 1.0, true);
    }
}

/// The background, darkened so the road reads on top (scaled when sizes differ).
fn copy_dimmed(out: &mut Canvas, bg: &Canvas, k: f32) {
    if bg.w == 0 || bg.h == 0 {
        out.clear();
        return;
    }
    let xmap: Vec<usize> = (0..out.w).map(|x| x * bg.w / out.w).collect();
    for y in 0..out.h {
        let src = &bg.px[(y * bg.h / out.h) * bg.w..][..bg.w];
        for (d, &sx) in out.px[y * out.w..(y + 1) * out.w].iter_mut().zip(&xmap) {
            let s = src[sx];
            *d = [s[0] * k, s[1] * k, s[2] * k];
        }
    }
}

/// The road's colour for an intensity: cool when calm, hot when intense.
fn intensity_colour(v: f32) -> Rgb {
    let x = (v.clamp(0.0, 1.0) * (COLOURS - 1) as f32).min((COLOURS - 1) as f32 - 0.001);
    let (i, f) = (x as usize, x.fract());
    let (a, b) = (PALETTE[i], PALETTE[i + 1]);
    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]
}

/// A note block: a box sitting on the road, flat-shaded, with a glow (greys don't glow).
/// `height` scales the block (a hold's tail cap is low).
#[allow(clippy::too_many_arguments)]
fn draw_block(out: &mut Canvas, view: &View, centre: V3, f: Frame, c: Rgb, light: f32, grey: bool, height: f32) {
    let (w, h, l) = (1.6, if grey { 1.8 } else { 1.0 } * height, 1.5);
    let r = f.right();
    let fwd = v3(f.heading.sin(), 0.0, f.heading.cos());
    let up = v3(0.0, 1.0, 0.0);
    let corner = |sx: f32, sy: f32, sz: f32| centre.add(r.mul(sx * w / 2.0)).add(up.mul(sy * h + 0.1)).add(fwd.mul(sz * l / 2.0));
    let Some(p): Option<Vec<(f32, f32, f32)>> = [
        corner(-1.0, 0.0, -1.0),
        corner(1.0, 0.0, -1.0),
        corner(1.0, 1.0, -1.0),
        corner(-1.0, 1.0, -1.0),
        corner(-1.0, 1.0, 1.0),
        corner(1.0, 1.0, 1.0),
    ]
    .iter()
    .map(|&q| view.project(q))
    .collect() else {
        return;
    };
    let xy = |i: usize| (p[i].0, p[i].1);
    let face = |k: f32| [c[0] * k * light, c[1] * k * light, c[2] * k * light];
    if !grey {
        let mid = ((p[0].0 + p[5].0) / 2.0, (p[0].1 + p[5].1) / 2.0);
        out.glow(mid.0, mid.1, (p[1].0 - p[0].0).abs() * 1.1, c, 0.35 * light);
    }
    // Top, then the front face (towards the camera).
    out.tri_flat([xy(3), xy(2), xy(5)], face(1.0), 1.0);
    out.tri_flat([xy(3), xy(5), xy(4)], face(1.0), 1.0);
    out.tri_flat([xy(0), xy(1), xy(2)], face(0.7), 1.0);
    out.tri_flat([xy(0), xy(2), xy(3)], face(0.7), 1.0);
}

/// What you ride.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Craft {
    /// A low-poly hover arrowhead (the Audiosurf look).
    #[default]
    Ship,
    Car,
    Plane,
}

impl Craft {
    pub const ALL: [Craft; 3] = [Craft::Ship, Craft::Car, Craft::Plane];

    pub fn label(self) -> &'static str {
        match self {
            Craft::Ship => "Hover ship",
            Craft::Car => "Car",
            Craft::Plane => "Plane",
        }
    }
}

/// A craft model: local vertices (x right, y up, z forward, metres), coloured triangles, how high
/// it rides, how much it banks, and where its engine glows.
struct Mesh {
    v: Vec<V3>,
    f: Vec<([usize; 3], Rgb)>,
    hover: f32,
    bank: f32,
    engine: V3,
}

impl Mesh {
    /// Add an axis-aligned box centred on `c` with size `s`.
    fn cuboid(&mut self, c: (f32, f32, f32), s: (f32, f32, f32), col: Rgb) {
        let base = self.v.len();
        for i in 0..8 {
            let (dx, dy, dz) = ((i & 1) as f32 - 0.5, ((i >> 1) & 1) as f32 - 0.5, ((i >> 2) & 1) as f32 - 0.5);
            self.v.push(v3(c.0 + dx * s.0, c.1 + dy * s.1, c.2 + dz * s.2));
        }
        for q in [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]] {
            self.f.push(([base + q[0], base + q[1], base + q[2]], col));
            self.f.push(([base + q[0], base + q[2], base + q[3]], col));
        }
    }
}

const BODY: Rgb = [0.85, 0.9, 1.0];

fn mesh(craft: Craft) -> Mesh {
    match craft {
        Craft::Ship => {
            let v = [(0.0, 0.35, 2.2), (-1.5, 0.15, -1.0), (1.5, 0.15, -1.0), (0.0, 0.75, -0.8), (0.0, 0.0, -1.1), (0.0, 0.3, -1.2)];
            let f = [[0, 1, 3], [0, 3, 2], [0, 4, 1], [0, 2, 4], [1, 5, 3], [3, 5, 2]];
            Mesh { v: v.iter().map(|&(x, y, z)| v3(x, y, z)).collect(), f: f.iter().map(|&t| (t, BODY)).collect(), hover: 0.6, bank: 0.25, engine: v3(0.0, 0.3, -1.6) }
        }
        Craft::Car => {
            let mut m = Mesh { v: Vec::new(), f: Vec::new(), hover: 0.0, bank: 0.06, engine: v3(0.0, 0.5, -1.95) };
            let tyre = [0.12, 0.12, 0.14];
            for (x, z) in [(-0.95, 1.15), (0.95, 1.15), (-0.95, -1.15), (0.95, -1.15)] {
                m.cuboid((x, 0.32, z), (0.32, 0.64, 0.72), tyre);
            }
            m.cuboid((0.0, 0.55, 0.0), (1.8, 0.5, 3.6), BODY);
            m.cuboid((0.0, 1.0, -0.3), (1.4, 0.45, 1.7), [0.35, 0.5, 0.75]);
            m
        }
        Craft::Plane => {
            let mut m = Mesh { v: Vec::new(), f: Vec::new(), hover: 1.5, bank: 0.45, engine: v3(0.0, 0.5, -2.0) };
            m.cuboid((0.0, 0.5, 0.0), (0.7, 0.6, 3.4), BODY);
            // Nose cone.
            let base = m.v.len();
            m.v.extend([v3(-0.35, 0.2, 1.7), v3(0.35, 0.2, 1.7), v3(0.35, 0.8, 1.7), v3(-0.35, 0.8, 1.7), v3(0.0, 0.45, 2.5)]);
            for (a, b) in [(0, 1), (1, 2), (2, 3), (3, 0)] {
                m.f.push(([base + a, base + b, base + 4], BODY));
            }
            m.cuboid((0.0, 0.5, 0.2), (4.6, 0.1, 1.0), [0.75, 0.8, 0.95]);
            m.cuboid((0.0, 0.55, -1.5), (1.7, 0.08, 0.5), [0.75, 0.8, 0.95]);
            m.cuboid((0.0, 1.0, -1.5), (0.1, 0.8, 0.6), [0.75, 0.8, 0.95]);
            m
        }
    }
}

/// The craft, banking with the steering (the plane also bobs), with an engine glow.
#[allow(clippy::too_many_arguments)]
fn draw_craft(out: &mut Canvas, view: &View, f: Frame, pos: V3, vx: f32, tint: Rgb, craft: Craft, t: f32) {
    let m = mesh(craft);
    let bank = (-vx * m.bank).clamp(-0.6, 0.6);
    let r = f.right();
    let fwd = v3(f.heading.sin(), 0.0, f.heading.cos());
    let up = v3(0.0, 1.0, 0.0);
    let hover = m.hover + if craft == Craft::Plane { 0.15 * (t * 2.1).sin() } else { 0.0 };
    let place = |q: V3| {
        let q = q.rot(0.0, 0.0, bank);
        pos.add(r.mul(q.x)).add(up.mul(q.y + hover)).add(fwd.mul(q.z))
    };
    let world: Vec<V3> = m.v.iter().map(|&q| place(q)).collect();
    let Some(p): Option<Vec<(f32, f32, f32)>> = world.iter().map(|&q| view.project(q)).collect() else { return };
    let light = v3(0.4, 0.8, -0.45).norm();
    let mut faces: Vec<(f32, [usize; 3], Rgb)> = m
        .f
        .iter()
        .map(|&([a, b, c], col)| {
            let n = world[b].sub(world[a]).cross(world[c].sub(world[a])).norm();
            let k = 0.35 + 0.65 * n.dot(light).abs();
            ((p[a].2 + p[b].2 + p[c].2) / 3.0, [a, b, c], [col[0] * k, col[1] * k, col[2] * k])
        })
        .collect();
    // Far faces first.
    faces.sort_by(|x, y| y.0.total_cmp(&x.0));
    for (_, [a, b, c], col) in faces {
        out.tri_flat([(p[a].0, p[a].1), (p[b].0, p[b].1), (p[c].0, p[c].1)], col, 1.0);
    }
    if let Some((x, y, z)) = view.project(place(m.engine)) {
        let glow = if craft == Craft::Car { [1.0, 0.15, 0.1] } else { tint };
        out.glow(x, y, 30.0 / z.max(1.0), glow, 0.9);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::analysis::{analyse, tests::drums};
    use crate::game::mapgen::{generate, Difficulty};

    #[test]
    fn the_road_starts_at_the_origin_and_follows_the_map() {
        let (s, _) = drums(124.0, 40.0, 10.0);
        let map = generate(&analyse(&s, None), Difficulty::Pro, 5);
        let g = RoadGeom::new(&map);
        let start = g.at(0.0);
        assert!(start.pos.len() < 1e-3);
        // The total length matches the map's distance.
        let end = map.distance.last().copied().unwrap();
        assert!((g.dist.last().copied().unwrap() - end).abs() < 1.0);
        // The calm intro climbs.
        assert!(g.at(map.distance_at(5.0)).pitch > 0.0);
    }

    #[test]
    fn the_road_reaches_the_bottom_of_the_picture_at_full_speed() {
        // Loud all the way: full rush (lowest, widest view), over a bright background.
        let (s, _) = drums(140.0, 40.0, 0.0);
        let map = generate(&analyse(&s, None), Difficulty::Elite, 5);
        let g = RoadGeom::new(&map);
        let ride = Ride::new(map, 0.0);
        let mut bg = Canvas::new(320, 180);
        bg.wash([1.0, 1.0, 1.0], 1.0);
        for craft in Craft::ALL {
            for t in [10.0, 20.3, 31.7] {
                let mut out = Canvas::new(480, 270);
                draw(&mut out, &bg, &ride, &g, t, craft);
                for y in [out.h - 1, out.h - 3] {
                    let row = &out.px[y * out.w..(y + 1) * out.w];
                    // The dimmed white background is a neutral grey of 0.45 (lines on the road are
                    // tinted, the road itself is dark).
                    let holes = row[out.w / 5..out.w * 4 / 5].iter().filter(|p| p.iter().all(|c| (c - 0.45).abs() < 0.08)).count();
                    assert_eq!(holes, 0, "{craft:?} at {t} s: the background shows through the road at row {y}");
                }
            }
        }
    }

    #[test]
    fn a_frame_draws_road_notes_and_craft() {
        let (s, _) = drums(124.0, 40.0, 0.0);
        let map = generate(&analyse(&s, None), Difficulty::Pro, 5);
        let g = RoadGeom::new(&map);
        let ride = Ride::new(map, 10.0);
        let bg = Canvas::new(320, 180);
        let mut out = Canvas::new(480, 270);
        draw(&mut out, &bg, &ride, &g, 13.0, Craft::Ship);
        for craft in [Craft::Car, Craft::Plane] {
            let mut other = Canvas::new(480, 270);
            draw(&mut other, &bg, &ride, &g, 13.0, craft);
            assert_ne!(other.px, out.px, "{craft:?} looks different");
        }
        let lit = out.px.iter().filter(|p| p[0] + p[1] + p[2] > 0.05).count();
        assert!(lit > out.px.len() / 10, "only {lit} pixels drawn");
        // The craft sits low in the middle of the picture: bright pixels there.
        let craft = (out.h * 55 / 100..out.h * 85 / 100)
            .flat_map(|y| (out.w * 40 / 100..out.w * 60 / 100).map(move |x| (x, y)))
            .filter(|&(x, y)| out.px[y * out.w + x].iter().sum::<f32>() > 1.5)
            .count();
        assert!(craft > 200, "only {craft} bright pixels where the craft should be");
    }
}
