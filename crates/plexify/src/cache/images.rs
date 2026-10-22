//! Artwork fetching with an on-disk LRU cache.
//!
//! Thumbnails are requested from the Plex photo transcoder at the size they are displayed, so
//! downloads and decoded bitmaps stay small. Decoded images go back to the UI thread, which owns
//! the textures. A small pool of worker threads serves requests newest-first, so the items the
//! user is currently looking at load before ones that already scrolled away.

use crate::plex::server::PmsClient;
use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtKey {
    /// Plex thumb path (`/library/metadata/1/thumb/123`) or absolute URL.
    pub thumb: String,
    /// Square edge in physical pixels.
    pub size: u32,
}

pub struct DecodedImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub type ArtResult = (ArtKey, Result<DecodedImage, String>);

struct Queue {
    items: VecDeque<ArtKey>,
    shutdown: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    cv: Condvar,
    server: Mutex<Option<(String, PmsClient)>>,
    offline_id: Mutex<String>,
    dir: PathBuf,
    cap_bytes: Mutex<u64>,
    written_since_trim: Mutex<u64>,
}

pub struct ImageService {
    shared: Arc<Shared>,
    results: std::sync::mpsc::Receiver<ArtResult>,
}

const MAX_QUEUE: usize = 96;
const WORKERS: usize = 3;

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

pub fn disk_name(server_id: &str, key: &ArtKey) -> String {
    format!("{:016x}_{}.jpg", fnv(&format!("{server_id}|{}", key.thumb)), key.size)
}

