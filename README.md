# PlexDio

A small, native Windows 10/11 x64 music player for Plex — a lean alternative to Plexamp.
One self-contained exe (9.4 MB), no installer, no runtime, no Electron.

![Home](docs/home.png)

## What it does

- **Sign in** with the Plex PIN flow: the code is shown in the app, the browser opens the
  plex.tv page, and the token is stored in the Windows Credential Manager (DPAPI-encrypted
  file as a fallback).
- **Pick a server and library**: every connection of a server is raced and ranked
  (local > remote > relay), dead connections are skipped, the choice is remembered.
- **Offline-first library**: a per-server SQLite cache with a full and a delta sync
  (`updatedAt>>=`/`addedAt>>=` plus a count-check sweep). The UI opens the cached library
  immediately, even with the server unreachable.
- **Browsing**: Home (recently added, mixes), Artists / Albums / Tracks / Genres, playlists,
  artist/album/playlist details, virtualised lists that stay smooth with 100 000 tracks.
- **Instant search**: in-memory, diacritic-folding prefix/substring search over artists,
  albums and tracks, plus a server `/hubs/search` supplement. `Ctrl+F`.
- **Playback**: gapless and crossfade (0–12 s), ReplayGain/loudness normalisation with a
  peak limiter, seek, volume, mute, prefetch and an on-disk audio cache. Codecs are decoded
  in-process with Symphonia (MP3, FLAC, AAC/M4A, ALAC, Vorbis, WAV); anything else — and
  Opus — goes through the server's universal transcoder.
- **Queue**: add next / add to queue, drag reorder, remove, clear, shuffle (weighted by rating
  and play count, artist-spread) and repeat off/all/one. The queue, track and position
  resume on the next start.
- **Reporting**: `/:/timeline` while playing (every 10 s and on state changes), `/:/scrobble`
  at 90 %, and star ratings through `/:/rate`.
- **Radio and autoplay**: start a station from a track, artist or album (sonic `nearest` when
  the server offers it, else related hubs, else same artist/genre from the cache); when the
  queue runs out, autoplay picks up from the last track.
- **Windows integration**: tray icon with menu, minimise/close to tray, media keys
  (`RegisterHotKey`, plus the System Media Transport Controls media overlay with metadata,
  status and seek timeline), start with Windows, a compact always-on-top mini-player, and
  keyboard shortcuts (`Space`, arrows, `Ctrl+←/→`, `Ctrl+↑/↓`, `Ctrl+M`, `Ctrl+F`).
- **Settings**: stream quality (original or a bitrate cap via the transcoder), crossfade,
  normalisation, autoplay, image and audio cache sizes with current usage and clear buttons,
  window behaviour, start with Windows, account/sign-out.

## Status

