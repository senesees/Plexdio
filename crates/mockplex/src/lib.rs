//! Mock plex.tv + Plex Media Server.
//!
//! Serves the JSON flavour of the endpoints Plexdio uses, backed by an in-memory library
//! (see [`library`]) and embedded audio fixtures, so the app and its tests can run without a
//! Plex account. One HTTP listener plays both roles: point `--plex-tv-url` at it and the
//! advertised server connection points back at it too.

pub mod library;

use library::{Album, Artist, Library, Track, AUDIO};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

pub const USER_TOKEN: &str = "mock-user-token";
pub const SERVER_TOKEN: &str = "mock-server-token";
pub const SERVER_ID: &str = "mock-server-0001";
pub const MUSIC_SECTION: &str = "1";
pub const EMPTY_SECTION: &str = "2";

#[derive(Debug, Clone)]
pub struct MockConfig {
    /// 0 = pick a free port.
    pub port: u16,
    /// Bind address (127.0.0.1 by default; 0.0.0.0 to reach it from a VM).
    pub bind: String,
    /// Host name advertised in connection URIs.
    pub advertise_host: String,
    /// PIN polls before the PIN is auto-approved (`None` = only via the /auth page).
    pub auto_approve_after: Option<u32>,
    /// Generate a large synthetic library with this many tracks instead of the fixture library.
    pub synthetic_tracks: Option<usize>,
    /// Serve `/nearest` (sonic similarity, a Plex Pass feature).
    pub sonic: bool,
    /// Print each request to stdout.
    pub verbose: bool,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            port: 0,
            bind: "127.0.0.1".into(),
            advertise_host: "127.0.0.1".into(),
            auto_approve_after: Some(2),
            synthetic_tracks: None,
            sonic: true,
            verbose: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoggedRequest {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
}

struct PinState {
    code: String,
    polls: u32,
    approved: bool,
}

pub struct State {
    pub config: MockConfig,
    pub library: Library,
    pins: HashMap<i64, PinState>,
    next_pin: i64,
    pub requests: Vec<LoggedRequest>,
    /// Token accepted for plex.tv calls; rotate to simulate expiry.
    pub valid_user_token: String,
    /// Simulate the server being down (all PMS endpoints fail with 503).
    pub server_down: bool,
    pub base_url: String,
}

pub struct MockServer {
    pub url: String,
    pub port: u16,
    pub state: Arc<Mutex<State>>,
    server: Arc<Server>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    pub fn start(config: MockConfig) -> std::io::Result<MockServer> {
        let server = Server::http(format!("{}:{}", config.bind, config.port))
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(config.port);
        let url = format!("http://{}:{port}", config.advertise_host);
        let library = match config.synthetic_tracks {
            Some(n) => Library::synthetic(n),
            None => Library::fixture(),
        };
        let state = Arc::new(Mutex::new(State {
            config,
            library,
            pins: HashMap::new(),
            next_pin: 1000,
            requests: Vec::new(),
            valid_user_token: USER_TOKEN.into(),
            server_down: false,
            base_url: url.clone(),
        }));
        let server = Arc::new(server);
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let server = server.clone();
            let state = state.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match server.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(Some(req)) => {
                            let state = state.clone();
                            std::thread::spawn(move || handle(req, &state));
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            })
        };
        Ok(MockServer { url, port, state, server, stop, handle: Some(handle) })
    }

    pub fn requests(&self) -> Vec<LoggedRequest> {
        self.state.lock().unwrap().requests.clone()
    }

    pub fn requests_to(&self, path: &str) -> Vec<LoggedRequest> {
        self.requests().into_iter().filter(|r| r.path == path).collect()
    }

    pub fn with_library<R>(&self, f: impl FnOnce(&mut Library) -> R) -> R {
        let mut state = self.state.lock().unwrap();
        let out = f(&mut state.library);
        state.library.reindex();
        out
    }

    pub fn set_server_down(&self, down: bool) {
        self.state.lock().unwrap().server_down = down;
    }

    /// Invalidate the user token (simulates an expired/revoked sign-in).
    pub fn expire_user_token(&self) {
        self.state.lock().unwrap().valid_user_token = "rotated".into();
    }

    /// Approve every pending PIN (what the user does in the browser).
    pub fn approve_all_pins(&self) {
        for p in self.state.lock().unwrap().pins.values_mut() {
            p.approved = true;
        }
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.server.unblock();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------------------------

type Resp = Response<Box<dyn Read + Send>>;

fn json_resp(status: u16, v: &Value) -> Resp {
    let body = serde_json::to_vec(v).unwrap_or_default();
    let len = body.len();
    Response::new(
        StatusCode(status),
        vec![Header::from_bytes("Content-Type", "application/json").unwrap()],
        Box::new(Cursor::new(body)) as Box<dyn Read + Send>,
        Some(len),
        None,
    )
    .with_chunked_threshold(usize::MAX)
}

fn status(code: u16) -> Resp {
    json_resp(code, &json!({ "error": code }))
}

fn bytes_resp(status: u16, ctype: &str, body: Vec<u8>, extra: Vec<Header>) -> Resp {
    let len = body.len();
    let mut headers = vec![Header::from_bytes("Content-Type", ctype).unwrap()];
    headers.extend(extra);
    Response::new(StatusCode(status), headers, Box::new(Cursor::new(body)) as Box<dyn Read + Send>, Some(len), None)
        .with_chunked_threshold(usize::MAX)
}

fn parse_query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            let dec = |s: &str| urlencoding::decode(&s.replace('+', " ")).map(|c| c.into_owned()).unwrap_or_default();
            (dec(k), dec(v))
        })
        .collect()
}

