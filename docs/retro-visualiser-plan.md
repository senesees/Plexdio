# Plan: retro 3D visualiser

**Status (2026-11-01): built.** Answers: only on the Now playing page (the small bars stay as they
are); all nine looks; on pause it fades (the Dashboard into its starfield) and then stops. Code:
`ui/retrovis/` (engine and presets) and `ui/app/now_playing.rs` (page, overlay, full screen,
settings); preview tool `examples/retrovis_preview.rs`. Measured on the Windows 11 VM: about 25 fps in
full screen at about 70 % of one core, mostly the software renderer scaling the image. The direct
blit into the window (fallback 3 below) is the remaining option if that is too much. Not done: the
mini-player option (dropped with the "Now playing only" answer), and rendering on a separate thread
(the effect itself is 1–5 ms, so it wasn't needed).

A full-screen visualiser in the style of the early-2000s console and PC players: the original
Xbox dashboard, Windows Media Player 7–10, the PlayStation CD player's Sound Scope, and Jeff
Minter's console light synths (Jaguar VLM, Xbox 360 Neon). It sits next to the existing small
bars/wave visualiser (`ui/visualiser.rs`), which stays as it is.

Reference images (not committed) are in `analysis/vis-refs/`: `console-grid.png` (Xbox, PS1 ×2,
Minter VLM) and `wmp-grid.png` (WMP Ambience, Battery, Alchemy, Plenoptic).

## What the originals look like

| Original | Look | What makes it work |
|---|---|---|
| **Xbox dashboard** (2001, "Geiss-like") | One slowly changing tint (purple, green), soft glowing streaks and rays flowing out of the centre | **Feedback:** each frame starts from the last one, zoomed in slightly, rotated, blurred and darkened, and the new waveform streaks are drawn on top. Pausing showed a swirling **starfield** (an easter egg). |
| **WMP Ambience / Battery / Plenoptic** | Swirls, spirals and smoke, with jagged waveform lines across the screen and colours fading through palettes | The same feedback idea with other warps (swirl, spiral, ripple, split screen) and a 256-colour palette per preset. Battery is data-driven: about 25 presets, each a stack of warp steps plus a palette. |
| **WMP Alchemy** | Kaleidoscope, star bursts, a spectrum mode | Feedback that is mirrored into segments, plus bursts drawn on beats. |
| **PS1 Sound Scope** (SCPH-7000 and later) | (a) Spectrum spikes mirrored about a horizontal line, in translucent green, yellow and red layers with motion trails. (b) Concentric rings that pulse with the bass and leave ghost trails. | Additive, translucent polygons at a low resolution, with a fading trail buffer. |
| **Jaguar VLM / NUON / 360 Neon** | 3D point clouds and polygon shapes, mirrored, flying through tunnels, very saturated | Real 3D (rotating meshes, point clouds, camera moves) composited over feedback. |

What they share: **a low-resolution frame buffer that is fed back into itself**, plus **simple 3D
primitives** drawn into it, driven by the bass, mid and treble levels and a **beat** signal. That
is what this plan builds.

## How it fits Plexdio

- Plexdio draws its whole UI on the CPU (no GPU), so the visualiser renders its own **pixel
  buffer at a low internal resolution** (default 320×180, or 480×270 / 640×360), uploads it as an
  egui texture each frame, and draws it scaled to the view. The low resolution is also the look:
  chunky PS1/Xbox-era pixels, with nearest-neighbour or smooth scaling as a setting.
- Audio comes from the existing tap (`audio/scope.rs`): the samples being heard right now. The
  current FFT moves into a shared `analysis` step.
- It only runs while it is on screen and music plays. When paused it eases out (on the Xbox
  preset, into the starfield) and then stops drawing.

## Parts

### 1. Engine: `ui/retrovis/` (M)
- `canvas.rs`: an RGBA `u32` buffer with additive and alpha blending; Bresenham and antialiased
  lines, thick lines, points/sprites, flat and Gouraud triangles, circles and rings; a 4×4 Bayer
  **dither** option (the PS1 look).
- `feedback.rs`: the heart of it. Each frame the previous buffer is sampled through a **warp map**
  (zoom, rotation, swirl, spiral, ripple, mirror/kaleidoscope, scroll, or a mix), then blurred and
  darkened by a decay factor. The warp map is precomputed as a lookup table whenever the preset
  or its parameters change, so a frame is one bilinear lookup per pixel (≈58 k at 320×180).
- `palette.rs`: 256-entry gradient palettes that cycle and crossfade, as in WMP.
- `analysis.rs`: per frame, from the audio tap: 32 spectrum bands; bass, mid and treble levels
  (smoothed, normalised against a running peak so quiet and loud tracks both move); **beat
  detection** (bass energy against a moving average, with a cooldown); a 512-sample waveform;
  overall loudness.
- `math3d.rs`: a small hand-written `Vec3` / 4×4 matrix, perspective projection, z-sorting and
  backface culling. PS1 style is optional: snap vertices to whole pixels, affine (wobbly)
  texture/colour interpolation, no z-buffer.
- `preset.rs`: a `Preset` trait (`frame(&mut canvas, &analysis, t, dt)`), a registry, **auto
  cycling** every N seconds or on a strong beat, and a ~1 s **crossfade** between presets.

### 2. Presets (M–L)

Inspired by the originals and written from scratch:

1. **Scope**: PS1 spikes. Mirrored spectrum spikes as translucent triangles in green, yellow and
   red layers, with trails (light feedback, no warp).
2. **Rings**: PS1 rings. Concentric rings whose radius and thickness follow bass, mid and treble,
   with ghost trails.
3. **Dashboard**: Xbox. Zoom and rotate feedback with heavy blur, one slowly shifting tint, glowing
   waveform streaks and rays from the centre; on pause, it fades into a **swirling starfield**.
4. **Ambient**: WMP Ambience/Battery. Swirl or spiral warps with jagged waveform lines across the
   screen and palette cycling. Several variants (swirl, smoke, split screen), each a parameter set,
   like Battery's presets.
5. **Kaleido**: WMP Alchemy. Kaleidoscope feedback in 4, 6 or 8 segments, with star bursts on beats.
6. **Polygon**: PS1 / 3D Alchemy. A low-poly rotating 3D shape (icosphere, torus or cube field)
   whose vertices are pushed out by the spectrum, flat-shaded with PS1 vertex snapping, over a
   slowly warping feedback background.
7. **Terrain**: a wireframe landscape scrolling towards the camera, its height made from the
   spectrum history (a classic 3D spectrum grid).
8. **Tunnel**: Minter-style. The camera flies down a tunnel of rings and polygons that pulse and
   change colour on beats.
9. **Cloud**: Jaguar VLM. A mirrored 3D point cloud (a few thousand points) that morphs between
   shapes on beats, with additive glowing points.

### 3. The visualiser view (S–M)
- A **Now playing** page (`Route::Visualiser`) where the visualiser fills the content area. It opens
  from a new button in the player bar, the `V` key, or a double-click on the bar's cover. `F11` or a
  double-click on the view makes it **full screen** (borderless; needs a new `WindowCmd::Fullscreen`
  in `ui/runner.rs`), and `Esc` leaves.
- An overlay that fades in on track changes and on mouse move, then out after a few seconds, like
  the Xbox: cover, title, artist, preset name. `←`/`→` change the preset, and `A` toggles auto cycling.
- **Mini-player:** an option to run the 2D presets (Scope, Rings, Dashboard) at a tiny resolution in
  the mini-player's strip, as well as the current bars and wave.

### 4. Settings (S)
In Settings → Window → Visualiser:
- Style: Bars / Wave / Retro.
- Default preset, or Random.
- Auto cycle: off / 15 s / 30 s / 60 s.
- Resolution: 320×180 / 480×270 / 640×360.
- Pixels: sharp / smooth.
- Dithering: on or off.
- Frame rate: 30 or 60.

All saved in the config.

## Performance

The risk is CPU time, because the final scaled image is also drawn by the software renderer.

- **Budget:** at 320×180 and 30 fps, the effect itself (feedback plus drawing) should stay under
  ~2 ms per frame. Drawing the scaled texture onto a 1180×700 view is the bigger cost.
- **Measure** with `tools/wine-app.sh stats` and on the Windows VM. The README footprint table
  gets a "visualiser on" row.
- **Ways out, in order:**
  1. Render the effect on its own thread (double-buffered), so the UI thread only uploads and draws.
  2. Lower the default resolution or frame rate.
  3. Skip the egui texture path in full screen and blit the scaled buffer straight into the
     softbuffer surface (integer scaling).
- It never runs hidden, minimized, paused (after the fade-out) or with the view closed.

## Testing
- Unit tests: canvas primitives (clipping, blending), the warp table (identity map, zoom
  direction), projection and culling, beat detection on a synthetic kick pattern, palette
  interpolation.
- A **preview tool**: `cargo run --example retrovis_preview` renders every preset from synthetic
  audio (sweeps, kicks, noise) into PNG contact sheets in `target/retrovis/`, so looks can be
  reviewed and compared with the reference images without starting the app.
- A smoke test: every preset runs 300 frames at each resolution without panicking, and fills a
  sensible share of the screen (not all black, not all white).
- Then look at it in the Linux build on a private display, and a final check on the Windows VM.

## Order
1. Engine plus the preview tool, with Scope and Rings (proves the pipeline and the look).
2. The visualiser view and full screen; measure the CPU cost.
3. The feedback presets: Dashboard (with the starfield), Ambient, Kaleido.
4. The 3D presets: Polygon, Terrain, Tunnel, Cloud.
5. Transitions, auto cycling, overlay, mini-player option, settings, README.

Rough size: L overall (about 4–6 days of work). Phase 1 decides the look, and phase 2 tells us
early whether the CPU budget holds.

## Questions for you
1. Where should it live: only in the full **Now playing** view (and full screen), or also replace the
   small bars behind the player controls?
2. Which look matters most to you, to build first: Xbox feedback, the PS1 Scope, WMP swirls, or real
   3D shapes?
3. Should it run when the music is paused (the Xbox starfield, then stop), or freeze straight away?
