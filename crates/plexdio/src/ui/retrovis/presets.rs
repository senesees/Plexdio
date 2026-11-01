//! The visualiser presets, each written from scratch in the style of an early-2000s player:
//! the PlayStation CD player's Sound Scope (Scope, Rings), the original Xbox dashboard
//! (Dashboard), Windows Media Player (Ambient, Kaleido), and 3D console light synths (Polygon,
//! Terrain, Tunnel, Cloud).

use super::analysis::{Analysis, WAVE};
use super::canvas::{Canvas, Rgb};
use super::feedback::{self, Warp};
use super::math3d::{icosphere, v3, Camera, V3};
use super::palette::{hex, hsv, scale, Palette};
use std::f32::consts::{PI, TAU};

/// A preset's own frame buffer (plus a scratch buffer for feedback).
pub struct Layer {
    pub cv: Canvas,
    tmp: Canvas,
    polar: Option<feedback::Polar>,
}

impl Layer {
    pub fn new(w: usize, h: usize) -> Self {
        Self { cv: Canvas::new(w, h), tmp: Canvas::new(w, h), polar: None }
    }

    /// Run one feedback step (see [`feedback::apply`]).
    pub fn feedback(&mut self, w: &Warp, t: f32) {
        let polar = self.polar.get_or_insert_with(|| feedback::Polar::new(self.cv.w, self.cv.h));
        feedback::apply(&self.cv, &mut self.tmp, w, t, polar);
        if w.blur {
            // The old frame is no longer needed: blur into it.
            feedback::blur(&self.tmp, &mut self.cv);
        } else {
            std::mem::swap(&mut self.cv, &mut self.tmp);
        }
    }
}

/// What a preset gets each frame.
pub struct Frame<'a> {
    pub a: &'a Analysis,
    pub t: f32,
    pub dt: f32,
    pub pal: &'a [Palette],
    /// Seconds the music has been paused (0 while playing).
    pub paused_for: f32,
}

impl Frame<'_> {
    /// Steps of feedback per frame are tuned for 30 fps; scale per-frame factors by this.
    fn per30(&self) -> f32 {
        (self.dt * 30.0).clamp(0.25, 4.0)
    }
}

pub trait Preset {
    fn name(&self) -> &'static str;
    fn draw(&mut self, layer: &mut Layer, f: &Frame);
    /// Keeps its picture for most of a pause (the Dashboard's starfield) instead of fading
    /// out from the start.
    fn holds_on_pause(&self) -> bool {
        false
    }
}

/// Small deterministic random numbers (no extra crate).
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

fn decay(d: f32, f: &Frame) -> f32 {
    d.powf(f.per30())
}

/// All presets, in menu order.
pub fn all() -> Vec<Box<dyn Preset>> {
    vec![
        Box::new(Scope),
        Box::new(Rings),
        Box::new(Dashboard::default()),
        Box::new(Ambient),
        Box::new(Kaleido { rng: Rng(7), bursts: Vec::new() }),
        Box::new(Polygon::new()),
        Box::new(Terrain::default()),
        Box::new(Tunnel::default()),
        Box::new(Cloud::new()),
    ]
}

// ------------------------------------------------------------------------------------ Scope

/// PS1 Sound Scope spikes: the spectrum as translucent triangles mirrored about a line, in
/// green, yellow and red layers, with short trails.
struct Scope;

