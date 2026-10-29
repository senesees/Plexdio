//! Playback engine: a dedicated thread owns the decoder, the mixer and the output sink.
//! The UI talks to it through a command channel and polls a shared [`PlaybackStatus`].
//!
//! The mixer renders sample-exact audio: gapless transitions concatenate the decoders with no
//! inserted silence, and crossfades overlap exactly `crossfade_ms` of equal-power mix.

use super::decode::TrackDecoder;
use super::eq::{Equalizer, BAND_COUNT};
use super::gain::{limit, track_gain};
use super::sink::{default_output_name, open_output, NullSink, Sink};
use super::source::{AudioCache, SourceSpec};
use crate::config::RepeatMode;
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
    /// Relative likelihood of being picked by smart shuffle (rating and play count).
    pub weight: f32,
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

    fn info(&self) -> TrackInfo {
        TrackInfo {
            id: self.id,
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            duration_ms: self.duration_ms,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackInfo {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
}

/// How the playing track reaches the speakers (for the Track info dialog).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    /// Converted by the server's transcoder, at this bitrate cap.
    pub transcode_kbps: Option<u32>,
    /// Played from a file on this computer (a download), not streamed.
    pub local: bool,
    /// Sample rate of the decoded file and of the output device.
    pub source_rate: u32,
    pub out_rate: u32,
    /// File type the decoder was given ("flac", "mp3", ...).
    pub format: Option<String>,
}

impl StreamInfo {
    fn of(a: &Active) -> Self {
        let transcode_kbps = match &a.spec.source {
            SourceSpec::Http { url, .. } if url.contains("/transcode/") => url
                .split(['?', '&'])
                .find_map(|kv| kv.strip_prefix("maxAudioBitrate="))
                .and_then(|v| v.parse().ok()),
            _ => None,
        };
        Self {
            transcode_kbps,
            local: matches!(a.spec.source, SourceSpec::Local(_)),
            source_rate: a.decoder.sample_rate(),
            out_rate: a.decoder.out_rate(),
            format: a.spec.ext_hint().map(str::to_ascii_lowercase),
        }
    }
}

/// One entry of the queue as shown to the UI: the queue slot (`index`) and whether it is the
/// track playing right now.
#[derive(Clone, Debug)]
pub struct QueueEntry {
    pub index: usize,
    pub current: bool,
    pub info: TrackInfo,
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
    pub shuffle: bool,
    pub repeat: RepeatMode,
    /// Queue snapshot in play order, rebuilt only when the queue changes.
    pub queue: Arc<Vec<QueueEntry>>,
    pub queue_gen: u64,
    pub error: Option<String>,
    /// Stream details of the playing track.
    pub stream: Option<StreamInfo>,
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
            shuffle: false,
            repeat: RepeatMode::Off,
            queue: Arc::new(Vec::new()),
            queue_gen: 0,
            error: None,
            stream: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub volume: f32,
    pub muted: bool,
    pub crossfade_ms: u32,
    pub normalization: bool,
    pub eq_enabled: bool,
    /// Gain per equaliser band in dB (see [`super::eq::BANDS`]).
    pub eq_gains: [f32; BAND_COUNT],
    /// Output device name; `None` follows the Windows default.
    pub output_device: Option<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self { volume: 1.0, muted: false, crossfade_ms: 0, normalization: true, eq_enabled: false, eq_gains: [0.0; BAND_COUNT], output_device: None }
    }
}

