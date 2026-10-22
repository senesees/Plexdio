//! Root application: sidebar, main content, bottom now-playing bar.

use crate::config::Config;
use egui::{Align, Layout, RichText};

use super::icons::{self, Icon};
use super::runner::UiApp;
use super::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Home,
    Search,
    Library,
    Playlists,
    Settings,
}

pub struct PlexifyApp {
    config: Config,
    nav: Nav,
}

impl PlexifyApp {
    pub fn new(ctx: &egui::Context, config: Config) -> Self {
        theme::apply(ctx);
        Self { config, nav: Nav::Home }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            let (r, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 13.0, theme::ACCENT);
            icons::paint(ui.painter(), r.shrink(6.0), Icon::Play, egui::Color32::from_rgb(24, 24, 24));
            ui.label(RichText::new("Plexify").size(22.0).strong());
        });
        ui.add_space(18.0);
        for (nav, icon, label) in [
            (Nav::Home, Icon::Home, "Home"),
            (Nav::Search, Icon::Search, "Search"),
            (Nav::Library, Icon::Library, "Library"),
            (Nav::Playlists, Icon::Playlist, "Playlists"),
        ] {
            nav_button(ui, &mut self.nav, nav, icon, label);
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(10.0);
            nav_button(ui, &mut self.nav, Nav::Settings, Icon::Settings, "Settings");
        });
    }

    fn now_playing_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal_centered(|ui| {
            ui.label(RichText::new("Nothing playing").color(theme::TEXT_DIM));
            ui.add_space(200.0);
            for i in [Icon::Shuffle, Icon::Prev] {
                icons::button(ui, i, 28.0, false);
            }
            icons::round_button(ui, Icon::Play, 38.0);
            for i in [Icon::Next, Icon::Repeat, Icon::RepeatOne, Icon::Queue, Icon::Volume, Icon::Mute, Icon::Star, Icon::StarFilled, Icon::Radio, Icon::MiniPlayer, Icon::Refresh, Icon::Trash, Icon::Plus, Icon::Back] {
                icons::button(ui, i, 28.0, false);
            }
        });
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        let title = match self.nav {
            Nav::Home => "Home",
            Nav::Search => "Search",
            Nav::Library => "Library",
            Nav::Playlists => "Playlists",
            Nav::Settings => "Settings",
        };
        ui.heading(title);
        ui.label(RichText::new(format!("client id {}", self.config.client_id)).small().color(theme::TEXT_DIM));
    }
}

fn nav_button(ui: &mut egui::Ui, current: &mut Nav, target: Nav, icon: Icon, label: &str) {
    let selected = *current == target;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 38.0), egui::Sense::click());
    if selected || resp.hovered() {
        let fill = if selected { egui::Color32::from_rgb(36, 36, 36) } else { egui::Color32::from_rgb(24, 24, 24) };
        ui.painter().rect_filled(rect, 6.0, fill);
    }
    let color = if selected || resp.hovered() { theme::TEXT } else { theme::TEXT_DIM };
    let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 22.0, rect.center().y), egui::vec2(22.0, 22.0));
    icons::paint(ui.painter(), icon_rect, icon, if selected { theme::ACCENT } else { color });
    ui.painter().text(
        egui::pos2(rect.left() + 44.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(15.0),
        color,
    );
    if resp.clicked() {
        *current = target;
    }
}

impl UiApp for PlexifyApp {
    fn on_window_created(&mut self, _ctx: &egui::Context, window: &winit::window::Window) {
        if let Some(hwnd) = crate::win::hwnd_of(window) {
            crate::win::single_instance::publish_window(hwnd);
        }
        crate::win::fix_wine_event_target();
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("now_playing")
            .exact_size(84.0)
            .frame(egui::Frame::new().fill(theme::BAR).inner_margin(egui::Margin::symmetric(12, 6)))
            .show(ui, |ui| self.now_playing_bar(ui));
        egui::Panel::left("sidebar")
            .exact_size(210.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::SIDEBAR).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(24, 8)))
            .show(ui, |ui| self.content(ui));
    }
}
