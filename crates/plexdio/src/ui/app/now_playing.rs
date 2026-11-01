//! The Now playing page: the retro visualiser filling the view (or the whole screen), with the
//! song's cover and title fading in on track changes and mouse movement, like the Xbox
//! dashboard. Keys: ↑/↓ change the preset, A toggles automatic changes, F11 or a double-click
//! toggles full screen, Esc leaves full screen (then the page).

use super::{PlexdioApp, Route};
use crate::audio::engine::PlayState;
use crate::ui::icons::{self, Icon};
use crate::ui::retrovis::{RetroPrefs, RetroVis};
use crate::ui::theme;
use egui::{Align2, Color32, FontId, Rect, RichText};

/// Seconds the overlay stays after the last mouse move or track change, and its fade time.
const OVERLAY_SECS: f64 = 3.0;
const OVERLAY_FADE: f64 = 0.6;

#[derive(Default)]
pub struct NowPlayingState {
    vis: Option<RetroVis>,
    tex: Option<egui::TextureHandle>,
    /// egui time of the last visualiser frame.
    last_frame: f64,
    /// The visualiser still has something to draw (false once a pause has faded out).
    animating: bool,
    samples: Vec<f32>,
    /// egui time of the last mouse move or track change (shows the overlay).
    activity: f64,
    track: Option<i64>,
    /// Full screen wanted, and what the window currently is.
    pub fullscreen: bool,
    pub fullscreen_applied: bool,
    /// The preset name flashes up briefly after a change.
    preset_shown: f64,
    /// Frames drawn and effect time since `stats_since` (logged every 10 s).
    stats_frames: u32,
    /// Flipped every visualiser frame (see the tint below).
    flip: bool,
    stats_ms: f64,
    stats_since: f64,
}