impl Preset for Scope {
    fn name(&self) -> &'static str {
        "Scope"
    }

    fn draw(&mut self, l: &mut Layer, f: &Frame) {
        let a = f.a;
        let cv = &mut l.cv;
        cv.fade(decay(0.6, f));
        let (w, h) = (cv.w as f32, cv.h as f32);
        let mid = h * 0.5;
        let n = 22;
        let span = w * 0.72;
        let x0 = (w - span) / 2.0;
        let sp = span / n as f32;
        // (colour, height scale, opacity, base width, skew): broad red wedges at the back, yellow
        // triangles, thin green needles in front.
        let layers: [(Rgb, f32, f32, f32, f32); 3] = [
            (hex(0xd01808), 0.9, 0.16, 2.6, (f.t * 0.4).sin() * sp * 2.5),
            (hex(0xffe830), 0.5, 0.3, 1.8, 0.0),
            (hex(0x30ff70), 0.85, 0.5, 0.45, 0.0),
        ];
        for (li, &(col, k, alpha, base, skew)) in layers.iter().enumerate() {
            for i in 0..n {
                // The red wedges only every third slot, rising with the bass.
                if li == 0 && i % 3 != 1 {
                    continue;
                }
                let b = (i * a.bands.len() / n + li * 3) % a.bands.len();
                let lev = if li == 0 { a.bass } else { a.bands[b] };
                // Squared so quiet bands stay low and peaks stand out.
                let hgt = lev * lev * h * 0.44 * k * (1.0 + 0.25 * a.pulse);
                if hgt < 1.0 {
                    continue;
                }
                let cx = x0 + (i as f32 + 0.5) * sp;
                let bw = sp * base / 2.0;
                let dim = scale(col, 0.55);
                cv.tri([(cx - bw, mid), (cx + bw, mid), (cx + skew, mid - hgt)], [dim, dim, col], alpha, true);
                cv.tri([(cx - bw, mid), (cx + bw, mid), (cx - skew, mid + hgt)], [dim, dim, col], alpha * 0.85, true);
                if li == 2 {
                    // Bright needle on the green spikes.
                    cv.line(cx, mid - hgt, cx, mid + hgt, col, 0.5, 1.0);
                }
            }
        }
        cv.line(x0, mid, x0 + span, mid, hex(0xfff080), 0.6 + a.level * 0.4, 1.0);
    }
}

// ------------------------------------------------------------------------------------ Rings

/// PS1 Sound Scope rings: three concentric rings that swell with bass, mid and treble, wobble
/// with the waveform and leave ghost trails as the picture zooms.
struct Rings;

impl Preset for Rings {
    fn name(&self) -> &'static str {
        "Rings"
    }

    fn draw(&mut self, l: &mut Layer, f: &Frame) {
        let a = f.a;
        l.feedback(&Warp { zoom: 1.0 + 0.025 * f.per30(), decay: decay(0.84, f), ..Default::default() }, f.t);
        let cv = &mut l.cv;
        let (w, h) = (cv.w as f32, cv.h as f32);
        let (cx, cy) = (w / 2.0, h / 2.0);
        let rings: [(f32, f32, Rgb); 3] = [(0.12, a.treble, hex(0xfff060)), (0.24, a.mid, hex(0xffd020)), (0.36, a.bass, hex(0xff3050))];
        for (i, &(base, lev, col)) in rings.iter().enumerate() {
            let r = h * base * (1.0 + 0.3 * lev + 0.1 * a.pulse * (i == 2) as u8 as f32);
            let amp = h * 0.03 * lev;
            let wave = &a.wave;
            let wob = |ang: f32| wave[((ang / TAU) * (WAVE - 1) as f32) as usize % WAVE] * amp;
            cv.ring(cx, cy, r, col, 0.55 + lev * 0.5, 1.0 + lev * 2.5, wob);
            if i == 2 {
                // White core of the outer ring.
                cv.ring(cx, cy, r, [1.0, 0.9, 0.9], 0.25 + lev * 0.3, 1.0, wob);
            }
        }
    }
}

// -------------------------------------------------------------------------------- Dashboard

/// Original Xbox dashboard: zooming, blurred feedback in one slowly changing tint, with glowing
/// waveform streaks and rays from the centre. When the music pauses it fades into a swirling
/// starfield.
#[derive(Default)]
struct Dashboard {
    stars: Vec<V3>,
    rng: Option<Rng>,
}

