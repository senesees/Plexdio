//! Spotify-style equaliser editor: the real frequency response drawn as a curve, with one
//! handle per band to drag up or down. Double-click a handle to reset that band.

use crate::audio::eq::{response_db, BANDS, BAND_COUNT, MAX_DB};
use crate::ui::theme;
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Sense, Shape, Stroke};

/// What happened in the editor this frame.
#[derive(Default)]
pub struct EqEdit {
    /// A gain changed (send to the engine now).
    pub changed: bool,
    /// An edit finished (drag released, click, reset): a good moment to save.
    pub committed: bool,
}

const HEIGHT: f32 = 190.0;
const LEFT: f32 = 46.0;
const BOTTOM: f32 = 22.0;

/// Horizontal position of `freq` on a log axis where the bands are evenly spaced.
fn x_of(plot: Rect, freq: f32) -> f32 {
    let lo = BANDS[0].log2() - 0.5;
    let hi = BANDS[BAND_COUNT - 1].log2() + 0.5;
    plot.left() + (freq.log2() - lo) / (hi - lo) * plot.width()
}

fn y_of(plot: Rect, db: f32) -> f32 {
    plot.center().y - db / MAX_DB * (plot.height() / 2.0 - 8.0)
}

fn db_of(plot: Rect, y: f32) -> f32 {
    let db = (plot.center().y - y) / (plot.height() / 2.0 - 8.0) * MAX_DB;
    ((db * 2.0).round() / 2.0).clamp(-MAX_DB, MAX_DB)
}

fn freq_label(f: f32) -> String {
    if f >= 1_000.0 {
        format!("{}K", (f / 1_000.0).round())
    } else {
        format!("{f}")
    }
}

pub fn show(ui: &mut egui::Ui, gains: &mut [f32; BAND_COUNT]) -> EqEdit {
    let mut edit = EqEdit::default();
    let enabled = ui.is_enabled();
    let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover());
    let plot = Rect::from_min_max(pos2(outer.left() + LEFT, outer.top() + 4.0), pos2(outer.right() - 8.0, outer.bottom() - BOTTOM));
    let p = ui.painter_at(outer);
    let accent = if enabled { theme::ACCENT } else { theme::TEXT_DIM.gamma_multiply(0.6) };

    // Grid and dB labels.
    for db in [MAX_DB, MAX_DB / 2.0, 0.0, -MAX_DB / 2.0, -MAX_DB] {
        let y = y_of(plot, db);
        let strong = db == 0.0;
        p.line_segment(
            [pos2(plot.left(), y), pos2(plot.right(), y)],
            Stroke::new(1.0, if strong { Color32::from_rgb(70, 70, 70) } else { Color32::from_rgb(44, 44, 44) }),
        );
        let label = if db > 0.0 { format!("+{db:.0} dB") } else { format!("{db:.0} dB") };
        p.text(pos2(plot.left() - 8.0, y), Align2::RIGHT_CENTER, label, FontId::proportional(11.0), theme::TEXT_DIM);
    }
    for &f in &BANDS {
        p.text(pos2(x_of(plot, f), plot.bottom() + 12.0), Align2::CENTER_CENTER, freq_label(f), FontId::proportional(11.0), theme::TEXT_DIM);
    }

    // Interaction first (so the handles reflect this frame's value).
    let col_w = plot.width() / BAND_COUNT as f32;
    let mut active: Option<usize> = None;
    for i in 0..BAND_COUNT {
        let cx = x_of(plot, BANDS[i]);
        let strip = Rect::from_min_max(pos2(cx - col_w / 2.0, plot.top()), pos2(cx + col_w / 2.0, plot.bottom()));
        let resp = ui.interact(strip, ui.scope_id().with(("eq-band", i)), Sense::click_and_drag());
        if resp.double_clicked() {
            gains[i] = 0.0;
            edit.changed = true;
            edit.committed = true;
        } else if resp.dragged() || resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let db = db_of(plot, pos.y);
                if db != gains[i] {
                    gains[i] = db;
                    edit.changed = true;
                }
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            edit.committed = true;
        }
        if resp.dragged() || resp.hovered() {
            active = Some(i);
        }
        if enabled {
            resp.on_hover_cursor(egui::CursorIcon::ResizeVertical);
        }
    }

    // Response curve, filled down to 0 dB.
    let steps = 160;
    let lo = (BANDS[0].log2() - 0.5).exp2();
    let hi = (BANDS[BAND_COUNT - 1].log2() + 0.5).exp2();
    let curve: Vec<egui::Pos2> = (0..=steps)
        .map(|k| {
            let f = lo * (hi / lo).powf(k as f32 / steps as f32);
            let db = response_db(gains, f).clamp(-MAX_DB - 2.0, MAX_DB + 2.0);
            pos2(x_of(plot, f), y_of(plot, db))
        })
        .collect();
    let zero = y_of(plot, 0.0);
    let fill = accent.gamma_multiply(0.18);
    for w in curve.windows(2) {
        // Thin quads between the curve and the 0 dB line.
        let quad = vec![w[0], w[1], pos2(w[1].x, zero), pos2(w[0].x, zero)];
        p.add(Shape::convex_polygon(quad, fill, Stroke::NONE));
    }
    p.add(Shape::line(curve, Stroke::new(2.0, accent)));

    // Band handles.
    for (i, &g) in gains.iter().enumerate() {
        let c = pos2(x_of(plot, BANDS[i]), y_of(plot, g));
        let hot = active == Some(i) && enabled;
        p.circle_filled(c, if hot { 7.0 } else { 5.5 }, if hot { Color32::WHITE } else { accent });
        if hot {
            let label = if g > 0.0 { format!("+{g:.1} dB") } else { format!("{g:.1} dB") };
            p.text(pos2(c.x, c.y - 16.0), Align2::CENTER_CENTER, label, FontId::proportional(12.0), theme::TEXT);
        }
    }
    edit
}
