# Plan: "Ride" — an Audiosurf-style game mode

**Status (2026-11-01): built: spike and steps 2–5 all done, plus a round of feedback fixes and hold notes.**

Hold notes:
- Analysis (VERSION 2) finds `sustains`.
  - A clear onset (strength ≥ 0.25, peak ≥ 35 % of the band's range) whose band energy stays
    within 45 % of the way down to the band's quiet floor.
  - It ends at the next strong attack (≥ 0.6) in that band, or after 4 s; it must last ≥ 0.5 s.
  - An onset floor of 5 % of the band's loudest attack keeps steady tones in sparse music from
    producing fake onsets.
  - Drums never qualify.
- Map generator:
  - Any onset at a note slot that starts a sustain makes the note a hold, in that sound's band
    (a chord over a louder kick still counts).
  - `settle_holds` puts the end on the half-beat grid (0.45–3 s). Notes inside are dropped,
    unless stronger by 0.2, which shortens the hold instead.
  - Lane jumps count from the hold's end, and greys avoid its lane for its whole length.
  - Real songs on Pro: a hold every 5–9 s, holding 10–20 % of the time (most on the orchestral
    piece).
- Play:
  - `NoteState` per note (Pending, Caught, Missed, GreyHit, Holding, Held, Broken), a `Holding`
    state, and `consumed` time per ribbon.
  - The head scores as a normal note. Each 0.1 s ridden within 0.26 of the lane pays a tenth of
    the note's points (times the multiplier). The end pays a bonus and +1 combo (`HoldDone`).
  - Leaving early is `HoldBroken`, with no combo loss. Seeks end an active hold unpaid.
- Look:
  - A 1.1 m ribbon in the note's colour along the lane, under the blocks, eaten from the craft
    and pulsing while held; it dims when missed or broken. The tail gets a low cap.
  - Sparks stream back from the craft while holding, and a big burst comes at the end.
  - The HUD shows a live "+N" over the craft and "Hold +N" at the end.

Feedback round (after first play):
- The cursor is hidden only over the game surface (`resp.hovered()`), not over controls.
- There are 5 lanes (2 m apart, catch window 0.18). Bands get per-section home lanes plus a
  walking pattern, and every other phrase (4 bars) mirrors the layout. Quick successions may move
  ≤ 2 lanes within 0.3 s and ≤ 1 within 0.2 s.
- Judder: the old `SongClock` stepped 15 % towards each stale report, often backwards. The new
  clock estimates the position as the report plus its age and follows it with a slew of at most
  ±10 % speed, never backwards while playing; it snaps only past 0.25 s. Frame `dt` is now real
  time since the last drawn frame, not since it was due.
- Missed notes stay on the road, dimmed, until behind the camera; caught ones and hit greys are
  gone (`Ride::collected`).
- Hit effects: a flash, 18 sparks and a shockwave ring travelling with the craft, an edge flash
  and a "+points" pop-up. A grey hit gives a red wash and a camera shake.
- The road is 96 % opaque (was 80 %).

Step 5 notes (AI tier, craft, settings, README):
- The Beat This! small model and its mel front end live in `assets/models/` and are embedded with
  `include_bytes!` (`game/ai.rs`). They are unpacked once into the cache's `models` folder, because
  the `beat-this` crate loads files.
  - rten and its crates use per-package `opt-level = 3`; in-app speed is about 2 % of song
    length on the VM, the same as the spike.
  - The first run takes ~5 s because it unpacks the model.
- A build now posts two results: the classic analysis, then the model-refined one, which replaces
  it in memory and in the database.
  - The ride in progress keeps its map; the refined one is used from the next ride, and
    prefetched songs get it before they start.
  - While riding, idle time also refines saved classic-only tracks (each tried once per session).
- Results on the three real test songs: the menu theme's tempo went from 125.9 bpm (classic, 4:3
  error) to 94.8 bpm, with alignment 1.73 → 2.53. One game track went 2.54 → 2.79; the other
  was unchanged.
- Basic Pitch (melodic notes) is not used yet. `tools/fix_basic_pitch_onnx.py` makes it
  rten-loadable if wanted later.