impl PlexdioApp {
    pub(super) fn now_playing_view(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let now = ui.input(|i| i.time);
        let ctx = ui.ctx().clone();
        let prefs = self.config.retro.clone();
        let playing = self.playback.state == PlayState::Playing && self.cast.is_none();
        let track = self.playback.track.as_ref().map(|t| t.id);
        let np = &mut self.np;
        if track != np.track {
            np.track = track;
            np.activity = now;
        }
        let vis = np.vis.get_or_insert_with(|| RetroVis::new(prefs.clone()));
        vis.prefs = prefs.clone();

        // A new visualiser frame at the chosen rate.
        let interval = 1.0 / prefs.fps.clamp(15, 60) as f64;
        if np.tex.is_none() || (now - np.last_frame >= interval * 0.9 && (np.animating || playing)) {
            let dt = if np.last_frame == 0.0 { interval } else { (now - np.last_frame).min(0.25) } as f32;
            np.last_frame = now;
            let mut rate = 48_000;
            let have = match self.engine.as_ref().filter(|_| playing) {
                Some(e) => e.scope().lock().map(|sc| {
                    rate = sc.rate;
                    sc.audible(crate::ui::visualiser::N, &mut np.samples)
                }).unwrap_or(false),
                None => false,
            };
            let audio = have.then_some((np.samples.as_slice(), rate));
            let clock = std::time::Instant::now();
            np.animating = vis.frame(audio, playing, dt, rect.aspect_ratio());
            np.stats_ms += clock.elapsed().as_secs_f64() * 1000.0;
            np.stats_frames += 1;
            if now - np.stats_since >= 10.0 {
                if np.stats_frames > 1 && np.stats_since > 0.0 {
                    let (w, h) = vis.size();
                    log::info!(
                        "visualiser {}: {:.1} fps, {:.2} ms per frame for the effect at {w}×{h}",
                        vis.name(),
                        np.stats_frames as f64 / (now - np.stats_since),
                        np.stats_ms / np.stats_frames as f64
                    );
                }
                np.stats_since = now;
                np.stats_frames = 0;
                np.stats_ms = 0.0;
            }
            let opts = if prefs.smooth { egui::TextureOptions::LINEAR } else { egui::TextureOptions::NEAREST };
            np.flip = !np.flip;
            let img = vis.image();
            match &mut np.tex {
                // The software renderer keeps a texture's first size, so a new size (the view
                // changed shape, e.g. full screen) needs a new texture.
                Some(t) if t.size() == img.size => t.set(img, opts),
                _ => np.tex = Some(ctx.load_texture("retrovis", img, opts)),
            }
        }
        if let Some(t) = &np.tex {
            // The software renderer caches drawn shapes by geometry, colour and texture id, not
            // by texture contents: alternating an invisible tint makes every new frame count.
            let tint = if np.flip { Color32::WHITE } else { Color32::from_rgb(254, 254, 254) };
            ui.painter().image(t.id(), rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), tint);
        }
        if np.animating || playing {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(interval));
        }

        // Input: mouse movement shows the overlay; double-click toggles full screen.
        let resp = ui.interact(rect, ui.scope_id().with("np-surface"), egui::Sense::click());
        if ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) && resp.hovered() {
            np.activity = now;
        }
        if resp.double_clicked() {
            np.fullscreen = !np.fullscreen;
        }
        let typing = ctx.egui_wants_keyboard_input();
        let (up, down, auto, f11, esc) = ui.input_mut(|i| {
            (
                !typing && i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                !typing && i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                !typing && i.consume_key(egui::Modifiers::NONE, egui::Key::A),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F11),
                !typing && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        let mut step = 0;
        if up {
            step = -1;
        }
        if down {
            step = 1;
        }
        if f11 {
            np.fullscreen = !np.fullscreen;
        }
        let mut leave = false;
        if esc {
            if np.fullscreen {
                np.fullscreen = false;
            } else {
                leave = true;
            }
        }
        let mut toggle_auto = auto;

        // The overlay.
        let since = now - np.activity;
        let alpha = (1.0 - ((since - OVERLAY_SECS) / OVERLAY_FADE).clamp(0.0, 1.0)) as f32;
        if alpha > 0.0 && alpha < 1.0 {
            ctx.request_repaint();
        } else if alpha >= 1.0 {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((OVERLAY_SECS - since).max(0.05)));
        }
        if np.fullscreen && alpha <= 0.0 && resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
        let name = vis.name();
        let auto_on = prefs.cycle_secs > 0;
        let fullscreen = np.fullscreen;
        let preset_alpha = alpha.max((1.0 - ((now - np.preset_shown - 1.5) / 0.5).clamp(0.0, 1.0)) as f32);
        if alpha > 0.0 || preset_alpha > 0.0 {
            let mut ov = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(18.0)).layout(egui::Layout::top_down(egui::Align::LEFT)));
            ov.multiply_opacity(alpha);
            // Top row: back, preset switcher, auto, full screen.
            ov.horizontal(|ui| {
                if !fullscreen && icons::button(ui, Icon::Back, 26.0, false).on_hover_text("Back (Esc)").clicked() {
                    leave = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let tip = if fullscreen { "Leave full screen (F11, Esc)" } else { "Full screen (F11)" };
                    if icons::button(ui, Icon::Fullscreen, 24.0, fullscreen).on_hover_text(tip).clicked() {
                        self.np.fullscreen = !self.np.fullscreen;
                    }
                    ui.add_space(8.0);
                    let auto_text = RichText::new("Auto").size(13.0).color(if auto_on { Color32::BLACK } else { theme::TEXT });
                    let fill = if auto_on { theme::ACCENT } else { Color32::from_black_alpha(140) };
                    if ui.add(egui::Button::new(auto_text).fill(fill).corner_radius(12)).on_hover_text("Change the look by itself (A)").clicked() {
                        toggle_auto = true;
                    }
                    ui.add_space(8.0);
                    if icons::button(ui, Icon::Next, 22.0, false).on_hover_text("Next look (↓)").clicked() {
                        step = 1;
                    }
                    ui.label(RichText::new(name).size(15.0).strong().color(theme::TEXT));
                    if icons::button(ui, Icon::Prev, 22.0, false).on_hover_text("Previous look (↑)").clicked() {
                        step = -1;
                    }
                });
            });
        }
        // The preset name also flashes up on its own after a change.
        if preset_alpha > alpha {
            let p = ui.painter().with_clip_rect(rect);
            let pos = egui::pos2(rect.right() - 24.0, rect.top() + 30.0);
            p.text(pos, Align2::RIGHT_CENTER, name, FontId::proportional(18.0), Color32::WHITE.gamma_multiply(preset_alpha));
        }
        if alpha > 0.0 {
            self.now_playing_card(ui, rect, alpha);
        }

        if step != 0 {
            if let Some(v) = &mut self.np.vis {
                v.step(step);
            }
            self.np.preset_shown = now;
            self.np.animating = true;
        }
        if toggle_auto {
            self.config.retro.cycle_secs = if self.config.retro.cycle_secs > 0 { 0 } else { 30 };
            self.save_config();
        }
        if leave {
            self.np.fullscreen = false;
            self.back();
            if self.route == Route::NowPlaying {
                self.route = Route::Home;
            }
        }
    }

    /// Cover, title and artist at the bottom left, over a soft dark band.
    fn now_playing_card(&mut self, ui: &mut egui::Ui, rect: Rect, alpha: f32) {
        let p = ui.painter().with_clip_rect(rect);
        let band = Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 150.0), rect.max);
        let mut mesh = egui::Mesh::default();
        let (top, bottom) = (Color32::from_black_alpha(0), Color32::from_black_alpha((170.0 * alpha) as u8));
        mesh.colored_vertex(band.left_top(), top);
        mesh.colored_vertex(band.right_top(), top);
        mesh.colored_vertex(band.right_bottom(), bottom);
        mesh.colored_vertex(band.left_bottom(), bottom);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        p.add(mesh);
        let Some(t) = self.playback.track.clone() else {
            p.text(rect.center(), Align2::CENTER_CENTER, "Play something to start the visualiser", FontId::proportional(18.0), theme::TEXT_DIM.gamma_multiply(alpha));
            return;
        };
        let thumb = self.library.as_ref().and_then(|l| l.track(t.id)).and_then(|t| t.thumb.as_deref().map(str::to_string));
        let art = Rect::from_min_size(egui::pos2(rect.left() + 28.0, rect.bottom() - 124.0), egui::vec2(96.0, 96.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(art));
        child.multiply_opacity(alpha);
        self.art.paint(&child, art, thumb.as_deref(), &t.album, crate::ui::art::Shape::Rounded(6));
        let x = art.right() + 18.0;
        let w = (rect.right() - x - 28.0).max(60.0);
        let white = Color32::WHITE.gamma_multiply(alpha);
        let dim = theme::TEXT_DIM.gamma_multiply(alpha);
        crate::ui::views::common::clipped(ui, egui::pos2(x, art.top() + 30.0), &t.title, 26.0, white, w, Align2::LEFT_CENTER);
        let sub = if t.album.is_empty() { t.artist.clone() } else { format!("{} · {}", t.artist, t.album) };
        crate::ui::views::common::clipped(ui, egui::pos2(x, art.top() + 64.0), &sub, 15.0, dim, w, Align2::LEFT_CENTER);
    }

    pub(super) fn settings_now_playing(&mut self, ui: &mut egui::Ui, changed: &mut bool) {
        let names = self.np.vis.as_ref().map(|v| v.names()).unwrap_or_else(|| RetroVis::new(RetroPrefs::default()).names());
        let r = &mut self.config.retro;
        let before = r.clone();
        super::setting_row(ui, "Start with", Some("The look the page opens with"), |ui| {
            let current = r.preset.clone().unwrap_or_else(|| "Random".into());
            egui::ComboBox::from_id_salt("retro-preset").selected_text(current).width(120.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut r.preset, None, "Random");
                for n in &names {
                    ui.selectable_value(&mut r.preset, Some(n.to_string()), *n);
                }
            });
        });
        super::setting_row(ui, "Change the look", Some("Switch to another look by itself, on a beat"), |ui| {
            let label = |s: u32| if s == 0 { "Never".to_string() } else { format!("Every {s} s") };
            egui::ComboBox::from_id_salt("retro-cycle").selected_text(label(r.cycle_secs)).width(120.0).show_ui(ui, |ui| {
                for s in [0, 15, 30, 60] {
                    ui.selectable_value(&mut r.cycle_secs, s, label(s));
                }
            });
        });
        super::setting_row(ui, "Resolution", Some("Lower is chunkier and lighter on the computer"), |ui| {
            let label = |w: u32| match w {
                320 => "Low (320)",
                480 => "Medium (480)",
                _ => "High (640)",
            };
            egui::ComboBox::from_id_salt("retro-res").selected_text(label(r.width)).width(120.0).show_ui(ui, |ui| {
                for w in [320, 480, 640] {
                    ui.selectable_value(&mut r.width, w, label(w));
                }
            });
        });
        super::setting_row(ui, "Smooth pixels", Some("Off keeps the sharp, blocky look of the old consoles"), |ui| {
            theme::toggle(ui, &mut r.smooth);
        });
        super::setting_row(ui, "Dithering", Some("The fine dot pattern of PS1-era graphics"), |ui| {
            theme::toggle(ui, &mut r.dither);
        });
        super::setting_row(ui, "Frame rate", Some("60 is smoother and uses more of the computer"), |ui| {
            egui::ComboBox::from_id_salt("retro-fps").selected_text(format!("{} fps", r.fps)).width(120.0).show_ui(ui, |ui| {
                for f in [30, 60] {
                    ui.selectable_value(&mut r.fps, f, format!("{f} fps"));
                }
            });
        });
        if *r != before {
            *changed = true;
        }
    }
}
