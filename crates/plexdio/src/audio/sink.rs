//! Output sinks. The real sink is WASAPI through cpal; tests and machines without an output
//! device use [`NullSink`], which drains at real time but produces no sound.

/// Stereo interleaved output target.
pub trait Sink {
    /// Output sample rate in Hz.
    fn sample_rate(&self) -> u32;
    /// Human-readable description for logs and the UI.
    fn name(&self) -> String;
    /// True when this sink actually reaches a speaker.
    fn is_real(&self) -> bool;
    /// Downcast hook (tests inspect the collecting sink).
    fn as_any(&self) -> &dyn std::any::Any;
    /// Queue interleaved stereo samples. Returns the number of *frames* accepted (may be short
    /// when the device ring is full).
    fn push(&mut self, samples: &[f32]) -> usize;
    /// Frames queued but not yet played.
    fn buffered_frames(&self) -> usize;
    fn pause(&mut self);
    fn resume(&mut self);
    fn set_volume(&mut self, volume: f32);
    fn flush(&mut self);
    /// True once the device went away (unplugged, disabled); the engine then reopens output.
    fn lost(&self) -> bool {
        false
    }
}

/// Discards audio, but keeps a real-time paced virtual ring so the engine behaves exactly as it
/// would with a device (Wine or a machine without speakers). The UI shows a "no audio device"
/// notice while this sink is in use.
pub struct NullSink {
    rate: u32,
    /// Frames handed over since the last flush.
    written: u64,
    /// Frames "played" at the moment `last` was taken.
    played_base: u64,
    last: std::time::Instant,
    paused: bool,
}

impl NullSink {
    pub fn new(rate: u32) -> Self {
        Self { rate, written: 0, played_base: 0, last: std::time::Instant::now(), paused: false }
    }

    fn played(&self) -> u64 {
        if self.paused {
            return self.played_base;
        }
        self.played_base + (self.last.elapsed().as_secs_f64() * self.rate as f64) as u64
    }
}

impl Sink for NullSink {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn name(&self) -> String {
        "No audio device".into()
    }

    fn is_real(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn push(&mut self, samples: &[f32]) -> usize {
        let played = self.played();
        if played > self.played_base {
            self.played_base = played;
            self.last = std::time::Instant::now();
        }
        // 500 ms of virtual buffering, like a normal device.
        let capacity = (self.rate / 2) as u64;
        let queued = self.written.saturating_sub(played);
        let space = capacity.saturating_sub(queued) as usize;
        let frames = (samples.len() / 2).min(space);
        self.written += frames as u64;
        frames
    }

    fn buffered_frames(&self) -> usize {
        self.written.saturating_sub(self.played()) as usize
    }

    fn pause(&mut self) {
        if !self.paused {
            self.played_base = self.played();
            self.paused = true;
        }
    }

    fn resume(&mut self) {
        if self.paused {
            self.paused = false;
            self.last = std::time::Instant::now();
        }
    }

    fn set_volume(&mut self, _volume: f32) {}

    fn flush(&mut self) {
        self.written = 0;
        self.played_base = 0;
        self.last = std::time::Instant::now();
    }
}

/// Collects everything pushed; used by unit tests that inspect rendered audio.
#[cfg(test)]
pub struct CollectSink {
    pub samples: Vec<f32>,
    pub rate: u32,
    pub volume: f32,
    paused: bool,
    /// Maximum frames accepted per push; `usize::MAX` = unlimited.
    pub capacity: usize,
}

#[cfg(test)]
impl CollectSink {
    pub fn new(rate: u32) -> Self {
        Self { samples: Vec::new(), rate, volume: 1.0, paused: false, capacity: usize::MAX }
    }
}

#[cfg(test)]
impl Sink for CollectSink {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn name(&self) -> String {
        "test".into()
    }