pub enum Command {
    PlayPlaylist(Vec<TrackSpec>, usize),
    /// Append tracks to the end of the queue.
    Enqueue(Vec<TrackSpec>),
    /// Insert tracks right after the current one.
    InsertNext(Vec<TrackSpec>),
    /// Remove the entry at this position of the play order (the current one is never removed).
    RemoveAt(usize),
    /// Move an entry within the play order.
    MoveInOrder(usize, usize),
    /// Drop everything after the current track.
    ClearUpcoming,
    SetShuffle(bool),
    SetRepeat(RepeatMode),
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
    /// Equaliser on/off and band gains in dB.
    SetEq(bool, [f32; BAND_COUNT]),
    /// Switch output to the named device; `None` follows the Windows default.
    SetOutputDevice(Option<String>),
    /// Replace the queue and load the given queue slot at `position_ms`, paused (session resume).
    Restore(Vec<TrackSpec>, usize, u64),
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
        self.spec.info()
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
    /// Play order: a permutation of queue slots. Identity when shuffle is off.
    order: Vec<usize>,
    /// Position within `order` of the track playing (or about to play).
    pos: usize,
    shuffle: bool,
    repeat: RepeatMode,
    rng: Rng,
    /// Queue cache for the UI; rebuilt only when something changes.
    snapshot: Arc<Vec<QueueEntry>>,
    snapshot_gen: u64,
    snapshot_dirty: bool,
    current: Option<Active>,
    /// Prefetched next track and the play-order position it belongs to.
    next: Option<Active>,
    next_pos: Option<usize>,
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
    eq: Equalizer,
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
            order: Vec::new(),
            pos: 0,
            shuffle: false,
            repeat: RepeatMode::Off,
            rng: Rng(seed()),
            snapshot: Arc::new(Vec::new()),
            snapshot_gen: 0,
            snapshot_dirty: false,
            current: None,
            next: None,
            next_pos: None,
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
            eq: Equalizer::new(rate),
        };
        core.eq.set(cfg.eq_enabled, cfg.eq_gains);
        core.apply_volume();
        core
    }

    pub fn sink_rate(&self) -> u32 {
        self.rate
    }

    /// True when the output device went away and should be reopened.
    pub fn sink_lost(&self) -> bool {
        self.sink.lost()
    }

    /// Move playback to another output. Audio still queued on the old device is dropped; when
    /// the sample rate differs the current track is reopened at the position heard last.
    pub fn replace_sink(&mut self, sink: Box<dyn Sink>) {
        let position_ms = self.position_ms();
        self.sink.flush();
        self.sink = sink;
        self.sink_name = self.sink.name();
        self.sink_real = self.sink.is_real();
        let rate = self.sink.sample_rate();
        if rate != self.rate {
            self.rate = rate;
            self.eq.set_rate(rate);
            self.next = None;
            self.next_pos = None;
            self.prefetch = None;
            if let Some(cur) = self.current.take() {
                match open_active(&self.cache, &self.http, &cur.spec, rate, self.normalization, self.crossfade_ms) {
                    Ok(mut active) => {
                        let frames = position_ms.saturating_mul(rate as u64) / 1000;
                        if let Err(e) = active.decoder.seek(position_ms) {
                            log::warn!("cannot restore position after output change: {e}");
                        } else {
                            active.frames_left = active.total_frames.map(|t| t.saturating_sub(frames));
                        }
                        self.current = Some(active);
                        self.frames_played = frames;
                    }
                    Err(e) => {
                        self.fail(format!("Cannot play “{}”: {e}", cur.spec.title));
                    }
                }
            }
        }
        self.apply_volume();
        if self.state == PlayState::Paused {
            self.sink.pause();
        }
        self.spawn_prefetch();
    }

    fn apply_volume(&mut self) {
        let v = if self.muted { 0.0 } else { self.volume };
        self.sink.set_volume(v);
    }

    pub fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::PlayPlaylist(tracks, index) => self.play_playlist(tracks, index),
            Command::Enqueue(tracks) => self.enqueue(tracks),
            Command::InsertNext(tracks) => self.insert_next(tracks),
            Command::RemoveAt(order_pos) => self.remove_at(order_pos),
            Command::MoveInOrder(from, to) => self.move_in_order(from, to),
            Command::ClearUpcoming => self.clear_upcoming(),
            Command::SetShuffle(on) => self.set_shuffle(on),
            Command::SetRepeat(mode) => self.set_repeat(mode),
            Command::Restore(tracks, index, position_ms) => self.restore(tracks, index, position_ms),
            Command::PlayPause => match self.state {
                PlayState::Playing => self.pause(),
                PlayState::Paused => self.resume(),
                PlayState::Idle => {
                    if !self.order.is_empty() {
                        self.start_pos(self.pos.min(self.order.len() - 1));
                    }
                }
            },
            Command::Pause => self.pause(),
            Command::Resume => self.resume(),
            Command::Next => match self.manual_next_pos() {
                Some(p) => self.start_pos(p),
                None => self.finish_queue(),
            },
            Command::Prev => {
                // Within the first seconds, go to the previous track; otherwise restart this one.
                if self.position_ms() > 3_000 || self.pos == 0 {
                    if let Some(cur) = self.current.as_mut() {
                        let _ = cur.decoder.seek(0);
                        cur.frames_left = cur.total_frames;
                        cur.fade_pos = 0;
                        self.frames_played = 0;
                        self.sink.flush();
                    }
                } else {
                    self.start_pos(self.pos - 1);
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
            Command::SetEq(on, gains) => self.eq.set(on, gains),
            // Opening devices happens on the engine thread loop, which owns the output stream.
            Command::SetOutputDevice(_) => {}
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
        self.order = (0..self.queue.len()).collect();
        self.pos = index.min(self.queue.len().saturating_sub(1));
        if self.shuffle && !self.order.is_empty() {
            // Keep the requested track first, then shuffle the rest.
            let mut rest = self.order.clone();
            let start = rest.remove(self.pos);
            weighted_shuffle(&mut rest, &self.queue, &mut self.rng);
            rest.insert(0, start);
            self.order = rest;
            self.pos = 0;
        }
        self.reset_playback();
        if self.queue.is_empty() {
            self.state = PlayState::Idle;
            self.snapshot_dirty = true;
            return;
        }
        self.start_pos(self.pos);
    }

    /// Replace the queue and load a paused track at a saved position (session resume).
    fn restore(&mut self, tracks: Vec<TrackSpec>, index: usize, position_ms: u64) {
        self.play_playlist(tracks, index);
        let Some(cur) = self.current.as_mut() else { return };
        if let Err(e) = cur.decoder.seek(position_ms) {
            log::warn!("cannot resume at {position_ms} ms: {e}");
            return;
        }
        let frames = position_ms.saturating_mul(self.rate as u64) / 1000;
        cur.frames_left = cur.total_frames.map(|t| t.saturating_sub(frames));
        cur.fade_pos = 0;
        self.frames_played = frames;
        self.sink.flush();
        self.pause();
    }

    fn reset_playback(&mut self) {
        self.next = None;
        self.next_pos = None;
        self.prefetch = None;
        self.current = None;
        self.error = None;
        self.sink.flush();
        self.snapshot_dirty = true;
    }

    fn enqueue(&mut self, tracks: Vec<TrackSpec>) {
        if tracks.is_empty() {
            return;
        }
        let base = self.queue.len();
        // Adding to an idle player (nothing loaded or the queue ran out) starts playing.
        let start = (self.current.is_none() && self.state == PlayState::Idle).then_some(base);
        self.queue.extend(tracks);
        self.order.extend(base..self.queue.len());
        self.snapshot_dirty = true;
        if let Some(pos) = start {
            self.start_pos(pos);
        } else {
            self.spawn_prefetch();
        }
    }

    fn insert_next(&mut self, tracks: Vec<TrackSpec>) {
        if tracks.is_empty() {
            return;
        }
        let base = self.queue.len();
        let idle = self.current.is_none() && self.state == PlayState::Idle;
        self.queue.extend(tracks);
        let at = if self.order.is_empty() { 0 } else { self.pos + 1 };
        let inserted: Vec<usize> = (base..self.queue.len()).collect();
        self.order.splice(at..at, inserted);
        // The prefetched track is no longer next in line.
        self.next = None;
        self.next_pos = None;
        self.prefetch = None;
        self.snapshot_dirty = true;
        if idle {
            self.start_pos(at);
        } else {
            self.spawn_prefetch();
        }
    }

    fn remove_at(&mut self, order_pos: usize) {
        if order_pos >= self.order.len() || order_pos == self.pos {
            return;
        }
        let slot = self.order.remove(order_pos);
        self.queue.remove(slot);
        for q in &mut self.order {
            if *q > slot {
                *q -= 1;
            }
        }
        if order_pos < self.pos {
            self.pos -= 1;
        }
        self.snapshot_dirty = true;
        // A removal can invalidate the prefetched track.
        self.next = None;
        self.next_pos = None;
        self.prefetch = None;
        if self.state == PlayState::Playing {
            self.spawn_prefetch();
        }
    }

    fn move_in_order(&mut self, from: usize, to: usize) {
        if from >= self.order.len() || to >= self.order.len() || from == to || from == self.pos || to == self.pos {
            return;
        }
        let item = self.order.remove(from);
        self.order.insert(to, item);
        // Keep `pos` on the same slot.
        if from < self.pos && to >= self.pos {
            self.pos -= 1;
        } else if from > self.pos && to <= self.pos {
            self.pos += 1;
        }
        self.snapshot_dirty = true;
        self.next = None;
        self.next_pos = None;
        self.prefetch = None;
        if self.state == PlayState::Playing {
            self.spawn_prefetch();
        }
    }

    fn clear_upcoming(&mut self) {
        if self.pos + 1 >= self.order.len() {
            return;
        }
        let drop_from = self.pos + 1;
        let dropped: std::collections::HashSet<usize> = self.order.drain(drop_from..).collect();
        self.queue = std::mem::take(&mut self.queue)
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !dropped.contains(i))
            .map(|(_, s)| s)
            .collect();
        // Reindex the remaining slots (they are all before the dropped ones in play order).
        let mut remap = std::collections::HashMap::new();
        let mut next = 0usize;
        for i in 0..self.order.len() + dropped.len() {
            if !dropped.contains(&i) {
                remap.insert(i, next);
                next += 1;
            }
        }
        for q in &mut self.order {
            *q = remap[&*q];
        }
        self.next = None;
        self.next_pos = None;
        self.prefetch = None;
        self.snapshot_dirty = true;
    }

    fn set_shuffle(&mut self, on: bool) {
        if self.shuffle == on {
            return;
        }
        self.shuffle = on;
        let cur = self.cur_slot();
        if on {
            let mut rest: Vec<usize> = (0..self.queue.len()).filter(|i| Some(*i) != cur).collect();
            weighted_shuffle(&mut rest, &self.queue, &mut self.rng);
            let mut order = Vec::with_capacity(self.queue.len());
            if let Some(c) = cur {
                order.push(c);
            }
            order.extend(rest);
            self.order = order;
            self.pos = 0;
        } else {
            self.order = (0..self.queue.len()).collect();
            self.pos = cur.unwrap_or(0);
        }
        self.snapshot_dirty = true;
        self.next = None;
        self.next_pos = None;
        self.prefetch = None;
        self.spawn_prefetch();
    }

    fn set_repeat(&mut self, mode: RepeatMode) {
        self.repeat = mode;
    }

    /// Queue slot of the current track.
    fn cur_slot(&self) -> Option<usize> {
        self.order.get(self.pos).copied()
    }

    /// Next position when the current track ends by itself (repeat-aware).
    fn auto_next_pos(&self) -> Option<usize> {
        if self.repeat == RepeatMode::One {
            return Some(self.pos);
        }
        if self.pos + 1 < self.order.len() {
            Some(self.pos + 1)
        } else if self.repeat == RepeatMode::All && !self.order.is_empty() {
            Some(0)
        } else {
            None
        }
    }

    /// Next position for the Next button: repeat-one does not trap the user on one track.
    fn manual_next_pos(&self) -> Option<usize> {
        if self.pos + 1 < self.order.len() {
            Some(self.pos + 1)
        } else if self.repeat != RepeatMode::Off && !self.order.is_empty() {
            Some(0)
        } else {
            None
        }
    }

    fn start_pos(&mut self, pos: usize) {
        if pos >= self.order.len() {
            self.finish_queue();
            return;
        }
        self.pos = pos;
        self.next = None;
        self.next_pos = None;
        self.prefetch = None;
        self.error = None;
        self.snapshot_dirty = true;
        let slot = self.order[pos];
        let spec = self.queue[slot].clone();
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
        if self.prefetch.is_some() || self.next.is_some() || self.state != PlayState::Playing {
            return;
        }
        let Some(next_pos) = self.auto_next_pos() else { return };
        let Some(slot) = self.order.get(next_pos).copied() else { return };
        let spec = self.queue[slot].clone();
        let (tx, rx) = mpsc::channel();
        let cache = self.cache.clone();
        let http = self.http.clone();
        let rate = self.rate;
        let normalization = self.normalization;
        let crossfade = self.crossfade_ms;
        log::debug!("prefetching #{} {}", next_pos, spec.title);
        let spawned = std::thread::Builder::new().name("audio-prefetch".into()).spawn(move || {
            let _ = tx.send(open_active(&cache, &http, &spec, rate, normalization, crossfade));
        });
        if spawned.is_ok() {
            self.prefetch = Some((next_pos, rx));
        }
    }

    fn poll_prefetch(&mut self) {
        let Some((next_pos, rx)) = self.prefetch.take() else { return };
        match rx.try_recv() {
            Ok(Ok(active)) => {
                if Some(next_pos) == self.auto_next_pos() && self.next.is_none() {
                    log::debug!("prefetch ready #{} {}", next_pos, active.spec.title);
                    self.next = Some(active);
                    self.next_pos = Some(next_pos);
                } else {
                    log::debug!("dropping stale prefetch #{} (now #{})", next_pos, self.pos);
                }
            }
            Ok(Err(e)) => log::warn!("prefetch failed: {e}"),
            Err(TryRecvError::Empty) => self.prefetch = Some((next_pos, rx)),
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
        if let (Some(next), Some(next_pos)) = (self.next.take(), self.next_pos.take()) {
            self.current = Some(next);
            self.pos = next_pos;
            self.snapshot_dirty = true;
            if let Some(cur) = self.current.as_ref() {
                log::debug!("gapless/crossfade into {} — {}", cur.spec.title, cur.spec.artist);
            }
            self.spawn_prefetch();
            return Ok(());
        }
        let Some(next_pos) = self.auto_next_pos() else {
            self.finish_queue();
            return Ok(());
        };
        self.pos = next_pos;
        let slot = self.order[next_pos];
        let spec = self.queue[slot].clone();
        match open_active(&self.cache, &self.http, &spec, self.rate, self.normalization, self.crossfade_ms) {
            Ok(active) => {
                log::info!("playing {} — {}", active.spec.title, active.spec.artist);
                self.current = Some(active);
                self.frames_played = 0;
                self.snapshot_dirty = true;
                self.spawn_prefetch();
                Ok(())
            }
            Err(e) => {
                self.fail(format!("Cannot play “{}”: {e}", spec.title));
                Ok(())
            }
        }
    }

    fn finish_queue(&mut self) {
        log::info!("queue finished");
        self.current = None;
        self.next = None;
        self.next_pos = None;
        self.prefetch = None;
        self.state = PlayState::Idle;
        self.frames_played = 0;
        self.snapshot_dirty = true;
    }

    fn fail(&mut self, message: String) {
        log::error!("{message}");
        self.error = Some(message);
        self.current = None;
        self.next = None;
        self.next_pos = None;
        self.prefetch = None;
        self.state = PlayState::Idle;
        self.snapshot_dirty = true;
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
            self.spawn_prefetch();
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
            let mut block = match self.produce(BLOCK_FRAMES) {
                Ok(b) => b,
                Err(e) => {
                    self.fail(e);
                    return;
                }
            };
            if block.is_empty() {
                break;
            }
            self.eq.process(&mut block);
            let frames = block.len() / 2;
            let accepted = self.sink.push(&block);
            self.frames_played += accepted as u64;
            if accepted < frames {
                break;
            }
        }
    }

    /// Rebuild the queue snapshot when something changed.
    fn refresh_snapshot(&mut self) {
        if !self.snapshot_dirty {
            return;
        }
        self.snapshot_dirty = false;
        let current = self.cur_slot();
        let entries: Vec<QueueEntry> = self
            .order
            .iter()
            .map(|&slot| QueueEntry {
                index: slot,
                current: Some(slot) == current,
                info: self.queue[slot].info(),
            })
            .collect();
        self.snapshot = Arc::new(entries);
        self.snapshot_gen += 1;
    }

    pub fn status(&mut self) -> PlaybackStatus {
        self.refresh_snapshot();
        let track = self.current.as_ref().map(Active::info);
        let duration_ms = track.as_ref().map_or(0, |t| t.duration_ms);
        PlaybackStatus {
            state: self.state,
            track,
            index: self.cur_slot().unwrap_or(0),
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
            shuffle: self.shuffle,
            repeat: self.repeat,
            queue: self.snapshot.clone(),
            queue_gen: self.snapshot_gen,
            error: self.error.clone(),
            stream: self.current.as_ref().map(StreamInfo::of),
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

/// xorshift64* seed: time-based so successive sessions differ.
fn seed() -> u64 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x2545_f491_4f6c_dd1d);
    t ^ 0x9e37_79b9_7f4a_7c15
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in [0, total).
    fn below(&mut self, total: f64) -> f64 {
        (self.next() as f64 / u64::MAX as f64) * total
    }
}