fn header(req: &Request, name: &str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

fn handle(req: Request, state: &Arc<Mutex<State>>) {
    let url = req.url().to_string();
    let (path, q) = url.split_once('?').unwrap_or((&url, ""));
    let path = urlencoding::decode(path).map(|p| p.into_owned()).unwrap_or_else(|_| path.to_string());
    let query = parse_query(q);
    let method = req.method().clone();
    let token = header(&req, "X-Plex-Token").or_else(|| query.get("X-Plex-Token").cloned());
    let range = header(&req, "Range");
    {
        let mut st = state.lock().unwrap();
        if st.config.verbose {
            println!("{method} {path} {q}");
        }
        st.requests.push(LoggedRequest { method: method.to_string(), path: path.clone(), query: query.clone() });
    }
    let resp = route(state, &method, &path, &query, token.as_deref(), range.as_deref());
    let _ = req.respond(resp);
}

fn route(state: &Arc<Mutex<State>>, method: &Method, path: &str, q: &HashMap<String, String>, token: Option<&str>, range: Option<&str>) -> Resp {
    let mut st = state.lock().unwrap();
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();

    // ---------------- plex.tv ----------------
    match (method, segs.as_slice()) {
        (Method::Post, ["api", "v2", "pins"]) => {
            let id = st.next_pin;
            st.next_pin += 1;
            let code = format!("mockcode{id}");
            st.pins.insert(id, PinState { code: code.clone(), polls: 0, approved: false });
            return json_resp(201, &json!({ "id": id, "code": code, "product": "Plexdio", "trusted": false,
                "expiresIn": 1800, "authToken": null, "newRegistration": null }));
        }
        (Method::Get, ["api", "v2", "pins", id]) => {
            let auto = st.config.auto_approve_after;
            let user_token = st.valid_user_token.clone();
            let Some(pin) = id.parse::<i64>().ok().and_then(|id| st.pins.get_mut(&id)) else {
                return status(404);
            };
            pin.polls += 1;
            if auto.is_some_and(|n| pin.polls > n) {
                pin.approved = true;
            }
            let tok = if pin.approved { json!(user_token) } else { Value::Null };
            return json_resp(200, &json!({ "id": id.parse::<i64>().unwrap_or(0), "code": pin.code, "authToken": tok, "expiresIn": 1700 }));
        }
        (Method::Get, ["auth"]) => {
            let html = "<html><body style='font-family:sans-serif;background:#222;color:#eee'>\
                <h2>Mock Plex sign-in</h2><p>The mock auto-approves PINs after a couple of polls.</p>\
                <p><a style='color:#e5a00d' href='/auth/approve'>Approve all pending PINs now</a></p></body></html>";
            return bytes_resp(200, "text/html", html.as_bytes().to_vec(), vec![]);
        }
        (Method::Get, ["auth", "approve"]) => {
            for p in st.pins.values_mut() {
                p.approved = true;
            }
            return bytes_resp(200, "text/html", b"<html><body>Approved. You can close this tab.</body></html>".to_vec(), vec![]);
        }
        (Method::Get, ["api", "v2", "user"]) => {
            if token != Some(st.valid_user_token.as_str()) {
                return status(401);
            }
            return json_resp(200, &json!({ "id": 42, "uuid": "mockuser", "username": "mockuser", "title": "Mock User",
                "email": "mock@example.invalid", "thumb": "", "authToken": st.valid_user_token }));
        }
        (Method::Get, ["api", "v2", "resources"]) => {
            if token != Some(st.valid_user_token.as_str()) {
                return status(401);
            }
            let base = st.base_url.clone();
            let port = base.rsplit(':').next().unwrap_or("32400").to_string();
            return json_resp(200, &json!([
                { "name": "Mock Music Server", "product": "Plex Media Server", "productVersion": "1.40.0",
                  "platform": "Linux", "clientIdentifier": SERVER_ID, "provides": "server", "owned": true,
                  "accessToken": SERVER_TOKEN, "presence": true, "httpsRequired": false,
                  "connections": [
                    // Dead local connection first: exercises connection racing.
                    { "protocol": "http", "address": "127.0.0.1", "port": 9, "uri": "http://127.0.0.1:9", "local": true, "relay": false, "IPv6": false },
                    { "protocol": "http", "address": "127.0.0.1", "port": port, "uri": base, "local": true, "relay": false, "IPv6": false },
                    { "protocol": "https", "address": "203.0.113.10", "port": 32400, "uri": "https://203-0-113-10.mock.plex.direct:32400", "local": false, "relay": false, "IPv6": false }
                  ] },
                { "name": "Friend's Server", "product": "Plex Media Server", "productVersion": "1.40.0",
                  "platform": "Windows", "clientIdentifier": "mock-shared-0002", "provides": "server", "owned": false,
                  "sourceTitle": "friend", "accessToken": "shared-token", "presence": false,
                  "connections": [
                    { "protocol": "https", "address": "198.51.100.7", "port": 32400, "uri": "https://198-51-100-7.mock.plex.direct:32400", "local": false, "relay": false, "IPv6": false }
                  ] },
                { "name": "Living Room TV", "product": "Plex for Android (TV)", "clientIdentifier": "mock-player-3",
                  "provides": "player,pubsub-player", "owned": true, "connections": [] }
            ]));
        }
        _ => {}
    }

    // ---------------- media server ----------------
    if st.server_down {
        return status(503);
    }
    if token != Some(SERVER_TOKEN) && token != Some(st.valid_user_token.as_str()) {
        return status(401);
    }
    let base = st.base_url.clone();
    let lib = &st.library;
    match (method, segs.as_slice()) {
        (Method::Get, ["identity"]) => json_resp(200, &json!({ "MediaContainer": {
            "size": 0, "machineIdentifier": SERVER_ID, "version": "1.40.0" } })),
        (Method::Get, ["library", "sections"]) => json_resp(200, &json!({ "MediaContainer": { "size": 3, "Directory": [
            { "key": MUSIC_SECTION, "type": "artist", "title": "Music", "agent": "tv.plex.agents.music", "uuid": "sec-music", "updatedAt": lib.clock },
            { "key": EMPTY_SECTION, "type": "artist", "title": "Empty Music", "agent": "tv.plex.agents.music", "uuid": "sec-empty", "updatedAt": library::BASE_TIME },
            { "key": "3", "type": "movie", "title": "Movies", "agent": "tv.plex.agents.movie", "uuid": "sec-movies" }
        ] } })),
        (Method::Get, ["library", "sections", sec, "all"]) => {
            let start: usize = q.get("X-Plex-Container-Start").and_then(|s| s.parse().ok()).unwrap_or(0);
            let size: usize = q.get("X-Plex-Container-Size").and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
            let (page, total) = section_items(lib, sec, q, start, size);
            json_resp(200, &json!({ "MediaContainer": { "size": page.len(), "totalSize": total, "offset": start, "Metadata": page } }))
        }
        // Test hook: add a track to the (first) album without touching anything else, so a
        // client with an already-synced cache sees it only through search.
        (Method::Post, ["__mock", "add-track"]) => {
            let title = q.get("title").cloned().unwrap_or_else(|| "Brand New Track".into());
            let Some(album) = st.library.albums.first().cloned() else { return status(404) };
            let id = st.library.tracks.iter().map(|t| t.id).max().unwrap_or(0) + 1;
            let index = st.library.tracks.iter().filter(|t| t.album_id == album.id).count() as i64 + 1;
            let now = st.library.tick();
            st.library.tracks.push(library::Track {
                id,
                album_id: album.id,
                title,
                index,
                audio: 0,
                view_count: 0,
                last_viewed_at: None,
                user_rating: None,
                added_at: now,
                updated_at: now,
            });
            st.library.reindex();
            json_resp(200, &json!({ "id": id }))
        }
        (Method::Get, ["library", "sections", sec, "genre"]) => {
            let mut genres: Vec<String> = if *sec == MUSIC_SECTION { lib.albums.iter().map(|a| a.genre.clone()).collect() } else { vec![] };
            genres.sort();
            genres.dedup();
            let dirs: Vec<Value> = genres.iter().enumerate().map(|(i, g)| json!({ "key": (300 + i).to_string(), "title": g })).collect();
            json_resp(200, &json!({ "MediaContainer": { "size": dirs.len(), "Directory": dirs } }))
        }
        (Method::Get, ["library", "metadata", id]) => {
            let id: u64 = id.parse().unwrap_or(0);
            match item_json(lib, id, true) {
                Some(v) => json_resp(200, &json!({ "MediaContainer": { "size": 1, "Metadata": [v] } })),
                None => status(404),
            }
        }
        (Method::Get, ["library", "metadata", id, "children"]) => {
            let id: u64 = id.parse().unwrap_or(0);
            let items: Vec<Value> = if lib.artist(id).is_some() {
                lib.albums.iter().filter(|a| a.artist_id == id).map(|a| album_json(lib, a)).collect()
            } else if lib.album(id).is_some() {
                let mut t: Vec<&Track> = lib.tracks.iter().filter(|t| t.album_id == id).collect();
                t.sort_by_key(|t| t.index);
                t.into_iter().map(|t| track_json(lib, t, false)).collect()
            } else {
                return status(404);
            };
            json_resp(200, &json!({ "MediaContainer": { "size": items.len(), "Metadata": items } }))
        }
        (Method::Get, ["library", "metadata", id, "nearest"]) => {
            if !st.config.sonic {
                return status(404);
            }
            let id: u64 = id.parse().unwrap_or(0);
            let limit: usize = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(20);
            let Some(seed) = lib.track(id) else { return status(404) };
            // "Similar" = same audio fixture or same genre, excluding the seed; deterministic.
            let seed_genre = lib.album(seed.album_id).map(|a| a.genre.clone()).unwrap_or_default();
            let items: Vec<Value> = lib
                .tracks
                .iter()
                .filter(|t| t.id != id)
                .filter(|t| t.audio == seed.audio || lib.album(t.album_id).is_some_and(|a| a.genre == seed_genre))
                .take(limit)
                .enumerate()
                .map(|(i, t)| {
                    let mut v = track_json(lib, t, false);
                    v["distance"] = json!(0.05 + i as f64 * 0.01);
                    v
                })
                .collect();
            json_resp(200, &json!({ "MediaContainer": { "size": items.len(), "Metadata": items } }))
        }
        (Method::Get, ["library", "metadata", id, "related"]) if lib.artist(id.parse().unwrap_or(0)).is_some() => {
            // Artist pages: Plex's "Similar Artists" hub. Mock similarity = same genre.
            let id: u64 = id.parse().unwrap_or(0);
            let genre = lib.artist(id).map(|a| a.genre.clone()).unwrap_or_default();
            let similar: Vec<Value> =
                lib.artists.iter().filter(|a| a.id != id && a.genre == genre).take(20).map(|a| artist_json(lib, a)).collect();
            json_resp(200, &json!({ "MediaContainer": { "size": 1, "Hub": [
                { "hubIdentifier": "artist.similar", "title": "Similar Artists", "type": "artist", "size": similar.len(), "Metadata": similar }
            ] } }))
        }
        (Method::Get, ["library", "metadata", id, "related"]) => {
            let id: u64 = id.parse().unwrap_or(0);
            let artist_id = lib.track(id).and_then(|t| lib.album(t.album_id)).map(|a| a.artist_id).or_else(|| lib.album(id).map(|a| a.artist_id));
            let albums: Vec<Value> = lib.albums.iter().filter(|a| Some(a.artist_id) == artist_id && a.id != id).map(|a| album_json(lib, a)).collect();
            json_resp(200, &json!({ "MediaContainer": { "size": 1, "Hub": [
                { "hubIdentifier": "artist.albums", "title": "More by this artist", "type": "album", "size": albums.len(), "Metadata": albums }
            ] } }))
        }
        (Method::Get, ["playlists"]) => {
            let items: Vec<Value> = lib.playlists.iter().map(|p| playlist_json(lib, p)).collect();
            json_resp(200, &json!({ "MediaContainer": { "size": items.len(), "Metadata": items } }))
        }
        (Method::Get, ["playlists", id, "items"]) => {
            let id: u64 = id.parse().unwrap_or(0);
            let Some(p) = lib.playlists.iter().find(|p| p.id == id) else { return status(404) };
            let start: usize = q.get("X-Plex-Container-Start").and_then(|s| s.parse().ok()).unwrap_or(0);
            let size: usize = q.get("X-Plex-Container-Size").and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
            let items: Vec<Value> = p
                .items
                .iter()
                .enumerate()
                .skip(start)
                .take(size)
                .filter_map(|(i, tid)| {
                    lib.track(*tid).map(|t| {
                        let mut v = track_json(lib, t, false);
                        v["playlistItemID"] = json!(90_000 + i);
                        v
                    })
                })
                .collect();
            json_resp(200, &json!({ "MediaContainer": { "size": items.len(), "totalSize": p.items.len(), "Metadata": items } }))
        }
        (Method::Get, ["hubs", "search"]) => {
            let query = q.get("query").map(|s| s.to_lowercase()).unwrap_or_default();
            let limit: usize = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(10);
            let m = |s: &str| !query.is_empty() && s.to_lowercase().contains(&query);
            let artists: Vec<Value> = lib.artists.iter().filter(|a| m(&a.title)).take(limit).map(|a| artist_json(lib, a)).collect();
            let albums: Vec<Value> = lib.albums.iter().filter(|a| m(&a.title)).take(limit).map(|a| album_json(lib, a)).collect();
            let tracks: Vec<Value> = lib.tracks.iter().filter(|t| m(&t.title)).take(limit).map(|t| track_json(lib, t, false)).collect();
            json_resp(200, &json!({ "MediaContainer": { "size": 3, "Hub": [
                { "hubIdentifier": "artist", "title": "Artists", "type": "artist", "size": artists.len(), "Metadata": artists },
                { "hubIdentifier": "album", "title": "Albums", "type": "album", "size": albums.len(), "Metadata": albums },
                { "hubIdentifier": "track", "title": "Tracks", "type": "track", "size": tracks.len(), "Metadata": tracks }
            ] } }))
        }
        (Method::Get, [":", "timeline"]) => json_resp(200, &json!({ "MediaContainer": { "size": 0 } })),
        (Method::Get, [":", "scrobble"]) => {
            let id: u64 = q.get("key").and_then(|s| s.parse().ok()).unwrap_or(0);
            let now = st.library.tick();
            match st.library.track_mut(id) {
                Some(t) => {
                    t.view_count += 1;
                    t.last_viewed_at = Some(now);
                    t.updated_at = now;
                    json_resp(200, &json!({}))
                }
                None => status(404),
            }
        }
        (Method::Put, [":", "rate"]) => {
            let id: u64 = q.get("key").and_then(|s| s.parse().ok()).unwrap_or(0);
            let rating: f64 = q.get("rating").and_then(|s| s.parse().ok()).unwrap_or(-1.0);
            let now = st.library.tick();
            match st.library.track_mut(id) {
                Some(t) => {
                    t.user_rating = (rating >= 0.0).then_some(rating);
                    t.updated_at = now;
                    json_resp(200, &json!({}))
                }
                None => status(404),
            }
        }
        (Method::Get, ["library", "parts", _part, _ver, file]) => match library::audio_by_name(file) {
            Some(a) => serve_bytes(a.bytes, mime_for(a.container), range),
            None => status(404),
        },
        (Method::Get, ["music", ":", "transcode", "universal", start]) if start.starts_with("start") => {
            // We can't transcode; hand out a fixed MP3 so the client path is exercised.
            serve_bytes(AUDIO[0].bytes, "audio/mpeg", range)
        }
        (Method::Get, ["photo", ":", "transcode"]) => {
            let w: u32 = q.get("width").and_then(|s| s.parse().ok()).unwrap_or(200).clamp(8, 2000);
            let h: u32 = q.get("height").and_then(|s| s.parse().ok()).unwrap_or(200).clamp(8, 2000);
            let src = q.get("url").cloned().unwrap_or_default();
            if src.contains("missing") {
                return status(404);
            }
            bytes_resp(200, "image/jpeg", art_jpeg(&src, w, h), vec![])
        }
        _ => {
            let _ = base;
            status(404)
        }
    }
}

fn mime_for(container: &str) -> &'static str {
    match container {
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "mp4" => "audio/mp4",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

fn serve_bytes(bytes: &'static [u8], mime: &str, range: Option<&str>) -> Resp {
    let total = bytes.len();
    let start = range
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.split('-').next())
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|s| *s < total);
    match start {
        Some(s) => bytes_resp(
            206,
            mime,
            bytes[s..].to_vec(),
            vec![
                Header::from_bytes("Content-Range", format!("bytes {s}-{}/{total}", total - 1)).unwrap(),
                Header::from_bytes("Accept-Ranges", "bytes").unwrap(),
            ],
        ),
        None => bytes_resp(200, mime, bytes.to_vec(), vec![Header::from_bytes("Accept-Ranges", "bytes").unwrap()]),
    }
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Deterministic "cover art": a diagonal two-colour gradient seeded by the source path.
fn art_jpeg(src: &str, w: u32, h: u32) -> Vec<u8> {
    let hsh = fnv(src);
    let c1 = [(hsh & 0xff) as u8, ((hsh >> 8) & 0xff) as u8, ((hsh >> 16) & 0xff) as u8];
    let c2 = [((hsh >> 24) & 0xff) as u8 / 2, ((hsh >> 32) & 0xff) as u8 / 2, ((hsh >> 40) & 0xff) as u8 / 2];
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let t = (x + y) as f32 / (w + h) as f32;
        let ring = (((x as f32 - w as f32 / 2.0).hypot(y as f32 - h as f32 / 2.0)) / (w.min(h) as f32 / 6.0)) as u32 % 2;
        let mix = |a: u8, b: u8| (a as f32 * (1.0 - t) + b as f32 * t) as u8;
        let k = if ring == 0 { 1.0 } else { 0.85 };
        image::Rgb([
            (mix(c1[0], c2[0]) as f32 * k) as u8,
            (mix(c1[1], c2[1]) as f32 * k) as u8,
            (mix(c1[2], c2[2]) as f32 * k) as u8,
        ])
    });
    let mut out = Vec::new();
    let _ = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img);
    out
}

