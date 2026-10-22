//! Audio sources: local files and progressive HTTP downloads into the audio cache.
//!
//! HTTP playback never waits for the whole file. A background thread appends bytes to a cache
//! file while the decoder reads the same file through [`ProgressiveSource`], blocking only when
//! it catches up with the download. Completed downloads are reused (and counted against the
//! audio-cache size limit), so replaying a track does not hit the network again.

use crate::plex::http::Http;
use crate::plex::PlexError;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime};
use symphonia::core::io::MediaSource;

/// Where a track's bytes come from.
#[derive(Clone, Debug)]
pub enum SourceSpec {
    Local(PathBuf),
    Http { url: String, key: String },
}

impl SourceSpec {
    pub fn is_http(&self) -> bool {
        matches!(self, SourceSpec::Http { .. })
    }
}

/// One completed or in-flight download, shared with the decoder.
struct DownloadState {
    file: File,
    available: u64,
    total: Option<u64>,
    done: bool,
    error: Option<String>,
}

struct Download {
    state: Mutex<DownloadState>,
    wake: Condvar,
    cancel: AtomicBool,
    /// Live readers; the last one to go away cancels the transfer.
    readers: AtomicUsize,
}

/// Downloads in flight, shared by every reader of the same cache file. Two decoders can read
/// the same track at once (prefetch and playback); they must not truncate each other's file.
fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<Download>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<Download>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A seekable view over a cache file that grows while it is being downloaded.
pub struct ProgressiveSource {
    dl: Arc<Download>,
    pos: u64,
    /// Scratch file to remove when the track ends (only used when the disk cache is off).
    temp: Option<PathBuf>,
}

impl ProgressiveSource {
    fn new(dl: Arc<Download>, temp: Option<PathBuf>) -> Self {
        dl.readers.fetch_add(1, Ordering::Relaxed);
        Self { dl, pos: 0, temp }
    }

    /// Wait until at least `at_least` bytes (or the end of the download) are on disk.
    pub fn wait_for(&self, at_least: u64, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut st = self.dl.state.lock().expect("download state poisoned");
        loop {
            if st.available >= at_least || st.done || st.error.is_some() {
                return st.available >= at_least;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            let (next, _) = self.dl.wake.wait_timeout(st, left.min(Duration::from_millis(100))).expect("poisoned");
            st = next;
        }
    }

    pub fn downloaded_bytes(&self) -> u64 {
        self.dl.state.lock().map(|s| s.available).unwrap_or(0)
    }

    pub fn total_bytes(&self) -> Option<u64> {
        self.dl.state.lock().ok().and_then(|s| s.total)
    }
}

impl Drop for ProgressiveSource {
    fn drop(&mut self) {
        // Only stop the transfer when the last reader lets go.
        let left = self.dl.readers.fetch_sub(1, Ordering::AcqRel);
        log::debug!("dropping stream reader at {} ({} left)", self.pos, left.saturating_sub(1));
        if left == 1 {
            self.dl.cancel.store(true, Ordering::Relaxed);
            self.dl.wake.notify_all();
        }
        if let Some(path) = self.temp.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Read for ProgressiveSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut st = self.dl.state.lock().map_err(|_| io::Error::other("download state poisoned"))?;
        loop {
            if self.pos < st.available {
                let want = ((st.available - self.pos) as usize).min(buf.len());
                let n = read_at(&st.file, &mut buf[..want], self.pos)?;
                if n == 0 {
                    log::debug!("stream short read at {} (available {})", self.pos, st.available);
                    return Ok(0);
                }
                self.pos += n as u64;
                return Ok(n);
            }
            if let Some(e) = &st.error {
                return Err(io::Error::other(e.clone()));
            }
            if st.done || self.dl.cancel.load(Ordering::Relaxed) {
                log::debug!(
                    "stream EOF at {}: available={} total={:?} done={} cancelled={} error={:?}",
                    self.pos,
                    st.available,
                    st.total,
                    st.done,
                    self.dl.cancel.load(Ordering::Relaxed),
                    st.error
                );
                return Ok(0);
            }
            let (next, _) = self
                .dl
                .wake
                .wait_timeout(st, Duration::from_millis(100))
                .map_err(|_| io::Error::other("download state poisoned"))?;
            st = next;
        }
    }
}

impl Seek for ProgressiveSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(p) => p,
            SeekFrom::Current(d) => self.pos.saturating_add_signed(d),
            SeekFrom::End(d) => {
                let st = self.dl.state.lock().map_err(|_| io::Error::other("download state poisoned"))?;
                // The response length is enough to position at the end; reads block until the
                // bytes arrive. MP4/M4A keeps its index at the end, so this is needed up front.
                match st.total {
                    Some(len) => len.saturating_add_signed(d),
                    None => return Err(io::Error::new(io::ErrorKind::Unsupported, "stream length unknown")),
                }
            }
        };
        log::trace!("stream seek {:?} -> {}", pos, target);
        self.pos = target;
        Ok(target)
    }
}

