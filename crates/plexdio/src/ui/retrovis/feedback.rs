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

/// Bilinear sample of `c` at a fractional pixel position; black outside.
#[inline]
fn sample(c: &Canvas, x: f32, y: f32) -> [f32; 3] {
    let (x, y) = (x - 0.5, y - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (ix, iy) = (x0 as i32, y0 as i32);
    let get = |px: i32, py: i32| -> [f32; 3] {
        if px < 0 || py < 0 || px >= c.w as i32 || py >= c.h as i32 {
            [0.0; 3]
        } else {
            c.px[py as usize * c.w + px as usize]
        }
    };
    let (a, b, d, e) = (get(ix, iy), get(ix + 1, iy), get(ix, iy + 1), get(ix + 1, iy + 1));
    let mut out = [0.0; 3];
    for k in 0..3 {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bot = d[k] + (e[k] - d[k]) * fx;
        out[k] = top + (bot - top) * fy;
    }
    out
}

/// Each pixel's position in polar form (radius in half-heights and angle), plain and mirrored
/// left-right, so swirl and kaleidoscope warps need no `atan2`/`sqrt` per pixel.
pub struct Polar {
    r: Vec<f32>,
    a: Vec<f32>,
    a_mirror: Vec<f32>,
}

impl Polar {
    pub fn new(w: usize, h: usize) -> Self {
        let half = h as f32 / 2.0;
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        let mut p = Polar { r: Vec::with_capacity(w * h), a: Vec::with_capacity(w * h), a_mirror: Vec::with_capacity(w * h) };
        for y in 0..h {
            for x in 0..w {
                let nx = (x as f32 + 0.5 - cx) / half;
                let ny = (y as f32 + 0.5 - cy) / half;
                p.r.push((nx * nx + ny * ny).sqrt());
                p.a.push(ny.atan2(nx));
                p.a_mirror.push(ny.atan2(nx.abs()));
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
    for y in 0..prev.h {
        for x in 0..prev.w {
            let i = y * prev.w + x;
            let (mut sx, mut sy) = if use_polar {
                // Zoom and rotation act on the radius and angle directly.
                let mut r = polar.r[i] * inv_zoom;
                let mut a = angles[i] - w.rot;
                if w.swirl != 0.0 {
                    a -= w.swirl * (1.0 - r.min(1.5) / 1.5);
                }
                if w.ripple != 0.0 {
                    r += w.ripple * (r * w.ripple_freq - t * 3.0).sin();
                }
                if seg > 0.0 {
                    a = a.rem_euclid(seg);
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
        let diff: f32 = b.px.iter().zip(&c.px).map(|(p, q)| (p[0] - q[0]).abs() + (p[1] - q[1]).abs()).sum();
        assert!(diff < 1e-2, "diff {diff}");
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
