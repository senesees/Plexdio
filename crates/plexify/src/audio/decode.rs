//! Symphonia-based decoding to interleaved stereo f32 at the output device rate.
//!
//! Formats we can decode directly: MP3, FLAC, AAC/M4A, ALAC, Ogg Vorbis, WAV/PCM.
//! Opus is not supported by Symphonia, so the server transcodes those tracks to MP3.

use std::collections::VecDeque;
use std::fmt;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use rubato::Resampler;
use symphonia::core::units::Time;

#[derive(Debug, Clone)]
pub struct DecodeError(pub String);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DecodeError {}

impl From<SymError> for DecodeError {
    fn from(e: SymError) -> Self {
        DecodeError(e.to_string())
    }
}

/// Resamples stereo f32 from the source rate to the sink rate with a windowed-sinc filter.
struct RateConverter {
    rs: rubato::SincFixedIn<f32>,
    input: Vec<f32>,
    output: VecDeque<f32>,
}

impl RateConverter {
    fn new(from: u32, to: u32) -> Result<Self, DecodeError> {
        use rubato::{SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
        let params = SincInterpolationParameters {
            sinc_len: 64,
            f_cutoff: 0.94,
            oversampling_factor: 64,
            interpolation: SincInterpolationType::Linear,
            window: WindowFunction::BlackmanHarris2,
        };
        let rs = SincFixedIn::<f32>::new(to as f64 / from as f64, 1.1, params, 1024, 2)
            .map_err(|e| DecodeError(format!("resampler: {e}")))?;
        Ok(Self { rs, input: Vec::with_capacity(4096), output: VecDeque::new() })
    }

    /// Feed interleaved stereo frames; converted output is appended to `self.output`.
    fn push(&mut self, frames: &[f32]) -> Result<(), DecodeError> {
        self.input.extend_from_slice(frames);
        let need = self.rs.input_frames_next();
        while self.input.len() >= need * 2 {
            let chunk: Vec<f32> = self.input.drain(..need * 2).collect();
            self.run(&chunk, false)?;
        }
        Ok(())
    }

    /// Convert whatever is left (end of track). Small tails are dropped by the filter.
    fn finish(&mut self) -> Result<(), DecodeError> {
        let frames = self.input.len() / 2;
        if frames == 0 {
            return Ok(());
        }
        let chunk = std::mem::take(&mut self.input);
        self.run(&chunk, true)
    }

    fn run(&mut self, chunk: &[f32], partial: bool) -> Result<(), DecodeError> {
        let frames = chunk.len() / 2;
        if frames == 0 {
            return Ok(());
        }
        let (pairs, _) = chunk.as_chunks::<2>();
        let l: Vec<f32> = pairs.iter().map(|p| p[0]).collect();
        let r: Vec<f32> = pairs.iter().map(|p| p[1]).collect();
        let out = if partial {
            self.rs.process_partial(Some(&[l, r]), None)
        } else {
            self.rs.process(&[l, r], None)
        }
        .map_err(|e| DecodeError(format!("resample: {e}")))?;
        for (a, b) in out[0].iter().zip(out[1].iter()) {
            self.output.push_back(*a);
            self.output.push_back(*b);
        }
        Ok(())
    }
}

/// Decodes one audio file (local or streamed) into interleaved stereo `f32` frames.
pub struct TrackDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    in_rate: u32,
    out_rate: u32,
    converter: Option<RateConverter>,
    pending: VecDeque<f32>,
    scratch: Option<SampleBuffer<f32>>,
    eof: bool,
    /// Total frames at `out_rate` if the container reports a duration.
    total_frames: Option<u64>,
    /// Milliseconds consumed from the source so far (seek target bookkeeping).
    offset_ms: u64,
    /// Frames handed to us by the codec since the last seek.
    frames_in: u64,
}

