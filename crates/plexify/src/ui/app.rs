//! Root application state: sign-in, server/library selection and the main shell
//! (sidebar, content, bottom now-playing bar).

use crate::cache::db::{self as cachedb, LibraryDb};
use crate::cache::sync::{sync_library, SyncProgress, SyncReport};
use crate::audio::engine::{Engine, EngineConfig, PlayState, PlaybackStatus, TrackSpec};
use crate::audio::source::{AudioCache, SourceSpec};
use crate::config::{Config, RepeatMode};
use crate::library_model::LibraryModel;
use crate::plex::auth::PlexTv;
use crate::plex::models::{Directory, Hub, Pin, PlexUser, Resource};
use crate::plex::resources::{pick_best, ServerConnection};
use crate::library_model::Id;
use crate::player::mixes::Mixes;
use crate::player::radio;
use crate::player::report::{Reporter, ReportState};
use crate::player::session::Session;
use crate::plex::server::{PlayState as ServerPlayState, PmsClient};
use crate::plex::{ClientInfo, Http, PlexError};
use crate::secure_store::SecretStore;
use egui::{Align, Layout, RichText};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use super::art::ArtCache;
use super::icons::{self, Icon};
use super::runner::WindowCmd;
use crate::win::smtc::{NowPlaying, Smtc, SmtcButton, SmtcEvents};
use crate::win::tray::{TrayEvent, TrayHandle};
use super::views::common::{fmt_duration, LibTab, RadioSeed, Route, UiAction, ViewCtx};
use super::views::library::BrowseState;
use super::views::search::SearchState;
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
    /// Library generation, query and hubs from `/hubs/search`.
    SearchHubs(u64, u64, String, Result<Vec<Hub>, PlexError>),
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

#[derive(Debug)]
enum BarCmd {
    Toggle,
    Next,
    Prev,
    Seek(u64),
    Volume(f32),
    Mute(bool),
    Shuffle(bool),
    Repeat,
    Queue,
    Rate(u8),
}

#[derive(Debug)]
enum QueueCmd {
    Remove(usize),
    Move(usize, usize),
    ClearUpcoming,
    Close,
}

/// What a finished radio fetch should do with its station.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RadioMode {
    /// Replace the queue and start listening (the user asked for a station).
    Play,
    /// Append to the finished queue and keep playing (autoplay).
    Autoplay,
}

/// A sonic-similarity lookup running on a worker thread.
struct RadioJob {
    rx: std::sync::mpsc::Receiver<Vec<Id>>,
    mode: RadioMode,
    seed: Id,
    /// Tracks queued before the station (album or artist seed) — they play first.
    base: Vec<Id>,
}

/// Fresh salt for a station's weighted shuffle.
fn radio_salt() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9)
}