impl Preset for Dashboard {
    fn name(&self) -> &'static str {
        "Dashboard"
    }

    fn holds_on_pause(&self) -> bool {
        true
    }

    fn draw(&mut self, l: &mut Layer, f: &Frame) {
        let a = f.a;
        let rng = self.rng.get_or_insert(Rng(0x9e37_79b9));
        l.feedback(
            &Warp {
                zoom: 1.0 + (0.022 + 0.03 * a.bass) * f.per30(),
                rot: 0.006 * (f.t * 0.13).sin() * f.per30(),
                swirl: 0.03,
                decay: decay(0.93, f),
                blur: true,
                ..Default::default()
            },
            f.t,
        );
        let cv = &mut l.cv;
        let (w, h) = (cv.w as f32, cv.h as f32);
        let (cx, cy) = (w / 2.0, h / 2.0);
        // Mostly the dashboard's purple, now and then its green; the change itself is quick.
        let s = (f.t * 0.02).cos();
        let mix = (s * 4.0).clamp(-1.0, 1.0) * 0.5 + 0.5;
        let tint = hsv(0.33 + 0.46 * mix, 0.85, 1.0);
        if !a.silent || a.level > 0.02 {
            cv.glow(cx, cy, h * (0.06 + 0.1 * a.bass), tint, 0.06 + 0.15 * a.pulse);
            // A wide, faint wash keeps the whole screen tinted, as on the Xbox.
            cv.wash(tint, 0.006 + 0.01 * a.level);
            // Two streaks: the waveform bent round the centre.
            for side in [0.0, PI] {
                let pts: Vec<(f32, f32)> = (0..96)
                    .map(|i| {
                        let u = i as f32 / 95.0;
                        let ang = side + f.t * 0.25 + (u - 0.5) * PI * 0.9;
                        let r = h * (0.26 + a.wave[i * (WAVE / 96)] * 0.1 * (0.4 + a.level));
                        (cx + r * ang.cos(), cy + r * ang.sin())
                    })
                    .collect();
                cv.polyline(&pts, tint, 0.18 + 0.3 * a.level, 1.6, false);
            }
            if a.beat {
                for _ in 0..10 {
                    let ang = rng.next() * TAU;
                    let len = h * (0.3 + 0.4 * rng.next());
                    cv.line(cx, cy, cx + len * ang.cos(), cy + len * ang.sin(), tint, 0.3, 1.0);
                }
            }
        }
        // The paused starfield.
        let stars = (f.paused_for / 2.0).min(1.0);
        if stars > 0.0 {
            if self.stars.is_empty() {
                self.stars = (0..260).map(|_| v3(rng.next() * 4.0 - 2.0, rng.next() * 4.0 - 2.0, 0.3 + rng.next() * 5.0)).collect();
            }
            let cam = Camera::new(cv.w, cv.h);
            let spin = f.dt * 0.4;
            for s in &mut self.stars {
                s.z -= f.dt * 1.6;
                *s = s.rot(0.0, 0.0, spin);
                if s.z < 0.2 {
                    *s = v3(rng.next() * 4.0 - 2.0, rng.next() * 4.0 - 2.0, 5.0);
                }
                if let Some((x, y, z)) = cam.project(*s) {
                    let b = (1.0 - z / 5.0).clamp(0.0, 1.0);
                    cv.glow(x, y, 0.7 + b * 1.2, [0.85, 0.9, 1.0], stars * b * 0.9);
                }
            }
        } else {
            self.stars.clear();
        }
    }
}

// ---------------------------------------------------------------------------------- Ambient

/// Windows Media Player style: smoky swirls with jagged waveform lines and cycling colours. It
/// moves through three variants (rays and lines, an icy spiral, a mirrored smoke ring).
struct Ambient;

