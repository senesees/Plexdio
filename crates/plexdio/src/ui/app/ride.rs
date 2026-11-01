//! Ride mode on the Now playing page: `G` (or the Ride button) turns the song into a road to
//! ride, over the visualiser. The track is built in the background while the music plays, the
//! ride starts wherever the song is, and it carries on into the next song by itself.

use super::PlexdioApp;
use crate::audio::source::{AudioCache, SourceSpec};
use crate::game::analysis::Analysis;
use crate::game::build::Build;
use crate::game::clock::SongClock;
use crate::game::mapgen::{generate, seed_for};
use crate::game::play::{Input, Ride};
use crate::game::render::{draw, RoadGeom};
use crate::ui::retrovis::canvas::Canvas;
use crate::ui::runner::VideoFrame;
use crate::ui::theme;
use egui::{Align2, Color32, FontId, Rect};
use std::collections::HashMap;
use std::rc::Rc;
use symphonia::core::io::MediaSource;

/// Width of the game picture (the height follows the view); sharper than the visualiser's
/// default so notes read at a distance.
const WIDTH: usize = 480;
/// Analyses kept in memory (a few songs back and ahead).
const KEEP: usize = 8;
/// How long the end-of-song result stays up.
const RESULT_SECS: f64 = 6.0;

#[derive(Default)]
pub struct RideState {
    /// Ride mode is on (it keeps going from song to song until turned off).
    pub on: bool,
    build: Option<Build>,
    session: Option<Session>,
    analyses: HashMap<i64, Rc<Analysis>>,
    canvas: Option<Canvas>,
    /// The pointer's last position, to tell when it moves.
    pointer: Option<egui::Pos2>,
    /// Steering gathered since the last game frame (UI passes happen more often than frames).
    pending: Input,
    /// Where the pointer last was across the view (a new ride starts steering there).
    pointer_x: Option<f32>,
    /// A message under the HUD (an error, or the last song's result) and when it appeared.
    notice: Option<(String, f64)>,
    /// Game frames and drawing time since the last log line (every 10 s).
    stats: (u32, f64, Option<std::time::Instant>),
    /// Fine timer resolution while a ride is on screen (60 fps frame timing on Windows).
    timer: Option<FineTimer>,
}

/// Raises the system timer resolution to 1 ms while it lives. Windows otherwise wakes the event
/// loop on 15.6 ms ticks, which turns a 60 fps frame request into 30–40 fps.
pub struct FineTimer;

impl FineTimer {
    fn new() -> Self {
        #[cfg(windows)]
        // SAFETY: plain call; balanced by timeEndPeriod in Drop.
        unsafe {
            windows_sys::Win32::Media::timeBeginPeriod(1);
        }
        FineTimer
    }
}

impl Drop for FineTimer {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: balances the timeBeginPeriod(1) in `new`.
        unsafe {
            windows_sys::Win32::Media::timeEndPeriod(1);
        }
    }
}

struct Session {
    track: i64,
    ride: Ride,
    geom: RoadGeom,
    clock: SongClock,
    reported: bool,
    /// Just started: steer towards the pointer straight away.
    fresh: bool,
}

impl RideState {
    /// A ride is on screen (the page draws at the game's frame rate).
    pub fn riding(&self) -> bool {
        self.session.is_some()
    }

    /// Collect steering from the pointer over `rect` and the arrow keys (every UI pass; the
    /// next game frame uses it).
    pub fn input(&mut self, ui: &egui::Ui, rect: Rect) {
        let (hover, left, right) = ui.input(|i| (i.pointer.hover_pos(), i.key_down(egui::Key::ArrowLeft), i.key_down(egui::Key::ArrowRight)));
        if let Some(p) = hover.filter(|p| rect.contains(*p)) {
            let x = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            self.pointer_x = Some(x);
            if Some(p) != self.pointer {
                self.pending.pointer = Some(x);
            }
        }
        if hover.is_some() {
            self.pointer = hover;
        }
        self.pending.left = left;
        self.pending.right = right;
    }