/// File extension for a codec name, used as the decoder hint and cache suffix.
fn codec_extension(codec: &str) -> &'static str {
    match codec {
        "mp3" => "mp3",
        "flac" => "flac",
        "aac" => "m4a",
        "alac" => "m4a",
        "vorbis" => "ogg",
        "opus" => "opus",
        "pcm" | "wav" => "wav",
        _ => "bin",
    }
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
    pub search: SearchState,
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
    /// Playback engine, created on the first play.
    pub engine: Option<Engine>,
    /// Last status polled from the engine.
    pub playback: PlaybackStatus,
    /// Turns engine status into `/:/timeline` and `/:/scrobble` calls.
    pub report: ReportState,
    pub reporter: Option<Reporter>,
    /// Base URL of the server the reporter belongs to.
    pub report_base: Option<String>,
    /// Last session written to disk (lets us skip unchanged writes).
    pub saved_session: Session,
    pub last_session_save: std::time::Instant,
    /// The saved queue has been restored already (once per run).
    pub restored: bool,
    /// Last track recorded in the local play history.
    pub last_played_id: Option<i64>,
    /// Whether the queue side panel is open.
    pub queue_open: bool,
    /// Play-order position of the queue row being dragged.
    pub queue_drag: Option<usize>,
    /// Home mixes (Most played, Recently played, Rediscover).
    pub mixes: Mixes,
    /// A play was recorded since the mixes were last built.
    mixes_dirty: bool,
    /// Sonic-similarity lookup in flight for a new radio station or autoplay.
    radio_job: Option<RadioJob>,
    /// Last track the engine actually played (the seed for autoplay).
    last_playing: Option<Id>,
    /// Set when a station was enqueued behind a finished queue. The engine reports its first
    /// Playing event a moment later, and the app only repaints while it knows it is playing, so
    /// keep drawing until that event has been seen (or the deadline passes).
    wake_poll: Option<std::time::Instant>,
    /// Compact always-on-top layout.
    mini: bool,
    /// Window geometry/behaviour changes still to be sent for the mini-player toggle.
    mini_geometry: bool,
    /// Tray icon and media keys (Windows).
    tray: Option<TrayHandle>,
    /// Windows media overlay (SMTC); absent when the OS does not offer it.
    smtc: Option<Smtc>,
    /// Buttons pressed on the media overlay, delivered on the next frame.
    smtc_events: SmtcEvents,
    /// A quit was requested from the tray menu.
    quitting: bool,
    /// The window title last sent to the window manager.
    window_title: String,
    /// Hide the window on the next frame (close to tray, or show after hiding).
    hide_window: bool,
    /// Client area size seen in the last frame, saved on exit so the window reopens as it was.
    last_window_size: Option<[f32; 2]>,
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
            search: SearchState::default(),
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
            engine: None,
            playback: PlaybackStatus::default(),
            report: ReportState::default(),
            reporter: None,
            report_base: None,
            saved_session: Session::load().unwrap_or_default(),
            last_session_save: std::time::Instant::now(),
            restored: false,
            last_played_id: None,
            queue_open: false,
            queue_drag: None,
            mixes: Mixes::default(),
            mixes_dirty: true,
            radio_job: None,
            last_playing: None,
            wake_poll: None,
            mini: false,
            mini_geometry: false,
            tray: None,
            smtc: None,
            smtc_events: SmtcEvents::default(),
            quitting: false,
            window_title: String::new(),
            hide_window: false,
            last_window_size: None,
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
        self.search = SearchState::default();
        self.offline = false;
        self.selected_server = None;
        self.reporter = None;
        self.report_base = None;
        self.mixes = Mixes::default();
        self.radio_job = None;
        self.wake_poll = None;
        Session::clear();
        self.saved_session = Session::default();
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
        self.search = SearchState::default();
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
                self.mixes = Mixes::default();
                self.mixes_dirty = true;
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

    /// Remember the current window size so the next start reopens as the user left it.
    fn save_window_size(&mut self) {
        if let Some(size) = self.last_window_size.take() {
            if self.config.window_size != Some(size) {
                self.config.window_size = Some(size);
                self.save_config();
            }
        }
    }

    /// Play/pause from a shortcut, the tray menu or a media key.
    fn toggle_playback(&mut self) {
        if let Some(eng) = &self.engine {
            eng.play_pause();
        }
        self.wake_poll = Some(std::time::Instant::now() + Duration::from_secs(2));
        self.poll_playback();
    }

    /// Skip forward (`dir > 0`) or back.
    fn skip_track(&mut self, dir: i32) {
        if let Some(eng) = &self.engine {
            if dir > 0 {
                eng.next();
            } else {
                eng.prev();
            }
        }
        self.wake_poll = Some(std::time::Instant::now() + Duration::from_secs(2));
        self.poll_playback();
    }

    /// Seek relative to the current position, clamped to the track.
    fn seek_by(&mut self, delta_ms: i64) {
        if self.playback.duration_ms == 0 {
            return;
        }
        let target = (self.playback.position_ms as i64 + delta_ms).clamp(0, self.playback.duration_ms as i64) as u64;
        if let Some(eng) = &self.engine {
            eng.seek(target);
        }
        self.wake_poll = Some(std::time::Instant::now() + Duration::from_secs(1));
        self.poll_playback();
    }

    fn nudge_volume(&mut self, delta: f32) {
        let v = (self.config.volume + delta).clamp(0.0, 1.0);
        if let Some(eng) = &self.engine {
            eng.set_volume(v);
            if v > 0.0 && self.config.muted {
                eng.set_muted(false);
                self.config.muted = false;
            }
        }
        self.config.volume = v;
        self.save_config();
        self.poll_playback();
    }

    fn toggle_mini(&mut self) {
        self.mini = !self.mini;
        self.mini_geometry = true;
    }

    /// Keyboard shortcuts while the main window has focus: space play/pause, arrows seek,
    /// Ctrl+arrows skip/volume, Ctrl+M mini-player.
    fn shortcuts(&mut self, ui: &mut egui::Ui) {
        let (mut toggle, mut seek, mut skip, mut volume, mut mini) = (false, 0i64, 0i32, 0.0f32, false);
        ui.input_mut(|i| {
            toggle = i.consume_key(egui::Modifiers::NONE, egui::Key::Space);
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                seek += 5_000;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                seek -= 5_000;
            }
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::ArrowRight) {
                skip += 1;
            }
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::ArrowLeft) {
                skip -= 1;
            }
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::ArrowUp) {
                volume += 0.05;
            }
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::ArrowDown) {
                volume -= 0.05;
            }
            mini = i.consume_key(egui::Modifiers::COMMAND, egui::Key::M);
        });
        if toggle {
            self.toggle_playback();
        }
        if seek != 0 {
            self.seek_by(seek);
        }
        if skip != 0 {
            self.skip_track(skip.signum());
        }
        if volume != 0.0 {
            self.nudge_volume(volume);
        }
        if mini {
            self.toggle_mini();
        }
    }

    /// Compact always-on-top layout: art, title, basic transport and a seek bar.
    fn mini_player(&mut self, ui: &mut egui::Ui) {
        enum Cmd {
            Toggle,
            Next,
            Prev,
            Seek(u64),
            Expand,
        }
        let st = self.playback.clone();
        let track = st.track.clone();
        let thumb = track
            .as_ref()
            .and_then(|t| self.library.as_ref().and_then(|l| l.track(t.id)))
            .and_then(|t| t.thumb.clone());
        let mut cmds = Vec::new();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(52.0, 52.0), egui::Sense::hover());
            match &track {
                Some(t) => self.art.paint(ui, rect, thumb.as_deref(), &t.title, super::art::Shape::Rounded(6)),
                None => {
                    ui.painter().rect_filled(rect, 6.0, theme::CARD_HOVER);
                }
            }
            ui.add_space(10.0);
            ui.vertical(|ui| {
                let (title, sub) = match &track {
                    Some(t) => (t.title.clone(), format!("{} · {}", t.artist, t.album)),
                    None => ("Nothing playing".to_string(), "Pick a track in the full window".to_string()),
                };
                ui.add(egui::Label::new(RichText::new(title).strong().size(15.0)).truncate());
                ui.add(egui::Label::new(RichText::new(sub).small().color(theme::TEXT_DIM)).truncate());
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if icons::button(ui, Icon::Prev, 15.0, false).on_hover_text("Previous").clicked() {
                        cmds.push(Cmd::Prev);
                    }
                    let playing = st.state == PlayState::Playing;
                    if icons::button(ui, if playing { Icon::Pause } else { Icon::Play }, 20.0, false)
                        .on_hover_text(if playing { "Pause" } else { "Play" })
                        .clicked()
                    {
                        cmds.push(Cmd::Toggle);
                    }
                    if icons::button(ui, Icon::Next, 15.0, false).on_hover_text("Next").clicked() {
                        cmds.push(Cmd::Next);
                    }
                    ui.add_space(6.0);
                    let mut pos = st.position_ms as f64 / 1000.0;
                    let dur = (st.duration_ms as f64 / 1000.0).max(0.001);
                    ui.spacing_mut().slider_width = 110.0;
                    if ui.add(egui::Slider::new(&mut pos, 0.0..=dur).show_value(false)).changed() {
                        cmds.push(Cmd::Seek((pos * 1000.0) as u64));
                    }
                    ui.label(
                        RichText::new(format!("{} / {}", fmt_duration(st.position_ms), fmt_duration(st.duration_ms)))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                });
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icons::button(ui, Icon::MiniPlayer, 18.0, false).on_hover_text("Back to the full window").clicked() {
                    cmds.push(Cmd::Expand);
                }
            });
        });
        for c in cmds {
            match c {
                Cmd::Toggle => self.toggle_playback(),
                Cmd::Next => self.skip_track(1),
                Cmd::Prev => self.skip_track(-1),
                Cmd::Seek(ms) => {
                    if let Some(eng) = &self.engine {
                        eng.seek(ms);
                    }
                    self.poll_playback();
                    // Keep polling while the seek lands.
                    self.wake_poll = Some(std::time::Instant::now() + Duration::from_secs(1));
                }
                Cmd::Expand => self.toggle_mini(),
            }
        }
        if st.state == PlayState::Playing {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Kick off the debounced server search when the query has settled and local results
    /// already came from the current snapshot. Called once per frame in the main shell.
    fn maybe_search_server(&mut self, now: f64) {
        if self.route != Route::Search {
            return;
        }
        let q = self.search.query.trim().to_lowercase();
        if q != self.search.last_seen {
            self.search.last_seen = q.clone();
            self.search.changed_at = now;
            self.search.server_for = None;
            self.search.server_hubs.clear();
            self.search.server_running = false;
        }
        if q.is_empty() || self.search.server_for.as_deref() == Some(q.as_str()) || self.search.server_running {
            return;
        }
        if now - self.search.changed_at < 0.4 || self.offline {
            return;
        }
        let (Some(pms), Some(section)) = (self.pms.clone(), self.section.clone()) else { return };
        let Some(section_key) = section.key.clone() else { return };
        self.search.server_running = true;
        self.search.server_seq += 1;
        let seq = self.search.server_seq;
        let gen = self.library_gen;
        let query = self.search.query.trim().to_string();
        log::debug!("server search for {query:?}");
        self.jobs.spawn("search-hubs", move || {
            let res = pms.search(&section_key, &query, 10);
            Msg::SearchHubs(seq, gen, query.to_lowercase(), res)
        });
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
                self.maybe_restore_session();
                self.refresh_mixes();
            }
            Msg::SearchHubs(seq, gen, query, res) => {
                if seq != self.search.server_seq {
                    return;
                }
                self.search.server_running = false;
                if gen == self.library_gen && query == self.search.query.trim().to_lowercase() {
                    self.search.server_for = Some(query);
                    match res {
                        Ok(hubs) => {
                            log::debug!("server search: {} hubs", hubs.len());
                            self.search.server_hubs = hubs;
                        }
                        Err(e) => {
                            log::debug!("server search failed: {e}");
                            self.search.server_hubs.clear();
                        }
                    }
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
            if self.route == Route::Search {
                self.search.focus = true;
            }
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

    /// Turn UI playback intents into engine commands.
    fn playback_action(&mut self, a: UiAction) {
        match a {
            UiAction::Play(ids, start) => self.play_ids(&ids, start),
            UiAction::PlayShuffled(ids) => {
                let mut ids = ids;
                // Fisher-Yates. Smart shuffle with history/artist spread arrives with the queue.
                let mut seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
                    .unwrap_or(0x9e37_79b9)
                    | 1;
                for i in (1..ids.len()).rev() {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    ids.swap(i, (seed as usize) % (i + 1));
                }
                self.play_ids(&ids, 0);
            }
            UiAction::PlayNext(ids) => {
                let specs = self.queue_specs(&ids);
                if specs.is_empty() {
                    return;
                }
                let n = specs.len();
                self.ensure_engine().insert_next(specs);
                self.banner = Some((format!("Added {n} {} to play next.", if n == 1 { "track" } else { "tracks" }), false));
                self.poll_playback();
            }
            UiAction::AddToQueue(ids) => {
                let specs = self.queue_specs(&ids);
                if specs.is_empty() {
                    return;
                }
                let n = specs.len();
                self.ensure_engine().enqueue(specs);
                self.banner = Some((format!("Added {n} {} to the queue.", if n == 1 { "track" } else { "tracks" }), false));
                self.poll_playback();
            }
            UiAction::Radio(seed) => self.start_radio(seed),
            _ => {}
        }
    }

    /// Start a station from a track, album or artist. Album and artist seeds play their tracks
    /// first, then the similar ones. The sonic lookup runs off the UI thread; without a server
    /// (or without Plex Pass) the local artist/genre fallback answers immediately.
    fn start_radio(&mut self, seed: RadioSeed) {
        let Some(lib) = self.library.as_ref() else {
            self.banner = Some(("Open a library first.".into(), true));
            return;
        };
        let (seed_id, base) = match &seed {
            RadioSeed::Track(id) => (*id, vec![*id]),
            RadioSeed::Album(id) => {
                let tracks = lib.album_track_ids(*id);
                match tracks.first() {
                    Some(&first) => (first, tracks),
                    None => {
                        self.banner = Some(("This album has no tracks.".into(), true));
                        return;
                    }
                }
            }
            RadioSeed::Artist(id) => {
                let tracks = lib.artist_track_ids(*id);
                match tracks.first() {
                    Some(&first) => (first, tracks),
                    None => {
                        self.banner = Some(("This artist has no tracks.".into(), true));
                        return;
                    }
                }
            }
        };
        self.start_station(seed_id, base, RadioMode::Play);
    }

    /// Ask the server for similar tracks (worker thread) and then build the station. With no
    /// server the local fallback is used directly.
    fn start_station(&mut self, seed: Id, base: Vec<Id>, mode: RadioMode) {
        let online = !self.offline && self.pms.is_some();
        if !online {
            self.finish_station(mode, seed, base, Vec::new());
            return;
        }
        let pms = self.pms.clone().expect("online implies a server");
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("radio".into())
            .spawn(move || {
                let ids = radio::sonic_ids(&pms, seed, radio::STATION_LEN);
                let _ = tx.send(ids);
            });
        if spawned.is_err() {
            log::warn!("cannot start the radio thread; using the local fallback");
            self.finish_station(mode, seed, base, Vec::new());
            return;
        }
        self.banner = Some(("Finding similar tracks…".into(), false));
        self.radio_job = Some(RadioJob { rx, mode, seed, base });
    }

    /// Build and apply a station once the server answer (possibly empty) is in.
    fn finish_station(&mut self, mode: RadioMode, seed: Id, base: Vec<Id>, from_server: Vec<Id>) {
        let Some(lib) = self.library.as_ref() else { return };
        let exclude: std::collections::HashSet<Id> = base.iter().copied().collect();
        let rest = radio::station(lib, seed, &from_server, &exclude, radio::STATION_LEN, radio_salt());
        let mut ids = base;
        ids.extend(rest);
        if ids.is_empty() {
            self.banner = Some(("No tracks available for this station.".into(), true));
            return;
        }
        match mode {
            RadioMode::Play => {
                self.banner = Some((format!("Radio: {} tracks.", ids.len()), false));
                self.play_ids(&ids, 0);
            }
            RadioMode::Autoplay => {
                if self.playback.state != PlayState::Idle {
                    log::debug!("dropping autoplay station: playback resumed meanwhile");
                    return;
                }
                let specs = self.queue_specs(&ids);
                if specs.is_empty() {
                    return;
                }
                let n = specs.len();
                self.ensure_engine().enqueue(specs);
                self.banner = Some((format!("Autoplay: added {n} tracks."), false));
                self.wake_poll = Some(std::time::Instant::now() + Duration::from_secs(3));
                self.poll_playback();
            }
        }
    }

    /// Apply a finished sonic lookup, if there is one.
    fn poll_radio(&mut self) {
        let Some(job) = &self.radio_job else { return };
        let from_server = match job.rx.try_recv() {
            Ok(ids) => ids,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Vec::new(),
        };
        let job = self.radio_job.take().expect("checked above");
        self.finish_station(job.mode, job.seed, job.base, from_server);
    }

    /// Rebuild the Home mixes from the library and the local play history.
    fn refresh_mixes(&mut self) {
        let Some(lib) = &self.library else { return };
        let db = self.db_path.as_ref().and_then(|p| LibraryDb::open(p).ok());
        self.mixes = Mixes::load(lib, db.as_ref(), crate::logging::unix_now());
        self.mixes_dirty = false;
    }

    /// Start playback of `ids` at `start`, building stream sources for the current server.
    fn play_ids(&mut self, ids: &[crate::library_model::Id], start: usize) {
        if self.pms.is_none() {
            self.banner = Some(("Connect to your server to play music.".into(), true));
            return;
        }
        let specs = self.track_specs(ids);
        if specs.is_empty() {
            self.banner = Some(("These tracks have no playable audio.".into(), true));
            return;
        }
        let idx = start.min(specs.len() - 1);
        self.ensure_engine().play_playlist(specs, idx);
        self.poll_playback();
    }

    /// Specs for queue edits: like [`Self::track_specs`] but reports why nothing could be queued.
    fn queue_specs(&mut self, ids: &[crate::library_model::Id]) -> Vec<TrackSpec> {
        if self.pms.is_none() {
            self.banner = Some(("Connect to your server to queue music.".into(), true));
            return Vec::new();
        }
        let specs = self.track_specs(ids);
        if specs.is_empty() {
            self.banner = Some(("These tracks have no playable audio.".into(), true));
        }
        specs
    }

    /// Create the engine on first use. Doing it lazily keeps startup free of audio-device work.
    fn ensure_engine(&mut self) -> &Engine {
        if self.engine.is_none() {
            let cache = AudioCache::new(
                crate::paths::audio_cache_dir(),
                self.config.audio_cache_mb.saturating_mul(1024 * 1024),
                self.config.audio_cache_enabled,
            );
            cache.cleanup_partials();
            let cfg = EngineConfig {
                volume: self.config.volume,
                muted: self.config.muted,
                crossfade_ms: (self.config.crossfade_secs.clamp(0.0, 12.0) * 1000.0).round() as u32,
                normalization: self.config.normalization,
            };
            log::info!("starting playback engine (crossfade {} ms, normalization {})", cfg.crossfade_ms, cfg.normalization);
            let eng = Engine::start(cache, self.http.clone(), cfg);
            eng.set_shuffle(self.config.shuffle);
            eng.set_repeat(self.config.repeat);
            self.engine = Some(eng);
        }
        self.engine.as_ref().expect("engine just created")
    }

    /// Stream descriptions for `ids`. Opus is not decodable locally and a bitrate cap asks the
    /// server to transcode, so both go through the universal transcoder.
    fn track_specs(&self, ids: &[crate::library_model::Id]) -> Vec<TrackSpec> {
        let (Some(lib), Some(pms)) = (self.library.as_ref(), self.pms.as_ref()) else {
            return Vec::new();
        };
        let cap = match self.config.quality {
            crate::config::StreamQuality::Kbps(k) => Some(k),
            crate::config::StreamQuality::Original => None,
        };
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(t) = lib.track(*id) else { continue };
            let codec = t.codec.as_deref().unwrap_or("").to_ascii_lowercase();
            let container = t.container.as_deref().unwrap_or("").to_ascii_lowercase();
            let source = if cap.is_some() || codec == "opus" {
                let kbps = cap.unwrap_or(320);
                let url = pms.transcode_url(&t.id.to_string(), kbps, &format!("plexify-{}", self.config.client_id), &self.config.client_id);
                SourceSpec::Http { url, key: format!("{}_t{kbps}.mp3", t.id) }
            } else if let Some(part) = t.part_key.as_deref() {
                let ext = if container.is_empty() { codec_extension(&codec) } else { container.as_str() };
                SourceSpec::Http { url: pms.part_url(part), key: format!("{}.{ext}", t.id) }
            } else {
                log::debug!("track {} has no part key; skipped", t.id);
                continue;
            };
            out.push(TrackSpec {
                id: t.id,
                title: t.title.to_string(),
                artist: t.display_artist().to_string(),
                album: t.album.to_string(),
                duration_ms: t.duration_ms as u64,
                gain_db: t.gain,
                album_gain_db: t.album_gain,
                // Smart shuffle favours well-rated and often-played tracks.
                weight: 1.0 + t.user_rating * 0.15 + (t.view_count as f32).ln_1p() * 0.6,
                source,
            });
        }
        out
    }

    fn poll_playback(&mut self) {
        let Some(e) = &self.engine else { return };
        let prev_state = self.playback.state;
        let st = e.status();
        let state = st.state;
        let track = st.track.as_ref().map(|t| t.id);
        if st.error.is_some() && st.error != self.playback.error {
            if let Some(msg) = st.error.clone() {
                self.banner = Some((msg, true));
            }
        }
        self.playback = st;
        if state == PlayState::Playing {
            if let Some(id) = track {
                self.last_playing = Some(id);
            }
        }
        self.report_playback();
        self.maybe_save_session(false);
        // The queue ran out: keep the music going with a station seeded by the last track.
        if prev_state == PlayState::Playing
            && state == PlayState::Idle
            && self.playback.error.is_none()
            && self.config.autoplay
            && self.radio_job.is_none()
        {
            if let Some(seed) = self.last_playing {
                log::info!("queue finished; starting autoplay from {seed}");
                self.start_station(seed, Vec::new(), RadioMode::Autoplay);
            }
        }
    }

    /// Report progress to the server and count plays locally. Called after every status poll.
    fn report_playback(&mut self) {
        let (state, track, duration, position, playing) = {
            let st = &self.playback;
            let state = match st.state {
                PlayState::Playing => ServerPlayState::Playing,
                PlayState::Paused => ServerPlayState::Paused,
                PlayState::Idle => ServerPlayState::Stopped,
            };
            let track = st.track.as_ref().map(|t| t.id);
            let duration = st.duration_ms.max(st.track.as_ref().map_or(0, |t| t.duration_ms));
            (state, track, duration, st.position_ms, st.state == PlayState::Playing)
        };
        let actions = self.report.observe(track, state, position, duration);
        if !actions.is_empty() {
            self.ensure_reporter();
            if let Some(r) = &self.reporter {
                r.apply(&actions);
            }
        }
        // Local play history (Most played mixes and shuffle weights read this).
        if playing && track != self.last_played_id {
            self.last_played_id = track;
            self.mixes_dirty = true;
            if let (Some(id), Some(path)) = (track, self.db_path.clone()) {
                let at = crate::logging::unix_now();
                std::thread::spawn(move || {
                    if let Ok(db) = LibraryDb::open(&path) {
                        if let Err(e) = db.record_play(id, at) {
                            log::debug!("could not record play: {e}");
                        }
                    }
                });
            }
        }
        if track.is_none() {
            self.last_played_id = None;
        }
    }

    /// (Re)create the reporting thread when the server it talks to changes.
    fn ensure_reporter(&mut self) {
        let Some(pms) = self.pms.clone() else { return };
        let base = pms.base().to_string();
        if self.reporter.is_some() && self.report_base.as_deref() == Some(base.as_str()) {
            return;
        }
        self.reporter = Some(Reporter::start(pms));
        self.report_base = Some(base);
    }

    /// Persist queue, slot, position and shuffle/repeat so a restart resumes in place.
    fn maybe_save_session(&mut self, force: bool) {
        if self.engine.is_none() {
            return;
        }
        let st = &self.playback;
        if st.queue.is_empty() {
            return;
        }
        if !force && self.last_session_save.elapsed() < Duration::from_secs(2) {
            return;
        }
        let mut pairs: Vec<(usize, i64)> = st.queue.iter().map(|e| (e.index, e.info.id)).collect();
        pairs.sort_by_key(|p| p.0);
        let sess = Session {
            queue: pairs.into_iter().map(|p| p.1).collect(),
            index: st.index,
            position_ms: st.position_ms,
            shuffle: st.shuffle,
            repeat: st.repeat,
        };
        if !force && sess == self.saved_session {
            return;
        }
        // Don't rewrite the file for a position-only change of a second or two.
        if !force
            && sess.queue == self.saved_session.queue
            && sess.index == self.saved_session.index
            && sess.shuffle == self.saved_session.shuffle
            && sess.repeat == self.saved_session.repeat
            && sess.position_ms.abs_diff(self.saved_session.position_ms) < 2_000
        {
            return;
        }
        if let Err(e) = sess.save() {
            log::debug!("could not save the session: {e}");
        }
        self.saved_session = sess;
        self.last_session_save = std::time::Instant::now();
    }

    /// Restore the queue saved by the previous run, paused at its old position.
    fn maybe_restore_session(&mut self) {
        if self.restored || self.offline || self.pms.is_none() {
            return;
        }
        self.restored = true;
        let Some(sess) = Session::load() else { return };
        if sess.queue.is_empty() || self.library.is_none() {
            return;
        }
        let lib = self.library.as_ref().expect("checked above");
        let mut ids = Vec::with_capacity(sess.queue.len());
        let mut index = 0usize;
        for (i, id) in sess.queue.iter().enumerate() {
            if lib.track(*id).is_some() {
                if i <= sess.index {
                    index = ids.len();
                }
                ids.push(*id);
            }
        }
        if ids.is_empty() {
            return;
        }
        let specs = self.track_specs(&ids);
        if specs.is_empty() {
            return;
        }
        let index = index.min(specs.len() - 1);
        log::info!("restoring session: {} tracks at slot {} ({})", specs.len(), index, sess.position_ms);
        self.config.shuffle = sess.shuffle;
        self.config.repeat = sess.repeat;
        let eng = self.ensure_engine();
        eng.set_shuffle(sess.shuffle);
        eng.set_repeat(sess.repeat);
        eng.restore(specs, index, sess.position_ms);
        self.saved_session = sess;
        self.poll_playback();
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
        let st = self.playback.clone();
        let Some(track) = st.track.clone() else {
            ui.add_space(8.0);
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("Nothing playing").color(theme::TEXT_DIM));
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Pick a track, an album or a playlist — double-click a row to play.")
                        .small()
                        .color(theme::TEXT_DIM),
                );
            });
            return;
        };

        let thumb = self.library.as_ref().and_then(|l| l.track(track.id)).and_then(|t| t.thumb.as_deref().map(str::to_string));
        let tex = thumb.as_deref().and_then(|t| self.art.get(t, 48.0));
        let mut cmds: Vec<BarCmd> = Vec::new();

        ui.horizontal_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
            match tex {
                Some(id) => {
                    ui.painter().image(id, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                }
                None => {
                    ui.painter().rect_filled(rect, 4.0, theme::CARD_HOVER);
                }
            }
            // Reserve a fixed slot for the title block so the transport controls never shift
            // sideways when the track changes.
            ui.add_space(10.0);
            let (info_rect, _) = ui.allocate_exact_size(egui::vec2(220.0, 48.0), egui::Sense::hover());
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(info_rect).layout(egui::Layout::top_down(egui::Align::LEFT)),
                |ui| {
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(RichText::new(&track.title).strong().color(theme::TEXT).size(14.0)).truncate());
                    ui.add(
                        egui::Label::new(RichText::new(format!("{} · {}", track.artist, track.album)).small().color(theme::TEXT_DIM))
                            .truncate(),
                    );
                },
            );

            ui.add_space(12.0);
            if icons::button(ui, Icon::Shuffle, 18.0, st.shuffle)
                .on_hover_text(if st.shuffle { "Shuffle is on" } else { "Shuffle the queue" })
                .clicked()
            {
                cmds.push(BarCmd::Shuffle(!st.shuffle));
            }
            ui.add_space(4.0);
            if icons::button(ui, Icon::Prev, 18.0, false).on_hover_text("Previous").clicked() {
                cmds.push(BarCmd::Prev);
            }
            let playing = st.state == PlayState::Playing;
            if icons::button(ui, if playing { Icon::Pause } else { Icon::Play }, 24.0, false)
                .on_hover_text(if playing { "Pause" } else { "Play" })
                .clicked()
            {
                cmds.push(BarCmd::Toggle);
            }
            if icons::button(ui, Icon::Next, 18.0, false).on_hover_text("Next").clicked() {
                cmds.push(BarCmd::Next);
            }
            ui.add_space(4.0);
            let (repeat_icon, repeat_hint) = match st.repeat {
                RepeatMode::Off => (Icon::Repeat, "Repeat off"),
                RepeatMode::All => (Icon::Repeat, "Repeat all"),
                RepeatMode::One => (Icon::RepeatOne, "Repeat one"),
            };
            if icons::button(ui, repeat_icon, 18.0, st.repeat != RepeatMode::Off)
                .on_hover_text(repeat_hint)
                .clicked()
            {
                cmds.push(BarCmd::Repeat);
            }
            ui.add_space(8.0);
            // Star rating of the current track (Plex ratings are 2 points per star).
            let rating = self.library.as_ref().and_then(|l| l.track(track.id)).map_or(0u8, |t| (t.user_rating.clamp(0.0, 10.0)).round() as u8);
            for star in 1..=5u8 {
                let filled = rating >= star * 2;
                let resp = icons::button(ui, if filled { Icon::StarFilled } else { Icon::Star }, 14.0, filled)
                    .on_hover_text(format!("{star} star{}", if star == 1 { "" } else { "s" }));
                if resp.clicked() {
                    cmds.push(BarCmd::Rate(if rating == star * 2 { 0 } else { star * 2 }));
                }
            }
            ui.add_space(8.0);
            if icons::button(ui, Icon::Queue, 18.0, self.queue_open).on_hover_text("Queue").clicked() {
                cmds.push(BarCmd::Queue);
            }

            ui.add_space(16.0);
            let dur = track.duration_ms.max(st.duration_ms) as f64 / 1000.0;
            let mut pos = (st.position_ms as f64 / 1000.0).clamp(0.0, dur.max(0.1));
            ui.spacing_mut().slider_width = 240.0;
            let resp = ui.add_enabled(dur > 0.5, egui::Slider::new(&mut pos, 0.0..=dur.max(0.1)).show_value(false).trailing_fill(true));
            if resp.changed() {
                cmds.push(BarCmd::Seek((pos * 1000.0) as u64));
            }
            // Fixed width as well: "0:08" and "1:02:03" would otherwise move the volume controls.
            ui.add_sized(
                [78.0, 18.0],
                egui::Label::new(
                    RichText::new(format!(
                        "{} / {}",
                        fmt_duration(st.position_ms),
                        fmt_duration(track.duration_ms.max(st.duration_ms))
                    ))
                    .small()
                    .color(theme::TEXT_DIM),
                )
                .halign(egui::Align::LEFT),
            );

            ui.add_space(12.0);
            if icons::button(ui, if st.muted { Icon::Mute } else { Icon::Volume }, 18.0, false)
                .on_hover_text(if st.muted { "Unmute" } else { "Mute" })
                .clicked()
            {
                cmds.push(BarCmd::Mute(!st.muted));
            }
            let mut vol = st.volume as f64;
            ui.spacing_mut().slider_width = 90.0;
            if ui.add(egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false)).changed() {
                cmds.push(BarCmd::Volume(vol as f32));
            }
        });

        let mut save = false;
        let mut rate = None;
        if let Some(eng) = &self.engine {
            for c in cmds {
                log::info!("transport: {:?}", c);
                match c {
                    BarCmd::Toggle => eng.play_pause(),
                    BarCmd::Next => eng.next(),
                    BarCmd::Prev => eng.prev(),
                    BarCmd::Seek(ms) => eng.seek(ms),
                    BarCmd::Volume(v) => {
                        eng.set_volume(v);
                        self.config.volume = v;
                        if v > 0.0 && self.config.muted {
                            eng.set_muted(false);
                            self.config.muted = false;
                        }
                        save = true;
                    }
                    BarCmd::Mute(m) => {
                        eng.set_muted(m);
                        self.config.muted = m;
                        save = true;
                    }
                    BarCmd::Shuffle(on) => {
                        eng.set_shuffle(on);
                        self.config.shuffle = on;
                        save = true;
                    }
                    BarCmd::Repeat => {
                        let mode = st.repeat.cycle();
                        eng.set_repeat(mode);
                        self.config.repeat = mode;
                        save = true;
                    }
                    BarCmd::Queue => self.queue_open = !self.queue_open,
                    BarCmd::Rate(r) => {
                        // Re-borrow after the loop's immutable borrows; deferred below.
                        rate = Some(r);
                    }
                }
            }
        }
        if let Some(r) = rate {
            self.rate_track(track.id, r);
        }
        if save {
            self.save_config();
        }
        if let Some(err) = &st.error {
            ui.ctx().request_repaint();
            let _ = err;
        }
        if st.state == PlayState::Playing {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// The queue side panel: play order, current track, drag to reorder, remove, clear upcoming.
    fn queue_panel(&mut self, ui: &mut egui::Ui) {
        let st = self.playback.clone();
        let mut cmds: Vec<QueueCmd> = Vec::new();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Queue").strong().size(16.0).color(theme::TEXT));
            ui.label(
                RichText::new(format!("{} {}", st.queue_len, if st.queue_len == 1 { "track" } else { "tracks" }))
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icons::button(ui, Icon::Close, 16.0, false).on_hover_text("Close the queue").clicked() {
                    cmds.push(QueueCmd::Close);
                }
            });
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let has_upcoming = st.queue.iter().any(|e| !e.current) && st.queue.len() > 1;
            if ui
                .add_enabled(
                    has_upcoming,
                    egui::Button::new(RichText::new("Clear upcoming").small())
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(egui::Stroke::new(1.0, theme::CARD_HOVER)),
                )
                .clicked()
            {
                cmds.push(QueueCmd::ClearUpcoming);
            }
        });
        ui.add_space(4.0);
        ui.separator();
        if st.queue.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("The queue is empty. Play something to fill it.").color(theme::TEXT_DIM));
        }
        let mut rows: Vec<(egui::Rect, usize)> = Vec::new();
        let mut drag_started = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (ord, entry) in st.queue.iter().enumerate() {
                let width = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 44.0), egui::Sense::click_and_drag());
                rows.push((rect, ord));
                let dragging_over = self.queue_drag.is_some_and(|from| from != ord)
                    && ui.ctx().pointer_latest_pos().is_some_and(|p| rect.contains(p));
                if entry.current {
                    ui.painter().rect_filled(rect, 6.0, theme::CARD);
                    let bar = egui::Rect::from_min_size(
                        egui::pos2(rect.left() + 2.0, rect.top() + 8.0),
                        egui::vec2(3.0, rect.height() - 16.0),
                    );
                    ui.painter().rect_filled(bar, 1.5, theme::ACCENT);
                } else if dragging_over {
                    ui.painter().rect_filled(rect, 6.0, theme::CARD_HOVER);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 6.0, theme::CARD);
                }
                if resp.drag_started() {
                    drag_started = Some(ord);
                }
                let text_left = rect.left() + 12.0;
                let clip = egui::Rect::from_min_max(
                    egui::pos2(text_left, rect.top()),
                    egui::pos2(rect.right() - 34.0, rect.bottom()),
                );
                let p = ui.painter().with_clip_rect(clip);
                p.text(
                    egui::pos2(text_left, rect.top() + 7.0),
                    egui::Align2::LEFT_TOP,
                    &entry.info.title,
                    egui::FontId::proportional(13.5),
                    if entry.current { theme::ACCENT } else { theme::TEXT },
                );
                p.text(
                    egui::pos2(text_left, rect.top() + 25.0),
                    egui::Align2::LEFT_TOP,
                    &entry.info.artist,
                    egui::FontId::proportional(11.0),
                    theme::TEXT_DIM,
                );
                if !entry.current && self.queue_drag.is_none() {
                    let del_rect = egui::Rect::from_center_size(
                        egui::pos2(rect.right() - 16.0, rect.center().y),
                        egui::vec2(24.0, 24.0),
                    );
                    let del = ui.interact(del_rect, ui.scope_id().with(("qdel", ord)), egui::Sense::click());
                    icons::paint(ui.painter(), del_rect.shrink(7.0), Icon::Close, if del.hovered() { theme::TEXT } else { theme::TEXT_DIM });
                    if del.clicked() {
                        cmds.push(QueueCmd::Remove(ord));
                    }
                }
            }
        });
        if let Some(from) = drag_started {
            self.queue_drag = Some(from);
        }
        // Finish a drag: find the row under the pointer when the button was released.
        if self.queue_drag.is_some() && ui.ctx().input(|i| i.pointer.any_released()) {
            if let (Some(from), Some(pos)) = (self.queue_drag, ui.ctx().pointer_latest_pos()) {
                if let Some((_, to)) = rows.iter().find(|(r, _)| r.contains(pos)) {
                    if *to != from {
                        cmds.push(QueueCmd::Move(from, *to));
                    }
                }
            }
            self.queue_drag = None;
        } else if let (Some(_), Some(pos)) = (self.queue_drag, ui.ctx().pointer_latest_pos()) {
            // Show where the row would land while it is being dragged.
            let target = rows.iter().find(|(r, _)| r.contains(pos)).map(|(r, _)| *r);
            if let Some(rect) = target {
                let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("queue_drag_marker")));
                p.line_segment(
                    [egui::pos2(rect.left(), rect.top()), egui::pos2(rect.right(), rect.top())],
                    egui::Stroke::new(2.0, theme::ACCENT),
                );
            }
        }
        let Some(eng) = &self.engine else { return };
        for c in cmds {
            match c {
                QueueCmd::Remove(o) => {
                    log::info!("queue: remove {o}");
                    eng.remove_at(o);
                }
                QueueCmd::Move(from, to) => {
                    log::info!("queue: move {from} to {to}");
                    eng.move_in_order(from, to);
                }
                QueueCmd::ClearUpcoming => {
                    log::info!("queue: clear upcoming");
                    eng.clear_upcoming();
                }
                QueueCmd::Close => self.queue_open = false,
            }
        }
        self.poll_playback();
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
        let now_playing = self.playback.track.as_ref().map(|t| t.id);
        {
            let mut vc = ViewCtx { lib, art: &mut self.art, actions: &mut actions, now_playing, mixes: &self.mixes };
            use super::views::{detail, library, search};
            match &route {
                Route::Home => detail::home_view(ui, &mut vc, &greeting),
                Route::Library(tab) => library::library_view(ui, &mut vc, &mut self.browse, *tab, self.library_ver),
                Route::Playlists => library::playlists_view(ui, &mut vc),
                Route::Album(id) => detail::album_view(ui, &mut vc, *id),
                Route::Artist(id) => detail::artist_view(ui, &mut vc, *id),
                Route::Playlist(id) => detail::playlist_view(ui, &mut vc, *id),
                Route::Genre(g) => detail::genre_view(ui, &mut vc, g),
                Route::Search => search::search_view(ui, &mut vc, &mut self.search, self.library_ver),
                Route::Settings => {}
            }
        }
        for a in actions {
            self.apply_action(a);
        }
    }

    fn settings_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.add_space(10.0);
        let mut changed = false;
        egui::ScrollArea::vertical().id_salt("settings").auto_shrink([false, false]).show(ui, |ui| {
            theme::card().show(ui, |ui| {
                ui.label(RichText::new("Playback").size(17.0).strong());
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Stream quality");
                    let mut q = self.config.quality;
                    ui.selectable_value(&mut q, crate::config::StreamQuality::Original, "Original");
                    for kbps in [320u32, 256, 192, 128] {
                        ui.selectable_value(&mut q, crate::config::StreamQuality::Kbps(kbps), format!("{kbps} kbps"));
                    }
                    if q != self.config.quality {
                        self.config.quality = q;
                        changed = true;
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Crossfade");
                    let mut xf = self.config.crossfade_secs;
                    if ui.add(egui::Slider::new(&mut xf, 0.0..=12.0).suffix(" s").step_by(0.5)).changed() {
                        self.config.crossfade_secs = xf;
                        if let Some(e) = &self.engine {
                            e.set_crossfade((xf * 1000.0).round() as u32);
                        }
                        changed = true;
                    }
                    if xf <= 0.0 {
                        ui.label(RichText::new("gapless").small().color(theme::TEXT_DIM));
                    }
                });
                ui.add_space(4.0);
                let mut norm = self.config.normalization;
                if ui.checkbox(&mut norm, "ReplayGain volume normalization").changed() {
                    self.config.normalization = norm;
                    if let Some(e) = &self.engine {
                        e.set_normalization(norm);
                    }
                    changed = true;
                }
                let mut autoplay = self.config.autoplay;
                if ui.checkbox(&mut autoplay, "Autoplay similar tracks when the queue ends").changed() {
                    self.config.autoplay = autoplay;
                    changed = true;
                }
            });

            ui.add_space(10.0);
            theme::card().show(ui, |ui| {
                ui.label(RichText::new("Window").size(17.0).strong());
                ui.add_space(6.0);
                if theme::secondary_button(ui, if self.mini { "Leave mini-player" } else { "Mini-player" })
                    .on_hover_text("A small always-on-top window (Ctrl+M)")
                    .clicked()
                {
                    self.toggle_mini();
                }
                ui.add_space(4.0);
                let tray = self.tray.is_some();
                let mut min_tray = self.config.minimize_to_tray;
                if ui.add_enabled(tray, egui::Checkbox::new(&mut min_tray, "Minimize to the tray")).changed() {
                    self.config.minimize_to_tray = min_tray;
                    changed = true;
                }
                let mut close_tray = self.config.close_to_tray;
                if ui.add_enabled(tray, egui::Checkbox::new(&mut close_tray, "Close to the tray")).changed() {
                    self.config.close_to_tray = close_tray;
                    changed = true;
                }
                if !tray {
                    ui.label(RichText::new("The tray icon is only available on Windows.").small().color(theme::TEXT_DIM));
                }
                let mut autostart = self.config.start_with_windows;
                let autostart_ok = cfg!(windows);
                if ui
                    .add_enabled(autostart_ok, egui::Checkbox::new(&mut autostart, "Start Plexify with Windows"))
                    .changed()
                {
                    if crate::win::autostart::set_enabled(autostart) {
                        self.config.start_with_windows = autostart;
                        changed = true;
                    } else {
                        self.banner = Some(("Could not update the Windows startup entry.".into(), true));
                    }
                }
            });

            ui.add_space(10.0);
            theme::card().show(ui, |ui| {
                ui.label(RichText::new("Storage").size(17.0).strong());
                ui.add_space(6.0);
                ui.label(format!("Library database: {}", human_bytes(crate::paths::dir_size(&crate::paths::db_dir()))));
                let (status, _) = self.sync_status_text();
                ui.label(RichText::new(status).small().color(theme::TEXT_DIM));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!self.offline && !self.sync.running, |ui| {
                        if theme::secondary_button(ui, "Refresh library").on_hover_text("Re-download the whole library").clicked() {
                            self.start_sync(true);
                        }
                    });
                    ui.add_enabled_ui(!self.sync.running, |ui| {
                        if theme::secondary_button(ui, "Clear library cache").clicked() {
                            self.clear_library_cache();
                        }
                    });
                });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label("Artwork cache");
                    ui.label(
                        RichText::new(format!("{} used", human_bytes(crate::paths::dir_size(&crate::paths::image_cache_dir()))))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                });
                let mut img_mb = self.config.image_cache_mb as f64;
                if ui
                    .add(egui::Slider::new(&mut img_mb, 50.0..=2000.0).suffix(" MB").step_by(50.0))
                    .changed()
                {
                    self.config.image_cache_mb = img_mb as u64;
                    self.art.svc.set_cap_mb(img_mb as u64);
                    changed = true;
                }
                if theme::secondary_button(ui, "Clear artwork cache").clicked() {
                    self.art.svc.clear_disk();
                    self.art.reset();
                }

                ui.add_space(10.0);
                let mut audio_on = self.config.audio_cache_enabled;
                if ui.checkbox(&mut audio_on, "Cache streamed audio").changed() {
                    self.config.audio_cache_enabled = audio_on;
                    changed = true;
                }
                ui.horizontal(|ui| {
                    ui.label("Audio cache");
                    ui.label(
                        RichText::new(format!("{} used", human_bytes(crate::paths::dir_size(&crate::paths::audio_cache_dir()))))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                });
                let mut audio_mb = self.config.audio_cache_mb as f64;
                if ui
                    .add(egui::Slider::new(&mut audio_mb, 100.0..=5000.0).suffix(" MB").step_by(100.0))
                    .changed()
                {
                    self.config.audio_cache_mb = audio_mb as u64;
                    changed = true;
                }
                if self.engine.is_some() {
                    ui.label(RichText::new("Audio cache changes apply after a restart.").small().color(theme::TEXT_DIM));
                }
                if theme::secondary_button(ui, "Clear audio cache").clicked() {
                    match remove_dir_all(crate::paths::audio_cache_dir()) {
                        Ok(()) => self.banner = Some(("Audio cache cleared.".into(), false)),
                        Err(e) => self.banner = Some((format!("Could not clear the audio cache: {e}"), true)),
                    }
                }
            });

            ui.add_space(10.0);
            theme::card().show(ui, |ui| {
                ui.label(RichText::new("Account").size(17.0).strong());
                ui.add_space(6.0);
                if let Some(u) = &self.user {
                    ui.label(format!("Signed in as {}", u.display_name()));
                }
                ui.label(RichText::new(format!("Token stored in: {}", self.store.describe())).small().color(theme::TEXT_DIM));
                ui.add_space(8.0);
                if theme::secondary_button(ui, "Sign out").clicked() {
                    self.sign_out();
                }
            });
        });
        if changed {
            self.save_config();
        }
    }
}