fn ts_filter(q: &HashMap<String, String>, key: &str) -> Option<i64> {
    q.get(&format!("{key}>>")).or_else(|| q.get(&format!("{key}>"))).and_then(|v| v.trim_start_matches('=').parse().ok())
}

/// One page of a section listing. Items are filtered and paginated *before* JSON is built, so
/// a page of a 100k-track library costs 1000 serializations instead of 100k.
fn section_items(lib: &Library, sec: &str, q: &HashMap<String, String>, start: usize, size: usize) -> (Vec<Value>, usize) {
    if sec != MUSIC_SECTION {
        return (vec![], 0);
    }
    let updated = ts_filter(q, "updatedAt");
    let added = ts_filter(q, "addedAt");
    let keep = |a: i64, u: i64| updated.is_none_or(|t| u >= t) && added.is_none_or(|t| a >= t);
    let paginate = |total: usize| (start.min(total), size.min(total.saturating_sub(start)));
    match q.get("type").map(String::as_str) {
        Some("8") => {
            let items: Vec<&Artist> = lib.artists.iter().filter(|a| keep(a.added_at, a.updated_at)).collect();
            let (s, n) = paginate(items.len());
            (items[s..s + n].iter().map(|a| artist_json(lib, a)).collect(), items.len())
        }
        Some("9") => {
            let items: Vec<&Album> = lib.albums.iter().filter(|a| keep(a.added_at, a.updated_at)).collect();
            let (s, n) = paginate(items.len());
            (items[s..s + n].iter().map(|a| album_json(lib, a)).collect(), items.len())
        }
        Some("10") | None => {
            let items: Vec<&Track> = lib.tracks.iter().filter(|t| keep(t.added_at, t.updated_at)).collect();
            let (s, n) = paginate(items.len());
            (items[s..s + n].iter().map(|t| track_json(lib, t, false)).collect(), items.len())
        }
        _ => (vec![], 0),
    }
}

