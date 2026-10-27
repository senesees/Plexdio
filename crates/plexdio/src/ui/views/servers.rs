//! Server and music-library picker.

use crate::plex::models::Directory;
use crate::ui::app::{PlexdioApp, ProbeStatus, Screen};
use crate::ui::icons::{self, Icon};
use crate::ui::theme;
use egui::{Color32, RichText, Sense};

enum Action {
    Select(String),
    Open(String, Directory),
    Refresh,
    Back,
    SignOut,
}

impl PlexdioApp {
    pub fn server_screen(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        let can_go_back = self.pms.is_some() && self.section.is_some();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(40, 28)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if can_go_back && icons::button(ui, Icon::Back, 26.0, false).on_hover_text("Back").clicked() {
                        action = Some(Action::Back);
                    }
                    ui.heading("Choose a music library");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::secondary_button(ui, "Sign out").clicked() {
                            action = Some(Action::SignOut);
                        }
                        if icons::button(ui, Icon::Refresh, 24.0, false).on_hover_text("Refresh servers").clicked() {
                            action = Some(Action::Refresh);
                        }
                        if let Some(u) = &self.user {
                            ui.label(RichText::new(u.display_name()).color(theme::TEXT_DIM));
                        }
                    });
                });
                ui.add_space(6.0);
                if let Some(e) = &self.servers_error {
                    ui.label(RichText::new(e).color(theme::ERROR));
                    ui.add_space(6.0);
                }
                if self.servers.is_empty() && self.servers_error.is_none() {
                    ui.horizontal(|ui| {
                        theme::spinner(ui, 18.0);
                        ui.label(RichText::new("Looking for servers…").color(theme::TEXT_DIM));
                    });
                }
                ui.add_space(10.0);
                ui.columns(2, |cols| {
                    // Left: servers.
                    cols[0].label(RichText::new("SERVERS").small().strong().color(theme::TEXT_DIM));
                    cols[0].add_space(4.0);
                    for entry in &self.servers {
                        let id = entry.res.client_identifier.clone();
                        let selected = self.selected_server.as_deref() == Some(id.as_str());
                        let (status, color) = match &entry.status {
                            ProbeStatus::Checking => ("Checking…".to_string(), theme::TEXT_DIM),
                            ProbeStatus::Ok(c) => (
                                format!(
                                    "{} · {} ms",
                                    if c.relay { "Relay" } else if c.local { "Local" } else { "Remote" },
                                    c.latency_ms
                                ),
                                Color32::from_rgb(110, 200, 120),
                            ),
                            ProbeStatus::Failed(e) => (format!("Unavailable: {e}"), theme::ERROR),
                        };
                        let owner = if entry.res.owned {
                            "Owned".to_string()
                        } else {
                            format!("Shared by {}", entry.res.source_title.as_deref().unwrap_or("a friend"))
                        };
                        if row(&mut cols[0], selected, &entry.res.name, &format!("{owner}  ·  {status}"), color).clicked() {
                            action = Some(Action::Select(id));
                        }
                    }
                    // Right: libraries of the selected server.
                    cols[1].label(RichText::new("MUSIC LIBRARIES").small().strong().color(theme::TEXT_DIM));
                    cols[1].add_space(4.0);
                    let Some(sel) = self.selected_server.clone() else { return };
                    let Some(entry) = self.servers.iter().find(|s| s.res.client_identifier == sel) else { return };
                    match (&entry.status, &entry.sections) {
                        (ProbeStatus::Failed(_), _) => {
                            cols[1].label(RichText::new("This server can't be reached right now.").color(theme::TEXT_DIM));
                            if theme::secondary_button(&mut cols[1], "Try again").clicked() {
                                action = Some(Action::Select(sel.clone()));
                            }
                        }
                        (_, None) => {
                            theme::spinner(&mut cols[1], 18.0);
                        }
                        (_, Some(Err(e))) => {
                            cols[1].label(RichText::new(e).color(theme::ERROR));
                        }
                        (_, Some(Ok(secs))) if secs.is_empty() => {
                            cols[1].label(RichText::new("No music libraries on this server.").color(theme::TEXT_DIM));
                        }
                        (_, Some(Ok(secs))) => {
                            for s in secs {
                                let current = self.section.as_ref().is_some_and(|c| c.uuid == s.uuid)
                                    && self.pms.as_ref().is_some_and(|p| p.conn.server_id == sel);
                                let title = s.title.clone().unwrap_or_else(|| "Music".into());
                                let sub = if current { "Currently open" } else { "Open library" };
                                if row(&mut cols[1], current, &title, sub, theme::TEXT_DIM).clicked() {
                                    action = Some(Action::Open(sel.clone(), s.clone()));
                                }
                            }
                        }
                    }
                });
            });
        match action {
            Some(Action::Select(id)) => self.select_server(&id),
            Some(Action::Open(id, sec)) => self.open_library(&id, sec),
            Some(Action::Refresh) => {
                self.servers.clear();
                self.refresh_servers();
            }
            Some(Action::Back) => self.screen = Screen::Main,
            Some(Action::SignOut) => self.sign_out(),
            None => {}
        }
    }
}

fn row(ui: &mut egui::Ui, selected: bool, title: &str, sub: &str, sub_color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 58.0), Sense::click());
    let fill = if selected {
        theme::CARD_HOVER
    } else if resp.hovered() {
        theme::CARD
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    if selected {
        ui.painter().rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())), 2.0, theme::ACCENT);
    }
    let p = ui.painter();
    // Truncated to the row so long text (e.g. an "Unavailable: …" error) can't run into the
    // other column.
    let text_w = (rect.width() - 24.0).max(0.0);
    let line = |text: &str, size: f32, color: Color32, y: f32| {
        let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), egui::FontId::proportional(size), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_w);
        let galley = p.layout_job(job);
        p.galley(rect.min + egui::vec2(16.0, y - galley.size().y / 2.0), galley, color);
    };
    line(title, 16.0, theme::TEXT, 18.0);
    line(sub, 12.5, sub_color, 40.0);
    ui.add_space(4.0);
    resp
}
