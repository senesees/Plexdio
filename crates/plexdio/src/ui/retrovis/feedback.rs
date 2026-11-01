//! Feedback: each frame starts from the previous one, pulled through a warp (zoom, rotation,
//! swirl, ripple, kaleidoscope, mirror, drift), softened and darkened. New drawing on top then
//! smears into trails. This is the trick behind the Xbox dashboard and most WMP presets.

use super::canvas::Canvas;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Warp {
    /// Above 1 the picture grows each frame (moves outwards); below 1 it shrinks.
    pub zoom: f32,
    /// Rotation per frame (radians).
    pub rot: f32,
    /// Extra rotation towards the centre (radians at the centre, fading to 0 at the edge).
    pub swirl: f32,
    /// Radial ripple: amplitude (in half-heights) and frequency.
    pub ripple: f32,
    pub ripple_freq: f32,
    /// Fold into this many mirrored segments (0 = off).
    pub kaleido: u32,
    /// Mirror the left half onto the right.
    pub mirror: bool,
    /// Drift per frame (in half-heights).
    pub dx: f32,
    pub dy: f32,
    /// Brightness kept per frame (0.9 = trails fade fast, 0.98 = long smoke).
    pub decay: f32,
    /// Soften with a small blur.
    pub blur: bool,
}

impl Default for Warp {
    fn default() -> Self {
        Self { zoom: 1.0, rot: 0.0, swirl: 0.0, ripple: 0.0, ripple_freq: 8.0, kaleido: 0, mirror: false, dx: 0.0, dy: 0.0, decay: 0.9, blur: false }
    }
}

/// `floor` without a library call (baseline x86-64 has no rounding instruction).
#[inline(always)]
fn ffloor(v: f32) -> f32 {
    let t = v as i32 as f32;
    if t > v {
        t - 1.0
    } else {
        t
    }
}

/// Bilinear sample of `c` at a fractional pixel position; black outside.
#[inline(always)]
fn sample(c: &Canvas, x: f32, y: f32) -> [f32; 3] {
    let (x, y) = (x - 0.5, y - 0.5);
    let (x0, y0) = (ffloor(x), ffloor(y));
    let (fx, fy) = (x - x0, y - y0);
    let (ix, iy) = (x0 as i32, y0 as i32);
    let (w, h) = (c.w as i32, c.h as i32);
    let (a, b, d, e) = if ix >= 0 && iy >= 0 && ix + 1 < w && iy + 1 < h {
        // Fast path (almost every pixel): all four taps inside, one bounds check.
        let i = iy as usize * c.w + ix as usize;
        let row = &c.px[i..i + c.w + 2];
        (row[0], row[1], row[c.w], row[c.w + 1])
    } else {
        let get = |px: i32, py: i32| -> [f32; 3] {
            if px < 0 || py < 0 || px >= w || py >= h {
                [0.0; 3]
            } else {
                c.px[py as usize * c.w + px as usize]
            }
        };
        (get(ix, iy), get(ix + 1, iy), get(ix, iy + 1), get(ix + 1, iy + 1))
    };
    let (w00, w10, w01, w11) = ((1.0 - fx) * (1.0 - fy), fx * (1.0 - fy), (1.0 - fx) * fy, fx * fy);
    [
        a[0] * w00 + b[0] * w10 + d[0] * w01 + e[0] * w11,
        a[1] * w00 + b[1] * w10 + d[1] * w01 + e[1] * w11,
        a[2] * w00 + b[2] * w10 + d[2] * w01 + e[2] * w11,
    ]
}

/// Each pixel's position in polar form (radius in half-heights and angle), plain and mirrored
/// left-right, so swirl and kaleidoscope warps need no `atan2`/`sqrt` per pixel.
pub struct Polar {
    r: Vec<f32>,
    a: Vec<f32>,
    a_mirror: Vec<f32>,
    /// Unit direction (cos, sin of the angle), plain and mirrored: lets warps that only
    /// depend on the radius rotate without any trig per pixel.
    dir: Vec<(f32, f32)>,
    dir_mirror: Vec<(f32, f32)>,
    r_max: f32,
}

/// Radius steps in the per-frame warp table.
const RBINS: usize = 2048;

impl Polar {
    pub fn new(w: usize, h: usize) -> Self {
        let half = h as f32 / 2.0;
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        let n = w * h;
        let mut p = Polar {
            r: Vec::with_capacity(n),
            a: Vec::with_capacity(n),
            a_mirror: Vec::with_capacity(n),
            dir: Vec::with_capacity(n),
            dir_mirror: Vec::with_capacity(n),
            r_max: 0.0,
        };
        for y in 0..h {
            for x in 0..w {
                let nx = (x as f32 + 0.5 - cx) / half;
                let ny = (y as f32 + 0.5 - cy) / half;
                let r = (nx * nx + ny * ny).sqrt();
                p.r.push(r);
                p.r_max = p.r_max.max(r);
                let (a, am) = (ny.atan2(nx), ny.atan2(nx.abs()));
                p.a.push(a);
                p.a_mirror.push(am);
                p.dir.push((a.cos(), a.sin()));
                p.dir_mirror.push((am.cos(), am.sin()));
            }
        }
        p
    }
}

