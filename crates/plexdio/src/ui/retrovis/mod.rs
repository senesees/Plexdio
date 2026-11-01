//! Retro visualiser for the full-screen Now playing page: low-resolution feedback effects and
//! simple 3D in the style of the original Xbox dashboard, Windows Media Player 7–10, the
//! PlayStation's Sound Scope and the console light synths. Everything is drawn on the CPU into a
//! small frame buffer that the page shows scaled up. See docs/retro-visualiser-plan.md.

pub mod analysis;
pub mod canvas;
pub mod feedback;
pub mod math3d;
pub mod palette;
pub mod presets;

use analysis::Analyzer;
use canvas::Canvas;
use presets::{Frame, Layer, Preset};
use serde::{Deserialize, Serialize};

/// Settings for the Now playing visualiser (saved in the config).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetroPrefs {
    /// Preset to start with; `None` picks one at random.
    pub preset: Option<String>,
    /// Change preset every this many seconds (0 = never).
    pub cycle_secs: u32,
    /// Width of the frame buffer in pixels (320, 480 or 640); the height follows the view.
    pub width: u32,
    /// Smooth scaling instead of sharp pixels.
    pub smooth: bool,
    /// Ordered dithering (PS1 look).
    pub dither: bool,
    /// Frames per second (30 or 60).
    pub fps: u32,
}

impl Default for RetroPrefs {
    fn default() -> Self {
        Self { preset: None, cycle_secs: 30, width: 320, smooth: false, dither: true, fps: 30 }
    }
}

/// How long a preset change crossfades.
const CROSSFADE: f32 = 1.5;
/// After this long paused the picture has faded and drawing stops (most looks fade faster, see
/// [`RetroVis::fade_window`]).
pub const FADE_STOP: f32 = 8.0;
/// Waiting this long for a beat before an automatic preset change happens anyway.
const BEAT_WAIT: f32 = 3.0;

pub struct RetroVis {
    presets: Vec<Box<dyn Preset>>,
    layers: Vec<Option<Layer>>,
    cur: usize,
    /// Preset being faded in, and how far (0..1).
    next: Option<(usize, f32)>,
    out: Canvas,
    analyzer: Analyzer,
    pals: Vec<palette::Palette>,
    pub prefs: RetroPrefs,
    since_switch: f32,
    paused_for: f32,
    t: f32,
    rgba: Vec<egui::Color32>,
    seed: u32,
}