fn artist_json(lib: &Library, a: &library::Artist) -> Value {
    let _ = lib;
    json!({
        "ratingKey": a.id.to_string(), "key": format!("/library/metadata/{}/children", a.id), "type": "artist",
        "title": a.title, "titleSort": a.title.trim_start_matches("The "), "summary": format!("{} is a mock artist.", a.title),
        "thumb": format!("/library/metadata/{}/thumb/{}", a.id, a.updated_at),
        "art": format!("/library/metadata/{}/art/{}", a.id, a.updated_at),
        "addedAt": a.added_at, "updatedAt": a.updated_at, "librarySectionID": 1,
        "Genre": [{ "tag": a.genre }]
    })
}

fn album_json(lib: &Library, a: &library::Album) -> Value {
    let artist = lib.artist(a.artist_id);
    let leafs = lib.leaf_counts.get(&a.id).copied().unwrap_or(0);
    // One album deliberately has no art to exercise the placeholder path.
    let thumb = if a.title == "Live at the Mill" { Value::Null } else { json!(format!("/library/metadata/{}/thumb/{}", a.id, a.updated_at)) };
    json!({
        "ratingKey": a.id.to_string(), "key": format!("/library/metadata/{}/children", a.id), "type": "album",
        "title": a.title, "parentRatingKey": a.artist_id.to_string(), "parentTitle": artist.map(|x| x.title.clone()),
        "parentThumb": artist.map(|x| format!("/library/metadata/{}/thumb/{}", x.id, x.updated_at)),
        "year": a.year, "thumb": thumb, "addedAt": a.added_at, "updatedAt": a.updated_at,
        "leafCount": leafs, "librarySectionID": 1, "Genre": [{ "tag": a.genre }]
    })
}