/// Write `prev` pulled through `w` into `out` (same size; no blur, see [`blur`]). `t` drives the
/// ripple's motion; `polar` must match the canvas size.
pub fn apply(prev: &Canvas, out: &mut Canvas, w: &Warp, t: f32, polar: &Polar) {
    let (cw, ch) = (prev.w as f32, prev.h as f32);
    let half = ch / 2.0;
    let (cx, cy) = (cw / 2.0, ch / 2.0);
    let use_polar = (w.swirl != 0.0 || w.ripple != 0.0 || w.kaleido > 1) && polar.r.len() == prev.px.len();
    let inv_zoom = 1.0 / w.zoom.max(0.01);
    let (rs, rc) = (-w.rot).sin_cos();
    let seg = if w.kaleido > 1 { std::f32::consts::TAU / w.kaleido as f32 } else { 0.0 };
    let angles = if w.mirror { &polar.a_mirror } else { &polar.a };
    if !use_polar && !w.mirror {
        // Zoom, rotation and drift only: the source position moves by a constant step along a
        // row, so it is stepped instead of recomputed per pixel.
        let k = inv_zoom;
        let (step_x, step_y) = (rc * k, rs * k);
        for y in 0..prev.h {
            let ny = (y as f32 + 0.5 - cy) / half;
            let nx0 = (0.5 - cx) / half;
            let mut sx = ((nx0 * rc - ny * rs) * k - w.dx) * half + cx;
            let mut sy = ((nx0 * rs + ny * rc) * k - w.dy) * half + cy;
            let row = &mut out.px[y * prev.w..(y + 1) * prev.w];
            for p in row {
                let c = sample(prev, sx, sy);
                *p = [c[0] * w.decay, c[1] * w.decay, c[2] * w.decay];
                sx += step_x;
                sy += step_y;
            }
        }
        return;
    }
    // Radius, swirl and ripple depend only on how far a pixel is from the centre: a table per
    // frame gives each pixel's new radius and its rotation (as cos, sin).
    let bin = (RBINS - 1) as f32 / polar.r_max.max(1e-3);
    let table: Vec<(f32, f32, f32, f32)> = if use_polar {
        (0..RBINS)
            .map(|k| {
                let r = k as f32 / bin * inv_zoom;
                let phi = w.rot + if w.swirl != 0.0 { w.swirl * (1.0 - r.min(1.5) / 1.5) } else { 0.0 };
                let rr = if w.ripple != 0.0 { r + w.ripple * (r * w.ripple_freq - t * 3.0).sin() } else { r };
                let (s, c) = phi.sin_cos();
                (rr, c, s, phi)
            })
            .collect()
    } else {
        Vec::new()
    };
    let dirs = if w.mirror { &polar.dir_mirror } else { &polar.dir };
    if use_polar && seg == 0.0 {
        for (i, p) in out.px.iter_mut().enumerate() {
            let (rr, c, s, _) = table[((polar.r[i] * bin) as usize).min(RBINS - 1)];
            let (ux, uy) = dirs[i];
            // Direction rotated back by the angle from the table.
            let (ca, sa) = (ux * c + uy * s, uy * c - ux * s);
            let (sx, sy) = (rr * ca - w.dx, rr * sa - w.dy);
            let v = sample(prev, sx * half + cx, sy * half + cy);
            *p = [v[0] * w.decay, v[1] * w.decay, v[2] * w.decay];
        }
        return;
    }
    for y in 0..prev.h {
        for x in 0..prev.w {
            let i = y * prev.w + x;
            let (mut sx, mut sy) = if use_polar {
                // Kaleidoscope: the angle itself is folded, so it is needed here.
                let (r, _, _, phi) = table[((polar.r[i] * bin) as usize).min(RBINS - 1)];
                let mut a = angles[i] - phi;
                if seg > 0.0 {
                    a -= seg * ffloor(a / seg);
                    if a > seg / 2.0 {
                        a = seg - a;
                    }
                }
                let (s, c) = a.sin_cos();
                (r * c, r * s)
            } else {
                let mut nx = (x as f32 + 0.5 - cx) / half;
                let ny = (y as f32 + 0.5 - cy) / half;
                if w.mirror {
                    nx = nx.abs();
                }
                let (nx, ny) = (nx * inv_zoom, ny * inv_zoom);
                (nx * rc - ny * rs, nx * rs + ny * rc)
            };
            sx -= w.dx;
            sy -= w.dy;
            let mut c = sample(prev, sx * half + cx, sy * half + cy);
            c[0] *= w.decay;
            c[1] *= w.decay;
            c[2] *= w.decay;
            out.px[i] = c;
        }
    }
}