fn remove_dir_all(dir: std::path::PathBuf) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
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
            self.smtc = crate::win::smtc::init(hwnd, self.smtc_events.clone());
        }
        crate::win::fix_wine_event_target();
        self.tray = crate::win::tray::start();
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        if !self.mini {
            if let Some(rect) = ui.ctx().input(|i| i.viewport().inner_rect) {
                // Ignore transient sizes while the mini-player geometry is being applied.
                if rect.width() >= 760.0 && rect.height() >= 480.0 {
                    self.last_window_size = Some([rect.width(), rect.height()]);
                }
            }
        }
        for msg in self.jobs.drain() {
            self.handle(msg);
        }
        self.poll_playback();
        self.poll_radio();
        if self.radio_job.is_some() {
            // No playback is running while the lookup is in flight, so keep polling for the
            // worker's answer even though the app is otherwise idle.
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        if let Some(deadline) = self.wake_poll {
            if self.playback.state == PlayState::Idle && std::time::Instant::now() < deadline {
                // An autoplay station was just queued; wait for the engine's first Playing event.
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            } else {
                self.wake_poll = None;
            }
        }
        self.art.begin_frame(ui.ctx());
        if ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) && self.screen == Screen::Main {
            self.navigate(Route::Search);
            self.search.focus = true;
        }
        if self.screen == Screen::Main && !ui.ctx().egui_wants_keyboard_input() {
            self.shortcuts(ui);
        }
        if ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Extra1)) {
            self.back();
        }
        match self.screen.clone() {
            Screen::Busy(text) => super::views::login::busy_screen(ui, &text),
            Screen::Login => self.login_screen(ui),
            Screen::ServerSelect => self.server_screen(ui),
            Screen::Main if self.mini => self.mini_player(ui),
            Screen::Main => {
                if self.mixes_dirty && self.route == Route::Home {
                    self.refresh_mixes();
                }
                egui::Panel::bottom("now_playing")
                    .exact_size(84.0)
                    .frame(egui::Frame::new().fill(theme::BAR).inner_margin(egui::Margin::symmetric(12, 6)))
                    .show(ui, |ui| self.now_playing_bar(ui));
                egui::Panel::left("sidebar")
                    .exact_size(220.0)
                    .resizable(false)
                    .frame(egui::Frame::new().fill(theme::SIDEBAR).inner_margin(egui::Margin::symmetric(10, 6)))
                    .show(ui, |ui| self.sidebar(ui));
                if self.queue_open {
                    egui::Panel::right("queue_panel")
                        .exact_size(320.0)
                        .resizable(false)
                        .frame(egui::Frame::new().fill(theme::BAR).inner_margin(egui::Margin::symmetric(10, 6)))
                        .show(ui, |ui| self.queue_panel(ui));
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(24, 8)))
                    .show(ui, |ui| self.content(ui));
                self.maybe_search_server(ui.ctx().input(|i| i.time));
            }
        }
    }

    fn on_exit(&mut self) {
        self.cancel_login();
        self.poll_playback();
        self.maybe_save_session(true);
        self.save_window_size();
        self.tray = None; // removes the icon and stops the tray thread
        self.smtc = None; // releases the media overlay session
    }

    /// Keep the window title in step with playback, handle tray/media-key events, and tell the
    /// runner about mini-player geometry changes.
    fn take_window_cmds(&mut self) -> Vec<WindowCmd> {
        let mut cmds = Vec::new();
        let mut events = Vec::new();
        if let Some(tray) = &self.tray {
            while let Some(e) = tray.try_recv() {
                events.push(e);
            }
        }
        for b in self.smtc_events.drain() {
            match b {
                SmtcButton::PlayPause => self.toggle_playback(),
                SmtcButton::Next => self.skip_track(1),
                SmtcButton::Prev => self.skip_track(-1),
            }
        }
        for e in events {
            match e {
                TrayEvent::Show => cmds.push(WindowCmd::Show),
                TrayEvent::PlayPause => self.toggle_playback(),
                TrayEvent::Next => self.skip_track(1),
                TrayEvent::Prev => self.skip_track(-1),
                TrayEvent::Quit => {
                    log::info!("quit requested from the tray");
                    self.quitting = true;
                }
            }
        }
        if std::mem::take(&mut self.hide_window) {
            self.save_window_size();
            cmds.push(WindowCmd::Hide);
        }
        if std::mem::take(&mut self.mini_geometry) {
            if self.mini {
                cmds.push(WindowCmd::Resizable(false));
                cmds.push(WindowCmd::AlwaysOnTop(true));
                cmds.push(WindowCmd::Size(420, 150));
            } else {
                let [w, h] = self.config.window_size.unwrap_or([1180.0, 760.0]);
                cmds.push(WindowCmd::AlwaysOnTop(false));
                cmds.push(WindowCmd::Resizable(true));
                cmds.push(WindowCmd::Size(w as u32, h as u32));
            }
        }
        if let Some(smtc) = &mut self.smtc {
            let playing = self.playback.state == PlayState::Playing;
            match &self.playback.track {
                Some(t) => smtc.update(&NowPlaying {
                    title: &t.title,
                    artist: &t.artist,
                    album: &t.album,
                    position_ms: self.playback.position_ms,
                    duration_ms: self.playback.duration_ms,
                    playing,
                }),
                None => smtc.update(&NowPlaying {
                    title: "Plexify",
                    artist: "",
                    album: "",
                    position_ms: 0,
                    duration_ms: 0,
                    playing: false,
                }),
            }
        }
        let title = match (&self.playback.track, self.playback.state) {
            (Some(t), PlayState::Playing | PlayState::Paused) => format!("{} — {} · Plexify", t.title, t.artist),
            _ => "Plexify".to_string(),
        };
        if title != self.window_title {
            if let Some(tray) = &self.tray {
                tray.set_tip(&title);
            }
            self.window_title = title.clone();
            cmds.push(WindowCmd::Title(title));
        }
        cmds
    }

    fn minimizes_to_tray(&self) -> bool {
        self.config.minimize_to_tray && self.tray.is_some()
    }

    fn on_close_requested(&mut self, _ctx: &egui::Context) -> bool {
        if self.config.close_to_tray && self.tray.is_some() && !self.quitting {
            log::info!("closing to the tray");
            self.hide_window = true;
            return false;
        }
        true
    }

    fn wants_exit(&self) -> bool {
        self.quitting
    }
}