    fn is_real(&self) -> bool {
        true
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn push(&mut self, samples: &[f32]) -> usize {
        if self.paused {
            return 0;
        }
        let frames = (samples.len() / 2).min(self.capacity);
        self.samples.extend_from_slice(&samples[..frames * 2]);
        frames
    }

    fn buffered_frames(&self) -> usize {
        0
    }

    fn pause(&mut self) {
        self.paused = true;
    }

    fn resume(&mut self) {
        self.paused = false;
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
    }

    fn flush(&mut self) {
        self.samples.clear();
    }
}

/// Open an output device by name, or the Windows default endpoint for `None` (or when the named
/// device is gone), falling back to a silent sink.
pub fn open_output(device: Option<&str>) -> Box<dyn Sink> {
    #[cfg(windows)]
    match cpal_sink::CpalSink::open(device) {
        Ok(s) => {
            log::info!("audio output: {} @ {} Hz", s.name(), s.sample_rate());
            return Box::new(s);
        }
        Err(e) => {
            log::warn!("no audio output device ({e}); continuing silently");
        }
    }
    #[cfg(not(windows))]
    let _ = device;
    Box::new(NullSink::new(44_100))
}

/// Names of every output device, for the settings list.
pub fn list_output_devices() -> Vec<String> {
    #[cfg(windows)]
    {
        use cpal::traits::{DeviceTrait, HostTrait};
        match cpal::default_host().output_devices() {
            Ok(devices) => return devices.filter_map(|d| d.name().ok()).collect(),
            Err(e) => log::warn!("cannot list output devices: {e}"),
        }
    }
    Vec::new()
}

/// Name of the current Windows default output device.
pub fn default_output_name() -> Option<String> {
    #[cfg(windows)]
    {
        use cpal::traits::{DeviceTrait, HostTrait};
        return cpal::default_host().default_output_device().and_then(|d| d.name().ok());
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(windows)]
mod cpal_sink {
    use super::Sink;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SizedSample, SampleFormat, StreamConfig};
    use rtrb::{Consumer, Producer, RingBuffer};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;

    const RING_FRAMES: usize = 8192;

    struct Shared {
        volume: AtomicU32,
        paused: AtomicBool,
        /// Set by `flush`; the callback drains queued audio before it plays anything new.
        flush: AtomicBool,
        /// Set by the error callback when the device is no longer available.
        lost: AtomicBool,
    }

    pub struct CpalSink {
        /// Dropping the stream stops output; the field exists to keep it alive.
        _stream: cpal::Stream,
        producer: Producer<f32>,
        shared: Arc<Shared>,
        rate: u32,
        device_name: String,
    }

    impl CpalSink {
        pub fn open(wanted: Option<&str>) -> Result<Self, String> {
            let host = cpal::default_host();
            let named = wanted.and_then(|w| {
                let found = host.output_devices().ok()?.find(|d| d.name().is_ok_and(|n| n == w));
                if found.is_none() {
                    log::warn!("output device \"{w}\" not found; using the Windows default");
                }
                found
            });
            let device = match named {
                Some(d) => d,
                None => host.default_output_device().ok_or("no default output device")?,
            };
            let name = device.name().unwrap_or_else(|_| "audio output".into());
            let supported = device.default_output_config().map_err(|e| format!("default output config: {e}"))?;
            let format = supported.sample_format();
            let config: StreamConfig = supported.into();
            let rate = config.sample_rate.0;
            let shared = Arc::new(Shared {
                volume: AtomicU32::new(1.0f32.to_bits()),
                paused: AtomicBool::new(false),
                flush: AtomicBool::new(false),
                lost: AtomicBool::new(false),
            });
            let (producer, consumer) = RingBuffer::<f32>::new(RING_FRAMES * 2);
            let stream = match format {
                SampleFormat::F32 => build::<f32>(&device, &config, consumer, shared.clone()),
                SampleFormat::I16 => build::<i16>(&device, &config, consumer, shared.clone()),
                SampleFormat::U16 => build::<u16>(&device, &config, consumer, shared.clone()),
                other => Err(format!("unsupported output sample format {other:?}")),
            }?;
            stream.play().map_err(|e| format!("start stream: {e}"))?;
            Ok(Self { _stream: stream, producer, shared, rate, device_name: name })
        }
    }

    fn build<T>(
        device: &cpal::Device,
        config: &StreamConfig,
        mut consumer: Consumer<f32>,
        shared: Arc<Shared>,
    ) -> Result<cpal::Stream, String>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = config.channels as usize;
        let err_shared = shared.clone();
        let err_fn = move |e: cpal::StreamError| {
            log::warn!("audio stream error: {e}");
            if matches!(e, cpal::StreamError::DeviceNotAvailable) {
                err_shared.lost.store(true, Ordering::Relaxed);
            }
        };
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                    if shared.flush.swap(false, Ordering::Relaxed) {
                        while consumer.pop().is_ok() {}
                    }
                    let volume = f32::from_bits(shared.volume.load(Ordering::Relaxed));
                    if shared.paused.load(Ordering::Relaxed) {
                        data.fill(T::EQUILIBRIUM);
                        return;
                    }
                    // The ring holds interleaved stereo; map each L/R pair onto the device layout
                    // (mono gets the average, extra channels beyond the first two stay silent).
                    for frame in data.chunks_mut(channels) {
                        let (l, r) = if consumer.slots() >= 2 {
                            let l = consumer.pop().unwrap_or(0.0);
                            let r = consumer.pop().unwrap_or(0.0);
                            (l * volume, r * volume)
                        } else {
                            (0.0, 0.0)
                        };
                        for (i, sample) in frame.iter_mut().enumerate() {
                            let v = match (channels, i) {
                                (1, _) => (l + r) * 0.5,
                                (_, 0) => l,
                                (_, 1) => r,
                                _ => 0.0,
                            };
                            *sample = T::from_sample(v);
                        }
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("build output stream: {e}"))
    }

    impl Sink for CpalSink {
        fn sample_rate(&self) -> u32 {
            self.rate
        }

        fn name(&self) -> String {
            format!("{} ({})", self.device_name, self.rate)
        }

        fn is_real(&self) -> bool {
            true
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn push(&mut self, samples: &[f32]) -> usize {
            // The callback has not drained the old audio yet. Wait for it (one device period,
            // a few ms) so the start of the new audio is not dropped.
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
            while self.shared.flush.load(Ordering::Relaxed) {
                if std::time::Instant::now() >= deadline {
                    return 0;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            // Commit whole frames in one chunk so the callback never sees half a frame.
            let n = (samples.len().min(self.producer.slots())) / 2 * 2;
            if n == 0 {
                return 0;
            }
            match self.producer.write_chunk_uninit(n) {
                Ok(chunk) => chunk.fill_from_iter(samples[..n].iter().copied()) / 2,
                Err(_) => 0,
            }
        }

        fn buffered_frames(&self) -> usize {
            if self.shared.flush.load(Ordering::Relaxed) {
                return 0;
            }
            // `slots()` is the *free* space; what is queued is the rest of the ring.
            (self.producer.buffer().capacity() - self.producer.slots()) / 2
        }

        fn pause(&mut self) {
            self.shared.paused.store(true, Ordering::Relaxed);
        }

        fn resume(&mut self) {
            self.shared.paused.store(false, Ordering::Relaxed);
        }

        fn set_volume(&mut self, volume: f32) {
            self.shared.volume.store(volume.clamp(0.0, 1.5).to_bits(), Ordering::Relaxed);
        }

        fn flush(&mut self) {
            self.shared.flush.store(true, Ordering::Relaxed);
        }

        fn lost(&self) -> bool {
            self.shared.lost.load(Ordering::Relaxed)
        }
    }
}