impl MediaSource for ProgressiveSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.total_bytes()
    }
}

#[cfg(unix)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buf, offset)
}

#[cfg(windows)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buf, offset)
}

#[cfg(unix)]
fn write_at(file: &File, buf: &[u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.write_at(buf, offset)
}

#[cfg(windows)]
fn write_at(file: &File, buf: &[u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_write(buf, offset)
}

/// Append at an explicit offset. Positioned I/O keeps the handle usable by concurrent readers:
/// on Windows a positioned read would otherwise move the shared file pointer that a sequential
/// write relies on, so later chunks would overwrite earlier ones.
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        match write_at(file, buf, offset) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "cache file accepted no bytes")),
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Disk cache for audio streams.
#[derive(Clone, Debug)]
pub struct AudioCache {
    dir: PathBuf,
    max_bytes: u64,
    enabled: bool,
}

impl AudioCache {
    pub fn new(dir: PathBuf, max_bytes: u64, enabled: bool) -> Self {
        Self { dir, max_bytes, enabled }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file_for(&self, key: &str) -> PathBuf {
        let safe: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        self.dir.join(format!("{safe}.audio"))
    }

    fn ensure_dir(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }

    /// Drop stale partial downloads left behind by an earlier run.
    pub fn cleanup_partials(&self) {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return };
        for entry in rd.flatten() {
            let path = entry.path();
            let done = path.with_extension("done").exists();
            let fresh = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| SystemTime::now().duration_since(t).ok())
                .is_some_and(|age| age < Duration::from_secs(6 * 3600));
            if !done && !fresh {
                log::debug!("removing stale partial audio download {}", path.display());
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// Total bytes used by cached audio.
    pub fn used_bytes(&self) -> u64 {
        crate::paths::dir_size(&self.dir)
    }

    /// Delete the least recently used completed downloads until the cache fits `max_bytes`.
    /// Files opened by the engine are skipped (Windows refuses to delete them anyway).
    pub fn enforce_limit(&self) {
        if !self.enabled || self.max_bytes == 0 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return };
        let mut entries: Vec<(SystemTime, u64, PathBuf, PathBuf)> = Vec::new();
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("audio") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            entries.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len(), path.clone(), path.with_extension("done")));
        }
        let mut total: u64 = entries.iter().map(|(_, len, _, _)| *len).sum();
        entries.sort_by_key(|(t, _, _, _)| *t);
        for (_, len, path, marker) in entries {
            if total <= self.max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
                let _ = std::fs::remove_file(&marker);
                log::debug!("audio cache evicted {}", path.display());
            }
        }
    }

    pub fn clear(&self) -> io::Result<()> {
        match std::fs::read_dir(&self.dir) {
            Ok(rd) => {
                for entry in rd.flatten() {
                    let _ = std::fs::remove_file(entry.path());
                }
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Open a source. For HTTP this starts a download in the background and returns immediately
    /// (unless the file is already fully cached, in which case it is a plain file read).
    pub fn open(&self, http: &Http, spec: &SourceSpec) -> Result<Box<dyn MediaSource>, String> {
        match spec {
            SourceSpec::Local(path) => {
                let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
                Ok(Box::new(file))
            }
            SourceSpec::Http { url, key } => {
                if !self.enabled {
                    return self.open_streaming(http, url, key, None);
                }
                self.ensure_dir().map_err(|e| format!("audio cache: {e}"))?;
                let path = self.file_for(key);
                let marker = path.with_extension("done");
                if marker.exists() {
                    if let Ok(file) = File::open(&path) {
                        // Refresh the LRU timestamp.
                        if let Ok(mut f) = OpenOptions::new().append(true).open(&path) {
                            let _ = f.write_all(&[]);
                            let _ = f.set_modified(SystemTime::now());
                        }
                        log::debug!("audio cache hit {} ({} bytes)", path.display(), file.metadata().map(|m| m.len()).unwrap_or(0));
                        return Ok(Box::new(file));
                    }
                }
                self.open_streaming(http, url, key, Some(path))
            }
        }
    }

    fn open_streaming(&self, http: &Http, url: &str, key: &str, path: Option<PathBuf>) -> Result<Box<dyn MediaSource>, String> {
        let (path, marker, temp) = match path {
            Some(p) => {
                let m = p.with_extension("done");
                (p, m, None)
            }
            None => {
                // Cache disabled: still need a scratch file for seeking.
                let mut p = std::env::temp_dir();
                p.push(format!("plexdio-stream-{}-{}.audio", std::process::id(), hash_key(key)));
                (p.clone(), PathBuf::new(), Some(p))
            }
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("stream cache: {e}"))?;
        }
        if temp.is_none() {
            // A decoder may already be pulling this file: share its transfer instead of
            // truncating the file and starting a second one.
            let mut reg = registry().lock().map_err(|_| "download registry poisoned".to_string())?;
            if let Some(dl) = reg.get(&path).and_then(Weak::upgrade) {
                let reusable = !dl.cancel.load(Ordering::Relaxed) && dl.state.lock().map(|s| s.error.is_none()).unwrap_or(false);
                if reusable {
                    log::debug!("joining in-flight download {}", path.display());
                    return Ok(Box::new(ProgressiveSource::new(dl, None)));
                }
            }
            let file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|e| format!("stream cache {}: {e}", path.display()))?;
            let dl = Arc::new(Download {
                state: Mutex::new(DownloadState { file, available: 0, total: None, done: false, error: None }),
                wake: Condvar::new(),
                cancel: AtomicBool::new(false),
                readers: AtomicUsize::new(0),
            });
            reg.insert(path.clone(), Arc::downgrade(&dl));
            drop(reg);
            return self.spawn_download(http, url, key, dl, path, marker, None);
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| format!("stream cache {}: {e}", path.display()))?;
        let dl = Arc::new(Download {
            state: Mutex::new(DownloadState { file, available: 0, total: None, done: false, error: None }),
            wake: Condvar::new(),
            cancel: AtomicBool::new(false),
            readers: AtomicUsize::new(0),
        });
        self.spawn_download(http, url, key, dl, path, marker, temp)
    }

    /// Start the transfer for an already-prepared cache file and hand back a reader.
    #[allow(clippy::too_many_arguments)]
    fn spawn_download(
        &self,
        http: &Http,
        url: &str,
        key: &str,
        dl: Arc<Download>,
        path: PathBuf,
        marker: PathBuf,
        temp: Option<PathBuf>,
    ) -> Result<Box<dyn MediaSource>, String> {
        log::debug!("audio download start {} ({})", key, path.display());
        let http = http.clone();
        let url = url.to_string();
        let thread_dl = dl.clone();
        let thread_path = path.clone();
        std::thread::Builder::new()
            .name("audio-download".into())
            .spawn(move || {
                let result = download(&http, &url, &thread_dl);
                if let Err(e) = &result {
                    log::warn!("audio download failed: {e}");
                }
                if let Ok(mut st) = thread_dl.state.lock() {
                    match result {
                        Ok(()) => {
                            let _ = st.file.sync_data();
                            st.done = true;
                            log::debug!("audio download done {} ({} bytes)", thread_path.display(), st.available);
                            if !marker.as_os_str().is_empty() {
                                let _ = File::create(&marker);
                            }
                        }
                        Err(e) => {
                            if thread_dl.cancel.load(Ordering::Relaxed) {
                                log::debug!("audio download cancelled");
                                let _ = std::fs::remove_file(&thread_path);
                                let _ = std::fs::remove_file(&marker);
                            } else {
                                st.error = Some(e);
                            }
                            st.done = true;
                        }
                    }
                }
                thread_dl.wake.notify_all();
            })
            .map_err(|e| format!("spawn download: {e}"))?;
        Ok(Box::new(ProgressiveSource::new(dl, temp)))
    }
}