/// Weighted shuffle without replacement: tracks rated higher or played often are more likely to
/// come next, but a track by the same artist as the previous pick is strongly deweighted so the
/// mix stays varied. `items` holds queue slots and is permuted in place.
fn weighted_shuffle(items: &mut [usize], specs: &[TrackSpec], rng: &mut Rng) {
    let mut last_artist: Option<String> = None;
    for i in 0..items.len() {
        let mut total = 0.0f64;
        let mut weights = Vec::with_capacity(items.len() - i);
        for &slot in &items[i..] {
            let s = &specs[slot];
            let mut w = (s.weight.max(0.05)) as f64;
            if last_artist.as_deref() == Some(s.artist.as_str()) {
                w *= 0.2;
            }
            weights.push(w);
            total += w;
        }
        let mut pick = i;
        if total > 0.0 {
            let mut target = rng.below(total);
            for (k, w) in weights.iter().enumerate() {
                target -= w;
                if target < 0.0 {
                    pick = i + k;
                    break;
                }
            }
        }
        items.swap(i, pick);
        last_artist = Some(specs[items[i]].artist.clone());
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
                let mut output = cfg.output_device.clone();
                let mut default_name = default_output_name();
                let sink = open_output(output.as_deref());
                let mut core = PlayerCore::new(sink, cache, http, cfg);
                let shared = Arc::new(Mutex::new(core.status()));
                let _ = ready_tx.send(shared.clone());
                let mut last_publish = Instant::now();
                let mut last_device_check = Instant::now();
                loop {
                    let idle = core.state != PlayState::Playing;
                    match rx.recv_timeout(if idle { Duration::from_millis(100) } else { Duration::from_millis(10) }) {
                        Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(Command::SetOutputDevice(device)) => {
                            log::info!("output device set to {}", device.as_deref().unwrap_or("Windows default"));
                            output = device;
                            default_name = default_output_name();
                            core.replace_sink(open_output(output.as_deref()));
                            last_publish = Instant::now() - Duration::from_secs(1);
                        }
                        Ok(cmd) => {
                            core.handle(cmd);
                            last_publish = Instant::now() - Duration::from_secs(1);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    // cpal stays on the device it opened, so follow Windows default changes and
                    // recover from unplugged devices here.
                    if last_device_check.elapsed() >= Duration::from_secs(1) {
                        last_device_check = Instant::now();
                        let lost = core.sink_lost();
                        let current_default = if output.is_none() || lost { default_output_name() } else { default_name.clone() };
                        let default_changed = output.is_none() && current_default != default_name;
                        if lost || default_changed {
                            log::info!("audio output changed (default now {}); reopening", current_default.as_deref().unwrap_or("none"));
                            default_name = current_default;
                            core.replace_sink(open_output(output.as_deref()));
                            last_publish = Instant::now() - Duration::from_secs(1);
                        }
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

    pub fn enqueue(&self, tracks: Vec<TrackSpec>) {
        self.send(Command::Enqueue(tracks));
    }

    pub fn insert_next(&self, tracks: Vec<TrackSpec>) {
        self.send(Command::InsertNext(tracks));
    }

    pub fn remove_at(&self, order_pos: usize) {
        self.send(Command::RemoveAt(order_pos));
    }

    pub fn move_in_order(&self, from: usize, to: usize) {
        self.send(Command::MoveInOrder(from, to));
    }

    pub fn clear_upcoming(&self) {
        self.send(Command::ClearUpcoming);
    }

    pub fn set_shuffle(&self, on: bool) {
        self.send(Command::SetShuffle(on));
    }

    pub fn set_repeat(&self, mode: RepeatMode) {
        self.send(Command::SetRepeat(mode));
    }

    pub fn restore(&self, tracks: Vec<TrackSpec>, index: usize, position_ms: u64) {
        self.send(Command::Restore(tracks, index, position_ms));
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

    pub fn set_eq(&self, on: bool, gains: [f32; BAND_COUNT]) {
        self.send(Command::SetEq(on, gains));
    }

    pub fn set_output_device(&self, device: Option<String>) {
        self.send(Command::SetOutputDevice(device));
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
        // Unique per run so parallel test binaries never share a cache directory. Nothing here is
        // a security decision: it only holds decoded audio written by this test process.
        // nosemgrep: rust.lang.security.temp-dir.temp-dir
        let dir = std::env::temp_dir().join(format!("plexdio-engine-test-{}", std::process::id()));
        AudioCache::new(dir, 64 << 20, false)
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
            weight: 1.0,
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

    fn many_specs(n: usize) -> Vec<TrackSpec> {
        (0..n)
            .map(|i| {
                let mut s = spec("gapless_a.flac", None);
                s.id = i as i64 + 1;
                s.title = format!("track {i}");
                s.artist = format!("artist {}", i % 3);
                s.weight = 1.0 + i as f32 % 4.0;
                s
            })
            .collect()
    }

    /// Queue snapshot as (slot, title), in play order.
    fn order_of(core: &mut PlayerCore) -> Vec<(usize, String)> {
        core.status().queue.iter().map(|e| (e.index, e.info.title.clone())).collect()
    }

    #[test]
    fn queue_editing_changes_the_play_order() {
        let mut core = PlayerCore::new(Box::new(crate::audio::sink::CollectSink::new(44_100)), cache(), http(), EngineConfig::default());
        core.handle(Command::PlayPlaylist(many_specs(3), 0));
        core.handle(Command::Enqueue(many_specs(1).into_iter().map(|mut s| { s.title = "appended".into(); s }).collect()));
        core.handle(Command::InsertNext(vec![{
            let mut s = spec("gapless_a.flac", None);
            s.title = "next".into();
            s
        }]));
        let titles: Vec<String> = order_of(&mut core).into_iter().map(|(_, t)| t).collect();
        assert_eq!(titles.iter().map(String::as_str).collect::<Vec<_>>(), ["track 0", "next", "track 1", "track 2", "appended"]);

        // Remove the inserted track (position 1 in play order).
        core.handle(Command::RemoveAt(1));
        assert_eq!(order_of(&mut core).len(), 4, "removed one");
        assert!(order_of(&mut core).iter().all(|(_, t)| t != "next"));

        // Moving an upcoming track must not disturb the current one.
        let before = order_of(&mut core);
        core.handle(Command::MoveInOrder(3, 1));
        let after = order_of(&mut core);
        assert_eq!(after[0].1, before[0].1, "current stayed first");
        assert_eq!(after[1].1, before[3].1, "moved up");
        assert_eq!(after.len(), before.len());

        core.handle(Command::ClearUpcoming);
        let titles: Vec<String> = order_of(&mut core).into_iter().map(|(_, t)| t).collect();
        assert_eq!(titles, ["track 0"], "only the current track remains");
    }

    #[test]
    fn queueing_into_an_idle_engine_starts_playback() {
        let mut core = PlayerCore::new(Box::new(crate::audio::sink::CollectSink::new(44_100)), cache(), http(), EngineConfig::default());
        core.handle(Command::InsertNext(vec![spec("gapless_a.flac", None)]));
        assert_eq!(core.state, PlayState::Playing, "play next with nothing loaded starts");
        assert_eq!(core.cur_slot(), Some(0));

        let mut core = PlayerCore::new(Box::new(crate::audio::sink::CollectSink::new(44_100)), cache(), http(), EngineConfig::default());
        core.handle(Command::Enqueue(many_specs(2)));
        assert_eq!(core.state, PlayState::Playing, "adding to an empty queue starts");
        assert_eq!(core.queue.len(), 2);
        assert_eq!(core.cur_slot(), Some(0));
    }

    #[test]
    fn repeat_all_wraps_and_repeat_one_repeats() {
        let mut core = PlayerCore::new(Box::new(crate::audio::sink::CollectSink::new(44_100)), cache(), http(), EngineConfig::default());
        core.handle(Command::PlayPlaylist(many_specs(2), 1));
        assert_eq!(core.auto_next_pos(), None, "end of queue, repeat off");
        assert_eq!(core.manual_next_pos(), None);
        core.handle(Command::SetRepeat(RepeatMode::All));
        assert_eq!(core.auto_next_pos(), Some(0), "wraps around");
        core.handle(Command::SetRepeat(RepeatMode::One));
        assert_eq!(core.auto_next_pos(), Some(1), "auto repeats the same track");
        assert_eq!(core.manual_next_pos(), Some(0), "manual next still moves on");
        core.handle(Command::SetRepeat(RepeatMode::Off));
    }

    #[test]
    fn shuffle_keeps_the_current_track_first_and_loses_nothing() {
        let mut core = PlayerCore::new(Box::new(crate::audio::sink::CollectSink::new(44_100)), cache(), http(), EngineConfig::default());
        core.handle(Command::PlayPlaylist(many_specs(12), 0));
        core.handle(Command::SetShuffle(true));
        let order = order_of(&mut core);
        assert_eq!(order.len(), 12);
        assert_eq!(order[0].1, "track 0", "the playing track stays first");
        let mut slots: Vec<usize> = order.iter().map(|(s, _)| *s).collect();
        slots.sort_unstable();
        assert_eq!(slots, (0..12).collect::<Vec<_>>(), "every track appears exactly once");
        assert!(core.status().shuffle);

        // Going back to sequential order restores the original sequence.
        core.handle(Command::SetShuffle(false));
        let titles: Vec<String> = order_of(&mut core).into_iter().map(|(_, t)| t).collect();
        assert_eq!(titles.iter().map(String::as_str).collect::<Vec<_>>(), (0..12).map(|i| format!("track {i}")).collect::<Vec<_>>());
        assert_eq!(order_of(&mut core)[0].0, 0, "position follows the current slot");
    }

    #[test]
    fn restore_loads_the_saved_position_paused() {
        let mut core = PlayerCore::new(Box::new(crate::audio::sink::CollectSink::new(44_100)), cache(), http(), EngineConfig::default());
        core.handle(Command::Restore(many_specs(3), 1, 2_000));
        assert_eq!(core.state, PlayState::Paused, "resume starts paused");
        assert_eq!(core.status().track.map(|t| t.title), Some("track 1".into()));
        assert!((core.position_ms() as i64 - 2_000).abs() < 100, "position {}", core.position_ms());
        core.handle(Command::Resume);
        core.pump();
        assert!(!core.collected().is_empty(), "resume plays from the saved position");
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
