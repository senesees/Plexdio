//! Custom window title bar (the native one is turned off with `with_decorations(false)`):
//! drag to move, double-click to maximize, themed mini-player/minimize/maximize/close buttons, and
//! resize handles along the window edges.

use crate::ui::icons::{self, Icon};
use crate::ui::theme;
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, ViewportCommand};

pub const HEIGHT: f32 = 32.0;
const BUTTON_W: f32 = 46.0;
/// Width of the invisible resize band along each window edge.
const EDGE: f32 = 5.0;
const CLOSE_HOVER: Color32 = Color32::from_rgb(196, 43, 28);

/// What the user did in the title bar that the app has to handle itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleAction {
    None,
    /// Close button (the app decides between closing to the tray and quitting).
    Close,
    /// Mini-player button.
    MiniPlayer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Glyph {
    MiniPlayer,
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Draw the title bar as a top panel. `title` is shown dimmed in the middle; `can_maximize` is
/// false for the fixed-size mini-player.
pub fn show(ui: &mut egui::Ui, title: &str, can_maximize: bool) -> TitleAction {
    let mut action = TitleAction::None;
    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
    egui::Panel::top("title_bar")
        .exact_size(HEIGHT)
        .resizable(false)
        .frame(egui::Frame::new().fill(theme::SIDEBAR))
        .show(ui, |ui| {
            let bar = ui.max_rect();
            let buttons = if can_maximize { 4.0 } else { 3.0 };
            let drag_rect = Rect::from_min_max(bar.min, egui::pos2(bar.right() - BUTTON_W * buttons, bar.bottom()));
            // Leave the resize bands along the top/left edges to `resize_edges`.
            let inset = if maximized || !can_maximize { 0.0 } else { EDGE };
            let grab = Rect::from_min_max(drag_rect.min + egui::vec2(inset, inset), drag_rect.max);

            // Drag area first, so the buttons placed on top of it win the hover.
            let drag = ui.interact(grab, ui.scope_id().with("title_drag"), Sense::click_and_drag());
            if can_maximize && drag.double_clicked() {
                ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            } else if drag.drag_started_by(egui::PointerButton::Primary) {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }

            // App mark and name on the left.
            let p = ui.painter();
            let mark = Rect::from_center_size(egui::pos2(bar.left() + 20.0, bar.center().y), egui::vec2(16.0, 16.0));
            p.circle_filled(mark.center(), 8.0, theme::ACCENT);
            icons::paint(p, mark.shrink(4.0), Icon::Play, theme::BAR);
            p.text(egui::pos2(bar.left() + 36.0, bar.center().y), Align2::LEFT_CENTER, "Plexdio", FontId::proportional(13.0), theme::TEXT);

            // Now-playing title, centered and clipped to the space between name and buttons.
            if !title.is_empty() && title != "Plexdio" {
                // Symmetric around the window center, clear of the name and the buttons.
                let cx = bar.center().x;
                let room = 2.0 * (cx - (bar.left() + 110.0)).min(drag_rect.right() - 12.0 - cx);
                if room > 40.0 {
                    super::views::common::clipped(
                        ui,
                        egui::pos2(cx, bar.center().y),
                        title,
                        12.5,
                        theme::TEXT_DIM,
                        room,
                        Align2::CENTER_CENTER,
                    );
                }
            }

            // Window buttons, right to left.
            let mut x = bar.right();
            let mut button = |ui: &mut egui::Ui, glyph: Glyph, tip: &str| -> bool {
                x -= BUTTON_W;
                let r = Rect::from_min_size(egui::pos2(x, bar.top()), egui::vec2(BUTTON_W, bar.height()));
                let resp = ui.interact(r, ui.scope_id().with(("title_btn", tip)), Sense::click()).on_hover_text(tip);
                let hovered = resp.hovered();
                if hovered {
                    let fill = if glyph == Glyph::Close { CLOSE_HOVER } else { theme::CARD_HOVER };
                    ui.painter().rect_filled(r, 0.0, fill);
                }
                let color = if hovered { Color32::WHITE } else { theme::TEXT_DIM };
                paint_glyph(ui.painter(), r.center(), glyph, color);
                resp.clicked()
            };
            if button(ui, Glyph::Close, "Close") {
                action = TitleAction::Close;
            }
            if can_maximize {
                let (glyph, tip) = if maximized { (Glyph::Restore, "Restore") } else { (Glyph::Maximize, "Maximize") };
                if button(ui, glyph, tip) {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                }
            }
            if button(ui, Glyph::Minimize, "Minimize") {
                ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
            }
            if button(ui, Glyph::MiniPlayer, "Mini-player (Ctrl+M)") {
                action = TitleAction::MiniPlayer;
            }
        });
    action
}

/// Thin-line window glyphs (Windows 11 style), 10 px wide.
fn paint_glyph(p: &egui::Painter, c: egui::Pos2, glyph: Glyph, color: Color32) {
    let s = Stroke::new(1.0, color);
    let h = 5.0;
    match glyph {
        Glyph::MiniPlayer => {
            icons::paint(p, Rect::from_center_size(c, egui::vec2(16.0, 16.0)), Icon::MiniPlayer, color);
        }
        Glyph::Minimize => {
            p.line_segment([egui::pos2(c.x - h, c.y), egui::pos2(c.x + h, c.y)], s);
        }
        Glyph::Maximize => {
            p.rect_stroke(Rect::from_center_size(c, egui::vec2(2.0 * h, 2.0 * h)), 1.0, s, egui::StrokeKind::Middle);
        }
        Glyph::Restore => {
            let back = Rect::from_min_size(egui::pos2(c.x - h + 2.0, c.y - h), egui::vec2(2.0 * h - 2.0, 2.0 * h - 2.0));
            p.line_segment([back.left_top(), back.right_top()], s);
            p.line_segment([back.right_top(), back.right_bottom()], s);
            p.rect_stroke(
                Rect::from_min_size(egui::pos2(c.x - h, c.y - h + 2.0), egui::vec2(2.0 * h - 2.0, 2.0 * h - 2.0)),
                1.0,
                s,
                egui::StrokeKind::Middle,
            );
        }
        Glyph::Close => {
            p.line_segment([egui::pos2(c.x - h, c.y - h), egui::pos2(c.x + h, c.y + h)], s);
            p.line_segment([egui::pos2(c.x + h, c.y - h), egui::pos2(c.x - h, c.y + h)], s);
        }
    }
}

/// Resize from the window edges and corners. Call after the panels so the resize cursor wins
/// over whatever widget sits under the edge.
pub fn resize_edges(ui: &mut egui::Ui) {
    use egui::viewport::ResizeDirection as D;
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
    let r = ctx.viewport_rect();
    let (left, right) = (pos.x - r.left() < EDGE, r.right() - pos.x < EDGE);
    let (top, bottom) = (pos.y - r.top() < EDGE, r.bottom() - pos.y < EDGE);
    let dir = match (left, right, top, bottom) {
        (true, _, true, _) => D::NorthWest,
        (_, true, true, _) => D::NorthEast,
        (true, _, _, true) => D::SouthWest,
        (_, true, _, true) => D::SouthEast,
        (true, ..) => D::West,
        (_, true, ..) => D::East,
        (_, _, true, _) => D::North,
        (_, _, _, true) => D::South,
        _ => return,
    };
    ctx.set_cursor_icon(match dir {
        D::NorthWest | D::SouthEast => egui::CursorIcon::ResizeNwSe,
        D::NorthEast | D::SouthWest => egui::CursorIcon::ResizeNeSw,
        D::West | D::East => egui::CursorIcon::ResizeHorizontal,
        D::North | D::South => egui::CursorIcon::ResizeVertical,
    });
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
    }
}
