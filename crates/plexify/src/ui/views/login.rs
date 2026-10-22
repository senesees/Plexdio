//! Sign-in screen (Plex PIN flow) and the generic busy screen.

use crate::ui::app::PlexifyApp;
use crate::ui::icons::{self, Icon};
use crate::ui::theme;
use egui::{Align, Layout, RichText};

/// Centered column helper for full-window screens.
pub fn centered(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG)).show(ui, |ui| {
        let avail = ui.available_size();
        let top = (avail.y * 0.18).max(20.0);
        ui.add_space(top);
        ui.horizontal(|ui| {
            ui.add_space(((avail.x - width) / 2.0).max(0.0));
            ui.allocate_ui_with_layout(egui::vec2(width, avail.y - top), Layout::top_down(Align::Center), add);
        });
    });
}

fn logo(ui: &mut egui::Ui, size: f32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), size / 2.0, theme::ACCENT);
    icons::paint(ui.painter(), r.shrink(size * 0.24), Icon::Play, egui::Color32::from_rgb(24, 24, 24));
}

pub fn busy_screen(ui: &mut egui::Ui, text: &str) {
    centered(ui, 420.0, |ui| {
        logo(ui, 64.0);
        ui.add_space(24.0);
        theme::spinner(ui, 28.0);
        ui.add_space(12.0);
        ui.label(RichText::new(text).size(16.0).color(theme::TEXT_DIM));
    });
}

impl PlexifyApp {
    pub fn login_screen(&mut self, ui: &mut egui::Ui) {
        let mut start = false;
        let mut cancel = false;
        let mut reopen = false;
        let mut copy = None;
        let waiting = self.login.pin.is_some();
        let busy = self.login.busy;
        let url = self.login.url.clone();
        let error = self.login.error.clone();
        let info = self.login.info.clone();
        let target = self.tv.base().to_string();
        let mock = self.opts.plex_tv_url.is_some();
        centered(ui, 460.0, |ui| {
            logo(ui, 72.0);
            ui.add_space(16.0);
            ui.label(RichText::new("Plexify").size(34.0).strong());
            ui.label(RichText::new("Your Plex music, light as a feather.").size(15.0).color(theme::TEXT_DIM));
            ui.add_space(28.0);
            theme::card().show(ui, |ui| {
                ui.set_width(400.0);
                ui.vertical_centered(|ui| {
                    if waiting {
                        ui.label(RichText::new("Approve the sign-in in your browser").size(17.0).strong());
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("A Plex page has opened. Sign in there and allow Plexify; this window continues automatically.")
                                .color(theme::TEXT_DIM),
                        );
                        ui.add_space(14.0);
                        theme::spinner(ui, 24.0);
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            ui.add_space(30.0);
                            if theme::secondary_button(ui, "Open browser again").clicked() {
                                reopen = true;
                            }
                            if theme::secondary_button(ui, "Copy link").clicked() {
                                copy = url.clone();
                            }
                        });
                        ui.add_space(8.0);
                        if ui.link("Cancel").clicked() {
                            cancel = true;
                        }
                    } else {
                        ui.label(RichText::new("Sign in to continue").size(17.0).strong());
                        ui.add_space(6.0);
                        ui.label(RichText::new("We'll open plex.tv in your browser. Your password never touches this app.").color(theme::TEXT_DIM));
                        ui.add_space(18.0);
                        ui.add_enabled_ui(!busy, |ui| {
                            if theme::primary_button(ui, "Sign in with Plex").clicked() {
                                start = true;
                            }
                        });
                        if busy {
                            ui.add_space(8.0);
                            theme::spinner(ui, 18.0);
                        }
                    }
                    if let Some(e) = &error {
                        ui.add_space(12.0);
                        ui.label(RichText::new(e).color(theme::ERROR));
                    }
                    if let Some(i) = &info {
                        ui.add_space(12.0);
                        ui.label(RichText::new(i).color(theme::TEXT_DIM));
                    }
                });
            });
            if mock {
                ui.add_space(12.0);
                ui.label(RichText::new(format!("Using Plex endpoint {target}")).small().color(theme::TEXT_DIM));
            }
        });
        if start {
            self.start_login();
        }
        if cancel {
            self.cancel_login();
        }
        if reopen {
            self.open_auth_page();
        }
        if let Some(u) = copy {
            ui.ctx().copy_text(u);
            self.login.info = Some("Link copied to clipboard.".into());
        }
    }
}
