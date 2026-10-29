//! Downloads for offline listening.
//!
//! The user *pins* albums, playlists or single songs. The songs those pins cover are
//! downloaded in original quality into `offline/<server>/<track id>.<ext>`, a folder the audio
//! cache's size limit never touches. Pins live in the library database, so a playlist that gains
//! songs on the server gets them downloaded after the next sync, and removed songs are deleted.
//! Plays made while offline are kept as pending scrobbles and sent once the server is back.

use super::db::LibraryDb;
use crate::library_model::{Id, LibraryModel};
use crate::plex::Http;
use rusqlite::params;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What can be kept offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PinKind {
    Album,
    Playlist,
    Track,
}

impl PinKind {
    fn as_str(self) -> &'static str {
        match self {
            PinKind::Album => "album",
            PinKind::Playlist => "playlist",
            PinKind::Track => "track",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "album" => Some(PinKind::Album),
            "playlist" => Some(PinKind::Playlist),
            "track" => Some(PinKind::Track),
            _ => None,
        }
    }
}

pub type Pin = (PinKind, Id);

/// Folder for one server's downloads.
pub fn offline_dir(server_id: &str) -> PathBuf {
    let clean: String = server_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    crate::paths::cache_dir().join("offline").join(clean)
}

impl LibraryDb {
    pub fn pins(&self) -> rusqlite::Result<Vec<Pin>> {
        let mut st = self.conn.prepare("SELECT kind, item_id FROM offline_pins ORDER BY added_at")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.filter_map(Result::ok).filter_map(|(k, id)| PinKind::parse(&k).map(|k| (k, id))).collect())
    }

    pub fn set_pin(&self, pin: Pin, on: bool, now: i64) -> rusqlite::Result<()> {
        if on {
            self.conn.execute(
                "INSERT OR IGNORE INTO offline_pins(kind, item_id, added_at) VALUES (?1, ?2, ?3)",
                params![pin.0.as_str(), pin.1, now],
            )?;
        } else {
            self.conn.execute("DELETE FROM offline_pins WHERE kind = ?1 AND item_id = ?2", params![pin.0.as_str(), pin.1])?;
        }
        Ok(())
    }

    pub fn clear_pins(&self) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM offline_pins", []).map(|_| ())
    }

    /// Finished downloads: track id → (file name in the offline folder, bytes).
    pub fn offline_files(&self) -> rusqlite::Result<HashMap<Id, (String, u64)>> {
        let mut st = self.conn.prepare("SELECT track_id, file, bytes FROM offline_tracks")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, (r.get::<_, String>(1)?, r.get::<_, i64>(2)?.max(0) as u64))))?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    pub fn set_offline_file(&self, track: Id, file: &str, bytes: u64, now: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO offline_tracks(track_id, file, bytes, done_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(track_id) DO UPDATE SET file = excluded.file, bytes = excluded.bytes, done_at = excluded.done_at",
            params![track, file, bytes as i64, now],
        )?;
        Ok(())
    }

    pub fn remove_offline_file(&self, track: Id) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM offline_tracks WHERE track_id = ?1", [track]).map(|_| ())
    }

    pub fn add_pending_scrobble(&self, track: Id, at: i64) -> rusqlite::Result<()> {
        self.conn.execute("INSERT INTO pending_scrobbles(track_id, played_at) VALUES (?1, ?2)", params![track, at]).map(|_| ())
    }

    /// Plays waiting to be reported: (row id, track id).
    pub fn pending_scrobbles(&self) -> rusqlite::Result<Vec<(i64, Id)>> {
        let mut st = self.conn.prepare("SELECT rowid, track_id FROM pending_scrobbles ORDER BY played_at")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    pub fn remove_pending_scrobble(&self, rowid: i64) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM pending_scrobbles WHERE rowid = ?1", [rowid]).map(|_| ())
    }
}

/// Songs the pins cover, in pin order without duplicates (songs no longer in the library are
/// left out).
pub fn wanted_tracks(lib: &LibraryModel, pins: &[Pin]) -> Vec<Id> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for &(kind, id) in pins {
        let ids: Vec<Id> = match kind {
            PinKind::Album => lib.album_track_ids(id),
            PinKind::Playlist => lib.playlist(id).map(|p| lib.playlist_rows(p).into_iter().map(|r| p.items[r]).collect()).unwrap_or_default(),
            PinKind::Track => lib.track(id).map(|t| vec![t.id]).unwrap_or_default(),
        };
        out.extend(ids.into_iter().filter(|t| seen.insert(*t)));
    }
    out
}