impl Preset for Ambient {
    fn name(&self) -> &'static str {
        "Ambient"
    }

    fn draw(&mut self, l: &mut Layer, f: &Frame) {
        let a = f.a;
        let variant = ((f.t / 25.0) as usize) % 3;
        let p = f.per30();
        let warp = match variant {
            0 => Warp { zoom: 1.0 + 0.012 * p, decay: decay(0.93, f), blur: true, ..Default::default() },
            1 => Warp { zoom: 1.0 + 0.02 * p, rot: 0.01 * p, swirl: 0.12 + 0.1 * a.bass, decay: decay(0.96, f), blur: true, ..Default::default() },
            _ => Warp { zoom: 1.0 + 0.03 * p, ripple: 0.01, ripple_freq: 10.0, mirror: true, decay: decay(0.95, f), blur: true, ..Default::default() },
        };
        l.feedback(&warp, f.t);
        let cv = &mut l.cv;
        let (w, h) = (cv.w as f32, cv.h as f32);
        let (cx, cy) = (w / 2.0, h / 2.0);
        match variant {
            0 => {
                let pal = &f.pal[3];
                for i in 0..90 {
                    let ang = TAU * i as f32 / 90.0 + f.t * 0.05;
                    let lev = a.bands[i % a.bands.len()];
                    let len = h * (0.15 + 0.4 * lev);
                    cv.line(cx, cy, cx + len * ang.cos(), cy + len * ang.sin(), pal.at(f.t * 0.02 + lev * 0.3), 0.12, 1.0);
                }
                for (yy, sign) in [(0.22, 1.0), (0.78, -1.0)] {
                    let pts: Vec<(f32, f32)> =
                        (0..128).map(|i| (w * i as f32 / 127.0, h * yy + sign * a.wave[i * (WAVE / 128)] * h * 0.07)).collect();
                    cv.polyline(&pts, pal.at(0.6 + f.t * 0.03), 0.8, 1.0, false);
                }
            }
            1 => {
                let pal = &f.pal[4];
                for arm in 0..6 {
                    let pts: Vec<(f32, f32)> = (0..48)
                        .map(|i| {
                            let r = i as f32 / 47.0;
                            let ang = arm as f32 * TAU / 6.0 + r * 3.0 + f.t * 0.5;
                            let rr = r * h * 0.55 * (1.0 + a.wave[i * (WAVE / 48)] * 0.08);
                            (cx + rr * ang.cos(), cy + rr * ang.sin())
                        })
                        .collect();
                    cv.polyline(&pts, pal.at(arm as f32 / 6.0 + f.t * 0.02), 0.35 + a.level * 0.5, 1.2, false);
                }
                cv.glow(cx, cy, h * 0.1, [1.0, 1.0, 1.0], 0.2 + a.pulse * 0.3);
            }
            _ => {
                let pal = &f.pal[5];
                let x = w * 0.3;
                let r = h * 0.2 * (1.0 + a.bass * 0.4);
                let wave = &a.wave;
                cv.ring(x, cy, r, pal.at(f.t * 0.02), 0.7, 1.2, |ang| wave[((ang / TAU) * (WAVE - 1) as f32) as usize % WAVE] * h * 0.06);
            }
        }
    }
}

// ---------------------------------------------------------------------------------- Kaleido

/// WMP Alchemy style: kaleidoscope feedback in six segments, orbiting glows and star bursts on
/// beats.
struct Kaleido {
    rng: Rng,
    bursts: Vec<(f32, f32, f32)>,
}

