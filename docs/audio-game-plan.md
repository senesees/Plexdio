# Plan: "Ride" — an Audiosurf-style game mode

**Status (2026-11-01): plan agreed, spike done (results below). Next: analysis and map generator.**

**Decisions:**
- Gameplay: collect and dodge. Coloured notes give points, grey blocks are to be avoided, and
  streaks build a combo multiplier. There is no match-3 grid and there are no ship rules.
- Models: a choice of hover ship, car or plane, all with the same gameplay (no runner).
- AI: Beat This! small is **built into the exe** (about 23 MB in total), so it works offline.
- Songs: the ride **continues into the next song**, whose map is built in the background, with a
  score toast between songs.
- Steering: **free**. The mouse's horizontal position is the road position and the arrow keys
  move smoothly; notes sit in 3 lanes.
- Failing: **never**. Every ride reaches the end of the song.
- Difficulty: Casual, Pro and Elite, with scores kept per level.
- Windows mouse and keyboard only (no gamepad).

Turn the song that is playing into a track you ride on, Audiosurf style. The track and its notes
are generated from the song itself. You steer a model (a hover ship, car, plane or runner) with the
mouse or the arrow keys and collect notes, and the retro visualiser plays behind it. The music
never stops: the game can be started in the middle of a song and picks up from where the song is.

## Spike results (2026-11-01)

Tested on three real songs (70 s, 100 s and 155 s), on Linux (32 cores) and on the Windows 11 VM
(4 vCPUs, i9-14900):
- **Beat This! small via `beat-this` 1.1 on rten** cross-builds to `x86_64-pc-windows-gnu` with
  system DLLs only. It loads in 0.02 s.
  - Analysis takes about **2 % of the song's length on the VM**: 1.7 s, 2.1 s and 3.1 s, so about
    5 s for a 4-minute song. It is effectively single-threaded, so it won't load the PC.
  - Results are identical on Linux and Windows.
- **Beat quality:**
  - On a steady orchestral piece it is excellent: interval variation 6 %, and it lands on hits
    better than librosa.
  - On two busy game tracks it **flips between tempo levels** (0.31 s ↔ 0.62 s beats) and jitters
    in intros. This is expected without the model's "DBN" smoothing step.
  - librosa's classic tracker picked half or other tempo levels on all three songs.
  - **Consequence:** the analyser needs its own *steady-grid pass*: pick one tempo level per
    section from the model's beats plus the onset curve, then fit a regular grid with phase
    corrections (dynamic programming). Notes snap to that grid, not to the raw beats.
- **Basic Pitch** (`nmp.onnx`, 230 KB) runs on rten once its tensor names are made unique.
  - The tf2onnx export gives an operator and its output the same name, which rten keeps in one
    namespace. A one-off ONNX rename script fixes it, and the fixed file is the one we embed.
  - Its outputs match ONNX Runtime to 4–5 significant figures.
  - Speed: about 20 ms per 2 s window, so about 2.5 s per 4-minute song.
  - So melodic note onsets are affordable too.
