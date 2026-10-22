//! Root application state: sign-in, server/library selection and the main shell
//! (sidebar, content, bottom now-playing bar).

use crate::cache::db::{self as cachedb, LibraryDb};
use crate::cache::sync::{sync_library, SyncProgress, SyncReport};
use crate::config::Config;
use crate::library_model::LibraryModel;
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

use super::art::ArtCache;
use super::icons::{self, Icon};
use super::views::common::{LibTab, Route, UiAction, ViewCtx};
use super::views::library::BrowseState;
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

/// Results delivered from worker threads.
pub enum Msg {
    PinCreated(u64, Result<Pin, PlexError>),
    PinFinished(u64, Result<String, PlexError>),
    Validated(Result<PlexUser, PlexError>),
    Servers(Result<Vec<Resource>, PlexError>),
    Probed(String, Result<ServerConnection, PlexError>),
    Sections(String, Result<Vec<Directory>, PlexError>),
    LibraryLoaded(u64, Result<Box<LibraryModel>, String>),
    SyncProgress(u64, SyncProgress),
    SyncDone(u64, Result<SyncReport, String>),
}

/// Background sync status shown in the sidebar.
#[derive(Default)]
pub struct SyncState {
    pub running: bool,
    pub progress: Option<SyncProgress>,
    pub error: Option<String>,
    pub last_report: Option<SyncReport>,
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
    pub route: Route,
    pub history: Vec<Route>,
    pub browse: BrowseState,
    pub art: ArtCache,
    /// Bumped whenever `library` is replaced (invalidates memoised lists).
    pub library_ver: u64,
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
    /// Cached library of the open section (shown immediately, refreshed after sync).
    pub library: Option<LibraryModel>,
    pub library_gen: u64,
    pub db_path: Option<std::path::PathBuf>,
    pub sync: SyncState,
    /// Running from the cache without a server connection.
    pub offline: bool,
    /// Server name shown in the sidebar (also known offline, from the cache).
    pub server_label: String,
}