impl Preset for Kaleido {
    fn name(&self) -> &'static str {
        "Kaleido"
    }

    fn draw(&mut self, l: &mut Layer, f: &Frame) {
        let a = f.a;
        let p = f.per30();
        l.feedback(&Warp { kaleido: 6, rot: 0.015 * p, zoom: 1.0 + 0.02 * p, decay: decay(0.955, f), blur: true, ..Default::default() }, f.t);
        let cv = &mut l.cv;
        let (w, h) = (cv.w as f32, cv.h as f32);
        let (cx, cy) = (w / 2.0, h / 2.0);
        let pal = &f.pal[6];
        for i in 0..5 {
            let lev = a.bands[(i * 6) % a.bands.len()];
            let ang = f.t * (0.3 + i as f32 * 0.11) + i as f32;
            let r = h * (0.08 + 0.3 * lev);
            cv.glow(cx + r * ang.cos(), cy + r * ang.sin(), 3.0 + 7.0 * lev, pal.at(f.t * 0.05 + i as f32 * 0.2), 0.5 + lev);
        }
        // A short waveform ribbon swinging around the middle (folded six times by the warp).
        let ang = f.t * 0.6;
        let (ux, uy) = (ang.cos(), ang.sin());
        let pts: Vec<(f32, f32)> = (0..64)
            .map(|i| {
                let u = i as f32 / 63.0 * h * 0.45;
                let o = a.wave[i * (WAVE / 64)] * h * 0.06;
                (cx + ux * u - uy * o, cy + uy * u + ux * o)
            })
            .collect();
        cv.polyline(&pts, pal.at(0.3 + f.t * 0.04), 0.5 + a.level, 1.0, false);
        if a.beat {
            self.bursts.push((self.rng.next() * w, self.rng.next() * h, 1.0));
        }
        for b in &mut self.bursts {
            for k in 0..14 {
                let ang = TAU * k as f32 / 14.0;
                let len = h * 0.18 * (1.2 - b.2);
                cv.line(b.0, b.1, b.0 + len * ang.cos(), b.1 + len * ang.sin(), pal.at(0.5 + f.t * 0.05), 0.4 * b.2, 1.0);
            }
            b.2 -= f.dt * 3.0;
        }
        self.bursts.retain(|b| b.2 > 0.0);
    }
}

// ---------------------------------------------------------------------------------- Polygon

/// A low-poly 3D ball pushed out by the spectrum, flat-shaded with PS1-style wobbly (pixel
/// snapped) vertices, over slow feedback trails.
struct Polygon {
    verts: Vec<V3>,
    faces: Vec<[usize; 3]>,
}

impl Polygon {
    fn new() -> Self {
        let (verts, faces) = icosphere(2);
        Self { verts, faces }
    }
}

impl Preset for Polygon {
    fn name(&self) -> &'static str {
        "Polygon"
    }

    fn draw(&mut self, l: &mut Layer, f: &Frame) {
        let a = f.a;
        let p = f.per30();
        l.feedback(&Warp { zoom: 1.0 + 0.02 * p, rot: 0.004 * p, decay: decay(0.7, f), blur: true, ..Default::default() }, f.t);
        let cv = &mut l.cv;
        let mut cam = Camera::new(cv.w, cv.h);
        cam.snap = true;
        let nb = a.bands.len();
        let (ax, ay) = (f.t * 0.4, f.t * 0.6);
        let pos: Vec<V3> = self
            .verts
            .iter()
            .map(|v| {
                let b = (((v.y + 1.0) / 2.0 * (nb - 1) as f32) as usize).min(nb - 1);
                let r = 1.0 + a.bands[b] * 0.55 + a.pulse * 0.12;
                v.mul(r).rot(ax, ay, 0.0).add(v3(0.0, 0.0, 2.9))
            })
            .collect();
        let light = v3(0.4, 0.6, -0.7).norm();
        let pal = &f.pal[7];
        let mut order: Vec<(f32, usize)> = self.faces.iter().enumerate().map(|(i, t)| ((pos[t[0]].z + pos[t[1]].z + pos[t[2]].z) / 3.0, i)).collect();
        order.sort_by(|x, y| y.0.total_cmp(&x.0));
        for (_, fi) in order {
            let t = self.faces[fi];
            let (p0, p1, p2) = (pos[t[0]], pos[t[1]], pos[t[2]]);
            let n = p1.sub(p0).cross(p2.sub(p0)).norm();
            // Facing the camera (which looks down +Z from the origin)?
            if n.dot(p0) >= 0.0 {
                continue;
            }
            let (Some(s0), Some(s1), Some(s2)) = (cam.project(p0), cam.project(p1), cam.project(p2)) else { continue };
            let shade = 0.35 + 0.85 * (-n.dot(light)).max(0.0);
            let col = scale(pal.at(f.t * 0.03 + fi as f32 / self.faces.len() as f32 * 0.35), shade);
            let tri = [(s0.0, s0.1), (s1.0, s1.1), (s2.0, s2.1)];
            cv.tri_flat(tri, col, 1.0);
            // Bright edges, PS1 wireframe style.
            cv.polyline(&tri, scale(col, 1.6), 0.25, 1.0, true);
        }
    }
}