fn hash_key(key: &str) -> u64 {
    // FNV-1a; keys are short rating keys, collisions are irrelevant in practice.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn download(http: &Http, url: &str, dl: &Arc<Download>) -> Result<(), String> {
    let (status, len, mut reader) = http.get_stream(url, None).map_err(|e: PlexError| e.to_string())?;
    if !(200..=299).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    log::debug!("audio download response {status} ({len:?} bytes)");
    {
        let mut st = dl.state.lock().map_err(|_| "state poisoned".to_string())?;
        st.total = len;
    }
    dl.wake.notify_all();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if dl.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let n = reader.read(&mut buf).map_err(|e| format!("download: {e}"))?;
        if n == 0 {
            return Ok(());
        }
        {
            let mut st = dl.state.lock().map_err(|_| "state poisoned".to_string())?;
            let at = st.available;
            write_all_at(&st.file, &buf[..n], at).map_err(|e| format!("cache write: {e}"))?;
            st.available += n as u64;
            // Publish before waking so readers never see a stale length.
            drop(st);
        }
        dl.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_download(data: &'static [u8]) -> Arc<Download> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let mut path = std::env::temp_dir();
        path.push(format!("plexdio-source-test-{}-{}.bin", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let file = OpenOptions::new().create(true).truncate(true).read(true).write(true).open(&path).unwrap();
        Arc::new(Download {
            state: Mutex::new(DownloadState { file, available: 0, total: Some(data.len() as u64), done: false, error: None }),
            wake: Condvar::new(),
            cancel: AtomicBool::new(false),
            readers: AtomicUsize::new(0),
        })
    }

    #[test]
    fn progressive_read_blocks_then_delivers_full_file() {
        let data: &'static [u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let dl = test_download(data);
        let mut src = ProgressiveSource::new(dl.clone(), None);
        assert_eq!(src.total_bytes(), Some(data.len() as u64));

        let writer = {
            let dl = dl.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                let mut st = dl.state.lock().unwrap();
                st.file.write_all(&data[..10]).unwrap();
                st.available = 10;
                drop(st);
                dl.wake.notify_all();
                std::thread::sleep(Duration::from_millis(30));
                let mut st = dl.state.lock().unwrap();
                st.file.write_all(&data[10..]).unwrap();
                st.available = data.len() as u64;
                st.done = true;
                drop(st);
                dl.wake.notify_all();
            })
        };

        let mut buf = [0u8; 8];
        let n = src.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], &data[..n]);
        let mut all = Vec::new();
        src.read_to_end(&mut all).unwrap();
        assert_eq!(all, data[n..]);
        writer.join().unwrap();
    }

    #[test]
    fn seeking_back_re_reads_cached_bytes() {
        let data: &'static [u8] = b"abcdefghij";
        let dl = test_download(data);
        {
            let mut st = dl.state.lock().unwrap();
            st.file.write_all(data).unwrap();
            st.available = data.len() as u64;
            st.done = true;
        }
        let mut src = ProgressiveSource::new(dl, None);
        let mut buf = [0u8; 4];
        src.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"abcd");
        src.seek(SeekFrom::Start(2)).unwrap();
        src.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"cdef");
    }

    #[test]
    fn seeking_to_the_end_works_before_the_download_finishes() {
        // MP4 keeps its index at the end; the reader must be able to jump there as soon as the
        // response length is known.
        let data: &'static [u8] = b"0123456789";
        let dl = test_download(data);
        {
            let mut st = dl.state.lock().unwrap();
            st.file.write_all(&data[..4]).unwrap();
            st.available = 4;
        }
        let mut src = ProgressiveSource::new(dl.clone(), None);
        let pos = src.seek(SeekFrom::End(-2)).unwrap();
        assert_eq!(pos, 8);

        // The bytes arrive later; the read waits for them.
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            let mut st = dl.state.lock().unwrap();
            st.file.write_all(&data[4..]).unwrap();
            st.available = data.len() as u64;
            st.done = true;
            drop(st);
            dl.wake.notify_all();
        });
        let mut tail = String::new();
        src.read_to_string(&mut tail).unwrap();
        assert_eq!(tail, "89");
        writer.join().unwrap();
    }

    #[test]
    fn a_second_reader_shares_the_download_instead_of_cancelling_it() {
        // Dropping one reader must not stop the transfer another reader still needs.
        let data: &'static [u8] = b"shared-bytes";
        let dl = test_download(data);
        {
            let mut st = dl.state.lock().unwrap();
            st.file.write_all(data).unwrap();
            st.available = data.len() as u64;
            st.done = true;
        }
        let mut first = ProgressiveSource::new(dl.clone(), None);
        let mut second = ProgressiveSource::new(dl.clone(), None);
        let mut buf = [0u8; 6];
        first.read_exact(&mut buf).unwrap();
        drop(first);
        assert!(!dl.cancel.load(Ordering::Relaxed), "last reader still owns the download");
        second.seek(SeekFrom::Start(0)).unwrap();
        let mut all = Vec::new();
        second.read_to_end(&mut all).unwrap();
        assert_eq!(all, data);
        drop(second);
        assert!(dl.cancel.load(Ordering::Relaxed), "last reader cancels");
    }

    #[test]
    fn cancelled_download_stops_the_reader() {
        // Cancellation is checked by the download loop; a reader that is behind the writer must
        // not spin forever.
        let dl = test_download(b"x");
        dl.cancel.store(true, Ordering::Relaxed);
        let mut src = ProgressiveSource::new(dl, None);
        let mut buf = [0u8; 4];
        assert_eq!(src.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn cache_keys_map_to_safe_filenames() {
        let cache = AudioCache::new(PathBuf::from("/tmp/plexdio-test"), 1 << 20, true);
        let p = cache.file_for("../evil key");
        assert_eq!(p.file_name().unwrap().to_str().unwrap(), "___evil_key.audio");
    }

    #[test]
    fn unused_hash_is_stable() {
        assert_eq!(hash_key("123"), hash_key("123"));
        assert_ne!(hash_key("123"), hash_key("124"));
    }
}
