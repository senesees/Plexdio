//! Discord Rich Presence: shows the playing track on the user's Discord profile.
//!
//! Talks to the local Discord client over its IPC pipe (`\\.\pipe\discord-ipc-N` on Windows,
//! `$XDG_RUNTIME_DIR/discord-ipc-N` elsewhere). Each message is a little-endian `opcode`,
//! `length`, then a JSON payload. A worker thread owns the connection, connects lazily, retries
//! while Discord is not running, and stays within Discord's rate limit (5 updates per 20 s).

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Built-in application ID from the Discord Developer Portal. Empty: users enter their own in
/// Settings (or set `PLEXDIO_DISCORD_CLIENT_ID`).
const CLIENT_ID: &str = "";

/// Step-by-step setup instructions (README on GitHub), linked from Settings.
pub const SETUP_URL: &str = "https://github.com/senesees/plexmusicplayer#discord-setup";

/// Minimum gap between activity updates (Discord allows 5 per 20 seconds).
const MIN_UPDATE_GAP: Duration = Duration::from_secs(4);
/// Wait between connection attempts while Discord is closed.
const RECONNECT_EVERY: Duration = Duration::from_secs(15);

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;

/// Discord application IDs are snowflakes: 17 to 20 digits.
pub fn valid_client_id(id: &str) -> bool {
    (17..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

/// The application ID to use: the one entered in Settings, else `PLEXDIO_DISCORD_CLIENT_ID`,
/// else the built-in one.
pub fn client_id(from_settings: &str) -> Option<String> {
    let user = from_settings.trim();
    if valid_client_id(user) {
        return Some(user.to_string());
    }
    std::env::var("PLEXDIO_DISCORD_CLIENT_ID")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| valid_client_id(s))
        .or_else(|| valid_client_id(CLIENT_ID).then(|| CLIENT_ID.to_string()))
}

/// What to show: the playing track and where it is (unix milliseconds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// When the track (virtually) started, so Discord's progress bar lines up with the player.
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    /// Look up a public album cover for the large image (see `cover_art`).
    pub cover: bool,
}

enum Msg {
    Set(Option<Presence>),
    Stop,
}

/// Handle to the presence worker. Dropping it clears the presence and closes the connection.
pub struct DiscordPresence {
    tx: Sender<Msg>,
}

impl DiscordPresence {
    pub fn start(client_id: String) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("discord-rpc".into())
            .spawn(move || worker(&client_id, rx))
            .map_err(|e| log::warn!("cannot start Discord presence: {e}"))
            .ok()?;
        Some(Self { tx })
    }

    /// Show `p`, or clear the activity with `None`. Only the latest value matters.
    pub fn set(&self, p: Option<Presence>) {
        let _ = self.tx.send(Msg::Set(p));
    }
}

impl Drop for DiscordPresence {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
    }
}

trait Pipe: Read + Write + Send {}
impl<T: Read + Write + Send> Pipe for T {}

struct Conn {
    pipe: Box<dyn Pipe>,
    nonce: u64,
}