impl PlexifyApp {
    pub fn new(ctx: &egui::Context, config: Config, opts: LaunchOptions, store: Arc<dyn SecretStore>) -> Self {
        theme::apply(ctx);
        let http = Http::new(ClientInfo::new(config.client_id.clone()));
        let tv = PlexTv::new(http.clone(), opts.plex_tv_url.as_deref());
        let config_image_mb = config.image_cache_mb;
        let mut app = Self {
            config,
            opts,
            route: Route::Home,
            history: Vec::new(),
            browse: BrowseState::default(),
            art: {
                let c = ctx.clone();
                ArtCache::new(crate::cache::images::ImageService::new(
                    crate::paths::image_cache_dir(),
                    config_image_mb,
                    move || c.request_repaint(),
                ))
            },
            library_ver: 0,
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
            library: None,
            library_gen: 0,
            db_path: None,
            sync: SyncState::default(),
            offline: false,
            server_label: String::new(),
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
        self.library = None;
        self.db_path = None;
        self.library_gen += 1;
        self.offline = false;
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
        let server_name = conn.server_name.clone();
        self.server_label = server_name.clone();
        self.pms = Some(PmsClient::new(self.http.clone(), conn.clone()));
        self.offline = false;
        let same = self.db_path.as_ref() == Some(&cachedb::db_path(server_id, section.key.as_deref().unwrap_or("")));
        self.section = Some(section.clone());
        self.screen = Screen::Main;
        self.route = Route::Home;
        self.history.clear();
        self.art.set_offline(false);
        self.art.svc.set_server(Some((server_id.to_string(), PmsClient::new(self.http.clone(), conn.clone()))));
        if !same || self.library.is_none() {
            // Sync starts once the cache is loaded (see Msg::LibraryLoaded).
            self.load_cached_library(server_id, &section, &server_name);
        } else {
            self.start_sync(false);
        }
    }

    /// Open the cache for a server/section and load it into memory on a worker thread.
    pub fn load_cached_library(&mut self, server_id: &str, section: &Directory, server_name: &str) {
        let key = section.key.clone().unwrap_or_default();
        let path = cachedb::db_path(server_id, &key);
        self.library_gen += 1;
        self.library = None;
        self.library_ver += 1;
        self.art.reset();
        self.sync = SyncState::default();
        self.db_path = Some(path.clone());
        let gen = self.library_gen;
        let title = section.title.clone().unwrap_or_else(|| "Music".into());
        let server_name = server_name.to_string();
        self.jobs.spawn("load-cache", move || {
            let res = LibraryDb::open(&path).and_then(|db| {
                db.set_meta("section_title", &title)?;
                if !server_name.is_empty() {
                    db.set_meta("server_name", &server_name)?;
                }
                LibraryModel::load(&db).map(Box::new)
            });
            Msg::LibraryLoaded(gen, res.map_err(|e| e.to_string()))
        });
    }

    /// Sync the open library in the background (delta unless `full`).
    pub fn start_sync(&mut self, full: bool) {
        if self.sync.running {
            return;
        }
        let (Some(pms), Some(section), Some(path)) = (self.pms.clone(), self.section.clone(), self.db_path.clone()) else {
            return;
        };
        self.sync.running = true;
        self.sync.error = None;
        self.sync.progress = None;
        let gen = self.library_gen;
        let (tx, ctx) = self.jobs.sender();
        let key = section.key.unwrap_or_default();
        self.jobs.spawn("sync", move || {
            let mut last_sent = std::time::Instant::now();
            let res = LibraryDb::open(&path).map_err(|e| e.to_string()).and_then(|mut db| {
                sync_library(&pms, &mut db, &key, full, &mut |p| {
                    if last_sent.elapsed() > std::time::Duration::from_millis(150) {
                        last_sent = std::time::Instant::now();
                        let _ = tx.send(Msg::SyncProgress(gen, p));
                        ctx.request_repaint();
                    }
                })
                .map_err(|e| match e {
                    crate::cache::sync::SyncError::Plex(p) => p.user_message(),
                    other => other.to_string(),
                })
            });
            Msg::SyncDone(gen, res)
        });
    }

    /// Start without a server: show the cached library of the remembered server/section.
    pub fn enter_offline(&mut self, reason: &str) -> bool {
        let (Some(server_id), Some(key)) = (self.config.last_server_id.clone(), self.config.last_section_key.clone()) else {
            return false;
        };
        let path = cachedb::db_path(&server_id, &key);
        if !path.exists() {
            return false;
        }
        let (title, server_name) = LibraryDb::open(&path)
            .ok()
            .map(|db| (db.meta("section_title").ok().flatten(), db.meta("server_name").ok().flatten()))
            .unwrap_or((None, None));
        log::warn!("entering offline mode ({reason}); using cache {}", path.display());
        let section = Directory { key: Some(key), title: title.or(Some("Music".into())), kind: Some("artist".into()), ..Default::default() };
        self.offline = true;
        self.server_label = server_name.clone().unwrap_or_default();
        self.pms = None;
        self.art.set_offline(true);
        self.art.svc.set_server(None);
        // Disk-cached art is keyed by server id, so it still shows offline.
        self.art.svc.set_server_id_only(&server_id);
        self.section = Some(section.clone());
        self.screen = Screen::Main;
        self.banner = Some((format!("Offline — showing your cached library. {reason}"), true));
        self.load_cached_library(&server_id, &section, server_name.as_deref().unwrap_or(""));
        true
    }

    /// Try to reconnect after an offline start.
    pub fn reconnect(&mut self) {
        self.banner = None;
        self.auto_select = true;
        if self.user.is_some() {
            self.screen = Screen::Busy("Connecting to your server…".into());
            self.refresh_servers();
        } else {
            self.validate_token();
        }
    }

    /// Wipe the cached library of the open section and re-sync if online.
    pub fn clear_library_cache(&mut self) {
        let Some(path) = self.db_path.clone() else { return };
        match LibraryDb::open(&path).and_then(|db| db.clear()) {
            Ok(()) => {
                log::info!("library cache cleared");
                self.library = Some(LibraryModel::default());
                self.banner = Some(("Library cache cleared.".into(), false));
                self.start_sync(true);
            }
            Err(e) => self.banner = Some((format!("Could not clear the cache: {e}"), true)),
        }
    }

    /// Re-read the cache into memory (after a sync changed it).
    pub fn reload_library(&mut self) {
        let Some(path) = self.db_path.clone() else { return };
        let gen = self.library_gen;
        self.jobs.spawn("reload-cache", move || {
            let res = LibraryDb::open(&path).and_then(|db| LibraryModel::load(&db).map(Box::new));
            Msg::LibraryLoaded(gen, res.map_err(|e| e.to_string()))
        });
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
                    if !self.enter_offline(&e.user_message()) {
                        self.servers_error = Some(format!("Offline: {}", e.user_message()));
                        self.screen = Screen::ServerSelect;
                    }
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
                    if !(self.auto_select && self.enter_offline(&e.user_message())) {
                        self.servers_error = Some(e.user_message());
                        self.screen = Screen::ServerSelect;
                    }
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
                        ProbeStatus::Failed(e) if self.auto_select => {
                            self.auto_select = false;
                            let e = e.clone();
                            if !self.enter_offline(&e) {
                                self.screen = Screen::ServerSelect;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Msg::LibraryLoaded(gen, res) if gen == self.library_gen => {
                let first_load = self.library.is_none();
                match res {
                    Ok(m) => {
                        self.library = Some(*m);
                        self.library_ver += 1;
                    }
                    Err(e) => {
                        log::error!("cannot load cache: {e}");
                        self.banner = Some((format!("Could not read the library cache: {e}"), true));
                        self.library = Some(LibraryModel::default());
                    }
                }
                if first_load && !self.offline {
                    self.start_sync(false);
                }
            }
            Msg::SyncProgress(gen, p) if gen == self.library_gen => self.sync.progress = Some(p),
            Msg::SyncDone(gen, res) if gen == self.library_gen => {
                self.sync.running = false;
                self.sync.progress = None;
                match res {
                    Ok(report) => {
                        let changed = report.upserted > 0 || report.deleted > 0 || report.playlists_refreshed > 0 || report.full;
                        self.sync.last_report = Some(report);
                        if changed {
                            self.reload_library();
                        } else if let Some(l) = &mut self.library {
                            l.last_sync_at = Some(crate::logging::unix_now());
                        }
                    }
                    Err(e) => {
                        log::warn!("sync failed: {e}");
                        self.sync.error = Some(e);
                    }
                }
            }
            Msg::LibraryLoaded(..) | Msg::SyncProgress(..) | Msg::SyncDone(..) => {}
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
        let server = self.server_label.clone();
        let lib = self.section.as_ref().and_then(|s| s.title.clone()).unwrap_or_default();
        let resp = ui.add_sized(
            [ui.available_width(), 44.0],
            egui::Button::new(RichText::new(format!("{lib}\n{server}")).size(12.5).color(theme::TEXT_DIM))
                .fill(egui::Color32::from_rgb(20, 20, 20))
                .corner_radius(8),
        );
        if resp.on_hover_text("Switch server or library").clicked() {
            self.auto_select = false;
            if self.offline {
                self.reconnect();
            } else {
                self.screen = Screen::ServerSelect;
            }
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let (text, color) = self.sync_status_text();
            ui.label(RichText::new(text).size(11.5).color(color));
        });
        ui.add_space(8.0);
        let cat = route_category(&self.route);
        let mut go = None;
        for (target, icon, label) in [
            (Route::Home, Icon::Home, "Home"),
            (Route::Search, Icon::Search, "Search"),
            (Route::Library(LibTab::Albums), Icon::Library, "Library"),
            (Route::Playlists, Icon::Playlist, "Playlists"),
        ] {
            if nav_button(ui, route_category(&target) == cat, icon, label) {
                go = Some(target);
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(10.0);
            if nav_button(ui, cat == 4, Icon::Settings, "Settings") {
                go = Some(Route::Settings);
            }
            if let Some(u) = &self.user {
                ui.label(RichText::new(u.display_name()).small().color(theme::TEXT_DIM));
            }
        });
        if let Some(r) = go {
            // Sidebar navigation starts a fresh history (keeps the library tab if already there).
            let r = match (&r, &self.route) {
                (Route::Library(_), Route::Library(t)) => Route::Library(*t),
                _ => r,
            };
            self.history.clear();
            self.route = r;
        }
    }

    pub fn navigate(&mut self, r: Route) {
        if r != self.route {
            let prev = std::mem::replace(&mut self.route, r);
            // Switching library tabs replaces instead of stacking.
            if !matches!((&prev, &self.route), (Route::Library(_), Route::Library(_))) {
                self.history.push(prev);
                if self.history.len() > 50 {
                    self.history.remove(0);
                }
            }
        }
    }

    pub fn back(&mut self) {
        if let Some(r) = self.history.pop() {
            self.route = r;
        }
    }

    fn apply_action(&mut self, a: UiAction) {
        match a {
            UiAction::Navigate(r) => self.navigate(r),
            UiAction::Rate(id, rating) => self.rate_track(id, rating),
            UiAction::Play(..) | UiAction::PlayShuffled(..) | UiAction::PlayNext(..) | UiAction::AddToQueue(..) | UiAction::Radio(..) => {
                self.playback_action(a)
            }
        }
    }

    /// Placeholder until the playback engine lands.
    fn playback_action(&mut self, a: UiAction) {
        log::info!("playback action: {a:?}");
        self.banner = Some(("Playback is not wired up yet.".into(), false));
    }

    /// Rate a track locally (cache + model) and on the server.
    pub fn rate_track(&mut self, id: crate::library_model::Id, rating: u8) {
        let value = (rating > 0).then_some(rating as f64);
        if let Some(l) = &mut self.library {
            l.apply_rating(id, rating as f32);
            self.library_ver += 1;
        }
        if let Some(path) = self.db_path.clone() {
            if let Ok(db) = LibraryDb::open(&path) {
                let _ = db.set_track_rating(id, value);
            }
        }
        if let Some(pms) = self.pms.clone() {
            std::thread::spawn(move || {
                if let Err(e) = pms.rate(&id.to_string(), (rating > 0).then_some(rating)) {
                    log::warn!("rating failed: {e}");
                }
            });
        }
    }

    /// One-line sync/connection status for the sidebar.
    pub fn sync_status_text(&self) -> (String, egui::Color32) {
        if self.offline {
            return ("Offline (cached library)".into(), theme::ERROR);
        }
        if self.sync.running {
            return match &self.sync.progress {
                Some(p) => match p.total {
                    Some(t) if t > 0 => (format!("Syncing {} {}/{}", p.stage, p.done, t), theme::ACCENT),
                    _ => (format!("Syncing {}…", p.stage), theme::ACCENT),
                },
                None => ("Syncing…".into(), theme::ACCENT),
            };
        }
        if let Some(e) = &self.sync.error {
            return (format!("Sync failed: {e}"), theme::ERROR);
        }
        match self.library.as_ref().and_then(|l| l.last_sync_at) {
            Some(ts) => (format!("Synced {}", crate::logging::ago(ts)), theme::TEXT_DIM),
            None => ("Not synced yet".into(), theme::TEXT_DIM),
        }
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
                if self.offline && ui.link("Reconnect").clicked() {
                    self.reconnect();
                }
                if icons::button(ui, Icon::Close, 16.0, false).clicked() {
                    self.banner = None;
                }
            });
            ui.add_space(6.0);
        }
        if self.route == Route::Settings {
            if !self.history.is_empty() && super::views::common::page_header(ui, "", true) {
                self.back();
            }
            self.settings_view(ui);
            return;
        }
        if !self.history.is_empty()
            && matches!(self.route, Route::Artist(_) | Route::Album(_) | Route::Playlist(_) | Route::Genre(_))
            && icons::button(ui, Icon::Back, 26.0, false).on_hover_text("Back").clicked()
        {
            self.back();
            return;
        }
        let Some(lib) = self.library.as_ref() else {
            ui.horizontal(|ui| {
                theme::spinner(ui, 16.0);
                ui.label(RichText::new("Loading your library…").color(theme::TEXT_DIM));
            });
            return;
        };
        if lib.is_empty() && !matches!(self.route, Route::Search) {
            ui.heading("Your library");
            ui.add_space(8.0);
            let msg = if self.sync.running {
                "Fetching your library for the first time…"
            } else if self.offline {
                "Nothing cached for this library yet. Connect to your server to load it."
            } else {
                "This library is empty. Add music to it in Plex and it will show up here."
            };
            ui.label(RichText::new(msg).color(theme::TEXT_DIM));
            return;
        }
        let mut actions = Vec::new();
        let route = self.route.clone();
        let greeting = greeting();
        {
            let mut vc = ViewCtx { lib, art: &mut self.art, actions: &mut actions, now_playing: None };
            use super::views::{detail, library};
            match &route {
                Route::Home => detail::home_view(ui, &mut vc, &greeting),
                Route::Library(tab) => library::library_view(ui, &mut vc, &mut self.browse, *tab, self.library_ver),
                Route::Playlists => library::playlists_view(ui, &mut vc),
                Route::Album(id) => detail::album_view(ui, &mut vc, *id),
                Route::Artist(id) => detail::artist_view(ui, &mut vc, *id),
                Route::Playlist(id) => detail::playlist_view(ui, &mut vc, *id),
                Route::Genre(g) => detail::genre_view(ui, &mut vc, g),
                Route::Search => {
                    ui.heading("Search");
                }
                Route::Settings => {}
            }
        }
        for a in actions {
            self.apply_action(a);
        }
    }

    fn settings_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.add_space(12.0);
        theme::card().show(ui, |ui| {
            ui.label(RichText::new("Library cache").strong());
            ui.label(format!("Database size: {}", human_bytes(crate::paths::dir_size(&crate::paths::db_dir()))));
            let (status, _) = self.sync_status_text();
            ui.label(RichText::new(status).small().color(theme::TEXT_DIM));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!self.offline && !self.sync.running, |ui| {
                    if theme::secondary_button(ui, "Refresh library").on_hover_text("Re-download the whole library").clicked() {
                        self.start_sync(true);
                    }
                });
                ui.add_enabled_ui(!self.sync.running, |ui| {
                    if theme::secondary_button(ui, "Clear cache").clicked() {
                        self.clear_library_cache();
                    }
                });
            });
        });
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

pub fn human_bytes(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.1} KB", n as f64 / 1024.0),
        1_048_576..=1_073_741_823 => format!("{:.1} MB", n as f64 / 1_048_576.0),
        _ => format!("{:.2} GB", n as f64 / 1_073_741_824.0),
    }
}

fn greeting() -> String {
    // Local hour isn't available without a tz crate; keep it neutral.
    "Welcome back".into()
}

/// Sidebar group a route belongs to (0 home, 1 search, 2 library, 3 playlists, 4 settings).
fn route_category(r: &Route) -> u8 {
    match r {
        Route::Home => 0,
        Route::Search => 1,
        Route::Library(_) | Route::Artist(_) | Route::Album(_) | Route::Genre(_) => 2,
        Route::Playlists | Route::Playlist(_) => 3,
        Route::Settings => 4,
    }
}

fn nav_button(ui: &mut egui::Ui, selected: bool, icon: Icon, label: &str) -> bool {
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
    resp.clicked()
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
        self.art.begin_frame(ui.ctx());
        if ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Extra1)) {
            self.back();
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