impl ImageService {
    /// `wake` is called after each result so the UI repaints.
    pub fn new(dir: PathBuf, cap_mb: u64, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue { items: VecDeque::new(), shutdown: false }),
            cv: Condvar::new(),
            server: Mutex::new(None),
            offline_id: Mutex::new(String::new()),
            dir,
            cap_bytes: Mutex::new(cap_mb * 1024 * 1024),
            written_since_trim: Mutex::new(0),
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = Arc::new(wake);
        for i in 0..WORKERS {
            let shared = shared.clone();
            let tx = tx.clone();
            let wake = wake.clone();
            let _ = std::thread::Builder::new().name(format!("art-{i}")).spawn(move || worker(shared, tx, wake));
        }
        // Enforce the cap once at startup (in the background).
        {
            let shared = shared.clone();
            let _ = std::thread::Builder::new().name("art-trim".into()).spawn(move || trim(&shared));
        }
        Self { shared, results: rx }
    }

    pub fn set_server(&self, server: Option<(String, PmsClient)>) {
        if let Ok(mut s) = self.shared.server.lock() {
            *s = server;
        }
    }

    /// Offline: no client, but keep the server id so disk-cached art is still found.
    pub fn set_server_id_only(&self, server_id: &str) {
        if let Ok(mut s) = self.shared.offline_id.lock() {
            *s = server_id.to_string();
        }
    }

    pub fn set_cap_mb(&self, mb: u64) {
        if let Ok(mut c) = self.shared.cap_bytes.lock() {
            *c = mb * 1024 * 1024;
        }
        trim(&self.shared);
    }

    pub fn request(&self, key: ArtKey) {
        let Ok(mut q) = self.shared.queue.lock() else { return };
        if q.items.contains(&key) {
            return;
        }
        q.items.push_back(key);
        while q.items.len() > MAX_QUEUE {
            q.items.pop_front(); // the oldest requests are probably off-screen by now
        }
        self.shared.cv.notify_one();
    }

    /// Forget queued requests (e.g. when switching libraries).
    pub fn clear_queue(&self) {
        if let Ok(mut q) = self.shared.queue.lock() {
            q.items.clear();
        }
    }

    pub fn drain(&self) -> Vec<ArtResult> {
        self.results.try_iter().collect()
    }

    pub fn dir(&self) -> &Path {
        &self.shared.dir
    }

    /// Delete every cached image.
    pub fn clear_disk(&self) {
        if let Ok(rd) = std::fs::read_dir(&self.shared.dir) {
            for e in rd.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

impl Drop for ImageService {
    fn drop(&mut self) {
        if let Ok(mut q) = self.shared.queue.lock() {
            q.shutdown = true;
        }
        self.shared.cv.notify_all();
    }
}

fn worker(shared: Arc<Shared>, tx: std::sync::mpsc::Sender<ArtResult>, wake: Arc<dyn Fn() + Send + Sync>) {
    loop {
        let key = {
            let Ok(mut q) = shared.queue.lock() else { return };
            loop {
                if q.shutdown {
                    return;
                }
                if let Some(k) = q.items.pop_back() {
                    break k;
                }
                q = match shared.cv.wait(q) {
                    Ok(q) => q,
                    Err(_) => return,
                };
            }
        };
        let res = load(&shared, &key);
        if tx.send((key, res)).is_err() {
            return;
        }
        wake();
    }
}

fn load(shared: &Shared, key: &ArtKey) -> Result<DecodedImage, String> {
    let server = shared.server.lock().ok().and_then(|s| s.clone());
    let offline_id = shared.offline_id.lock().map(|s| s.clone()).unwrap_or_default();
    let server_id = server.as_ref().map(|s| s.0.as_str()).unwrap_or(&offline_id);
    let path = shared.dir.join(disk_name(server_id, key));
    // Disk hit (also works offline).
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(img) = decode(&bytes, key.size) {
            // Touch for LRU.
            if let Ok(f) = std::fs::File::options().write(true).open(&path) {
                let _ = f.set_modified(SystemTime::now());
            }
            return Ok(img);
        }
        let _ = std::fs::remove_file(&path);
    }
    let Some((_, pms)) = server else {
        return Err("offline".into());
    };
    let url = if key.thumb.starts_with("http://") || key.thumb.starts_with("https://") {
        key.thumb.clone()
    } else {
        pms.photo_url(&key.thumb, key.size, key.size)
    };
    let (_, _, rd) = pms.http().get_stream(&url, None).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    rd.take(16 * 1024 * 1024).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let img = decode(&bytes, key.size)?;
    if std::fs::write(&path, &bytes).is_ok() {
        let mut w = shared.written_since_trim.lock().map(|g| *g).unwrap_or(0);
        w += bytes.len() as u64;
        let cap = shared.cap_bytes.lock().map(|c| *c).unwrap_or(u64::MAX);
        if w > cap / 20 {
            w = 0;
            trim(shared);
        }
        if let Ok(mut g) = shared.written_since_trim.lock() {
            *g = w;
        }
    }
    Ok(img)
}

/// Decode JPEG/PNG and fit into `size`x`size` (keeping aspect ratio).
pub fn decode(bytes: &[u8], size: u32) -> Result<DecodedImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let img = if img.width() > size || img.height() > size {
        img.resize(size, size, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Ok(DecodedImage { width: rgba.width() as usize, height: rgba.height() as usize, rgba: rgba.into_raw() })
}

/// Delete least-recently-used files until the directory is under the cap.
fn trim(shared: &Shared) {
    let cap = shared.cap_bytes.lock().map(|c| *c).unwrap_or(u64::MAX);
    trim_dir(&shared.dir, cap);
}

pub fn trim_dir(dir: &Path, cap: u64) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= cap {
        return 0;
    }
    files.sort_by_key(|f| f.0);
    let mut removed = 0;
    for (_, len, p) in files {
        if total <= cap * 9 / 10 {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
            removed += 1;
        }
    }
    log::info!("image cache trimmed: removed {removed} files");
    removed
}

/// Wait helper for tests.
pub fn wait_for(svc: &ImageService, n: usize, timeout: Duration) -> Vec<ArtResult> {
    let start = std::time::Instant::now();
    let mut out = Vec::new();
    while out.len() < n && start.elapsed() < timeout {
        out.extend(svc.drain());
        std::thread::sleep(Duration::from_millis(10));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_removes_oldest_first() {
        let dir = std::env::temp_dir().join(format!("plexify-art-trim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..10 {
            let p = dir.join(format!("{i}.jpg"));
            std::fs::write(&p, vec![0u8; 1000]).unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + i)).unwrap();
        }
        let removed = trim_dir(&dir, 5000);
        assert!(removed >= 5);
        assert!(!dir.join("0.jpg").exists(), "oldest goes first");
        assert!(dir.join("9.jpg").exists(), "newest stays");
        assert!(crate::paths::dir_size(&dir) <= 5000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_names_are_stable_and_distinct() {
        let a = ArtKey { thumb: "/library/metadata/1/thumb/2".into(), size: 160 };
        let b = ArtKey { thumb: "/library/metadata/1/thumb/2".into(), size: 320 };
        assert_eq!(disk_name("s", &a), disk_name("s", &a));
        assert_ne!(disk_name("s", &a), disk_name("s", &b));
        assert_ne!(disk_name("s", &a), disk_name("t", &a));
    }
}
