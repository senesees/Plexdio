//! Root application state: sign-in, server/library selection and the main shell
//! (sidebar, content, bottom now-playing bar).

use crate::config::Config;
use crate::plex::auth::PlexTv;
use crate::plex::models::{Directory, Pin, PlexUser, Resource};
use crate::plex::resources::{pick_best, ServerConnection};
use crate::plex::server::PmsClient;
use crate::plex::{ClientInfo, Http, PlexError};
use crate::secure_store::SecretStore;
use egui::{Align, Layout, RichText};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use super::icons::{self, Icon};
use super::jobs::Jobs;
use super::runner::UiApp;
use super::theme;

pub const TOKEN_NAME: &str = "plex-token";

/// Command-line options.
#[derive(Debug, Clone, Default)]
pub struct LaunchOptions {
    /// Use this base URL instead of https://plex.tv (mock server).
    pub plex_tv_url: Option<String>,
    /// Don't open a browser for sign-in; only log/show the URL.
    pub no_browser: bool,
}

impl LaunchOptions {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        let mut o = LaunchOptions {
            plex_tv_url: std::env::var("PLEXIFY_PLEX_TV_URL").ok().filter(|s| !s.is_empty()),
            no_browser: std::env::var_os("PLEXIFY_NO_BROWSER").is_some(),
        };
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--plex-tv-url" => o.plex_tv_url = it.next(),
                "--no-browser" => o.no_browser = true,
                s if s.starts_with("--plex-tv-url=") => o.plex_tv_url = Some(s["--plex-tv-url=".len()..].to_string()),
                _ => {}
            }
        }
        o
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Home,
    Search,
    Library,
    Playlists,
    Settings,
}

/// Results delivered from worker threads.
pub enum Msg {
    PinCreated(u64, Result<Pin, PlexError>),
    PinFinished(u64, Result<String, PlexError>),
    Validated(Result<PlexUser, PlexError>),
    Servers(Result<Vec<Resource>, PlexError>),
    Probed(String, Result<ServerConnection, PlexError>),
    Sections(String, Result<Vec<Directory>, PlexError>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    /// Busy with a message (startup validation, connecting).
    Busy(String),
    Login,
    ServerSelect,
    Main,
}

#[derive(Default)]
pub struct LoginState {
    pub attempt: u64,
    pub busy: bool,
    pub pin: Option<Pin>,
    pub url: Option<String>,
    pub cancel: Option<Arc<AtomicBool>>,
    pub error: Option<String>,
    pub info: Option<String>,
}

pub enum ProbeStatus {
    Checking,
    Ok(ServerConnection),
    Failed(String),
}

pub struct ServerEntry {
    pub res: Resource,
    pub status: ProbeStatus,
    pub sections: Option<Result<Vec<Directory>, String>>,
}

pub struct PlexifyApp {
    pub config: Config,
    pub opts: LaunchOptions,
    pub nav: Nav,
    pub screen: Screen,
    pub jobs: Jobs<Msg>,
    pub http: Http,
    pub tv: PlexTv,
    pub store: Arc<dyn SecretStore>,
    pub token: Option<String>,
    pub user: Option<PlexUser>,
    pub login: LoginState,
    pub servers: Vec<ServerEntry>,
    pub servers_error: Option<String>,
    pub selected_server: Option<String>,
    /// After startup, jump straight into the remembered server/library if it is reachable.
    pub auto_select: bool,
    pub pms: Option<PmsClient>,
    pub section: Option<Directory>,
    /// Transient notice shown at the top of the content area: (text, is_error).
    pub banner: Option<(String, bool)>,
}

impl PlexifyApp {
    pub fn new(ctx: &egui::Context, config: Config, opts: LaunchOptions, store: Arc<dyn SecretStore>) -> Self {
        theme::apply(ctx);
        let http = Http::new(ClientInfo::new(config.client_id.clone()));
        let tv = PlexTv::new(http.clone(), opts.plex_tv_url.as_deref());
        let mut app = Self {
            config,
            opts,
            nav: Nav::Home,
            screen: Screen::Login,
            jobs: Jobs::new(ctx.clone()),
            http,
            tv,
            store,
            token: None,
            user: None,
            login: LoginState::default(),
            servers: Vec::new(),
            servers_error: None,
            selected_server: None,
            auto_select: true,
            pms: None,
            section: None,
            banner: None,
        };
        app.token = app.store.get();
        if app.token.is_some() {
            app.validate_token();
        }
        app
    }