impl RetroVis {
    pub fn new(prefs: RetroPrefs) -> Self {
        let presets = presets::all();
        let n = presets.len();
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos() | 1);
        let mut v = Self {
            presets,
            layers: (0..n).map(|_| None).collect(),
            cur: 0,
            next: None,
            out: Canvas::new(1, 1),
            analyzer: Analyzer::default(),
            pals: palette::palettes(),
            prefs,
            since_switch: 0.0,
            paused_for: 0.0,
            t: 0.0,
            rgba: Vec::new(),
            seed,
        };
        v.cur = v.starting_preset();
        v
    }

    fn random(&mut self) -> u32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed
    }

    fn starting_preset(&mut self) -> usize {
        match self.prefs.preset.as_deref().and_then(|p| self.presets.iter().position(|x| x.name() == p)) {
            Some(i) => i,
            None => self.random() as usize % self.presets.len(),
        }
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.presets.iter().map(|p| p.name()).collect()
    }

    /// The preset on screen (or the one fading in).
    pub fn name(&self) -> &'static str {
        self.presets[self.next.map_or(self.cur, |n| n.0)].name()
    }

    /// Crossfade to preset `i`.
    pub fn select(&mut self, i: usize) {
        let i = i % self.presets.len();
        if i != self.cur && self.next.map(|n| n.0) != Some(i) {
            self.next = Some((i, 0.0));
            self.since_switch = 0.0;
        }
    }

    /// Next (`dir > 0`) or previous preset.
    pub fn step(&mut self, dir: i32) {
        let n = self.presets.len() as i32;
        let from = self.next.map_or(self.cur, |x| x.0) as i32;
        self.select((from + dir).rem_euclid(n) as usize);
    }

    pub fn size(&self) -> (usize, usize) {
        (self.out.w, self.out.h)
    }

    /// Frame-buffer size for a view of this aspect ratio (width / height).
    fn wanted_size(&self, aspect: f32) -> (usize, usize) {
        let w = self.prefs.width.clamp(160, 1280) as usize;
        let h = ((w as f32 / aspect.max(0.2)).round() as usize).clamp(60, w * 2);
        (w, h)
    }

    /// Advance one frame. `audio` is the latest samples being heard (oldest first) and their
    /// rate, or `None` when nothing plays. Returns false once a pause has faded out (nothing
    /// left to draw until the music starts again).
    pub fn frame(&mut self, audio: Option<(&[f32], u32)>, playing: bool, dt: f32, aspect: f32) -> bool {
        let size = self.wanted_size(aspect);
        if size != (self.out.w, self.out.h) {
            self.out = Canvas::new(size.0, size.1);
            self.layers.iter_mut().for_each(|l| *l = None);
        }
        self.paused_for = if playing { 0.0 } else { self.paused_for + dt };
        if self.paused_for > self.fade_window().1 + 0.3 {
            return false;
        }
        self.t += dt;
        let a = self.analyzer.update(audio.filter(|_| playing), dt).clone();
        // Automatic preset changes land on a beat when there is one.
        if playing && self.prefs.cycle_secs > 0 && self.next.is_none() {
            self.since_switch += dt;
            let due = self.since_switch >= self.prefs.cycle_secs as f32;
            if due && (a.beat || self.since_switch >= self.prefs.cycle_secs as f32 + BEAT_WAIT) {
                let n = self.presets.len();
                let pick = (self.cur + 1 + self.random() as usize % (n - 1)) % n;
                self.select(pick);
            }
        }
        let f = Frame { a: &a, t: self.t, dt, pal: &self.pals, paused_for: self.paused_for };
        let (w, h) = size;
        let draw = |i: usize, presets: &mut Vec<Box<dyn Preset>>, layers: &mut Vec<Option<Layer>>| {
            let layer = layers[i].get_or_insert_with(|| Layer::new(w, h));
            presets[i].draw(layer, &f);
        };
        draw(self.cur, &mut self.presets, &mut self.layers);
        let cur = self.layers[self.cur].as_ref().expect("drawn").cv.clone();
        self.out = cur;
        if let Some((n, k)) = self.next {
            draw(n, &mut self.presets, &mut self.layers);
            let k2 = (k + dt / CROSSFADE).min(1.0);
            let ease = k2 * k2 * (3.0 - 2.0 * k2);
            if let Some(l) = &self.layers[n] {
                self.out.mix_from(&l.cv, ease);
            }
            if k2 >= 1.0 {
                self.layers[self.cur] = None;
                self.cur = n;
                self.next = None;
            } else {
                self.next = Some((n, k2));
            }
        }
        // Paused: fade to black by the time drawing stops. The Dashboard keeps its starfield
        // and only fades over the last seconds.
        if self.paused_for > 0.0 {
            let (start, end) = self.fade_window();
            let k = ((self.paused_for - start) / (end - start)).clamp(0.0, 1.0);
            let keep = 1.0 - k * k * (3.0 - 2.0 * k);
            if keep < 1.0 {
                self.out.fade(keep);
            }
        }
        true
    }

    /// When, during a pause, the picture fades (start and end, in seconds paused). The Dashboard
    /// shows its starfield first.
    fn fade_window(&self) -> (f32, f32) {
        if self.presets[self.cur].holds_on_pause() {
            (FADE_STOP - 2.5, FADE_STOP)
        } else {
            (0.3, 4.2)
        }
    }

    /// The current frame as an image for an egui texture.
    pub fn image(&mut self) -> egui::ColorImage {
        self.out.to_rgba(&mut self.rgba, self.prefs.dither);
        egui::ColorImage::new([self.out.w, self.out.h], std::mem::take(&mut self.rgba))
    }

    /// The current frame (the preview tool writes it to PNG).
    pub fn canvas(&self) -> &Canvas {
        &self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * 0.05).sin() * 0.5 + (i as f32 * 0.003).sin() * 0.5).collect()
    }

    #[test]
    fn follows_the_view_shape_and_fades_out_when_paused() {
        let mut v = RetroVis::new(RetroPrefs { preset: Some("Rings".into()), cycle_secs: 0, ..Default::default() });
        let audio = sine(4096);
        assert!(v.frame(Some((&audio, 48_000)), true, 1.0 / 30.0, 16.0 / 9.0));
        assert_eq!(v.size(), (320, 180));
        v.frame(Some((&audio, 48_000)), true, 1.0 / 30.0, 2.0);
        assert_eq!(v.size(), (320, 160));
        // Paused: keeps animating the fade for a while, then stops on a black picture.
        let mut frames = 0;
        while v.frame(None, false, 0.1, 2.0) {
            frames += 1;
            assert!(frames < 200);
        }
        assert!((40..=50).contains(&frames), "{frames} fade frames");
        assert!(v.canvas().brightness() < 1e-3, "faded out");
    }

    #[test]
    fn stepping_crossfades_to_the_next_preset() {
        let mut v = RetroVis::new(RetroPrefs { preset: Some("Scope".into()), cycle_secs: 0, ..Default::default() });
        let audio = sine(4096);
        v.step(1);
        assert_eq!(v.name(), "Rings");
        for _ in 0..60 {
            v.frame(Some((&audio, 48_000)), true, 1.0 / 30.0, 16.0 / 9.0);
        }
        assert_eq!(v.name(), "Rings");
        assert!(v.next.is_none(), "crossfade finished");
        let img = v.image();
        assert_eq!(img.size, [320, 180]);
        v.step(-2);
        assert_eq!(v.name(), "Cloud", "wraps around");
    }

    #[test]
    fn three_d_looks_fade_out_too() {
        // Tunnel draws its rings whatever the audio does; the pause fade still darkens it.
        let mut v = RetroVis::new(RetroPrefs { preset: Some("Tunnel".into()), cycle_secs: 0, ..Default::default() });
        let audio = sine(4096);
        for _ in 0..30 {
            v.frame(Some((&audio, 48_000)), true, 1.0 / 30.0, 16.0 / 9.0);
        }
        let lit = v.canvas().brightness();
        for _ in 0..40 {
            v.frame(None, false, 0.1, 16.0 / 9.0);
        }
        assert!(v.canvas().brightness() < lit * 0.2, "darker after 4 s paused");
    }

    #[test]
    fn auto_cycle_changes_preset() {
        let mut v = RetroVis::new(RetroPrefs { preset: Some("Scope".into()), cycle_secs: 1, ..Default::default() });
        let audio = sine(4096);
        for _ in 0..(30 * 6) {
            v.frame(Some((&audio, 48_000)), true, 1.0 / 30.0, 16.0 / 9.0);
        }
        assert_ne!(v.name(), "Scope");
    }
}