- Craft: hover ship, car (wheels, cabin, tail lights) and plane (bobbing, banking), built from
  box meshes in `render.rs`.
- The Settings → Ride card has difficulty (a change applies at once), craft, 30/60 fps, audio
  delay (−100…400 ms) and "Smarter beats" (the AI) on or off.
- The README has Ride's feature section, screenshots (`docs/ride.png`, `docs/ride-craft.png`),
  status, architecture, footprint and testing. The exe is 24.7 MB.

Step 4 notes (integration):
- Analyses are saved in the library database (`ride_maps`, JSON, about 125 KB for a 4-minute
  song after rounding and short onset keys). They are keyed by track and analyser `VERSION`, and
  only the 300 most recently used are kept.
  - "Clear" on the library cache removes them; Settings → Storage → Ride tracks shows them and
    can clear them on its own.
- Scores go in `ride_scores` (track, difficulty, score, accuracy, best combo, time). They survive
  cache clears. A ride is saved once at least 5 notes were reached.
- While riding, the next song in the queue is built in the background, so each ride starts at
  0.0 s. Failed builds are remembered and not retried.
- An end-of-song card shows the title and score, then "New personal best!", "First ride of this
  song" or the best score and this ride's place, plus accuracy, combo and level. The HUD shows the
  song's best score while riding.
- Restarting the song (Previous, or seeking to the start) scores the interrupted ride and rides
  again from the top. This is the "retry".

Step 3 notes (game core):
- Modules: `game/clock.rs` (smoothed audible-position clock), `game/play.rs` (steering, catch and
  miss, greys, combo ×1–×4, seek skip, 2 s run-up), `game/render.rs` (road geometry, chase camera,
  road, notes, craft, bursts over the dimmed visualiser), `game/build.rs` (background analysis
  through a second `AudioCache` reader, below-normal priority on Windows) and `ui/app/ride.rs`
  (ride mode: `G` or the Ride button, HUD, carrying on into the next song).
- In the app:
  - ← → steer instead of seek while riding.
  - Leaving the page ends ride mode.
  - The visualiser backdrop keeps its own 30 fps while the game runs at 60.
- Performance: the game draw is 0.42 ms per frame on Linux and ~2.7 ms on the VM (480×300). That
  needed `Canvas::tri_flat`, a scanline fill; the Polygon look uses it too.
  - Full screen on the VM is a steady 60 fps at ~50–55 % of a core. That figure is inflated by the
    mock's 8 s songs, which keep fading the song overlay in; real songs are estimated in the high
    30s %.
- Frame pacing lessons: on Windows the timer resolution is raised to 1 ms (`timeBeginPeriod`)
  while riding. The repaint delay must be counted from when it is requested, keeping a steady
  frame cadence; otherwise frames run at 40–45 fps.
- Preview: `ride_preview --frames` renders six moments of an autopilot ride.

Step 2 notes:
- The classic analysis is fast: a 2:35 song decodes in 0.36 s and analyses in 0.15 s (Linux).
- The beat grid is steady (interval variation 1–5 %).
- Lessons:
  - The onset envelope must be smoothed (±2 frames) before the tempo autocorrelation.
  - The tempo comb scores a period together with its multiples (not halves).
  - Bass is weighted up, and a half-beat shift check catches grids locked onto off-beat hats.
  - Onset and beat times need +20 ms (`LAG`), because frames are centred.
- Known weak spots, which the AI tier (step 5) should fix: a 4:3 tempo error on an orchestral
  piece (126 vs the model's 95 bpm), and the grid sitting half a beat off for part of one song.

**Decisions:**
- Gameplay: collect and dodge. Coloured notes give points, grey blocks are to be avoided, and
  streaks build a combo multiplier. There is no match-3 grid and there are no ship rules.
- Models: a choice of hover ship, car or plane, all with the same gameplay (no runner).
- AI: Beat This! small is **built into the exe** (about 23 MB in total), so it works offline.
- Songs: the ride **continues into the next song**, whose map is built in the background, with a
  score toast between songs.
- Steering: **free**. The mouse's horizontal position is the road position and the arrow keys
  move smoothly; notes sit in 5 lanes (3 in the first version).
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