/// Download state as the UI sees it, for the open library.
#[derive(Default)]
pub struct OfflineState {
    /// Folder of this server's downloads (`None` before a library is open).
    pub dir: Option<PathBuf>,
    pub pins: HashSet<Pin>,
    /// Finished downloads: track id → (file name, bytes).
    pub files: HashMap<Id, (String, u64)>,
    /// Waiting or being downloaded.
    pub queued: HashSet<Id>,
    /// The file being downloaded: (track, bytes so far, size).
    pub current: Option<(Id, u64, Option<u64>)>,
    /// Songs whose download failed this session (not retried until asked).
    pub failed: HashMap<Id, String>,
    /// Bumped whenever `files` changes.
    pub ver: u64,
}

impl OfflineState {
    pub fn has(&self, id: Id) -> bool {
        self.files.contains_key(&id)
    }

    /// Local file of a downloaded song.
    pub fn path(&self, id: Id) -> Option<PathBuf> {
        let (file, _) = self.files.get(&id)?;
        Some(self.dir.as_ref()?.join(file))
    }

    pub fn pinned(&self, pin: Pin) -> bool {
        self.pins.contains(&pin)
    }

    /// (downloaded, total) of `ids`.
    pub fn progress(&self, ids: &[Id]) -> (usize, usize) {
        (ids.iter().filter(|id| self.has(**id)).count(), ids.len())
    }

    pub fn bytes(&self) -> u64 {
        self.files.values().map(|(_, b)| *b).sum()
    }
}

/// One file to fetch.
#[derive(Debug, Clone)]
pub struct Job {
    pub track: Id,
    pub url: String,
    /// File extension ("flac", "mp3", ...).
    pub ext: String,
}

/// What the download worker reports to the UI.
#[derive(Debug, Clone)]
pub enum Event {
    /// Bytes received so far of the file being downloaded.
    Progress { track: Id, done: u64, total: Option<u64> },
    Finished { track: Id, file: String, bytes: u64 },
    Failed { track: Id, error: String },
}

/// The download worker: one file at a time, in the order queued.
pub struct Downloader {
    /// Jobs with the generation they were queued in.
    tx: Sender<(u64, Job)>,
    /// Bumped by [`Downloader::cancel_all`]; jobs queued before it are skipped.
    generation: Arc<Mutex<u64>>,
}

/// Download attempts per file (each resumes where the last one stopped).
const ATTEMPTS: u32 = 5;

impl Downloader {
    /// Start the worker writing into `dir`. Finished files are recorded in the database at
    /// `db_path`; `notify` gets every [`Event`] (and should wake the UI).
    pub fn start(http: &Http, dir: PathBuf, db_path: PathBuf, notify: impl Fn(Event) + Send + 'static) -> Self {
        // Big files take longer than the client's normal request limit.
        let http = Http::with_timeout(http.info.clone(), Duration::from_secs(3600)).without_retries();
        let (tx, rx) = mpsc::channel::<(u64, Job)>();
        let generation = Arc::new(Mutex::new(0u64));
        let gen_worker = generation.clone();
        let spawned = std::thread::Builder::new().name("downloads".into()).spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            while let Ok((job_gen, job)) = rx.recv() {
                if job_gen != *gen_worker.lock().unwrap_or_else(|e| e.into_inner()) {
                    continue;
                }
                let ev = match fetch(&http, &dir, &job, &notify) {
                    Ok((file, bytes)) => {
                        let saved = LibraryDb::open(&db_path).and_then(|db| db.set_offline_file(job.track, &file, bytes, crate::logging::unix_now()));
                        match saved {
                            Ok(()) => Event::Finished { track: job.track, file, bytes },
                            Err(e) => Event::Failed { track: job.track, error: format!("cannot record the download: {e}") },
                        }
                    }
                    Err(error) => {
                        log::warn!("download of track {} failed: {error}", job.track);
                        Event::Failed { track: job.track, error }
                    }
                };
                notify(ev);
            }
        });
        if spawned.is_err() {
            log::warn!("cannot start the download thread");
        }
        Self { tx, generation }
    }

    pub fn queue(&self, job: Job) {
        let g = *self.generation.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self.tx.send((g, job));
    }

    /// Drop everything queued (the file being fetched finishes or fails on its own).
    pub fn cancel_all(&self) {
        *self.generation.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }
}

