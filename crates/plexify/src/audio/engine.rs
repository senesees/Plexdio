//! Playback engine: a dedicated thread owns the decoder, the mixer and the output sink.
//! The UI talks to it through a command channel and polls a shared [`PlaybackStatus`].
//!
//! The mixer renders sample-exact audio: gapless transitions concatenate the decoders with no
//! inserted silence, and crossfades overlap exactly `crossfade_ms` of equal-power mix.

use super::decode::TrackDecoder;
use super::gain::{limit, track_gain};
use super::sink::{open_default, NullSink, Sink};
use super::source::{AudioCache, SourceSpec};
use crate::plex::http::Http;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BLOCK_FRAMES: usize = 1024;
/// Keep the device ring at least this full.
const TARGET_BUFFER: usize = 4096;
/// Frames are rendered up to this far ahead of the play position.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    Idle,
    Playing,
    Paused,
}

#[derive(Clone, Debug)]
pub struct TrackSpec {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    /// Plex `gain` (dB) and `albumGain`; `None` means no loudness data.
    pub gain_db: Option<f32>,
    pub album_gain_db: Option<f32>,
    pub source: SourceSpec,
}

impl TrackSpec {
    fn ext_hint(&self) -> Option<&str> {
        match &self.source {
            SourceSpec::Local(path) => Path::new(path).extension().and_then(|e| e.to_str()),
            // The server transcoder always produces MP3; direct streams keep their part extension.
            SourceSpec::Http { url, .. } if url.contains("/transcode/") => Some("mp3"),
            SourceSpec::Http { key, .. } => key.rsplit_once('.').map(|(_, ext)| ext),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TrackInfo {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
}

#[derive(Clone, Debug)]
pub struct PlaybackStatus {
    pub state: PlayState,
    pub track: Option<TrackInfo>,
    pub index: usize,
    pub queue_len: usize,
    /// Position in the current track.
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: f32,
    pub muted: bool,
    pub sink: String,
    pub sink_real: bool,
    pub error: Option<String>,
}

impl Default for PlaybackStatus {
    fn default() -> Self {
        Self {
            state: PlayState::Idle,
            track: None,
            index: 0,
            queue_len: 0,
            position_ms: 0,
            duration_ms: 0,
            volume: 1.0,
            muted: false,
            sink: "—".into(),
            sink_real: false,
            error: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub volume: f32,
    pub muted: bool,
    pub crossfade_ms: u32,
    pub normalization: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self { volume: 1.0, muted: false, crossfade_ms: 0, normalization: true }
    }
}

pub enum Command {
    PlayPlaylist(Vec<TrackSpec>, usize),
    PlayPause,
    Pause,
    Resume,
    Next,
    Prev,
    Seek(u64),
    SetVolume(f32),
    SetMuted(bool),
    SetCrossfade(u32),
    SetNormalization(bool),
    Stop,
    Shutdown,
}

struct Active {
    spec: TrackSpec,
    decoder: TrackDecoder,
    gain: f32,
    /// Frames left at the output rate, when the container reports a duration.
    frames_left: Option<u64>,
    total_frames: Option<u64>,
    /// Frames already mixed from this track during a crossfade.
    fade_pos: u64,
    fade_len: u64,
}

impl Active {
    fn read(&mut self, frames: usize) -> Result<Vec<f32>, String> {
        let mut out = Vec::with_capacity(frames * 2);
        while out.len() < frames * 2 {
            let want = frames - out.len() / 2;
            let block = self.decoder.next_block(want).map_err(|e| e.to_string())?;
            if block.is_empty() {
                break;
            }
            out.extend_from_slice(&block);
        }
        Ok(out)
    }

    fn info(&self) -> TrackInfo {
        TrackInfo {
            id: self.spec.id,
            title: self.spec.title.clone(),
            artist: self.spec.artist.clone(),
            album: self.spec.album.clone(),
            duration_ms: self.spec.duration_ms,
        }
    }
}

fn open_active(cache: &AudioCache, http: &Http, spec: &TrackSpec, rate: u32, normalization: bool, crossfade_ms: u32) -> Result<Active, String> {
    let source = cache.open(http, &spec.source)?;
    let decoder = TrackDecoder::open(source, spec.ext_hint(), rate)
        .map_err(|e| format!("{}: {e}", spec.title))?;
    log::debug!("opened {} ({}) at {} Hz", spec.title, spec.ext_hint().unwrap_or("?"), rate);
    let total_frames = decoder.total_frames();
    let fade_len = if crossfade_ms == 0 || total_frames.is_none() {
        0
    } else {
        (crossfade_ms as u64 * rate as u64) / 1000
    };
    let gain = track_gain(spec.gain_db, spec.album_gain_db, normalization);
    Ok(Active { spec: spec.clone(), decoder, gain, frames_left: total_frames, total_frames, fade_pos: 0, fade_len })
}

pub struct PlayerCore {
    sink: Box<dyn Sink>,
    rate: u32,
    cache: AudioCache,
    http: Http,
    queue: Vec<TrackSpec>,
    index: usize,
    current: Option<Active>,
    next: Option<Active>,
    prefetch: Option<(usize, Receiver<Result<Active, String>>)>,
    crossfade_ms: u32,
    normalization: bool,
    volume: f32,
    muted: bool,
    state: PlayState,
    error: Option<String>,
    frames_played: u64,
    sink_name: String,
    sink_real: bool,
}

impl PlayerCore {
    pub fn new(sink: Box<dyn Sink>, cache: AudioCache, http: Http, cfg: EngineConfig) -> Self {
        let rate = sink.sample_rate();
        let sink_name = sink.name();
        let sink_real = sink.is_real();
        let mut core = Self {
            sink,
            rate,
            cache,
            http,
            queue: Vec::new(),
            index: 0,
            current: None,
            next: None,
            prefetch: None,
            crossfade_ms: cfg.crossfade_ms,
            normalization: cfg.normalization,
            volume: cfg.volume,
            muted: cfg.muted,
            state: PlayState::Idle,
            error: None,
            frames_played: 0,
            sink_name,
            sink_real,
        };
        core.apply_volume();
        core
    }

    pub fn sink_rate(&self) -> u32 {
        self.rate
    }

    fn apply_volume(&mut self) {
        let v = if self.muted { 0.0 } else { self.volume };
        self.sink.set_volume(v);
    }

    pub fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::PlayPlaylist(tracks, index) => self.play_playlist(tracks, index),
            Command::PlayPause => match self.state {
                PlayState::Playing => self.pause(),
                PlayState::Paused => self.resume(),
                PlayState::Idle => {
                    if !self.queue.is_empty() {
                        self.start_index(self.index.min(self.queue.len() - 1));
                    }
                }
            },
            Command::Pause => self.pause(),
            Command::Resume => self.resume(),
            Command::Next => {
                if self.index + 1 < self.queue.len() {
                    self.next = None;
                    self.start_index(self.index + 1);
                } else {
                    self.finish_queue();
                }
            }
            Command::Prev => {
                // Within the first seconds, go to the previous track; otherwise restart this one.
                if self.position_ms() > 3_000 || self.index == 0 {
                    if let Some(cur) = self.current.as_mut() {
                        let _ = cur.decoder.seek(0);
                        cur.frames_left = cur.total_frames;
                        cur.fade_pos = 0;
                        self.frames_played = 0;
                        self.sink.flush();
                    }
                } else {
                    self.start_index(self.index - 1);
                }
            }
            Command::Seek(ms) => self.seek(ms),
            Command::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                self.apply_volume();
            }
            Command::SetMuted(m) => {
                self.muted = m;
                self.apply_volume();
            }
            Command::SetCrossfade(ms) => {
                self.crossfade_ms = ms.min(12_000);
                let fade = (self.crossfade_ms as u64 * self.rate as u64) / 1000;
                for a in [self.current.as_mut(), self.next.as_mut()].into_iter().flatten() {
                    a.fade_len = if a.total_frames.is_some() { fade } else { 0 };
                    if a.fade_pos > a.fade_len {
                        a.fade_pos = 0;
                    }
                }
            }
            Command::SetNormalization(on) => {
                self.normalization = on;
                for a in [self.current.as_mut(), self.next.as_mut()].into_iter().flatten() {
                    a.gain = track_gain(a.spec.gain_db, a.spec.album_gain_db, on);
                }
            }
            Command::Stop => {
                self.sink.flush();
                self.current = None;
                self.next = None;
                self.prefetch = None;
                self.state = PlayState::Idle;
                self.frames_played = 0;
            }
            Command::Shutdown => {}
        }
    }

    fn play_playlist(&mut self, tracks: Vec<TrackSpec>, index: usize) {
        self.queue = tracks;
        self.index = index.min(self.queue.len().saturating_sub(1));
        self.next = None;
        self.prefetch = None;
        self.current = None;
        self.error = None;
        self.sink.flush();
        if self.queue.is_empty() {
            self.state = PlayState::Idle;
            return;
        }
        self.start_index(self.index);
    }

    fn start_index(&mut self, index: usize) {
        if index >= self.queue.len() {
            self.finish_queue();
            return;
        }
        self.index = index;
        self.next = None;
        self.prefetch = None;
        self.error = None;
        let spec = self.queue[index].clone();
        match open_active(&self.cache, &self.http, &spec, self.rate, self.normalization, self.crossfade_ms) {
            Ok(active) => {
                log::info!("playing {} — {} ({})", active.spec.title, active.spec.artist, source_kind(&spec.source));
                self.current = Some(active);
                self.frames_played = 0;
                self.state = PlayState::Playing;
                self.sink.resume();
                self.spawn_prefetch();
            }
            Err(e) => {
                log::warn!("cannot play {}: {e}", spec.title);
                self.fail(format!("Cannot play “{}”: {e}", spec.title));
            }
        }
    }

    fn spawn_prefetch(&mut self) {
        if self.prefetch.is_some() || self.next.is_some() {
            return;
        }
        let Some(spec) = self.queue.get(self.index + 1).cloned() else { return };
        let (tx, rx) = mpsc::channel();
        let cache = self.cache.clone();
        let http = self.http.clone();
        let rate = self.rate;
        let normalization = self.normalization;
        let crossfade = self.crossfade_ms;
        let idx = self.index + 1;
        log::debug!("prefetching #{} {}", idx, spec.title);
        let spawned = std::thread::Builder::new().name("audio-prefetch".into()).spawn(move || {
            let _ = tx.send(open_active(&cache, &http, &spec, rate, normalization, crossfade));
        });
        if spawned.is_ok() {
            self.prefetch = Some((idx, rx));
        }
    }

    fn poll_prefetch(&mut self) {
        let Some((idx, rx)) = self.prefetch.take() else { return };
        match rx.try_recv() {
            Ok(Ok(active)) => {
                if idx == self.index + 1 && self.next.is_none() {
                    log::debug!("prefetch ready #{} {}", idx, active.spec.title);
                    self.next = Some(active);
                } else {
                    log::debug!("dropping stale prefetch #{} (now #{})", idx, self.index);
                }
            }
            Ok(Err(e)) => log::warn!("prefetch failed: {e}"),
            Err(TryRecvError::Empty) => self.prefetch = Some((idx, rx)),
            Err(TryRecvError::Disconnected) => {}
        }
    }

    /// Mix the current track, promoting the next one at its end. Returns when the queue ends.
    fn produce(&mut self, want: usize) -> Result<Vec<f32>, String> {
        let mut out = Vec::with_capacity(want * 2);
        let mut left = want;
        let mut guard = 0;
        while left > 0 && guard < 16 {
            guard += 1;
            let Some(mut cur) = self.current.take() else { break };
            let before = out.len();
            let mut next_frames_played = 0u64;
            let mut ended = false;

            let fade_active = cur.fade_len > 0 && self.next.is_some() && cur.frames_left.is_some_and(|l| l <= cur.fade_len);
            if fade_active {
                let fade_left = cur.fade_len.saturating_sub(cur.fade_pos);
                let avail = cur.frames_left.unwrap_or(0).min(fade_left) as usize;
                let k = left.min(avail);
                let a = cur.read(k)?;
                let n = a.len() / 2;
                if n == 0 {
                    ended = true;
                } else {
                    let gain_cur = cur.gain;
                    let b = match self.next.as_mut() {
                        Some(nx) => nx.read(n)?,
                        None => Vec::new(),
                    };
                    let m = n.min(b.len() / 2);
                    let gain_next = self.next.as_ref().map_or(1.0, |nx| nx.gain);
                    for i in 0..m {
                        let t = (cur.fade_pos + i as u64 + 1) as f32 / cur.fade_len as f32;
                        let (g_in, g_out) = ((t * std::f32::consts::FRAC_PI_2).sin(), (t * std::f32::consts::FRAC_PI_2).cos());
                        out.push(limit(a[2 * i] * gain_cur * g_out + b[2 * i] * gain_next * g_in));
                        out.push(limit(a[2 * i + 1] * gain_cur * g_out + b[2 * i + 1] * gain_next * g_in));
                    }
                    // A next track shorter than the fade: finish the block with the current one.
                    for i in m..n {
                        out.push(limit(a[2 * i] * gain_cur));
                        out.push(limit(a[2 * i + 1] * gain_cur));
                    }
                    if let Some(nx) = self.next.as_mut() {
                        nx.frames_left = nx.frames_left.map(|l| l.saturating_sub(m as u64));
                    }
                    next_frames_played = m as u64;
                    cur.fade_pos = (cur.fade_pos + n as u64).min(cur.fade_len);
                    cur.frames_left = cur.frames_left.map(|l| l.saturating_sub(n as u64));
                    left -= n;
                    if cur.frames_left == Some(0) || cur.fade_pos >= cur.fade_len {
                        ended = true;
                    }
                }
            } else {
                // Stop exactly at the fade boundary so the crossfade starts on the right sample.
                let cap = match cur.frames_left {
                    Some(l) if cur.fade_len > 0 && self.next.is_some() && l > cur.fade_len => left.min((l - cur.fade_len) as usize),
                    Some(l) => left.min(l as usize),
                    None => left,
                };
                let a = cur.read(cap)?;
                let n = a.len() / 2;
                let g = cur.gain;
                for s in &a {
                    out.push(limit(*s * g));
                }
                cur.frames_left = cur.frames_left.map(|l| l.saturating_sub(n as u64));
                left -= n;
                if n < cap || cur.frames_left == Some(0) {
                    ended = true;
                }
            }

            self.current = Some(cur);
            if ended {
                self.finish_track(next_frames_played)?;
            } else if out.len() == before {
                break;
            }
        }
        Ok(out)
    }

    /// Called when the current track reached its end.
    fn finish_track(&mut self, next_played: u64) -> Result<(), String> {
        self.frames_played = next_played;
        if self.next.is_some() {
            self.current = self.next.take();
            self.index += 1;
            if let Some(cur) = self.current.as_ref() {
                log::debug!("gapless/crossfade into {} — {}", cur.spec.title, cur.spec.artist);
            }
            self.spawn_prefetch();
            return Ok(());
        }
        if self.index + 1 < self.queue.len() {
            let idx = self.index + 1;
            let spec = self.queue[idx].clone();
            match open_active(&self.cache, &self.http, &spec, self.rate, self.normalization, self.crossfade_ms) {
                Ok(active) => {
                    log::info!("playing {} — {}", active.spec.title, active.spec.artist);
                    self.current = Some(active);
                    self.index = idx;
                    self.frames_played = 0;
                    self.spawn_prefetch();
                    Ok(())
                }
                Err(e) => {
                    self.fail(format!("Cannot play “{}”: {e}", spec.title));
                    Ok(())
                }
            }
        } else {
            self.finish_queue();
            Ok(())
        }
    }

    fn finish_queue(&mut self) {
        log::info!("queue finished");
        self.current = None;
        self.next = None;
        self.prefetch = None;
        self.state = PlayState::Idle;
        self.frames_played = 0;
    }

    fn fail(&mut self, message: String) {
        log::error!("{message}");
        self.error = Some(message);
        self.current = None;
        self.next = None;
        self.prefetch = None;
        self.state = PlayState::Idle;
    }

    fn pause(&mut self) {
        if self.state == PlayState::Playing {
            self.sink.pause();
            self.state = PlayState::Paused;
            log::info!("paused at {} ms", self.position_ms());
        }
    }

    fn resume(&mut self) {
        if self.state == PlayState::Paused {
            self.sink.resume();
            self.state = PlayState::Playing;
            log::info!("resumed at {} ms", self.position_ms());
        }
    }

    fn seek(&mut self, ms: u64) {
        let Some(cur) = self.current.as_mut() else { return };
        match cur.decoder.seek(ms) {
            Ok(()) => {
                let pos_frames = ms.saturating_mul(self.rate as u64) / 1000;
                cur.frames_left = cur.total_frames.map(|t| t.saturating_sub(pos_frames));
                cur.fade_pos = 0;
                self.frames_played = pos_frames;
                self.sink.flush();
                self.error = None;
            }
            Err(e) => log::warn!("seek failed: {e}"),
        }
    }

    fn position_ms(&self) -> u64 {
        let buffered = self.sink.buffered_frames() as u64;
        let played = self.frames_played.saturating_sub(buffered);
        played.saturating_mul(1000) / self.rate.max(1) as u64
    }

    /// Fill the output ring, then publish status. Called by the engine loop.
    pub fn pump(&mut self) {
        if self.state != PlayState::Playing {
            return;
        }
        self.poll_prefetch();
        let mut guard = 0;
        while self.sink.buffered_frames() < TARGET_BUFFER && guard < 12 {
            guard += 1;
            let block = match self.produce(BLOCK_FRAMES) {
                Ok(b) => b,
                Err(e) => {
                    self.fail(e);
                    return;
                }
            };
            if block.is_empty() {
                break;
            }
            let frames = block.len() / 2;
            let accepted = self.sink.push(&block);
            self.frames_played += accepted as u64;
            if accepted < frames {
                break;
            }
        }
    }

    pub fn status(&self) -> PlaybackStatus {
        let track = self.current.as_ref().map(Active::info);
        let duration_ms = track.as_ref().map_or(0, |t| t.duration_ms);
        PlaybackStatus {
            state: self.state,
            track,
            index: self.index,
            queue_len: self.queue.len(),
            position_ms: if self.state == PlayState::Idle && self.current.is_none() {
                if self.frames_played == 0 { 0 } else { self.position_ms() }
            } else {
                self.position_ms()
            },
            duration_ms,
            volume: self.volume,
            muted: self.muted,
            sink: self.sink_name.clone(),
            sink_real: self.sink_real,
            error: self.error.clone(),
        }
    }
}

#[cfg(test)]
impl PlayerCore {
    /// Everything the collecting sink received.
    fn collected(&self) -> Vec<f32> {
        self.sink
            .as_any()
            .downcast_ref::<crate::audio::sink::CollectSink>()
            .expect("test sink")
            .samples
            .clone()
    }