    /// Advance the ride and draw it into `video` over the visualiser `background`. Returns
    /// false when there is no ride (the caller shows the plain visualiser).
    #[allow(clippy::too_many_arguments)]
    pub fn frame(&mut self, background: &Canvas, position_ms: u64, playing: bool, dt: f32, aspect: f32, dither: bool, video: &mut VideoFrame) -> bool {
        let Some(s) = &mut self.session else {
            self.timer = None;
            return false;
        };
        self.timer.get_or_insert_with(FineTimer::new);
        let mut input = std::mem::take(&mut self.pending);
        // Held keys stay held.
        self.pending.left = input.left;
        self.pending.right = input.right;
        if s.fresh {
            s.fresh = false;
            input.pointer = input.pointer.or(self.pointer_x);
        }
        let t = s.clock.update(position_ms, playing, dt as f64) as f32;
        s.ride.update(t, if playing { input } else { Input::default() }, dt);
        let h = ((WIDTH as f32 / aspect.max(0.2)).round() as usize).clamp(90, WIDTH * 2);
        let canvas = self.canvas.get_or_insert_with(|| Canvas::new(WIDTH, h));
        if canvas.h != h {
            *canvas = Canvas::new(WIDTH, h);
        }
        let clock = std::time::Instant::now();
        draw(canvas, background, &s.ride, &s.geom, t);
        canvas.to_rgb_u32(&mut video.pixels, dither);
        let (frames, ms, since) = &mut self.stats;
        *frames += 1;
        *ms += clock.elapsed().as_secs_f64() * 1000.0;
        let since = since.get_or_insert_with(std::time::Instant::now);
        if since.elapsed().as_secs() >= 10 {
            log::info!("ride: {:.1} fps, {:.2} ms per frame for the game at {}×{}", *frames as f64 / since.elapsed().as_secs_f64(), *ms / *frames as f64, canvas.w, canvas.h);
            self.stats = (0, 0.0, Some(std::time::Instant::now()));
        }
        video.w = canvas.w;
        video.h = canvas.h;
        true
    }
}

impl PlexdioApp {
    /// Turn ride mode on or off.
    pub(super) fn toggle_ride(&mut self) {
        let r = &mut self.np.ride;
        r.on = !r.on;
        if !r.on {
            r.build = None;
            r.session = None;
            r.notice = None;
            r.timer = None;
        }
    }

    /// Keep ride mode in step with what is playing: build the song's track, start the ride,
    /// finish it, move on to the next song. Called every frame the page is shown.
    pub(super) fn ride_manage(&mut self, now: f64, ctx: &egui::Context) {
        if !self.np.ride.on {
            return;
        }
        if self.cast.is_some() {
            self.np.ride.on = false;
            self.np.ride.session = None;
            self.np.ride.build = None;
            self.np.ride.notice = Some(("Ride needs the music to play on this computer.".into(), now));
            return;
        }
        let Some(track) = self.playback.track.as_ref().map(|t| t.id) else { return };
        let position = self.playback.position_ms as f32 / 1000.0;

        // A finished build.
        if let Some(b) = &self.np.ride.build {
            if let Some(result) = b.take() {
                let id = b.track;
                self.np.ride.build = None;
                match result {
                    Ok(a) => {
                        let r = &mut self.np.ride;
                        if r.analyses.len() >= KEEP {
                            if let Some(&old) = r.analyses.keys().find(|&&k| k != track) {
                                r.analyses.remove(&old);
                            }
                        }
                        r.analyses.insert(id, Rc::new(a));
                    }
                    Err(e) => {
                        self.np.ride.notice = Some((format!("Couldn't build a track for this song ({e})."), now));
                        if id == track {
                            self.np.ride.on = false;
                            return;
                        }
                    }
                }
            }
        }

        // The song changed: the old ride is over.
        if self.np.ride.session.as_ref().is_some_and(|s| s.track != track) {
            self.finish_ride(now);
            self.np.ride.session = None;
        }
        if self.np.ride.session.is_none() {
            if let Some(a) = self.np.ride.analyses.get(&track).cloned() {
                let difficulty = self.config.ride.difficulty;
                let map = generate(&a, difficulty, seed_for(&track.to_string()));
                let geom = RoadGeom::new(&map);
                log::info!("ride: starting track {track} at {position:.1} s ({}, {} notes)", difficulty.label(), map.colour_notes());
                self.np.ride.session = Some(Session { track, ride: Ride::new(map, position), geom, clock: SongClock::default(), reported: false, fresh: true });
            } else if self.np.ride.build.as_ref().is_none_or(|b| b.track != track) {
                self.start_build(track, ctx);
            }
        }
        // Report the result once the song's map has played out (the next song follows).
        if let Some(s) = &self.np.ride.session {
            if s.ride.finished() && !s.reported {
                self.finish_ride(now);
            }
        }
    }