// ---------------------------------------------------------------------------------- Terrain

/// A wireframe landscape made from the spectrum's recent history, scrolling towards the camera.
#[derive(Default)]
struct Terrain {
    rows: std::collections::VecDeque<Vec<f32>>,
    acc: f32,
}

const TERRAIN_ROWS: usize = 36;
const ROW_RATE: f32 = 18.0;

impl Preset for Terrain {
    fn name(&self) -> &'static str {
        "Terrain"
    }

    fn draw(&mut self, l: &mut Layer, f: &Frame) {
        let a = f.a;
        let cv = &mut l.cv;
        cv.fade(decay(0.45, f));
        self.acc += f.dt * ROW_RATE;
        while self.acc >= 1.0 {
            self.acc -= 1.0;
            self.rows.push_front(a.bands.to_vec());
            self.rows.truncate(TERRAIN_ROWS);
        }
        if self.rows.len() < 2 {
            return;
        }
        let cam = Camera::new(cv.w, cv.h);
        let cols = a.bands.len();
        let pal = &f.pal[0];
        let point = |r: usize, c: usize, rows: &std::collections::VecDeque<Vec<f32>>| -> Option<(f32, f32, f32, f32)> {
            let lev = rows[r][c];
            // Symmetric: low notes in the middle, highs at both sides.
            let x = (c as f32 / (cols - 1) as f32 - 0.5) * 5.0;
            let z = 0.9 + (r as f32 + self.acc) * 0.28;
            let p = v3(x, -0.9 + lev * 1.1, z).rot(-0.3, 0.0, 0.0).add(v3(0.0, 0.1, 0.0));
            cam.project(p).map(|(sx, sy, _)| (sx, sy, lev, z))
        };
        for r in 0..self.rows.len() {
            let pts: Vec<(f32, f32, f32, f32)> = (0..cols).filter_map(|c| point(r, c, &self.rows)).collect();
            let fadez = 1.0 - r as f32 / TERRAIN_ROWS as f32;
            for w2 in pts.windows(2) {
                let lev = (w2[0].2 + w2[1].2) / 2.0;
                cv.line(w2[0].0, w2[0].1, w2[1].0, w2[1].1, pal.at(lev * 0.6 + f.t * 0.02), 0.7 * fadez, 1.0);
            }
            if r + 1 < self.rows.len() {
                for c in (0..cols).step_by(2) {
                    if let (Some(p0), Some(p1)) = (point(r, c, &self.rows), point(r + 1, c, &self.rows)) {
                        cv.line(p0.0, p0.1, p1.0, p1.1, pal.at(p0.2 * 0.6 + f.t * 0.02), 0.4 * fadez, 1.0);
                    }
                }
            }
        }
    }
}

// ----------------------------------------------------------------------------------- Tunnel

/// Flying down a tunnel of pulsing polygon rings that twist and flash colour on beats.
#[derive(Default)]
struct Tunnel {
    travel: f32,
    hue: f32,
}