    /// Wait (without decoding) until the prefetch thread delivered the next track.
    fn wait_prefetch(&mut self) {
        let start = Instant::now();
        while self.next.is_none() && start.elapsed() < Duration::from_secs(10) {
            self.poll_prefetch();
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn source_kind(spec: &SourceSpec) -> &'static str {
    match spec {
        SourceSpec::Local(_) => "local",
        SourceSpec::Http { url, .. } if url.contains("/transcode/") => "transcode",
        SourceSpec::Http { .. } => "direct",
    }
}

/// UI-facing handle. Commands are queued; status is polled.
pub struct Engine {
    tx: Sender<Command>,
    status: Arc<Mutex<PlaybackStatus>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Engine {
    pub fn start(cache: AudioCache, http: Http, cfg: EngineConfig) -> Self {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("audio-engine".into())
            .spawn(move || {
                // The output stream is created on this thread and never moves: cpal streams are
                // deliberately not `Send` on all platforms.
                let sink = open_default();
                let mut core = PlayerCore::new(sink, cache, http, cfg);
                let shared = Arc::new(Mutex::new(core.status()));
                let _ = ready_tx.send(shared.clone());
                let mut last_publish = Instant::now();
                loop {
                    let idle = core.state != PlayState::Playing;
                    match rx.recv_timeout(if idle { Duration::from_millis(100) } else { Duration::from_millis(10) }) {
                        Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(cmd) => {
                            core.handle(cmd);
                            last_publish = Instant::now() - Duration::from_secs(1);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    core.pump();
                    if last_publish.elapsed() >= Duration::from_millis(100) {
                        if let Ok(mut s) = shared.lock() {
                            *s = core.status();
                        }
                        last_publish = Instant::now();
                    }
                }
                core.handle(Command::Stop);
            })
            .expect("spawn audio engine");
        let status = ready_rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
        Self { tx, status, thread: Some(thread) }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn play_playlist(&self, tracks: Vec<TrackSpec>, index: usize) {
        self.send(Command::PlayPlaylist(tracks, index));
    }

    pub fn play_pause(&self) {
        self.send(Command::PlayPause);
    }

    pub fn next(&self) {
        self.send(Command::Next);
    }

    pub fn prev(&self) {
        self.send(Command::Prev);
    }

    pub fn seek(&self, ms: u64) {
        self.send(Command::Seek(ms));
    }

    pub fn set_volume(&self, volume: f32) {
        self.send(Command::SetVolume(volume));
    }

    pub fn set_muted(&self, muted: bool) {
        self.send(Command::SetMuted(muted));
    }

    pub fn set_crossfade(&self, ms: u32) {
        self.send(Command::SetCrossfade(ms));
    }

    pub fn set_normalization(&self, on: bool) {
        self.send(Command::SetNormalization(on));
    }

    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    pub fn status(&self) -> PlaybackStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        // Do not join: the app is exiting and the thread may be blocked on a slow download.
        self.thread.take();
    }
}

/// Convenience for tests and for machines without an output device.
pub fn null_core(cache: AudioCache, http: Http, cfg: EngineConfig, rate: u32) -> PlayerCore {
    PlayerCore::new(Box::new(NullSink::new(rate)), cache, http, cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plex::http::{ClientInfo, Http};
    use std::path::PathBuf;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../mockplex/fixtures/audio")
    }

    fn http() -> Http {
        Http::new(ClientInfo::new("test-client"))
    }

    fn cache() -> AudioCache {
        AudioCache::new(std::env::temp_dir().join("plexify-engine-test"), 64 << 20, false)
    }

    fn spec(file: &str, gain_db: Option<f32>) -> TrackSpec {
        TrackSpec {
            id: 1,
            title: file.into(),
            artist: "test".into(),
            album: "test".into(),
            duration_ms: 0,
            gain_db,
            album_gain_db: None,
            source: SourceSpec::Local(fixtures().join(file)),
        }
    }

    fn render_file(file: &str, rate: u32) -> Vec<f32> {
        let mut dec = TrackDecoder::open(Box::new(std::fs::File::open(fixtures().join(file)).unwrap()), Path::new(file).extension().and_then(|e| e.to_str()), rate).unwrap();
        let mut out = Vec::new();
        loop {
            let block = dec.next_block(1024).unwrap();
            if block.is_empty() {
                break;
            }
            out.extend_from_slice(&block);
        }
        out
    }

    #[test]
    fn decodes_every_supported_fixture() {
        for file in ["t01.mp3", "t02.flac", "t03.m4a", "t04.ogg", "t06.wav", "t07_alac.m4a"] {
            let out = render_file(file, 44_100);
            assert!(out.len() > 44_100, "{file} decoded only {} samples", out.len());
            assert!(out.iter().all(|s| s.is_finite()), "{file} produced non-finite samples");
        }
    }

    #[test]
    fn opus_is_reported_as_unsupported() {
        let err = TrackDecoder::open(Box::new(std::fs::File::open(fixtures().join("t05.opus")).unwrap()), Some("opus"), 44_100);
        assert!(err.is_err(), "opus should need the transcoder");
    }

    #[test]
    fn gapless_concatenation_is_sample_exact() {
        let (a, b) = ("gapless_a.flac", "gapless_b.flac");
        let expect_a = render_file(a, 44_100);
        let expect_b = render_file(b, 44_100);
        let mut core = PlayerCore::new(Box::new(crate::audio::sink::CollectSink::new(44_100)), cache(), http(), EngineConfig { crossfade_ms: 0, normalization: false, ..Default::default() });
        core.handle(Command::PlayPlaylist(vec![spec(a, None), spec(b, None)], 0));
        assert_eq!(core.state, PlayState::Playing);
        let mut got = Vec::new();
        let mut spins = 0;
        while core.state == PlayState::Playing {
            spins += 1;
            assert!(spins < 20_000, "engine did not finish");
            core.pump();
            got = core.collected();
        }
        let expected: Vec<f32> = expect_a.iter().chain(expect_b.iter()).copied().collect();
        assert_eq!(got.len(), expected.len(), "gapless output length differs");
        for (i, (g, e)) in got.iter().zip(expected.iter()).enumerate() {
            assert!((g - e).abs() < 1e-5, "sample {i} differs: {g} vs {e}");
        }
    }

    #[test]
    fn crossfade_overlaps_by_the_configured_length() {
        let (a, b) = ("gapless_a.flac", "gapless_b.flac");
        let expect_a = render_file(a, 44_100);
        let expect_b = render_file(b, 44_100);
        let fade_ms = 200u32;
        let fade_frames = (fade_ms as u64 * 44_100 / 1000) as usize;
        let mut core = PlayerCore::new(Box::new(crate::audio::sink::CollectSink::new(44_100)), cache(), http(), EngineConfig { crossfade_ms: fade_ms, normalization: false, ..Default::default() });
        core.handle(Command::PlayPlaylist(vec![spec(a, None), spec(b, None)], 0));
        core.wait_prefetch();
        let mut spins = 0;
        while core.state == PlayState::Playing {
            spins += 1;
            assert!(spins < 20_000, "engine did not finish");
            core.pump();
        }
        let got = core.collected();
        assert_eq!(got.len(), expect_a.len() + expect_b.len() - fade_frames * 2, "overlap length");
        // The first mixed sample is still dominated by the outgoing track...
        let start = expect_a.len() - fade_frames * 2;
        assert!((got[start] - expect_a[start]).abs() < 1e-3, "fade starts at full gain of A");
        // ...and right after the overlap the incoming track plays unmodified.
        for i in 0..8 {
            let idx = expect_a.len() + i;
            assert!((got[idx] - expect_b[fade_frames * 2 + i]).abs() < 1e-5, "next track after the fade, sample {i}");
        }
    }

    #[test]
    fn replay_gain_scales_the_mix() {
        let file = "t08_loud.flac";
        let plain = {
            let mut core = PlayerCore::new(Box::new(crate::audio::sink::CollectSink::new(44_100)), cache(), http(), EngineConfig { normalization: false, ..Default::default() });
            core.handle(Command::PlayPlaylist(vec![spec(file, Some(-6.0206))], 0));
            let mut spins = 0;
            while core.state == PlayState::Playing {
                spins += 1;
                assert!(spins < 20_000);
                core.pump();
            }
            core.collected()
        };
        let gained = {
            let mut core = PlayerCore::new(Box::new(crate::audio::sink::CollectSink::new(44_100)), cache(), http(), EngineConfig { normalization: true, ..Default::default() });
            core.handle(Command::PlayPlaylist(vec![spec(file, Some(-6.0206))], 0));
            let mut spins = 0;
            while core.state == PlayState::Playing {
                spins += 1;
                assert!(spins < 20_000);
                core.pump();
            }
            core.collected()
        };
        assert!(plain.len() > 1000);
        let idx = plain.iter().position(|s| s.abs() > 0.01).expect("signal");
        let ratio = gained[idx] / plain[idx];
        assert!((ratio - 0.5).abs() < 1e-3, "expected -6 dB, got ratio {ratio}");
    }

    #[test]
    fn pause_stops_rendering_and_resume_continues() {
        let mut core = PlayerCore::new(Box::new(crate::audio::sink::CollectSink::new(44_100)), cache(), http(), EngineConfig::default());
        core.handle(Command::PlayPlaylist(vec![spec("t02.flac", None)], 0));
        core.pump();
        let before = core.collected().len();
        assert!(before > 0);
        core.handle(Command::PlayPause);
        assert_eq!(core.state, PlayState::Paused);
        core.pump();
        assert_eq!(core.collected().len(), before, "no audio while paused");
        core.handle(Command::PlayPause);
        assert_eq!(core.state, PlayState::Playing);
        core.pump();
        assert!(core.collected().len() > before, "resume continues");
    }

    #[test]
    fn seek_repositions_within_the_track() {
        let file = "t02.flac";
        let mut core = PlayerCore::new(Box::new(crate::audio::sink::CollectSink::new(44_100)), cache(), http(), EngineConfig::default());
        core.handle(Command::PlayPlaylist(vec![spec(file, None)], 0));
        assert_eq!(core.state, PlayState::Playing, "open failed: {:?}", core.error);
        core.handle(Command::Seek(5_000));
        assert!((core.position_ms() as i64 - 5_000).abs() < 50, "position after seek: {}", core.position_ms());
        core.pump();
        let pos = core.position_ms();
        assert!((5_000..6_500).contains(&pos), "position after seek+pump: {pos}");
    }
}