- **Size:** rten adds about 3.3 MB. The models are 10.6 MB (beat), 0.27 MB (mel) and 0.23 MB (notes).
  The expected exe is about **25 MB**.
  - **Gotcha:** our release profile is `opt-level = "z"`; rten and the analysis code need
    `[profile.release.package.rten] opt-level = 3` (and the same for rten's crates) or inference
    slows down.
  - `beat-this` only loads models from a file path, so the embedded bytes are written once to the
    cache dir, or loaded through our own small `Model` implementation on `rten::Model::load`.
- **Getting the audio:** decoding happens through a second `AudioCache` reader (see below). The
  crate's own `decode` feature (symphonia 0.6, all codecs) is not used; we pass samples to
  `analyze_audio`.

## What Audiosurf does (and what we copy)

- **The track follows the music.** In calm parts it climbs slowly; in intense parts it drops
  steeply and you go fast. The whole track pulses with the beat.
- **Blocks follow the instruments.** They sit on beats and onsets and are coloured by intensity.
  Hot colours (red, yellow) are worth more, cool ones (blue, violet) less. Grey blocks are
  obstacles.
- **Three lanes** (or more), steered with the mouse.
- **The analysis is cached.** It took Audiosurf 20–30 s per song in 2008, and a replay of the
  same song loads instantly.
- Audiosurf also collects blocks into a 3×7 grid and scores match-3 combinations, and ship types
  change the rules. We leave both out (collect and dodge only).

## Research: music analysis, and whether AI helps

**Classic signal processing is enough for a good first version.** It is fast and needs no model:
- spectral-flux **onsets** in three bands (low, mid and high, roughly kick, snare/voice and hats);
- a loudness and **intensity** curve;
- **tempo and a beat grid**, using onset-envelope autocorrelation plus a dynamic-programming beat
  tracker (Ellis 2007);
- **sections** (verse, chorus, drop), found from jumps in smoothed intensity and timbre.

All of this runs in well under a second for a 4-minute song once it is decoded. Decoding is
symphonia, which the app already uses.

**Small AI models are available and can run in pure Rust:**

| Model | What it gives | Size | Rust / runtime |
|---|---|---|---|
| **Beat This!** small (ISMIR 2024) | Beats and **downbeats** (bar starts), state of the art, no tempo assumptions | 2.1 M parameters, ~10 MB ONNX | Ready-made [`beat-this`](https://crates.io/crates/beat-this) crate on the pure-Rust **rten** runtime. No DLLs, MIT licence. About 0.7 s for a 9 s clip and 12 s for a 13:48 song on an M4, so ~4 s per 4-minute song on a fast PC and maybe 10–15 s on an old one. |
| **Basic Pitch** (Spotify) | Note **onsets** and pitches (polyphonic), for melodic notes and lane choice | <17 K parameters, <1 MB | An ONNX export exists; whether rten supports all of its operators still needs checking. |

The pure-Rust options are rten (used by `beat-this`) and tract. `ort` (ONNX Runtime) is out,
because it needs a native DLL and would break the "one exe, system DLLs only" rule.

**Recommendation:** two tiers.
1. **Signal processing** builds the map immediately (about 1 s), so a ride can start at once,
   even mid-song.
2. **Beat This!** refines the beat grid and adds downbeats in the background. The improved map is
   cached, and it is used from the next section onwards if it is ready in time, or from the next
   play otherwise. Basic Pitch is a later, optional addition for melodic notes.

## How it fits Plexdio

- **Getting the whole song while it plays.** `AudioCache::open` already shares one download
  between several readers (prefetch and playback) through its registry. The analyser opens a
  *second reader* on the same source and decodes it on a worker thread at 22 050 Hz mono. There
  is no second download, and playback is untouched. Offline (downloaded) tracks are read from
  disk.
- **Rendering.** It uses the same CPU pipeline as the visualiser. The game draws its 3D track and
  model into a retrovis `Canvas` at 480×270 by default, *over* a dimmed visualiser frame, and the
  result goes to the window through the direct `VideoFrame` blit. The cost is about the same as
  the visualiser, plus the track. The HUD (score, combo, multiplier) is small egui text on top.
- **Timing.** The game clock is the *audible* playback position, not wall-clock time. The engine
  already counts written and buffered samples for the scope tap, so this adds a precise
  `audible_position()`, interpolated between updates. A latency offset setting, with a simple
  "tap along" calibration, covers Bluetooth headphones.
- **Starting mid-song.** Pressing **Ride** (a button on the Now playing page, or `G`) starts the
  analysis if there is no cached map. The visualiser keeps playing, with a small progress readout
  ("Building track… 60 %"). The ride then starts at the current position plus a ~3 s run-up, so
  the first notes come from the horizon. `Esc` returns to the plain visualiser, and the music
  carries on throughout.
- **Song changes.** While riding, the next song in the queue is analysed in the background, so
  the ride continues smoothly into it. Seeking re-syncs the ride. Pausing the music
  pauses the ride. Casting turns the game off, because the audio is not local.

## Parts

### 1. Analysis: `game/analysis.rs` (M)
Decoding to 22 050 Hz mono through a second `AudioCache` reader, then:
- an STFT (the existing FFT code);
- per-band spectral flux and onset picking;
- RMS intensity;
- tempo plus the beat tracker;
- section boundaries.

The output is `Analysis { duration, beats, downbeats, onsets[3], intensity curve, sections }`.
It is versioned, so a better analyser invalidates old caches.

### 2. AI tier: `game/ai.rs` (M, can be switched off)
The `beat-this` crate (rten) with the small model, giving beats and downbeats. It runs at low
thread priority after the signal-processing pass. Later, optionally, Basic Pitch for melodic
onsets. The model is embedded in the exe (`include_bytes!`).

### 3. Map generator: `game/mapgen.rs` (M)
`Analysis` becomes a `Map`:
- **The road** as a spline over time:
  - slope from intensity (uphill when calm, downhill when loud);
  - speed from slope;
  - curves and banking from section changes and the beat;
  - width pulsing on beats.
- **Notes:**
  - time comes from onsets snapped to the beat grid;
  - the lane comes from the band (low → left, mid → centre, high → right), shuffled per section
    so the patterns don't repeat;
  - colour and points come from local intensity.
- **Obstacles (greys)** go in gaps.
- **Density per difficulty** (Casual / Pro / Elite), with a minimum gap between notes and a
  maximum lane jump so the map is always playable.

It is deterministic (the seed is the track key), so a map can be rebuilt from its analysis. A
`examples/ride_preview.rs` tool plots map and analysis PNGs, like `retrovis_preview`.

### 4. Game: `game/play.rs`, `game/render.rs`, `game/models.rs` (L)
- **State:** the position along the road (from the audio clock), lateral position, collected and
  missed notes, score, combo and multiplier (going up with streaks, reset by a grey), plus
  per-song stats.
- **Input:** free steering. The mouse's horizontal position is the road position (absolute
  within the view, smoothed), and `←`/`→` or `A`/`D` steer smoothly.
- **Rendering**, with `math3d` (already used by the Polygon, Terrain and Tunnel looks):
  - the road ribbon with lane lines and beat pulses;
  - the notes as glowing blocks with additive glow;
  - the model with a PS1-style flat-shaded low-poly look;
  - a chase camera that leans into curves;
  - sparks and flashes on collection, which also feed the visualiser's beat input.
- **Models**, built in code (no asset files): hover ship, car, plane, and a runner with a
  procedural run cycle. We build the ship, car and plane (no runner).

### 5. Storage (S)
- Schema v3 (`LibraryDb::migrate`, which keeps existing data).
- `ride_maps`: key = track rating key + media part + duration + analyser version. It stores the
  compressed analysis blob plus whether the AI pass is done. It counts against a size limit,
  like the audio cache, and has a "clear" button in Settings → Storage.
- `ride_scores`: track, difficulty, model, score, accuracy, longest combo, date. These give a top
  10 per song and difficulty, and a "personal best" flash.

### 6. UI (M)
- The **Ride** button and `G` on the Now playing page.
- The progress readout while a map builds.
- The HUD: score, multiplier, combo, a progress bar, and the top-right best score.
- An end-of-song card (score, accuracy, best, retry); retry means "ride from the start", which
  seeks the song to 0.
- A **Ride** card in Settings: difficulty, model, controls, latency offset (with a tap test), AI
  analysis on/off, background brightness, cache size.
- Full screen works as on the visualiser page (`F11`).

## Performance budget
- Analysis: decoding a 4-minute song plus signal processing takes ≤ 1.5 s on the VM, on a worker
  thread at below-normal priority. The Beat This! small model takes ~5–15 s in the background.
- Game frame at 480×270 and 60 fps: the visualiser background is about 1–3 ms and the track and
  model about 1–2 ms. The blit and present are as measured (about 1 ms). The target is ≤ 35 % of
  a core in full screen on the VM; 30 fps is a fallback setting.
- Memory: the decoded analysis buffer is about 21 MB for 4 minutes of mono f32. It is dropped
  after analysis; only features are kept, about 100 KB.

## Testing
- **Unit tests:**
  - the beat tracker finds the tempo of synthetic click and kick tracks (90–180 bpm) within 1 %;
  - onset picking on synthetic hits;
  - the map generator is deterministic and always playable (gaps and lane jumps within limits);
  - collision and scoring;
  - the audio clock with a fake engine;
  - cache key invalidation by analyser version.
- **mockplex:** a longer generated fixture with known bpm and sections, so the game is testable
  end to end.
- **Preview tool:** PNG plots of intensity, beats, sections and the road profile, to review maps
  without playing.
- **Manual:** the Linux build on a private display, and the Windows 11 VM for CPU and timing
  (an audio latency check needs real speakers, so you would check that by ear).

## Order
1. **Spike** (about 1 day):
   - decoding and signal processing on real songs (speed, beat quality);
   - `beat-this` on rten in the cross-built exe (size added, speed on the VM);
   - Basic Pitch operator support in rten.

   Report back before going further.
2. Analysis, map generator and preview tool.
3. Game core: road, model, input, audio clock, scoring (playable from the Now playing page).
4. Integration: starting mid-song, background builds, next-song prefetch, seek and skip,
   caching, scores and the results card.
5. The AI tier, the other models, difficulty, settings, README and screenshots.

Rough size: L to XL overall (about 6–9 days). The spike decides the AI part.