impl Preset for Tunnel {
    fn name(&self) -> &'static str {
        "Tunnel"
    }

    fn draw(&mut self, l: &mut Layer, f: &Frame) {
        let a = f.a;
        l.feedback(&Warp { zoom: 1.0 + 0.02 * f.per30(), decay: decay(0.72, f), ..Default::default() }, f.t);
        let cv = &mut l.cv;
        let cam = Camera::new(cv.w, cv.h);
        let speed = 1.0 + a.bass * 3.0 + a.pulse;
        self.travel += f.dt * speed;
        if a.beat {
            self.hue += 0.17;
        }
        let spacing = 0.5;
        let off = self.travel % spacing;
        let base = (self.travel / spacing).floor() as i32;
        let sides = 10;
        let pal = &f.pal[7];
        let ring = |i: i32| -> Vec<Option<(f32, f32, f32)>> {
            let z = 0.4 + i as f32 * spacing - off;
            let idx = (base + i).rem_euclid(a.bands.len() as i32) as usize;
            let r = 1.0 + 0.3 * a.bands[idx] + 0.15 * a.pulse;
            let twist = (base + i) as f32 * 0.12 + f.t * 0.3;
            let cxo = 0.4 * (f.t * 0.7 + z * 0.35).sin();
            let cyo = 0.3 * (f.t * 0.5 + z * 0.3).cos();
            (0..sides)
                .map(|k| {
                    let ang = twist + TAU * k as f32 / sides as f32;
                    cam.project(v3(cxo + r * ang.cos(), cyo + r * ang.sin(), z))
                })
                .collect()
        };
        let mut prev: Option<Vec<Option<(f32, f32, f32)>>> = None;
        for i in (0..22).rev() {
            let cur = ring(i);
            let z = 0.4 + i as f32 * spacing - off;
            let fadez = (1.0 - z / 11.0).clamp(0.0, 1.0);
            let col = pal.at(self.hue + z * 0.04 - f.t * 0.05);
            for k in 0..sides {
                if let (Some(p0), Some(p1)) = (cur[k], cur[(k + 1) % sides]) {
                    cv.line(p0.0, p0.1, p1.0, p1.1, col, 0.8 * fadez, 1.0 + fadez);
                }
                if let Some(pv) = &prev {
                    if let (Some(p0), Some(p1)) = (cur[k], pv[k]) {
                        cv.line(p0.0, p0.1, p1.0, p1.1, col, 0.35 * fadez, 1.0);
                    }
                }
            }
            prev = Some(cur);
        }
    }
}

// ------------------------------------------------------------------------------------ Cloud

/// A mirrored 3D point cloud that morphs between shapes on beats, like the console light synths.
struct Cloud {
    pts: Vec<V3>,
    shape: usize,
    since: f32,
}

const CLOUD_POINTS: usize = 1100;
const SHAPES: usize = 5;

impl Cloud {
    fn new() -> Self {
        Self { pts: (0..CLOUD_POINTS).map(|i| target(0, i)).collect(), shape: 0, since: 0.0 }
    }
}

/// Point `i` of shape `s` (sphere, torus, double helix, cube, wave sheet).
fn target(s: usize, i: usize) -> V3 {
    let u = i as f32 / CLOUD_POINTS as f32;
    match s % SHAPES {
        0 => {
            // Fibonacci sphere.
            let y = 1.0 - 2.0 * u;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let ang = i as f32 * 2.399_963;
            v3(r * ang.cos(), y, r * ang.sin())
        }
        1 => {
            let (a1, a2) = (u * TAU * 23.0, u * TAU);
            let r = 0.75 + 0.3 * a1.cos();
            v3(r * a2.cos(), 0.3 * a1.sin(), r * a2.sin())
        }
        2 => {
            let ang = u * TAU * 4.0 + if i.is_multiple_of(2) { 0.0 } else { PI };
            v3(0.6 * ang.cos(), u * 2.4 - 1.2, 0.6 * ang.sin())
        }
        3 => {
            let k = (i % 6) as f32;
            let (a1, b1) = (((i * 7919) % 97) as f32 / 48.0 - 1.0, ((i * 104_729) % 89) as f32 / 44.0 - 1.0);
            match k as usize {
                0 => v3(1.0, a1, b1),
                1 => v3(-1.0, a1, b1),
                2 => v3(a1, 1.0, b1),
                3 => v3(a1, -1.0, b1),
                4 => v3(a1, b1, 1.0),
                _ => v3(a1, b1, -1.0),
            }
            .mul(0.7)
        }
        _ => {
            let (gx, gz) = ((i % 33) as f32 / 16.0 - 1.0, (i / 33) as f32 / 16.0 - 1.0);
            v3(gx * 1.2, 0.25 * (gx * 4.0).sin() * (gz * 3.0).cos(), gz * 1.2)
        }
    }
}