    // ------------------------------------------------------------------ flow

    fn validate_token(&mut self) {
        let Some(token) = self.token.clone() else { return };
        self.screen = Screen::Busy("Signing in…".into());
        let tv = self.tv.clone();
        self.jobs.spawn("validate", move || Msg::Validated(tv.user(&token)));
    }

    pub fn start_login(&mut self) {
        self.cancel_login();
        self.login.attempt += 1;
        self.login.busy = true;
        self.login.error = None;
        self.login.info = None;
        let attempt = self.login.attempt;
        let tv = self.tv.clone();
        self.jobs.spawn("pin", move || Msg::PinCreated(attempt, tv.create_pin()));
    }

    pub fn cancel_login(&mut self) {
        if let Some(c) = self.login.cancel.take() {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.login.pin = None;
        self.login.url = None;
        self.login.busy = false;
    }

    pub fn open_auth_page(&self) {
        if let Some(url) = &self.login.url {
            log::info!("sign-in URL: {url}");
            if !self.opts.no_browser && !crate::win::open_url(url) {
                log::warn!("could not open the browser");
            }
        }
    }

    pub fn sign_out(&mut self) {
        log::info!("signing out");
        let _ = self.store.delete();
        self.token = None;
        self.user = None;
        self.servers.clear();
        self.pms = None;
        self.section = None;
        self.selected_server = None;
        self.config.last_server_id = None;
        self.config.last_section_key = None;
        self.config.last_connection_uri = None;
        self.save_config();
        self.login = LoginState { info: Some("You have been signed out.".into()), ..Default::default() };
        self.screen = Screen::Login;
    }

    pub fn refresh_servers(&mut self) {
        let Some(token) = self.token.clone() else { return };
        self.servers_error = None;
        let tv = self.tv.clone();
        self.jobs.spawn("resources", move || Msg::Servers(tv.servers(&token)));
    }

    fn probe_server(&mut self, idx: usize) {
        let Some(token) = self.token.clone() else { return };
        let entry = &mut self.servers[idx];
        entry.status = ProbeStatus::Checking;
        let res = entry.res.clone();
        let info = self.http.info.clone();
        self.jobs.spawn("probe", move || {
            let id = res.client_identifier.clone();
            Msg::Probed(id, pick_best(&info, &res, &token, Duration::from_secs(4)))
        });
    }

    pub fn select_server(&mut self, id: &str) {
        self.selected_server = Some(id.to_string());
        let Some(idx) = self.servers.iter().position(|s| s.res.client_identifier == id) else { return };
        match &self.servers[idx].status {
            ProbeStatus::Ok(conn) => {
                let pms = PmsClient::new(self.http.clone(), conn.clone());
                self.servers[idx].sections = None;
                self.jobs.spawn("sections", move || Msg::Sections(pms.conn.server_id.clone(), pms.music_sections()));
            }
            ProbeStatus::Failed(_) => self.probe_server(idx),
            ProbeStatus::Checking => {}
        }
    }

    pub fn open_library(&mut self, server_id: &str, section: Directory) {
        let Some(entry) = self.servers.iter().find(|s| s.res.client_identifier == server_id) else { return };
        let ProbeStatus::Ok(conn) = &entry.status else { return };
        log::info!(
            "opening library '{}' on '{}' via {}",
            section.title.as_deref().unwrap_or("?"),
            conn.server_name,
            conn.uri
        );
        self.config.last_server_id = Some(server_id.to_string());
        self.config.last_section_key = section.key.clone();
        self.config.last_connection_uri = Some(conn.uri.clone());
        self.save_config();
        self.pms = Some(PmsClient::new(self.http.clone(), conn.clone()));
        self.section = Some(section);
        self.screen = Screen::Main;
        self.nav = Nav::Home;
    }

    pub fn save_config(&self) {
        if let Err(e) = self.config.save() {
            log::warn!("failed to save config: {e}");
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::PinCreated(attempt, res) if attempt == self.login.attempt => match res {
                Ok(pin) => {
                    let url = self.tv.auth_url(&pin);
                    let cancel = Arc::new(AtomicBool::new(false));
                    self.login.url = Some(url);
                    self.login.pin = Some(pin.clone());
                    self.login.cancel = Some(cancel.clone());
                    self.open_auth_page();
                    let tv = self.tv.clone();
                    let timeout = Duration::from_secs(pin.expires_in.unwrap_or(900).clamp(60, 1800) as u64);
                    self.jobs.spawn("pin-wait", move || {
                        Msg::PinFinished(attempt, tv.wait_for_pin(&pin, Duration::from_secs(1), timeout, &cancel))
                    });
                }
                Err(e) => {
                    self.login.busy = false;
                    self.login.error = Some(e.user_message());
                }
            },
            Msg::PinFinished(attempt, res) if attempt == self.login.attempt => {
                self.login.busy = false;
                self.login.pin = None;
                self.login.cancel = None;
                match res {
                    Ok(token) => {
                        if let Err(e) = self.store.set(&token) {
                            log::error!("could not store token securely: {e}");
                        }
                        log::info!("signed in (token stored in {})", self.store.describe());
                        self.token = Some(token);
                        self.validate_token();
                    }
                    Err(PlexError::Cancelled) => {}
                    Err(PlexError::Timeout) | Err(PlexError::NotFound) => {
                        self.login.error = Some("The sign-in code expired. Please try again.".into())
                    }
                    Err(e) => self.login.error = Some(e.user_message()),
                }
            }
            Msg::PinCreated(..) | Msg::PinFinished(..) => {} // stale attempt
            Msg::Validated(res) => match res {
                Ok(user) => {
                    log::info!("signed in as {}", user.display_name());
                    self.user = Some(user);
                    self.screen = Screen::Busy("Finding your servers…".into());
                    self.refresh_servers();
                }
                Err(PlexError::Unauthorized) => {
                    log::warn!("stored token rejected; signing out");
                    let _ = self.store.delete();
                    self.token = None;
                    self.login = LoginState { error: Some(PlexError::Unauthorized.user_message()), ..Default::default() };
                    self.screen = Screen::Login;
                }
                Err(e) => {
                    log::warn!("cannot validate token: {e}");
                    self.servers_error = Some(format!("Offline: {}", e.user_message()));
                    self.screen = Screen::ServerSelect;
                }
            },
            Msg::Servers(res) => match res {
                Ok(list) => {
                    self.servers = list
                        .into_iter()
                        .map(|res| ServerEntry { res, status: ProbeStatus::Checking, sections: None })
                        .collect();
                    // Owned servers first, then by name.
                    self.servers.sort_by(|a, b| (!a.res.owned, &a.res.name).cmp(&(!b.res.owned, &b.res.name)));
                    for i in 0..self.servers.len() {
                        self.probe_server(i);
                    }
                    let remembered = self.config.last_server_id.clone();
                    let resume = self.auto_select
                        && remembered.as_ref().is_some_and(|id| self.servers.iter().any(|s| &s.res.client_identifier == id));
                    if resume {
                        self.selected_server = remembered;
                        self.screen = Screen::Busy("Connecting to your server…".into());
                    } else {
                        self.auto_select = false;
                        self.selected_server = self.servers.first().map(|s| s.res.client_identifier.clone());
                        self.screen = Screen::ServerSelect;
                    }
                }
                Err(PlexError::Unauthorized) => {
                    let _ = self.store.delete();
                    self.token = None;
                    self.login = LoginState { error: Some(PlexError::Unauthorized.user_message()), ..Default::default() };
                    self.screen = Screen::Login;
                }
                Err(e) => {
                    self.servers_error = Some(e.user_message());
                    self.screen = Screen::ServerSelect;
                }
            },
            Msg::Probed(id, res) => {
                let Some(idx) = self.servers.iter().position(|s| s.res.client_identifier == id) else { return };
                self.servers[idx].status = match res {
                    Ok(c) => ProbeStatus::Ok(c),
                    Err(e) => ProbeStatus::Failed(e.user_message()),
                };
                if self.selected_server.as_deref() == Some(&id) {
                    match &self.servers[idx].status {
                        ProbeStatus::Ok(_) => self.select_server(&id),
                        ProbeStatus::Failed(_) if self.auto_select => {
                            self.auto_select = false;
                            self.screen = Screen::ServerSelect;
                        }
                        _ => {}
                    }
                }
            }
            Msg::Sections(id, res) => {
                let Some(idx) = self.servers.iter().position(|s| s.res.client_identifier == id) else { return };
                let res = res.map_err(|e| e.user_message());
                if self.auto_select {
                    self.auto_select = false;
                    let want = self.config.last_section_key.clone();
                    let found = res.as_ref().ok().and_then(|secs| secs.iter().find(|d| d.key == want).cloned());
                    self.servers[idx].sections = Some(res);
                    match found {
                        Some(sec) => self.open_library(&id, sec),
                        None => self.screen = Screen::ServerSelect,
                    }
                } else {
                    self.servers[idx].sections = Some(res);
                }
            }
        }
    }

    // ------------------------------------------------------------------ main shell

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            let (r, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 13.0, theme::ACCENT);
            icons::paint(ui.painter(), r.shrink(6.0), Icon::Play, egui::Color32::from_rgb(24, 24, 24));
            ui.label(RichText::new("Plexify").size(22.0).strong());
        });
        ui.add_space(12.0);
        // Server / library switcher.
        let server = self.pms.as_ref().map(|p| p.conn.server_name.clone()).unwrap_or_default();
        let lib = self.section.as_ref().and_then(|s| s.title.clone()).unwrap_or_default();
        let resp = ui.add_sized(
            [ui.available_width(), 44.0],
            egui::Button::new(RichText::new(format!("{lib}\n{server}")).size(12.5).color(theme::TEXT_DIM))
                .fill(egui::Color32::from_rgb(20, 20, 20))
                .corner_radius(8),
        );
        if resp.on_hover_text("Switch server or library").clicked() {
            self.auto_select = false;
            self.screen = Screen::ServerSelect;
        }
        ui.add_space(10.0);
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
            if let Some(u) = &self.user {
                ui.label(RichText::new(u.display_name()).small().color(theme::TEXT_DIM));
            }
        });
    }

    fn now_playing_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal_centered(|ui| {
            ui.label(RichText::new("Nothing playing").color(theme::TEXT_DIM));
        });
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        if let Some((text, is_error)) = self.banner.clone() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(text).color(if is_error { theme::ERROR } else { theme::TEXT_DIM }));
                if icons::button(ui, Icon::Close, 16.0, false).clicked() {
                    self.banner = None;
                }
            });
        }
        match self.nav {
            Nav::Settings => self.settings_view(ui),
            nav => {
                let title = match nav {
                    Nav::Home => "Home",
                    Nav::Search => "Search",
                    Nav::Library => "Library",
                    Nav::Playlists => "Playlists",
                    Nav::Settings => unreachable!(),
                };
                ui.heading(title);
            }
        }
    }

    fn settings_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.add_space(12.0);
        theme::card().show(ui, |ui| {
            ui.label(RichText::new("Account").strong());
            if let Some(u) = &self.user {
                ui.label(format!("Signed in as {}", u.display_name()));
            }
            ui.label(RichText::new(format!("Token stored in: {}", self.store.describe())).small().color(theme::TEXT_DIM));
            ui.add_space(8.0);
            if theme::secondary_button(ui, "Sign out").clicked() {
                self.sign_out();
            }
        });
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
        for msg in self.jobs.drain() {
            self.handle(msg);
        }
        match self.screen.clone() {
            Screen::Busy(text) => super::views::login::busy_screen(ui, &text),
            Screen::Login => self.login_screen(ui),
            Screen::ServerSelect => self.server_screen(ui),
            Screen::Main => {
                egui::Panel::bottom("now_playing")
                    .exact_size(84.0)
                    .frame(egui::Frame::new().fill(theme::BAR).inner_margin(egui::Margin::symmetric(12, 6)))
                    .show(ui, |ui| self.now_playing_bar(ui));
                egui::Panel::left("sidebar")
                    .exact_size(220.0)
                    .resizable(false)
                    .frame(egui::Frame::new().fill(theme::SIDEBAR).inner_margin(egui::Margin::symmetric(10, 6)))
                    .show(ui, |ui| self.sidebar(ui));
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(24, 8)))
                    .show(ui, |ui| self.content(ui));
            }
        }
    }

    fn on_exit(&mut self) {
        self.cancel_login();
    }
}
