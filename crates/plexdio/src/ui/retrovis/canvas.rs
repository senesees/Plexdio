//! A small floating-point RGB frame buffer with additive drawing: lines, glowing points,
//! triangles and rings. Values above 1.0 are allowed while drawing (they read as glow) and are
//! clamped when the frame is turned into an image.

/// Linear RGB, nominally 0..=1.
pub type Rgb = [f32; 3];

#[derive(Clone)]
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Rgb>,
}

/// 4×4 Bayer matrix for ordered dithering (the PS1 look), as offsets in 1/255 steps.
const BAYER: [[f32; 4]; 4] = [[0.0, 8.0, 2.0, 10.0], [12.0, 4.0, 14.0, 6.0], [3.0, 11.0, 1.0, 9.0], [15.0, 7.0, 13.0, 5.0]];

impl Canvas {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, px: vec![[0.0; 3]; w * h] }
    }

    pub fn clear(&mut self) {
        self.px.iter_mut().for_each(|p| *p = [0.0; 3]);
    }

    /// Add `c * a` to every pixel (a faint tint over the whole picture).
    pub fn wash(&mut self, c: Rgb, a: f32) {
        let add = [c[0] * a, c[1] * a, c[2] * a];
        for p in &mut self.px {
            p[0] += add[0];
            p[1] += add[1];
            p[2] += add[2];
        }
    }

    /// Multiply everything by `k` (trails fade out).
    pub fn fade(&mut self, k: f32) {
        for p in &mut self.px {
            p[0] *= k;
            p[1] *= k;
            p[2] *= k;
        }
    }

    #[inline]
    pub fn add(&mut self, x: i32, y: i32, c: Rgb, a: f32) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let p = &mut self.px[y as usize * self.w + x as usize];
        p[0] += c[0] * a;
        p[1] += c[1] * a;
        p[2] += c[2] * a;
    }

    /// Additive point at a fractional position, spread over the four nearest pixels.
    pub fn splat(&mut self, x: f32, y: f32, c: Rgb, a: f32) {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (ix, iy) = (x0 as i32, y0 as i32);
        self.add(ix, iy, c, a * (1.0 - fx) * (1.0 - fy));
        self.add(ix + 1, iy, c, a * fx * (1.0 - fy));
        self.add(ix, iy + 1, c, a * (1.0 - fx) * fy);
        self.add(ix + 1, iy + 1, c, a * fx * fy);
    }

    /// A soft round glow of radius `r` pixels (brightest in the middle).
    pub fn glow(&mut self, x: f32, y: f32, r: f32, c: Rgb, a: f32) {
        if r <= 0.75 {
            return self.splat(x, y, c, a);
        }
        let (x0, x1) = ((x - r).floor() as i32, (x + r).ceil() as i32);
        let (y0, y1) = ((y - r).floor() as i32, (y + r).ceil() as i32);
        let inv = 1.0 / (r * r);
        for py in y0..=y1 {
            for px in x0..=x1 {
                let (dx, dy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
                let d = (dx * dx + dy * dy) * inv;
                if d < 1.0 {
                    let k = (1.0 - d) * (1.0 - d);
                    self.add(px, py, c, a * k);
                }
            }
        }
    }

    /// Additive line; `width` above ~1.5 draws it as a row of glows.
    #[allow(clippy::too_many_arguments)]
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgb, a: f32, width: f32) {
        let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
        // Skip lines entirely off the canvas.
        let (w, h) = (self.w as f32, self.h as f32);
        if (x0 < -width && x1 < -width) || (y0 < -width && y1 < -width) || (x0 > w + width && x1 > w + width) || (y0 > h + width && y1 > h + width) {
            return;
        }
        let steps = (len * if width > 1.5 { 1.0 / (width * 0.35) } else { 2.0 }).ceil().clamp(1.0, 4000.0) as usize;
        let per = a * (len / steps as f32).min(1.0) * if width > 1.5 { 0.6 } else { 1.0 };
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let (x, y) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            if width > 1.5 {
                self.glow(x, y, width / 2.0, c, per);
            } else {
                self.splat(x, y, c, per);
            }
        }
    }

    pub fn polyline(&mut self, pts: &[(f32, f32)], c: Rgb, a: f32, width: f32, closed: bool) {
        for w in pts.windows(2) {
            self.line(w[0].0, w[0].1, w[1].0, w[1].1, c, a, width);
        }
        if closed && pts.len() > 2 {
            let (f, l) = (pts[0], pts[pts.len() - 1]);
            self.line(l.0, l.1, f.0, f.1, c, a, width);
        }
    }

    /// Ring of radius `r` whose radius is pushed out by `wobble(angle)` (pixels).
    #[allow(clippy::too_many_arguments)]
    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, c: Rgb, a: f32, width: f32, wobble: impl Fn(f32) -> f32) {
        let n = ((r * std::f32::consts::TAU / 3.0) as usize).clamp(12, 360);
        let pts: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                let ang = std::f32::consts::TAU * i as f32 / n as f32;
                let rr = r + wobble(ang);
                (cx + rr * ang.cos(), cy + rr * ang.sin())
            })
            .collect();
        self.polyline(&pts, c, a, width, true);
    }

    /// Filled triangle with a colour per corner (Gouraud). `additive` adds light; otherwise the
    /// colour is blended over what is there with opacity `a`.
    pub fn tri(&mut self, p: [(f32, f32); 3], c: [Rgb; 3], a: f32, additive: bool) {
        let minx = p.iter().map(|v| v.0).fold(f32::MAX, f32::min).floor().max(0.0) as i32;
        let maxx = p.iter().map(|v| v.0).fold(f32::MIN, f32::max).ceil().min(self.w as f32 - 1.0) as i32;
        let miny = p.iter().map(|v| v.1).fold(f32::MAX, f32::min).floor().max(0.0) as i32;
        let maxy = p.iter().map(|v| v.1).fold(f32::MIN, f32::max).ceil().min(self.h as f32 - 1.0) as i32;
        if minx > maxx || miny > maxy {
            return;
        }
        let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
        if area.abs() < 1e-3 {
            return;
        }
        let inv = 1.0 / area;
        for y in miny..=maxy {
            let fy = y as f32 + 0.5;
            for x in minx..=maxx {
                let fx = x as f32 + 0.5;
                let w0 = ((p[1].0 - fx) * (p[2].1 - fy) - (p[2].0 - fx) * (p[1].1 - fy)) * inv;
                let w1 = ((p[2].0 - fx) * (p[0].1 - fy) - (p[0].0 - fx) * (p[2].1 - fy)) * inv;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let col = [
                    c[0][0] * w0 + c[1][0] * w1 + c[2][0] * w2,
                    c[0][1] * w0 + c[1][1] * w1 + c[2][1] * w2,
                    c[0][2] * w0 + c[1][2] * w1 + c[2][2] * w2,
                ];
                let px = &mut self.px[y as usize * self.w + x as usize];
                if additive {
                    px[0] += col[0] * a;
                    px[1] += col[1] * a;
                    px[2] += col[2] * a;
                } else {
                    px[0] += (col[0] - px[0]) * a;
                    px[1] += (col[1] - px[1]) * a;
                    px[2] += (col[2] - px[2]) * a;
                }
            }
        }
    }

    /// `self = self * (1 - k) + other * k` (crossfading two presets of the same size).
    pub fn mix_from(&mut self, other: &Canvas, k: f32) {
        if other.px.len() != self.px.len() {
            return;
        }
        for (a, b) in self.px.iter_mut().zip(&other.px) {
            a[0] += (b[0] - a[0]) * k;
            a[1] += (b[1] - a[1]) * k;
            a[2] += (b[2] - a[2]) * k;
        }
    }

    /// 8-bit RGBA pixels (clamped, with optional ordered dithering).
    pub fn to_rgba(&self, out: &mut Vec<egui::Color32>, dither: bool) {
        out.clear();
        out.reserve(self.px.len());
        for (i, p) in self.px.iter().enumerate() {
            let d = if dither { BAYER[(i / self.w) & 3][(i % self.w) & 3] / 16.0 - 0.5 } else { 0.0 };
            let q = |v: f32| ((v.clamp(0.0, 1.0) * 255.0 + d).round()).clamp(0.0, 255.0) as u8;
            out.push(egui::Color32::from_rgb(q(p[0]), q(p[1]), q(p[2])));
        }
    }

    /// Mean brightness (tests and the "has the fade finished" check).
    pub fn brightness(&self) -> f32 {
        self.px.iter().map(|p| (p[0] + p[1] + p[2]) / 3.0).sum::<f32>() / self.px.len().max(1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawing_is_clipped_and_additive() {
        let mut c = Canvas::new(8, 8);
        c.add(-1, 0, [1.0; 3], 1.0);
        c.add(8, 8, [1.0; 3], 1.0);
        assert_eq!(c.brightness(), 0.0);
        c.add(2, 2, [1.0, 0.0, 0.0], 0.5);
        c.add(2, 2, [1.0, 0.0, 0.0], 0.5);
        assert_eq!(c.px[2 * 8 + 2], [1.0, 0.0, 0.0]);
        // Splat keeps the total energy.
        let mut s = Canvas::new(8, 8);
        s.splat(3.3, 4.6, [1.0; 3], 1.0);
        assert!((s.brightness() * 64.0 - 1.0).abs() < 1e-4);
    }

    #[test]
    fn triangles_fill_inside_only() {
        let mut c = Canvas::new(10, 10);
        c.tri([(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)], [[1.0; 3]; 3], 1.0, true);
        assert!(c.px[10 + 1][0] > 0.9, "inside");
        assert_eq!(c.px[9 * 10 + 9][0], 0.0, "outside");
        // Off-canvas triangles are ignored.
        c.tri([(-50.0, -50.0), (-40.0, -50.0), (-50.0, -40.0)], [[1.0; 3]; 3], 1.0, true);
    }

    #[test]
    fn lines_reach_both_ends_and_fade_works() {
        let mut c = Canvas::new(20, 5);
        c.line(1.0, 2.0, 18.0, 2.0, [1.0; 3], 1.0, 1.0);
        assert!(c.px[2 * 20 + 1][0] > 0.2 && c.px[2 * 20 + 18][0] > 0.2);
        let before = c.brightness();
        c.fade(0.5);
        assert!((c.brightness() - before * 0.5).abs() < 1e-5);
    }

    #[test]
    fn dithering_stays_within_one_step() {
        let mut c = Canvas::new(4, 4);
        c.px.iter_mut().for_each(|p| *p = [0.5; 3]);
        let mut out = Vec::new();
        c.to_rgba(&mut out, true);
        assert!(out.iter().all(|p| (p.r() as i32 - 128).abs() <= 1));
    }
}