impl Preset for Cloud {
    fn name(&self) -> &'static str {
        "Cloud"
    }

    fn draw(&mut self, l: &mut Layer, f: &Frame) {
        let a = f.a;
        l.feedback(&Warp { zoom: 1.0 + 0.012 * f.per30(), decay: decay(0.8, f), blur: true, ..Default::default() }, f.t);
        self.since += f.dt;
        if (a.beat && self.since > 2.0) || self.since > 7.0 {
            self.shape = (self.shape + 1) % SHAPES;
            self.since = 0.0;
        }
        let k = (f.dt * 3.0).min(1.0);
        for (i, p) in self.pts.iter_mut().enumerate() {
            *p = p.lerp(target(self.shape, i), k);
        }
        let cv = &mut l.cv;
        let cam = Camera::new(cv.w, cv.h);
        let w = cv.w as f32;
        let pal = &f.pal[7];
        let (ax, ay) = (0.35 * (f.t * 0.2).sin(), f.t * 0.5);
        let grow = 1.0 + a.bass * 0.35 + a.pulse * 0.15;
        let size = 0.8 + a.treble * 1.4;
        for (i, p) in self.pts.iter().enumerate() {
            let q = p.mul(grow).rot(ax, ay, 0.0).add(v3(0.0, 0.0, 3.0));
            if let Some((x, y, z)) = cam.project(q) {
                let b = (1.6 - (z - 2.0) * 0.6).clamp(0.15, 1.0);
                let col = pal.at(i as f32 / CLOUD_POINTS as f32 + f.t * 0.05);
                // Mirrored left-right.
                if x < w / 2.0 {
                    cv.glow(x, y, size, col, b * 0.5);
                    cv.glow(w - x, y, size, col, b * 0.5);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::retrovis::palette::palettes;

    #[test]
    fn every_preset_draws_something_and_stays_finite() {
        let pal = palettes();
        let mut an = Analysis { silent: false, ..Default::default() };
        for (i, b) in an.bands.iter_mut().enumerate() {
            *b = 0.3 + 0.5 * ((i as f32) * 0.4).sin().abs();
        }
        an.bass = 0.7;
        an.mid = 0.5;
        an.treble = 0.4;
        an.level = 0.55;
        an.wave = (0..WAVE).map(|i| (i as f32 * 0.2).sin()).collect();
        for mut p in all() {
            let mut l = Layer::new(160, 90);
            for n in 0..60 {
                an.beat = n % 15 == 0;
                an.pulse = if an.beat { 1.0 } else { an.pulse * 0.9 };
                p.draw(&mut l, &Frame { a: &an, t: n as f32 / 30.0, dt: 1.0 / 30.0, pal: &pal, paused_for: 0.0 });
            }
            let b = l.cv.brightness();
            assert!(b > 0.002, "{} drew nothing ({b})", p.name());
            assert!(l.cv.px.iter().all(|px| px.iter().all(|v| v.is_finite())), "{} produced NaN", p.name());
        }
    }

    #[test]
    fn dashboard_shows_stars_when_paused() {
        let pal = palettes();
        let an = Analysis::default();
        let mut p = Dashboard::default();
        let mut l = Layer::new(160, 90);
        for n in 0..90 {
            p.draw(&mut l, &Frame { a: &an, t: n as f32 / 30.0, dt: 1.0 / 30.0, pal: &pal, paused_for: n as f32 / 30.0 });
        }
        assert!(l.cv.brightness() > 0.001, "starfield visible");
    }
}
