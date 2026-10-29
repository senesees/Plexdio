//! Vector icons drawn with the egui painter (no icon font needed, crisp at any DPI).

use egui::{pos2, vec2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Ui};
use std::f32::consts::{PI, TAU};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Next,
    Prev,
    Shuffle,
    Repeat,
    RepeatOne,
    Volume,
    Mute,
    Queue,
    Home,
    Search,
    Library,
    Playlist,
    Settings,
    Star,
    StarFilled,
    Radio,
    MiniPlayer,
    Close,
    Back,
    Refresh,
    Plus,
    /// Three dots (more options).
    More,
    /// Microphone (lyrics).
    Lyrics,
    /// Arrow into a tray (download for offline).
    Download,
    /// Screen with signal arcs (play on another device).
    Cast,
    Trash,
    Up,
    Down,
}

/// Paint `icon` into `rect` using `color`.
pub fn paint(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height());
    let u = s / 24.0; // design grid: 24x24
    let at = |x: f32, y: f32| pos2(c.x + (x - 12.0) * u, c.y + (y - 12.0) * u);
    let stroke = Stroke::new((2.0 * u).max(1.2), color);
    let fill_poly = |pts: Vec<Pos2>| Shape::convex_polygon(pts, color, Stroke::NONE);
    match icon {
        Icon::Play => {
            p.add(fill_poly(vec![at(7.0, 4.5), at(19.5, 12.0), at(7.0, 19.5)]));
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(at(6.0, 4.5), at(10.0, 19.5)), 1.0 * u, color);
            p.rect_filled(Rect::from_min_max(at(14.0, 4.5), at(18.0, 19.5)), 1.0 * u, color);
        }
        Icon::Next => {
            p.add(fill_poly(vec![at(5.0, 5.0), at(16.0, 12.0), at(5.0, 19.0)]));
            p.rect_filled(Rect::from_min_max(at(16.5, 5.0), at(19.0, 19.0)), 0.5 * u, color);
        }
        Icon::Prev => {
            p.add(fill_poly(vec![at(19.0, 5.0), at(19.0, 19.0), at(8.0, 12.0)]));
            p.rect_filled(Rect::from_min_max(at(5.0, 5.0), at(7.5, 19.0)), 0.5 * u, color);
        }
        Icon::Shuffle => {
            p.line_segment([at(3.0, 7.0), at(7.0, 7.0)], stroke);
            p.line_segment([at(7.0, 7.0), at(15.0, 17.0)], stroke);
            p.line_segment([at(15.0, 17.0), at(19.0, 17.0)], stroke);
            p.line_segment([at(3.0, 17.0), at(7.0, 17.0)], stroke);
            p.line_segment([at(7.0, 17.0), at(15.0, 7.0)], stroke);
            p.line_segment([at(15.0, 7.0), at(19.0, 7.0)], stroke);
            p.add(fill_poly(vec![at(18.0, 4.0), at(22.0, 7.0), at(18.0, 10.0)]));
            p.add(fill_poly(vec![at(18.0, 14.0), at(22.0, 17.0), at(18.0, 20.0)]));
        }
        Icon::Repeat | Icon::RepeatOne => {
            p.add(Shape::line(
                vec![at(5.0, 14.0), at(5.0, 7.0), at(17.0, 7.0)],
                stroke,
            ));
            p.add(fill_poly(vec![at(16.0, 4.0), at(20.0, 7.0), at(16.0, 10.0)]));
            p.add(Shape::line(
                vec![at(19.0, 10.0), at(19.0, 17.0), at(7.0, 17.0)],
                stroke,
            ));
            p.add(fill_poly(vec![at(8.0, 14.0), at(4.0, 17.0), at(8.0, 20.0)]));
            if icon == Icon::RepeatOne {
                p.text(
                    at(12.0, 12.2),
                    egui::Align2::CENTER_CENTER,
                    "1",
                    egui::FontId::proportional(9.0 * u),
                    color,
                );
            }
        }
        Icon::Volume | Icon::Mute => {
            p.add(fill_poly(vec![at(3.0, 9.0), at(7.0, 9.0), at(12.0, 4.5), at(12.0, 19.5), at(7.0, 15.0), at(3.0, 15.0)]));
            if icon == Icon::Mute {
                p.line_segment([at(15.0, 9.0), at(21.0, 15.0)], stroke);
                p.line_segment([at(21.0, 9.0), at(15.0, 15.0)], stroke);
            } else {
                arc(p, at(12.0, 12.0), 4.5 * u, -PI / 3.5, PI / 3.5, stroke);
                arc(p, at(12.0, 12.0), 8.5 * u, -PI / 3.5, PI / 3.5, stroke);
            }
        }
        Icon::Queue | Icon::Playlist => {
            for (i, y) in [6.0, 11.0, 16.0].into_iter().enumerate() {
                let x1 = if icon == Icon::Queue && i == 2 { 12.0 } else { 20.0 };
                p.line_segment([at(4.0, y), at(x1, y)], stroke);
            }
            if icon == Icon::Queue {
                p.add(fill_poly(vec![at(15.0, 13.0), at(21.0, 16.5), at(15.0, 20.0)]));
            } else {
                p.line_segment([at(4.0, 21.0), at(14.0, 21.0)], stroke);
            }
        }
        Icon::Home => {
            p.add(Shape::closed_line(
                vec![at(4.0, 11.0), at(12.0, 4.0), at(20.0, 11.0), at(20.0, 20.0), at(4.0, 20.0)],
                stroke,
            ));
            p.rect_filled(Rect::from_min_max(at(10.0, 14.0), at(14.0, 20.0)), 0.0, color);
        }
        Icon::Search => {
            p.circle_stroke(at(10.5, 10.5), 6.0 * u, stroke);
            p.line_segment([at(15.0, 15.0), at(20.5, 20.5)], Stroke::new(stroke.width * 1.3, color));
        }
        Icon::Library => {
            p.line_segment([at(5.0, 4.0), at(5.0, 20.0)], stroke);
            p.line_segment([at(10.0, 4.0), at(10.0, 20.0)], stroke);
            p.line_segment([at(14.0, 5.0), at(19.0, 20.0)], stroke);
        }
        Icon::Settings => {
            let r_out = 9.5 * u;
            let r_in = 7.0 * u;
            let mut pts = Vec::new();
            for i in 0..32 {
                let a = i as f32 / 32.0 * TAU;
                let tooth = (i / 2) % 2 == 0;
                let r = if tooth { r_out } else { r_in };
                pts.push(c + vec2(a.cos(), a.sin()) * r);
            }
            p.add(Shape::closed_line(pts, stroke));
            p.circle_stroke(c, 3.0 * u, stroke);
        }
        Icon::Star | Icon::StarFilled => {
            let mut pts = Vec::new();
            for i in 0..10 {
                let a = -PI / 2.0 + i as f32 * PI / 5.0;
                let r = if i % 2 == 0 { 9.5 * u } else { 4.0 * u };
                pts.push(c + vec2(a.cos(), a.sin()) * r + vec2(0.0, 1.0 * u));
            }
            if icon == Icon::StarFilled {
                // Star is not convex: fill as a fan of triangles around the centre.
                for i in 0..10 {
                    let a = pts[i];
                    let b = pts[(i + 1) % 10];
                    p.add(Shape::convex_polygon(vec![c + vec2(0.0, u), a, b], color, Stroke::NONE));
                }
            } else {
                p.add(Shape::closed_line(pts, Stroke::new(stroke.width * 0.75, color)));
            }
        }
        Icon::Radio => {
            p.circle_filled(c, 2.5 * u, color);
            arc(p, c, 6.0 * u, -PI / 4.0, PI / 4.0, stroke);
            arc(p, c, 6.0 * u, PI - PI / 4.0, PI + PI / 4.0, stroke);
            arc(p, c, 10.0 * u, -PI / 4.0, PI / 4.0, stroke);
            arc(p, c, 10.0 * u, PI - PI / 4.0, PI + PI / 4.0, stroke);
        }
        Icon::MiniPlayer => {
            p.rect_stroke(Rect::from_min_max(at(3.0, 5.0), at(21.0, 19.0)), 2.0 * u, stroke, egui::StrokeKind::Middle);
            p.rect_filled(Rect::from_min_max(at(12.0, 12.0), at(19.0, 17.0)), 1.0 * u, color);
        }
        Icon::Close => {
            p.line_segment([at(6.0, 6.0), at(18.0, 18.0)], stroke);
            p.line_segment([at(18.0, 6.0), at(6.0, 18.0)], stroke);
        }
        Icon::Back => {
            p.add(Shape::line(vec![at(15.0, 5.0), at(8.0, 12.0), at(15.0, 19.0)], stroke));
        }
        Icon::Up => {
            p.add(Shape::line(vec![at(6.0, 15.0), at(12.0, 9.0), at(18.0, 15.0)], stroke));
        }
        Icon::Down => {
            p.add(Shape::line(vec![at(6.0, 9.0), at(12.0, 15.0), at(18.0, 9.0)], stroke));
        }
        Icon::Refresh => {
            arc(p, c, 7.5 * u, -PI * 0.25, PI * 1.45, stroke);
            p.add(fill_poly(vec![at(15.0, 3.5), at(20.5, 6.0), at(16.0, 10.0)]));
        }
        Icon::Plus => {
            p.line_segment([at(12.0, 5.0), at(12.0, 19.0)], stroke);
            p.line_segment([at(5.0, 12.0), at(19.0, 12.0)], stroke);
        }
        Icon::Cast => {
            // Screen outline open at the bottom-left, where the signal arcs sit.
            p.add(Shape::line(vec![at(3.0, 9.0), at(3.0, 5.0), at(21.0, 5.0), at(21.0, 19.0), at(14.0, 19.0)], stroke));
            for r in [4.0f32, 8.0] {
                let arc: Vec<Pos2> = (0..=8)
                    .map(|i| {
                        let a = std::f32::consts::FRAC_PI_2 * i as f32 / 8.0;
                        at(3.0 + r * a.sin(), 19.0 - r * a.cos())
                    })
                    .collect();
                p.add(Shape::line(arc, stroke));
            }
            p.circle_filled(at(3.5, 18.5), 1.6 * u, color);
        }
        Icon::Download => {
            p.line_segment([at(12.0, 3.5), at(12.0, 15.0)], stroke);
            p.add(Shape::line(vec![at(7.0, 10.5), at(12.0, 15.5), at(17.0, 10.5)], stroke));
            p.add(Shape::line(vec![at(4.0, 15.0), at(4.0, 20.0), at(20.0, 20.0), at(20.0, 15.0)], stroke));
        }
        Icon::Lyrics => {
            // Capsule head, a U-shaped holder, the stem and the base.
            let head = egui::Rect::from_min_max(at(9.0, 3.0), at(15.0, 14.0));
            p.rect_stroke(head, 3.0 * u, stroke, egui::StrokeKind::Middle);
            let holder: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 12.0;
                    at(12.0 - 6.0 * a.cos(), 11.0 + 6.0 * a.sin())
                })
                .collect();
            p.add(Shape::line(holder, stroke));
            p.line_segment([at(12.0, 17.0), at(12.0, 21.0)], stroke);
            p.line_segment([at(8.5, 21.0), at(15.5, 21.0)], stroke);
        }
        Icon::More => {
            for x in [5.5, 12.0, 18.5] {
                p.circle_filled(at(x, 12.0), 1.8 * u, color);
            }
        }
        Icon::Trash => {
            p.line_segment([at(4.0, 6.0), at(20.0, 6.0)], stroke);
            p.line_segment([at(9.0, 6.0), at(10.0, 3.5)], stroke);
            p.line_segment([at(15.0, 6.0), at(14.0, 3.5)], stroke);
            p.add(Shape::line(vec![at(6.0, 6.0), at(7.0, 20.0), at(17.0, 20.0), at(18.0, 6.0)], stroke));
        }
    }
}

fn arc(p: &egui::Painter, c: Pos2, r: f32, a0: f32, a1: f32, stroke: Stroke) {
    let n = 16;
    let pts = (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(Shape::line(pts, stroke));
}

/// A frameless clickable icon. `active` tints it with the accent colour.
pub fn button(ui: &mut Ui, icon: Icon, size: f32, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let color = if active {
        super::theme::ACCENT
    } else if resp.hovered() {
        Color32::WHITE
    } else {
        super::theme::TEXT_DIM
    };
    paint(ui.painter(), rect.shrink(size * 0.12), icon, color);
    resp
}

/// Round filled button (used for the big play/pause control).
pub fn round_button(ui: &mut Ui, icon: Icon, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let bg = if resp.hovered() { Color32::WHITE } else { Color32::from_rgb(235, 235, 235) };
    let r = if resp.is_pointer_button_down_on() { size * 0.47 } else { size * 0.5 };
    ui.painter().circle_filled(rect.center(), r, bg);
    paint(ui.painter(), rect.shrink(size * 0.27), icon, Color32::BLACK);
    resp
}