Tested against **mockplex**, the mock Plex server in this repository, under Wine and on a
real Windows 11 VM. **It has never talked to a real Plex server or plex.tv** — see
[What is not verified](#what-is-not-verified), which also has the manual checklist for a
real account.

| Area | Verified |
|---|---|
| Cross-build, PE32+ x86-64, system-DLL-only imports | ✅ every build (`./build.sh`) |
| PIN sign-in, token storage, server/library selection | ✅ mock server, Wine + Windows 11 |
| Full/delta sync, offline start from cache, 100 000 tracks | ✅ mock server, Wine + Windows 11 |
| Browsing, artwork, virtualised scrolling, search | ✅ Wine + Windows 11 screenshots |
| Gapless, crossfade, gain, seek, prefetch | ✅ sample-accurate unit/integration tests |
| Queue, shuffle/repeat, resume, timeline/scrobble/rating | ✅ tests + mock request log |
| Radio/autoplay, Home mixes | ✅ mock server (sonic on and off) |
| Settings persistence, tray, media keys, mini-player, shortcuts | ✅ Wine |
| SMTC overlay (metadata, status, timeline, buttons) | ✅ Windows 11 VM |
| Real audio output | ❌ no audio device in Wine or the VM (null sink path) |
| Real Plex account, real server, Plex Pass sonic data | ❌ not available in this environment |

## Build

Requirements: Rust stable with the `x86_64-pc-windows-gnu` target and a MinGW-w64 linker.
SQLite is compiled in; there is nothing else to install.

```sh
./build.sh          # release build -> dist/plexdio.exe (+ dist/plexdio.ico)
./build.sh test     # or: cargo test --workspace
make dist           # same as ./build.sh
```

`build.sh` prints the size and the DLL import table; it must list only system DLLs
(`KERNEL32`, `USER32`, `GDI32`, `ole32`, `combase`, `ws2_32`, `bcryptprimitives`, …) and no
`libgcc_s_seh-1.dll` / `libwinpthread-1.dll` / `msvcp*.dll`.

The app also builds and runs natively on Linux for development (`cargo run -p plexdio`);
every Windows-specific module has a no-op stub.

Release profile: `opt-level="z"`, fat LTO, one codegen unit, `panic="abort"`, stripped,
static C runtime.

## Running against the mock server

```sh
cargo run -p mockplex -- --port 32500           # fixture library: 5 artists, 7 albums, 25 tracks
cargo run -p mockplex -- --port 32500 --synthetic 100000   # large-library stress test
./dist/plexdio.exe --plex-tv-url http://127.0.0.1:32500
```

The endpoint can also be set with `PLEXDIO_PLEX_TV_URL`. The mock auto-approves the PIN after
a couple of polls and serves a JSON Plex API on both the plex.tv and the PMS paths. Other
switches: `--manual-approve`, `--no-sonic` (exercise the radio fallback), `--quiet`.

`mockplex.exe` is also cross-compiled (`cargo build --release -p mockplex --target
x86_64-pc-windows-gnu`) so the player can be tested on a Windows machine without a network.

## Architecture

```
crates/plexdio
  main.rs            single instance, logging, config, launch
  config.rs          %APPDATA%\PlexDio\config.json
  secure_store.rs    Credential Manager -> DPAPI file fallback
  logging.rs         rotating file log (1 MB x 3)
  plex/              HTTP client, PIN auth, resource/connection racing, models, endpoints
  cache/             SQLite schema/db, full+delta sync, artwork disk cache, audio cache
  library_model.rs   compact in-memory indexes (interned Arc<str>) for lists and search
  audio/             source (progressive download + range seek), symphonia decoder,
                     mixer (gapless/crossfade), gain, sinks (cpal / null / file)
  player/            queue, shuffle, mixes, radio, reporting, session resume
  ui/                CPU-rendered egui: runner, app state, views, icons, artwork cache
  win/               tray + media keys, SMTC, autostart, single instance, Wine workarounds
crates/mockplex      mock plex.tv + Plex Media Server (tiny_http)
```

Notable choices:

- **CPU-rendered egui** (winit + softbuffer + a software egui renderer) instead of an OpenGL
  backend: the test VM only exposes OpenGL 1.1, and software painting is plenty for a UI at
  four frames a second. It also keeps the import table system-only.
- **On-demand painting.** The UI only redraws for input, for the four-per-second seek-bar
  tick while playing, and when a worker reports progress. Idle is event-driven.
- **Plain threads, no async runtime.** HTTP, sync, decoding and artwork run on worker
  threads and talk to the UI over channels; this keeps RAM and binary size down.
- **No XML.** Every request asks for `application/json`.

Data locations: `%APPDATA%\PlexDio\config.json` (settings, resume queue),
`%LOCALAPPDATA%\PlexDio\db\*.sqlite` (library cache), `…\images` (artwork, LRU),
`…\audio` (audio cache, LRU), `…\logs\plexdio.log`.

## Footprint

Release exe: **9 887 232 bytes (9.43 MiB)**, PE32+ x86-64, system DLLs only.

| Environment | Library | Idle working set | Idle CPU | Playing CPU |
|---|---|---|---|---|
| Windows 11 VM | 25 tracks | 69.8 MB (private 44.6 MB) | 0.05 % of a core | 4.5 % of a core |
| Windows 11 VM | 100 000 tracks | 113.6 MB (private 84.8 MB) | 0.05 % of a core | — |
| Wine 9 on Linux | 25 tracks | 72.1 MB (includes Wine) | ~0.1 % of a core | ~8 % of a core |

The 100 000-track case is the stress case: the compact in-memory index plus the 24 MB SQLite
cache push the *working set* past 100 MB, while private memory stays under it. A library of a
few thousand tracks sits around 70 MB. Sync of 112 000 rows takes 3.1 s on the VM (~200 ms to
rebuild the in-memory model); first paint of the full list is immediate because lists are
virtualised.

## Wine notes

The app runs under Wine with two adaptations, both in `src/win/`:

- winit's hidden "Thread Event Target" window is hidden at start-up. Wine never delivers the
  `WM_PAINT` winit schedules on it, so the event loop would spin a core (see
  `win::fix_wine_event_target`).
- The System Media Transport Controls are not connected on Wine: the stub accepts the
  connection but never raises button events, and keeps the UI thread busy. Media keys still
  work through `RegisterHotKey`.

Wine has no tray area and no audio device, so the tray icon/menu and real sound cannot be
checked there; both were exercised on the Windows VM (tray) or are covered by the null-sink
tests (audio).

## Testing

```sh
cargo test --workspace                                  # 84 tests: parsing, sync, audio, queue, ...
cargo test --workspace --target x86_64-pc-windows-gnu   # same suite under Wine
semgrep scan --config p/default --metrics off crates     # 0 findings
cargo clippy --workspace --all-targets -- -D warnings
```

The integration tests start an in-process mock server and cover the PIN flow, connection
racing with a dead connection, paging, full and delta sync with add/update/delete, offline
reads, the queue and shuffle rules, radio with and without sonic data, and the audio pipeline
(gapless sample counts, crossfade length, ReplayGain application) with generated fixtures.

## What is not verified

- **A real Plex account and a real server.** Everything was tested against `mockplex`. The
  real plex.tv PIN page, real token formats, real library quirks and Plex Pass features
  (`nearest` sonic data, transcoder variants) are untested.
- **Real audio output.** Neither Wine nor the Windows VM has an audio device, so playback ran
  through the null sink. Decoding, mixing and caching are covered by tests and the engine
  logs, but nobody has heard it yet.
- **The tray icon and menu** were only exercised on the Windows VM (the icon appears and the
  log confirms registration); the popup menu's look and the tooltip were not photographed.
- **Hardware media keys** were verified through SMTC (`TryTogglePlayPauseAsync`) and through
  `xdotool` under Wine; an actual keyboard's media keys were not pressed.
- **Long-running behaviour** (hours of playback, cache eviction under real use, sleep/resume)
  is not covered.
- **Queue restore while the server is unreachable.** A normal start resumes the saved queue, track
  and position. If the app starts offline it opens the cached library without restoring the queue
  (playback needs the server); the queue comes back on the next start once the server is reachable.

### Manual checklist for a real Plex account

1. Start `plexdio.exe` on Windows 10/11; click **Sign in with Plex**, complete the browser
   flow, and confirm the server list appears with your own and shared servers.
2. Open a music library; let the first sync finish (progress shows in the sidebar). Restart
   the app and confirm it opens on the library without asking again.
3. Play an MP3, a FLAC and an ALAC/M4A track; check sound, gapless between tracks of an album,
   seek, next/previous and the volume slider.
4. Enable crossfade (5 s) in Settings and confirm the tracks overlap; toggle normalisation and
   compare loudness between a loud and a quiet album.
5. Set a bitrate cap (e.g. 192 kbps) and confirm the log reports `(transcode)` for the next
   track; set it back to Original.
6. Press play/pause/next on the keyboard's media keys; open the Windows media flyout (volume
   keys) and confirm the title/artist and the seek bar; use the flyout's buttons.
7. Minimise to tray, close to tray, restore from the tray icon; toggle the mini-player
   (`Ctrl+M`); check it stays on top.
8. Search (`Ctrl+F`) for an artist with accents; check the results match the server.
9. Start a station from a track and from an artist; let a queue run out with autoplay on.
10. In the mock's place, rate a track 5 stars and check it appears in Most played after a
    while; sign out and back in to confirm the token is cleared and re-stored.