impl TrackDecoder {
    /// Open a media source. `ext_hint` is the file extension when known (e.g. "flac"); it helps
    /// probing streams without a name. Output is interleaved stereo at `out_rate`.
    pub fn open(source: Box<dyn MediaSource>, ext_hint: Option<&str>, out_rate: u32) -> Result<Self, DecodeError> {
        let mss = MediaSourceStream::new(source, MediaSourceStreamOptions::default());
        let mut hint = Hint::new();
        if let Some(ext) = ext_hint {
            hint.with_extension(ext);
        }
        let format_opts = FormatOptions { enable_gapless: true, ..Default::default() };
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &format_opts, &MetadataOptions::default())
            .map_err(|e| {
                log::debug!("probe failed: {e:?}");
                DecodeError(format!("unsupported or corrupt audio: {e}"))
            })?;
        let format = probed.format;
        let track = format
            .default_track()
            .filter(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| DecodeError("no decodable audio track".into()))?;
        let track_id = track.id;
        let in_rate = track.codec_params.sample_rate.unwrap_or(44_100);
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| DecodeError(format!("no decoder for this codec: {e}")))?;
        let total_frames = track.codec_params.n_frames.map(|n| n.saturating_mul(out_rate as u64) / in_rate as u64);
        let converter = if in_rate == out_rate { None } else { Some(RateConverter::new(in_rate, out_rate)?) };
        Ok(Self {
            format,
            decoder,
            track_id,
            in_rate,
            out_rate,
            converter,
            pending: VecDeque::new(),
            scratch: None,
            eof: false,
            total_frames,
            offset_ms: 0,
            frames_in: 0,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.in_rate
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    pub fn total_frames(&self) -> Option<u64> {
        self.total_frames
    }

    pub fn is_eof(&self) -> bool {
        self.eof && self.pending.is_empty()
    }

    /// Decode up to `max_frames` stereo frames. Returns an empty vector at end of stream.
    pub fn next_block(&mut self, max_frames: usize) -> Result<Vec<f32>, DecodeError> {
        let want = max_frames * 2;
        let mut errors = 0;
        while self.pending.len() < want && !self.eof {
            match self.decode_one() {
                Ok(true) => errors = 0,
                Ok(false) => {}
                Err(e) => {
                    errors += 1;
                    if errors > 8 {
                        return Err(e);
                    }
                }
            }
        }
        let take = want.min(self.pending.len());
        Ok(self.pending.drain(..take).collect())
    }

    /// Decode one packet into `self.pending`. Returns Ok(false) if the packet was skipped.
    fn decode_one(&mut self) -> Result<bool, DecodeError> {
        let packet = match self.format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                self.finish();
                return Ok(false);
            }
            Err(SymError::ResetRequired) => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != self.track_id || packet.dur() == 0 {
            return Ok(false);
        }
        let decoded = match self.decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(e)) => {
                log::debug!("skipping bad audio packet: {e}");
                return Ok(false);
            }
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                self.finish();
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        let spec = *decoded.spec();
        let frames = decoded.frames();
        if frames == 0 {
            return Ok(false);
        }
        self.frames_in += frames as u64;
        let scratch = match self.scratch.as_mut() {
            Some(s) if s.capacity() >= frames * spec.channels.count() => s,
            _ => {
                // Rebuild when the codec changes channel layout mid-stream.
                self.scratch = Some(SampleBuffer::<f32>::new(frames as u64, spec));
                self.scratch.as_mut().expect("just set")
            }
        };
        scratch.copy_interleaved_ref(decoded);
        let src = scratch.samples();
        let ch = spec.channels.count();
        let mut stereo = Vec::with_capacity(frames * 2);
        if ch == 1 {
            for s in src.iter().take(frames) {
                stereo.push(*s);
                stereo.push(*s);
            }
        } else {
            for f in src.chunks_exact(ch).take(frames) {
                stereo.push(f[0]);
                stereo.push(f[1]);
            }
        }
        match self.converter.as_mut() {
            Some(c) => {
                c.push(&stereo)?;
                while let Some(s) = c.output.pop_front() {
                    self.pending.push_back(s);
                }
            }
            None => self.pending.extend(stereo),
        }
        Ok(true)
    }

    fn finish(&mut self) {
        if self.eof {
            return;
        }
        self.eof = true;
        if let Some(c) = self.converter.as_mut() {
            if let Err(e) = c.finish() {
                log::debug!("resampler flush failed: {e}");
            }
            while let Some(s) = c.output.pop_front() {
                self.pending.push_back(s);
            }
        }
    }

    /// Seek to `position_ms` (source timeline). Drops buffered output.
    pub fn seek(&mut self, position_ms: u64) -> Result<(), DecodeError> {
        let to = SeekTo::Time { time: Time::new(position_ms / 1000, (position_ms % 1000) as f64 / 1000.0), track_id: Some(self.track_id) };
        self.format.seek(SeekMode::Accurate, to)?;
        self.decoder.reset();
        self.pending.clear();
        self.eof = false;
        self.frames_in = 0;
        self.offset_ms = position_ms;
        if let Some(c) = self.converter.as_mut() {
            c.input.clear();
            c.output.clear();
        }
        Ok(())
    }

    /// Source position of the decoder (it runs ahead of playback by the buffered frames).
    pub fn position_ms(&self) -> u64 {
        let in_rate = self.in_rate.max(1) as u64;
        self.offset_ms + self.frames_in.saturating_mul(1000) / in_rate
    }
}