fn track_json(lib: &Library, t: &Track, detail: bool) -> Value {
    let album = lib.album(t.album_id);
    let artist = album.and_then(|a| lib.artist(a.artist_id));
    let audio = &AUDIO[t.audio];
    let mut stream = json!({ "id": t.id * 10, "streamType": 2, "codec": audio.codec, "channels": 2, "selected": true });
    if detail {
        stream["gain"] = json!(audio.gain_db);
        stream["albumGain"] = json!(audio.gain_db);
        stream["peak"] = json!(0.95);
        stream["loudness"] = json!(-18.0 - audio.gain_db);
    }
    json!({
        "ratingKey": t.id.to_string(), "key": format!("/library/metadata/{}", t.id), "type": "track",
        "title": t.title, "index": t.index, "parentIndex": 1,
        "parentRatingKey": t.album_id.to_string(), "parentTitle": album.map(|a| a.title.clone()),
        "grandparentRatingKey": artist.map(|a| a.id.to_string()), "grandparentTitle": artist.map(|a| a.title.clone()),
        "thumb": album.map(|a| format!("/library/metadata/{}/thumb/{}", a.id, a.updated_at)),
        "parentThumb": album.map(|a| format!("/library/metadata/{}/thumb/{}", a.id, a.updated_at)),
        "grandparentThumb": artist.map(|a| format!("/library/metadata/{}/thumb/{}", a.id, a.updated_at)),
        "parentYear": album.map(|a| a.year),
        "duration": audio.duration_ms, "viewCount": t.view_count, "lastViewedAt": t.last_viewed_at,
        "userRating": t.user_rating, "addedAt": t.added_at, "updatedAt": t.updated_at, "librarySectionID": 1,
        "Media": [{
            "id": t.id, "duration": audio.duration_ms, "bitrate": audio.bytes.len() as u64 * 8 / audio.duration_ms.max(1),
            "audioChannels": 2, "audioCodec": audio.codec, "container": audio.container,
            "Part": [{ "id": t.id, "key": format!("/library/parts/{}/{}/{}", t.id, t.updated_at, audio.name),
                       "duration": audio.duration_ms, "size": audio.bytes.len(), "container": audio.container,
                       "file": format!("/music/{}", audio.name), "Stream": [stream] }]
        }]
    })
}

fn playlist_json(lib: &Library, p: &library::Playlist) -> Value {
    let dur: u64 = p.items.iter().filter_map(|id| lib.track(*id)).map(|t| AUDIO[t.audio].duration_ms).sum();
    json!({
        "ratingKey": p.id.to_string(), "key": format!("/playlists/{}/items", p.id), "type": "playlist",
        "title": p.title, "playlistType": "audio", "smart": p.smart, "leafCount": p.items.len(),
        "duration": dur, "composite": format!("/playlists/{}/composite/{}", p.id, p.updated_at),
        "addedAt": p.added_at, "updatedAt": p.updated_at
    })
}

fn item_json(lib: &Library, id: u64, detail: bool) -> Option<Value> {
    if let Some(t) = lib.track(id) {
        return Some(track_json(lib, t, detail));
    }
    if let Some(a) = lib.album(id) {
        return Some(album_json(lib, a));
    }
    if let Some(a) = lib.artist(id) {
        return Some(artist_json(lib, a));
    }
    lib.playlists.iter().find(|p| p.id == id).map(|p| playlist_json(lib, p))
}