/// Download one file to `<dir>/<track>.<ext>` through a `.part` file, resuming a partial one.
fn fetch(http: &Http, dir: &Path, job: &Job, notify: &dyn Fn(Event)) -> Result<(String, u64), String> {
    let name = format!("{}.{}", job.track, job.ext);
    let part = dir.join(format!("{name}.part"));
    let mut last_err = String::new();
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2u64.pow(attempt.min(4))));
        }
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let (status, len, mut body) = match http.get_stream(&job.url, Some(have)) {
            Ok(r) => r,
            // Gone or not allowed: trying again won't help.
            Err(e @ (crate::plex::PlexError::NotFound | crate::plex::PlexError::Unauthorized)) => return Err(e.user_message()),
            Err(e) => {
                last_err = e.user_message();
                continue;
            }
        };
        // 206: the server continues where the part file ends; 200: it starts over.
        let (mut file, mut done) = if status == 206 && have > 0 {
            match std::fs::OpenOptions::new().append(true).open(&part) {
                Ok(f) => (f, have),
                Err(e) => return Err(e.to_string()),
            }
        } else {
            match std::fs::File::create(&part) {
                Ok(f) => (f, 0),
                Err(e) => return Err(e.to_string()),
            }
        };
        let total = len.map(|l| l + done);
        let mut buf = vec![0u8; 64 * 1024];
        let mut last_note = std::time::Instant::now();
        let res: Result<(), String> = loop {
            match body.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    if let Err(e) = file.write_all(&buf[..n]) {
                        // Disk full or gone: retrying won't help.
                        return Err(format!("cannot write the file: {e}"));
                    }
                    done += n as u64;
                    if last_note.elapsed() > Duration::from_millis(250) {
                        last_note = std::time::Instant::now();
                        notify(Event::Progress { track: job.track, done, total });
                    }
                }
                Err(e) => break Err(e.to_string()),
            }
        };
        drop(file);
        match res {
            Ok(()) if total.is_none_or(|t| done >= t) => {
                let dest = dir.join(&name);
                std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
                return Ok((name, done));
            }
            Ok(()) => last_err = format!("connection closed at byte {done}"),
            Err(e) => last_err = e,
        }
        log::info!("download of track {} interrupted ({last_err}); resuming at byte {done}", job.track);
    }
    Err(last_err)
}

/// Delete files in `dir` that are not in `keep` (stale downloads and abandoned `.part` files of
/// songs no longer wanted). Returns how many were removed.
pub fn prune_dir(dir: &Path, keep: &HashSet<String>) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let base = name.strip_suffix(".part").unwrap_or(&name);
        if !keep.contains(base) && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_and_files_round_trip() {
        let db = LibraryDb::open_in_memory().unwrap();
        db.set_pin((PinKind::Album, 5), true, 1).unwrap();
        db.set_pin((PinKind::Playlist, 9), true, 2).unwrap();
        db.set_pin((PinKind::Album, 5), true, 3).unwrap();
        assert_eq!(db.pins().unwrap(), vec![(PinKind::Album, 5), (PinKind::Playlist, 9)]);
        db.set_pin((PinKind::Album, 5), false, 4).unwrap();
        assert_eq!(db.pins().unwrap(), vec![(PinKind::Playlist, 9)]);
        db.set_offline_file(42, "42.flac", 1000, 5).unwrap();
        assert_eq!(db.offline_files().unwrap()[&42], ("42.flac".to_string(), 1000));
        db.remove_offline_file(42).unwrap();
        assert!(db.offline_files().unwrap().is_empty());
        db.add_pending_scrobble(7, 10).unwrap();
        let p = db.pending_scrobbles().unwrap();
        assert_eq!(p.len(), 1);
        db.remove_pending_scrobble(p[0].0).unwrap();
        assert!(db.pending_scrobbles().unwrap().is_empty());
    }

    #[test]
    fn prune_keeps_wanted_files_and_their_parts() {
        let dir = std::env::temp_dir().join(format!("plexdio-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["1.flac", "2.mp3", "3.ogg.part", "4.flac.part"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let keep: HashSet<String> = ["1.flac".to_string(), "4.flac".to_string()].into();
        assert_eq!(prune_dir(&dir, &keep), 2);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, vec!["1.flac", "4.flac.part"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
