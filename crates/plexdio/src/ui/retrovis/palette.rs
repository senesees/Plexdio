//! Colour helpers: HSV and 256-entry gradient palettes that presets cycle through, as WMP did.

use super::canvas::Rgb;

pub fn hsv(h: f32, s: f32, v: f32) -> Rgb {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

pub fn hex(c: u32) -> Rgb {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]
}

pub fn scale(c: Rgb, k: f32) -> Rgb {
    [c[0] * k, c[1] * k, c[2] * k]
}

pub fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// A looping gradient, sampled with `at(0..1)` (wraps).
#[derive(Clone)]
pub struct Palette(Vec<Rgb>);

impl Palette {
    pub fn new(stops: &[u32]) -> Self {
        let cols: Vec<Rgb> = stops.iter().map(|c| hex(*c)).collect();
        let n = cols.len().max(1);
        let lut = (0..256)
            .map(|i| {
                let t = i as f32 / 256.0 * n as f32;
                let k = t.floor() as usize;
                lerp(cols[k % n], cols[(k + 1) % n], t - t.floor())
            })
            .collect();
        Self(lut)
    }

    pub fn at(&self, t: f32) -> Rgb {
        self.0[((t.rem_euclid(1.0)) * 256.0) as usize & 255]
    }
}

/// Palettes in the spirit of the originals.
pub fn palettes() -> Vec<Palette> {
    vec![
        Palette::new(&[0x00ff60, 0xfff000, 0xff2000, 0x20ff90]), // PS1 Sound Scope
        Palette::new(&[0x2a0040, 0xb000ff, 0xff60ff, 0x4000a0]), // Xbox purple
        Palette::new(&[0x002000, 0x40ff40, 0xc0ff60, 0x008040]), // Xbox green
        Palette::new(&[0x300000, 0xff4020, 0xffa060, 0x800000]), // Ambience red
        Palette::new(&[0x102040, 0x80b0e0, 0xffffff, 0x4070a0]), // Battery ice
        Palette::new(&[0x0010a0, 0x2060ff, 0x20ff40, 0x008020]), // Plenoptic blue/green
        Palette::new(&[0x400040, 0x40e0ff, 0x80ff40, 0xff40c0]), // Alchemy
        Palette::new(&[0xff0080, 0xffe000, 0x00ffe0, 0x8000ff]), // Minter neon
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_primaries_and_palette_wrap() {
        assert_eq!(hsv(0.0, 1.0, 1.0), [1.0, 0.0, 0.0]);
        let g = hsv(1.0 / 3.0, 1.0, 1.0);
        assert!(g[1] > 0.99 && g[0] < 0.01);
        let p = Palette::new(&[0xff0000, 0x0000ff]);
        assert_eq!(p.at(0.0), [1.0, 0.0, 0.0]);
        assert_eq!(p.at(1.0), p.at(0.0), "wraps");
    }
}