/// Soften `src` into `dst` with a small cross-shaped blur (centre weighted).
pub fn blur(src: &Canvas, dst: &mut Canvas) {
    let (w, h) = (src.w, src.h);
    for y in 0..h {
        let up = if y > 0 { y - 1 } else { y };
        let down = if y + 1 < h { y + 1 } else { y };
        for x in 0..w {
            let left = if x > 0 { x - 1 } else { x };
            let right = if x + 1 < w { x + 1 } else { x };
            let c = src.px[y * w + x];
            let (n, s, l, r) = (src.px[up * w + x], src.px[down * w + x], src.px[y * w + left], src.px[y * w + right]);
            let mut o = [0.0; 3];
            for k in 0..3 {
                o[k] = c[k] * 0.4 + (n[k] + s[k] + l[k] + r[k]) * 0.15;
            }
            dst.px[y * w + x] = o;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_keeps_the_picture_and_decay_darkens() {
        let mut a = Canvas::new(16, 9);
        a.px[4 * 16 + 5] = [1.0, 0.5, 0.25];
        let mut b = Canvas::new(16, 9);
        apply(&a, &mut b, &Warp { decay: 1.0, ..Default::default() }, 0.0, &Polar::new(16, 9));
        assert!((b.px[4 * 16 + 5][0] - 1.0).abs() < 1e-4);
        apply(&a, &mut b, &Warp { decay: 0.5, ..Default::default() }, 0.0, &Polar::new(16, 9));
        assert!((b.px[4 * 16 + 5][1] - 0.25).abs() < 1e-4);
    }

    #[test]
    fn zooming_in_moves_things_outwards() {
        // A dot right of the centre ends up further right after a zoom-in step.
        let mut a = Canvas::new(40, 20);
        a.px[10 * 40 + 28] = [1.0; 3];
        let mut b = Canvas::new(40, 20);
        apply(&a, &mut b, &Warp { zoom: 1.2, decay: 1.0, ..Default::default() }, 0.0, &Polar::new(40, 20));
        let row: Vec<f32> = (0..40).map(|x| b.px[10 * 40 + x][0]).collect();
        let peak = row.iter().enumerate().max_by(|p, q| p.1.total_cmp(q.1)).unwrap().0;
        assert!(peak > 28, "peak at {peak}");
    }

    #[test]
    fn polar_and_plain_paths_agree() {
        // A pure zoom through the polar path (forced by a tiny swirl) matches the plain path.
        let mut a = Canvas::new(32, 18);
        for (i, p) in a.px.iter_mut().enumerate() {
            *p = [(i % 7) as f32 / 7.0, (i % 5) as f32 / 5.0, 0.5];
        }
        let polar = Polar::new(32, 18);
        let (mut b, mut c) = (Canvas::new(32, 18), Canvas::new(32, 18));
        apply(&a, &mut b, &Warp { zoom: 1.1, rot: 0.2, decay: 1.0, ..Default::default() }, 0.0, &polar);
        apply(&a, &mut c, &Warp { zoom: 1.1, rot: 0.2, swirl: 1e-7, decay: 1.0, ..Default::default() }, 0.0, &polar);
        // The polar path looks the radius up in a table (steps of ~1/1000 of the half height), so
        // allow a tiny per-pixel difference.
        let diff = b.px.iter().zip(&c.px).map(|(p, q)| (p[0] - q[0]).abs().max((p[1] - q[1]).abs())).fold(0.0f32, f32::max);
        assert!(diff < 0.02, "max diff {diff}");
    }

    #[test]
    fn fast_floor_rounds_down() {
        assert_eq!(ffloor(-1.5), -2.0);
        assert_eq!(ffloor(-2.0), -2.0);
        assert_eq!(ffloor(2.0), 2.0);
    }

    #[test]
    fn blur_keeps_the_average() {
        let mut a = Canvas::new(10, 10);
        a.px.iter_mut().for_each(|p| *p = [0.5; 3]);
        a.px[55] = [1.0; 3];
        let mut b = Canvas::new(10, 10);
        blur(&a, &mut b);
        assert!((b.brightness() - a.brightness()).abs() < 1e-4);
        assert!(b.px[55][0] < 1.0 && b.px[54][0] > 0.5);
    }

    #[test]
    fn mirror_copies_the_right_side_to_the_left() {
        let mut a = Canvas::new(20, 10);
        a.px[5 * 20 + 15] = [1.0; 3];
        let mut b = Canvas::new(20, 10);
        apply(&a, &mut b, &Warp { mirror: true, decay: 1.0, ..Default::default() }, 0.0, &Polar::new(20, 10));
        assert!(b.px[5 * 20 + 4][0] > 0.5, "mirrored onto the left");
    }
}