    /// Put the finished (or interrupted) ride's result up.
    fn finish_ride(&mut self, now: f64) {
        let Some(s) = &mut self.np.ride.session else { return };
        if s.reported || s.ride.caught + s.ride.missed == 0 {
            return;
        }
        s.reported = true;
        let r = &s.ride;
        let msg = format!("{} points · {:.0} % caught · best combo {}", group(r.score), r.accuracy() * 100.0, r.best_combo);
        log::info!("ride: track {} finished: {msg}", s.track);
        self.np.ride.notice = Some((msg, now));
    }

    /// Start analysing `track` from the same source the player uses.
    fn start_build(&mut self, track: i64, ctx: &egui::Context) {
        let Some(spec) = self.track_specs(&[track]).into_iter().next() else {
            self.np.ride.notice = Some(("This song has no audio to build a track from.".into(), 0.0));
            return;
        };
        let ext = spec.ext_hint().map(str::to_string);
        let source = spec.source.clone();
        let http = self.http.clone();
        let cache = AudioCache::new(crate::paths::audio_cache_dir(), self.config.audio_cache_mb.saturating_mul(1024 * 1024), self.config.audio_cache_enabled);
        let ctx = ctx.clone();
        log::info!("ride: building a track for {track}");
        self.np.ride.build = Some(Build::start(
            track,
            Box::new(move || {
                let src: Box<dyn MediaSource> = match &source {
                    SourceSpec::Local(p) => Box::new(std::fs::File::open(p).map_err(|e| format!("open {}: {e}", p.display()))?),
                    // A second reader on the player's own download (or the cached file).
                    SourceSpec::Http { .. } => cache.open(&http, &source)?,
                };
                Ok((src, ext))
            }),
            move || ctx.request_repaint(),
        ));
    }

    /// Score, multiplier, combo and progress; "building" while the track is made; the last
    /// result for a while.
    pub(super) fn ride_hud(&mut self, ui: &egui::Ui, rect: Rect, now: f64) {
        let r = &self.np.ride;
        if !r.on {
            return;
        }
        let p = ui.painter().with_clip_rect(rect);
        let shadow = Color32::from_black_alpha(160);
        let text = |pos: egui::Pos2, align: Align2, s: &str, size: f32, c: Color32| {
            p.text(pos + egui::vec2(1.5, 1.5), align, s, FontId::proportional(size), shadow);
            p.text(pos, align, s, FontId::proportional(size), c);
        };
        let top = rect.top() + 62.0;
        match (&r.session, &r.build) {
            (Some(s), _) => {
                let ride = &s.ride;
                text(egui::pos2(rect.left() + 28.0, top), Align2::LEFT_TOP, &group(ride.score), 30.0, Color32::WHITE);
                let mult = ride.multiplier();
                let line = if ride.combo > 0 { format!("×{mult}   combo {}", ride.combo) } else { format!("×{mult}") };
                text(egui::pos2(rect.left() + 30.0, top + 38.0), Align2::LEFT_TOP, &line, 15.0, if mult > 1 { theme::ACCENT } else { theme::TEXT_DIM });
                // Progress through the song along the top edge.
                let f = (s.clock.now() as f32 / ride.map.duration.max(1.0)).clamp(0.0, 1.0);
                let bar = Rect::from_min_size(rect.left_top() + egui::vec2(0.0, 2.0), egui::vec2(rect.width() * f, 3.0));
                p.rect_filled(bar, 0.0, theme::ACCENT.gamma_multiply(0.8));
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            }
            (None, Some(b)) => {
                let pct = (b.progress() * 100.0).round();
                text(rect.center(), Align2::CENTER_CENTER, &format!("Building the track… {pct:.0} %"), 20.0, Color32::WHITE);
                text(rect.center() + egui::vec2(0.0, 28.0), Align2::CENTER_CENTER, "The ride starts where the song is. Steer with the mouse or ← →.", 14.0, theme::TEXT_DIM);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            }
            (None, None) => {}
        }
        if let Some((msg, at)) = &r.notice {
            let age = now - at;
            if age < RESULT_SECS {
                let a = (1.0 - ((age - RESULT_SECS + 1.0).max(0.0)) as f32).clamp(0.0, 1.0);
                text(egui::pos2(rect.center().x, rect.top() + 70.0), Align2::CENTER_TOP, msg, 17.0, Color32::WHITE.gamma_multiply(a));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }
}

/// 12345 → "12 345".
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('\u{2009}');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn groups_thousands() {
        assert_eq!(super::group(0), "0");
        assert_eq!(super::group(999), "999");
        assert_eq!(super::group(12_345), "12\u{2009}345");
        assert_eq!(super::group(1_234_567), "1\u{2009}234\u{2009}567");
    }
}
