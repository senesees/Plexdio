//! Draws a ride into a retrovis [`Canvas`]: the visualiser (dimmed) as the background, then the
//! road in 3D with a chase camera, the notes as glowing blocks, the craft and hit bursts. Flat
//! shading and painter's order, PS1 style, all on the CPU.

use super::mapgen::{Map, NoteKind, COLOURS, ROAD_HZ};
use super::play::{lane_x, Event, Ride};
use crate::ui::retrovis::canvas::{Canvas, Rgb};
use crate::ui::retrovis::math3d::{v3, Camera, V3};

/// Note colours, cool to hot.
pub const PALETTE: [Rgb; COLOURS as usize] = [[0.25, 0.4, 1.0], [0.7, 0.3, 1.0], [0.15, 0.95, 0.5], [1.0, 0.8, 0.15], [1.0, 0.25, 0.15]];
const GREY: Rgb = [0.45, 0.45, 0.5];

/// Road half-width in metres, and metres per unit of steering (lanes sit at ±0.7 of it).
const HALF_WIDTH: f32 = 5.0;
const STEER_M: f32 = 4.2;
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
pub fn draw(out: &mut Canvas, background: &Canvas, ride: &Ride, geom: &RoadGeom, t: f32) {
    copy_dimmed(out, background, 0.45);
    let map = &ride.map;
    let d_ship = map.distance_at(t);
    let ship = geom.at(d_ship);
    // The camera sits behind the craft, follows its steering a little and looks along the road.
    let behind = geom.at(d_ship - CAM_BACK);
    let lat = ride.x * STEER_M;
    let cam_pos = behind.pos.add(v3(0.0, CAM_UP, 0.0)).add(behind.right().mul(lat * 0.55));
    let view = View { cam: Camera { focal: 1.25, ..Camera::new(out.w, out.h) }, pos: cam_pos, yaw: ship.heading, pitch: ship.pitch - CAM_TILT };

    let road = map.road_at(t);
    let tint = intensity_colour(road.intensity);
    // The edges flash on every beat.
    let since_beat = map.beats.iter().rev().find(|&&b| b <= t).map_or(9.0, |b| t - b);
    let pulse = (-since_beat * 7.0).exp();

    // Road, far to near.
    let first = ((d_ship - BEHIND) / SEG).floor() as i64;
    // The road ends with the song.
    let last = ((d_ship + AHEAD).min(geom.length()) / SEG).ceil() as i64;
    let mut edges_l: Vec<(f32, f32)> = Vec::new();
    let mut edges_r: Vec<(f32, f32)> = Vec::new();
    for k in (first..last).rev() {
        let (d0, d1) = (k as f32 * SEG, (k + 1) as f32 * SEG);
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
        out.tri_flat([(p[0].0, p[0].1), (p[1].0, p[1].1), (p[2].0, p[2].1)], c, 0.8);
        out.tri_flat([(p[0].0, p[0].1), (p[2].0, p[2].1), (p[3].0, p[3].1)], c, 0.8);
        edges_l.push((p[0].0, p[0].1));
        edges_r.push((p[1].0, p[1].1));
        // Lane dividers, dashed.
        if k.rem_euclid(3) != 0 {
            for side in [-0.35f32, 0.35] {
                let m = side * STEER_M * 2.0;
                if let (Some(a), Some(b)) = (view.project(f0.pos.add(f0.right().mul(m))), view.project(f1.pos.add(f1.right().mul(m)))) {
                    out.line(a.0, a.1, b.0, b.1, tint, 0.5 * fog, 1.0);
                }
            }
        }
    }
    let edge = 0.6 + 1.2 * pulse;
    out.polyline(&edges_l, tint, edge, 2.0, false);
    out.polyline(&edges_r, tint, edge, 2.0, false);

    // Notes, far to near.
    let from = ride.next_note();
    let ahead: Vec<_> = map.notes[from..].iter().take_while(|n| map.distance_at(n.t) < d_ship + AHEAD).collect();
    for n in ahead.iter().rev() {
        let d = map.distance_at(n.t);
        let f = geom.at(d);
        let centre = f.pos.add(f.right().mul(lane_x(n.lane) * STEER_M));
        let (c, grey) = match n.kind {
            NoteKind::Colour(c) => (PALETTE[c as usize], false),
            NoteKind::Grey => (GREY, true),
        };
        let far = ((d - d_ship) / AHEAD).clamp(0.0, 1.0);
        draw_block(out, &view, centre, f, c, 1.0 - far * 0.6, grey);
    }

    // Bursts for what was just caught.
    let ship_pos = ship.pos.add(ship.right().mul(lat));
    for e in &ride.events {
        if let Event::Caught { t: te, colour, .. } = *e {
            let age = t - te;
            if (0.0..0.4).contains(&age) {
                if let Some((x, y, z)) = view.project(ship_pos.add(v3(0.0, 0.8, 1.5))) {
                    let r = (6.0 + age * 60.0) * 8.0 / z.max(1.0);
                    out.glow(x, y, r, PALETTE[colour as usize], 1.2 * (1.0 - age / 0.4));
                }
            }
        }
    }

    draw_craft(out, &view, ship, ship_pos, ride.vx, tint);
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
fn draw_block(out: &mut Canvas, view: &View, centre: V3, f: Frame, c: Rgb, light: f32, grey: bool) {
    let (w, h, l) = (2.2, if grey { 2.0 } else { 1.1 }, 1.6);
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

/// The hover craft: a low-poly arrowhead, banking with the steering, with an engine glow.
fn draw_craft(out: &mut Canvas, view: &View, f: Frame, pos: V3, vx: f32, tint: Rgb) {
    // Local model: x right, y up, z forward (metres).
    const V: [(f32, f32, f32); 6] = [(0.0, 0.35, 2.2), (-1.5, 0.15, -1.0), (1.5, 0.15, -1.0), (0.0, 0.75, -0.8), (0.0, 0.0, -1.1), (0.0, 0.3, -1.2)];
    const FACES: [[usize; 3]; 6] = [[0, 1, 3], [0, 3, 2], [0, 4, 1], [0, 2, 4], [1, 5, 3], [3, 5, 2]];
    let bank = (-vx * 0.25).clamp(-0.6, 0.6);
    let r = f.right();
    let fwd = v3(f.heading.sin(), 0.0, f.heading.cos());
    let up = v3(0.0, 1.0, 0.0);
    let hover = 0.6;
    let world: Vec<V3> = V
        .iter()
        .map(|&(x, y, z)| {
            let m = v3(x, y, z).rot(0.0, 0.0, bank);
            pos.add(r.mul(m.x)).add(up.mul(m.y + hover)).add(fwd.mul(m.z))
        })
        .collect();
    let Some(p): Option<Vec<(f32, f32, f32)>> = world.iter().map(|&q| view.project(q)).collect() else { return };
    let light = v3(0.4, 0.8, -0.45).norm();
    let mut faces: Vec<(f32, [usize; 3], f32)> = FACES
        .iter()
        .map(|&[a, b, c]| {
            let n = world[b].sub(world[a]).cross(world[c].sub(world[a])).norm();
            let depth = (p[a].2 + p[b].2 + p[c].2) / 3.0;
            (depth, [a, b, c], 0.35 + 0.65 * n.dot(light).abs())
        })
        .collect();
    faces.sort_by(|x, y| y.0.total_cmp(&x.0));
    let body: Rgb = [0.85, 0.9, 1.0];
    for (_, [a, b, c], k) in faces {
        let col = [body[0] * k, body[1] * k, body[2] * k];
        out.tri_flat([(p[a].0, p[a].1), (p[b].0, p[b].1), (p[c].0, p[c].1)], col, 1.0);
    }
    // Engine glow at the tail, in the road's colour.
    if let Some((x, y, z)) = view.project(world[5].add(fwd.mul(-0.4))) {
        out.glow(x, y, 30.0 / z.max(1.0), tint, 0.9);
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
    fn a_frame_draws_road_notes_and_craft() {
        let (s, _) = drums(124.0, 40.0, 0.0);
        let map = generate(&analyse(&s, None), Difficulty::Pro, 5);
        let g = RoadGeom::new(&map);
        let ride = Ride::new(map, 10.0);
        let bg = Canvas::new(320, 180);
        let mut out = Canvas::new(480, 270);
        draw(&mut out, &bg, &ride, &g, 13.0);
        let lit = out.px.iter().filter(|p| p[0] + p[1] + p[2] > 0.05).count();
        assert!(lit > out.px.len() / 10, "only {lit} pixels drawn");
        // The craft sits low in the middle of the picture.
        let mid = out.px[(out.h * 3 / 4) * out.w + out.w / 2];
        assert!(mid.iter().sum::<f32>() > 0.3, "{mid:?}");
    }
}