impl Conn {
    fn open(client_id: &str) -> Result<Self, String> {
        let mut last = String::from("Discord is not running");
        for i in 0..10 {
            match open_pipe(i) {
                Ok(pipe) => {
                    let mut conn = Conn { pipe, nonce: 0 };
                    match conn.handshake(client_id) {
                        Ok(()) => return Ok(conn),
                        Err(e) => last = e,
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => last = format!("discord-ipc-{i}: {e}"),
            }
        }
        Err(last)
    }

    fn handshake(&mut self, client_id: &str) -> Result<(), String> {
        self.write(OP_HANDSHAKE, &json!({ "v": 1, "client_id": client_id }))?;
        let (op, reply) = self.read()?;
        match (op, reply.get("evt").and_then(Value::as_str)) {
            (OP_FRAME, Some("READY")) => {
                let user = reply.pointer("/data/user/username").and_then(Value::as_str).unwrap_or("?");
                log::info!("Discord presence connected (user {user})");
                Ok(())
            }
            (OP_CLOSE, _) => Err(format!("Discord refused the connection: {}", reply.get("message").and_then(Value::as_str).unwrap_or("closed"))),
            _ => Err(format!("unexpected Discord handshake reply: {reply}")),
        }
    }

    fn set_activity(&mut self, p: Option<&Presence>, cover_url: Option<&str>) -> Result<(), String> {
        self.nonce += 1;
        let activity = p.map(|p| activity_json(p, cover_url)).unwrap_or(Value::Null);
        let cmd = json!({
            "cmd": "SET_ACTIVITY",
            "args": { "pid": std::process::id(), "activity": activity },
            "nonce": self.nonce.to_string(),
        });
        self.write(OP_FRAME, &cmd)?;
        // Discord answers every command; reading keeps the pipe from filling up.
        let (op, reply) = self.read()?;
        if op == OP_CLOSE {
            return Err("Discord closed the connection".into());
        }
        if reply.get("evt").and_then(Value::as_str) == Some("ERROR") {
            log::warn!("Discord rejected the activity: {}", reply.pointer("/data/message").and_then(Value::as_str).unwrap_or("?"));
        }
        Ok(())
    }

    fn write(&mut self, op: u32, payload: &Value) -> Result<(), String> {
        let body = payload.to_string();
        let mut frame = Vec::with_capacity(8 + body.len());
        frame.extend_from_slice(&op.to_le_bytes());
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(body.as_bytes());
        self.pipe.write_all(&frame).and_then(|_| self.pipe.flush()).map_err(|e| format!("Discord write: {e}"))
    }

    fn read(&mut self) -> Result<(u32, Value), String> {
        let mut header = [0u8; 8];
        self.pipe.read_exact(&mut header).map_err(|e| format!("Discord read: {e}"))?;
        let op = u32::from_le_bytes(header[..4].try_into().expect("4 bytes"));
        let len = u32::from_le_bytes(header[4..].try_into().expect("4 bytes")) as usize;
        if len > 1 << 20 {
            return Err(format!("Discord frame too large ({len} bytes)"));
        }
        let mut body = vec![0u8; len];
        self.pipe.read_exact(&mut body).map_err(|e| format!("Discord read: {e}"))?;
        let value = serde_json::from_slice(&body).map_err(|e| format!("Discord reply: {e}"))?;
        Ok((op, value))
    }
}

/// Discord limits text fields to 2..=128 characters.
fn field(s: &str) -> String {
    let mut s: String = s.chars().take(128).collect();
    while s.chars().count() < 2 {
        s.push(' ');
    }
    s
}

/// `cover_url`: a public https album cover; the Plexdio logo (art asset `plexdio`) then moves
/// to the small badge, like Spotify's presence.
fn activity_json(p: &Presence, cover_url: Option<&str>) -> Value {
    let mut timestamps = json!({ "start": p.start_ms });
    if let Some(end) = p.end_ms {
        timestamps["end"] = json!(end);
    }
    let mut assets = match cover_url {
        Some(url) => json!({ "large_image": url, "small_image": "plexdio", "small_text": "Plexdio" }),
        None => json!({ "large_image": "plexdio" }),
    };
    if !p.album.trim().is_empty() {
        assets["large_text"] = json!(field(&p.album));
    }
    json!({
        // 2 = "Listening to", which also shows the progress bar from the timestamps.
        "type": 2,
        "details": field(&p.title),
        "state": field(&format!("by {}", p.artist)),
        "timestamps": timestamps,
        "assets": assets,
    })
}

#[cfg(windows)]
fn open_pipe(i: u32) -> std::io::Result<Box<dyn Pipe>> {
    let f = std::fs::OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}"))?;
    Ok(Box::new(f))
}

#[cfg(unix)]
fn open_pipe(i: u32) -> std::io::Result<Box<dyn Pipe>> {
    let dirs = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"].iter().filter_map(|v| std::env::var(v).ok()).chain(["/tmp".to_string()]);
    let mut last = std::io::Error::from(std::io::ErrorKind::NotFound);
    for dir in dirs {
        match std::os::unix::net::UnixStream::connect(std::path::Path::new(&dir).join(format!("discord-ipc-{i}"))) {
            Ok(s) => return Ok(Box::new(s)),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn worker(client_id: &str, rx: Receiver<Msg>) {
    log::debug!("Discord presence worker started");
    let mut conn: Option<Conn> = None;
    let mut want: Option<Presence> = None;
    // What Discord currently shows; `None` until the first successful update.
    let mut shown: Option<Option<Presence>> = None;
    let mut last_send = Instant::now() - MIN_UPDATE_GAP;
    let mut next_connect = Instant::now();
    let mut warned = false;
    // Cover lookups by (artist, album); `None` = nothing public found, don't ask again.
    let mut covers: std::collections::HashMap<(String, String), Option<String>> = std::collections::HashMap::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Msg::Set(p)) => want = p,
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        // Coalesce a burst of updates into the latest one.
        loop {
            match rx.try_recv() {
                Ok(Msg::Set(p)) => want = p,
                Ok(Msg::Stop) | Err(mpsc::TryRecvError::Disconnected) => {
                    if let Some(c) = conn.as_mut() {
                        let _ = c.set_activity(None, None);
                    }
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if shown.as_ref() == Some(&want) {
            continue;
        }
        if conn.is_none() {
            // Nothing to clear on a connection that does not exist yet.
            if want.is_none() || Instant::now() < next_connect {
                continue;
            }
            match Conn::open(client_id) {
                Ok(c) => {
                    conn = Some(c);
                    warned = false;
                }
                Err(e) => {
                    if !warned {
                        log::info!("Discord presence unavailable ({e}); retrying in the background");
                        warned = true;
                    }
                    next_connect = Instant::now() + RECONNECT_EVERY;
                    continue;
                }
            }
        }
        if last_send.elapsed() < MIN_UPDATE_GAP {
            continue;
        }
        let cover_url = match want.as_ref().filter(|p| p.cover) {
            Some(p) => covers
                .entry((p.artist.clone(), p.album.clone()))
                .or_insert_with(|| {
                    let found = crate::cover_art::lookup(&p.artist, &p.album);
                    log::debug!("album cover for {} — {}: {}", p.artist, p.album, found.as_deref().unwrap_or("none"));
                    found
                })
                .clone(),
            None => None,
        };
        let Some(c) = conn.as_mut() else { continue };
        last_send = Instant::now();
        match c.set_activity(want.as_ref(), cover_url.as_deref()) {
            Ok(()) => shown = Some(want.clone()),
            Err(e) => {
                log::info!("Discord presence lost ({e}); reconnecting");
                conn = None;
                shown = None;
                next_connect = Instant::now() + Duration::from_secs(5);
            }
        }
    }
    if let Some(c) = conn.as_mut() {
        let _ = c.set_activity(None, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ids_are_validated_and_settings_win() {
        assert!(valid_client_id("123456789012345678"));
        assert!(!valid_client_id("1234"), "too short");
        assert!(!valid_client_id("12345678901234567x"), "digits only");
        assert_eq!(client_id(" 123456789012345678 ").as_deref(), Some("123456789012345678"));
    }

    #[test]
    fn activity_has_listening_type_and_valid_fields() {
        let p = Presence { title: "X".into(), artist: "Band".into(), album: "".into(), start_ms: 1_000, end_ms: Some(181_000), cover: false };
        let a = activity_json(&p, None);
        assert_eq!(a["assets"]["large_image"], "plexdio");
        assert!(a["assets"].get("small_image").is_none());
        assert_eq!(a["type"], 2);
        assert_eq!(a["details"], "X ", "padded to Discord's 2-character minimum");
        assert_eq!(a["state"], "by Band");
        assert_eq!(a["timestamps"]["end"], 181_000);
        assert!(a["assets"].get("large_text").is_none(), "no empty album tooltip");
        let long = "a".repeat(300);
        assert_eq!(field(&long).chars().count(), 128);
        // With a cover, the cover is the large image and the logo becomes the badge.
        let a = activity_json(&p, Some("https://e/c.jpg"));
        assert_eq!(a["assets"]["large_image"], "https://e/c.jpg");
        assert_eq!(a["assets"]["small_image"], "plexdio");
    }

    #[cfg(unix)]
    #[test]
    fn talks_the_ipc_protocol_to_a_fake_discord() {
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("plexdio-discord-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("discord-ipc-0");
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut frames = Vec::new();
            let reply = |s: &mut std::os::unix::net::UnixStream, v: Value| {
                let body = v.to_string();
                s.write_all(&OP_FRAME.to_le_bytes()).unwrap();
                s.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
                s.write_all(body.as_bytes()).unwrap();
            };
            for _ in 0..2 {
                let mut h = [0u8; 8];
                s.read_exact(&mut h).unwrap();
                let mut body = vec![0u8; u32::from_le_bytes(h[4..].try_into().unwrap()) as usize];
                s.read_exact(&mut body).unwrap();
                let v: Value = serde_json::from_slice(&body).unwrap();
                frames.push((u32::from_le_bytes(h[..4].try_into().unwrap()), v));
                reply(&mut s, json!({ "evt": if frames.len() == 1 { "READY" } else { "" }, "data": {} }));
            }
            frames
        });
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        let mut conn = Conn::open("123").expect("connects to the fake socket");
        let p = Presence { title: "Song".into(), artist: "Band".into(), album: "LP".into(), start_ms: 5, end_ms: None, cover: false };
        conn.set_activity(Some(&p), None).unwrap();
        let frames = server.join().unwrap();
        assert_eq!(frames[0].0, OP_HANDSHAKE);
        assert_eq!(frames[0].1["client_id"], "123");
        assert_eq!(frames[1].0, OP_FRAME);
        assert_eq!(frames[1].1["cmd"], "SET_ACTIVITY");
        assert_eq!(frames[1].1["args"]["activity"]["details"], "Song");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
