# Plexdio

A small, native Windows 10/11 x64 music player for Plex — a lean alternative to Plexamp.
One self-contained exe (about 25 MB, most of it Ride's beat-tracking model), no installer, no
runtime, no Electron.

*Formerly **Plexify**, then briefly **PlexDio**. Settings, the library cache, the saved sign-in and the start-with-Windows
entry carry over automatically on first start.*

![Home](docs/home.png)

| Album | Artist |
|---|---|
| ![Album page with the playing track and the hover play button](docs/album.png) | ![Artist page with Discography and Fans also like](docs/artist.png) |
| **Radio** | **Settings** |
| ![Song radio page](docs/radio.png) | ![Settings with the equaliser](docs/settings.png) |

Mini-player:

![Mini-player](docs/mini.png)

Now playing visualiser (`V`, `F11` for full screen):

![Now playing page with the Tunnel visualiser and the song overlay](docs/now-playing.png)

Its nine retro looks, inspired by the Xbox dashboard, Windows Media Player and the PlayStation's
Sound Scope:

![The nine visualiser looks: Scope, Rings, Dashboard, Ambient, Kaleido, Polygon, Terrain, Tunnel and Cloud](docs/visualiser-looks.png)

**Ride**, an Audiosurf-style game on the song that is playing (`G` on the Now playing page):

![Riding a song: the road, coloured notes to catch and the hover ship over the Tunnel visualiser](docs/ride.png)

Ride in a hover ship, a car or a plane:

![The hover ship, the car and the plane](docs/ride-craft.png)

## What it does

- **Sign in** with the Plex PIN flow: the code is shown in the app, the browser opens the
  plex.tv page, and the token is stored in the Windows Credential Manager (DPAPI-encrypted
  file as a fallback).
- **Pick a server and library**: every connection of a server is raced and ranked
  (local > remote > relay), dead connections are skipped, the choice is remembered. The
  sidebar shows the open library and server and switches between them.
- **Offline-first library**: a per-server SQLite cache with a full and a delta sync
  (`updatedAt>>=`/`addedAt>>=` plus a count-check sweep). The UI opens the cached library
  immediately, even with the server unreachable.
- **Browsing**: Home (recently added, **Made for you** mixes, playlists), Artists / Albums /
  Tracks / Genres, artist/album/playlist pages, virtualised lists that stay smooth with
  100 000 tracks. Artist pages scroll as one page: Popular, Discography, **Fans also like**
  (similar artists from the server's related hubs, or genre matches from your library) and
  the artist's bio; album pages show the album's description.
- **Made for you**: Most played, Recently played, Your favourites, Rediscover, Forgotten
  favourites, New additions, Deep cuts, and mixes for your top genres and decades. Each opens
  as its own page, like a playlist.
- **Library sorting and filters**: sort by name, artist, year, date added, plays or rating
  (reversible); filter chips for Unplayed, Downloaded, rating, decade and genre; an A–Z jump bar
  for name order. The choices are remembered.
- **Playlist editing**: create, rename and delete playlists; *Add to playlist* from any song's
  right-click menu, the selection, or an album/mix/playlist page (with a prompt for songs
  already in it); remove songs and drag them into a new order. Smart playlists stay read-only.
- **Multi-select**: click, `Ctrl`+click, `Shift`+click and `Ctrl+A` in any song list; a bar and
  the right-click menu play, queue, add to a playlist or remove the selection.
- **Instant search**: in-memory, diacritic-folding prefix/substring search over artists,
  albums, songs, playlists and genres with a **Top result**, plus a server `/hubs/search`
  supplement. `Ctrl+F`; `↑`/`↓` move through the results, `Enter` opens (or plays) and
  `Shift+Enter` adds to the queue.
- **Lyrics**: a side panel with the server's lyrics (sidecar `.lrc`/`.txt`, or LyricFind with
  Plex Pass). Time-synced lyrics follow the song, centred on the current line; click a line to
  jump there. Lyrics are cached, so they also show offline.
- **Track info**: format, bitrate, sample rate/bit depth, channels, size, file path, plays,
  dates and ReplayGain, and for the playing song how it plays (original or converted, and any
  resampling).
- **Downloads for offline listening**: keep albums, playlists, mixes or single songs on this
  computer in original quality (Opus as 320 kbps MP3, which the player can decode). Downloaded
  playlists follow changes on the server. Downloaded songs play without a connection; plays made
  offline are reported once the server is back. Optional: download only on the home network.
- **Cast** (experimental): play on other Plex players on your account or server (Plexamp, Plex
  apps), several at once if you like. The now-playing bar shows and controls what they play;
  *This computer* takes the music back where they were.
- **Playback**: gapless and crossfade (0–12 s), ReplayGain/loudness normalisation, a
  **10-band equaliser** with presets, seek, volume, mute, prefetch and an on-disk audio
  cache. Codecs are decoded in-process with Symphonia (MP3, FLAC, AAC/M4A, ALAC, Vorbis,
  WAV); anything else — and Opus — goes through the server's universal transcoder. A dropped
  connection mid-song reconnects and resumes the download where it stopped (HTTP `Range`).
- **Now-playing bar**: Spotify-style three columns that fit any window width. Click the
  cover, title or album to open the album, the artist to open the artist. In track lists, a
  single click on the hover play icon plays the song (double-click a row still works).
- **Queue**: add next / add to queue, drag reorder, remove, clear, shuffle (weighted by rating
  and play count, artist-spread) and repeat off/all/one. The queue, track and position
  resume on the next start.
- **Radio and autoplay**: start a station from a track, artist or album; it opens on its own
  **Radio page** listing the station's tracks ("With A, B, C and more"). Sources: sonic
  `nearest` when the server offers it, else same artist/genre from the cache. When the queue
  runs out, autoplay picks up from the last track.
- **Reporting**: `/:/timeline` while playing (every 10 s and on state changes), `/:/scrobble`
  at 90 %, and star ratings through `/:/rate`.
- **Discord Rich Presence** (optional): your Discord status shows "Listening to Plexdio" with
  the song, artist, album cover and a progress bar while music plays; pausing clears it. See
  [Discord setup](#discord-setup).
- **Visualiser** (optional): spectrum bars or a waveform behind the player controls and along
  the mini-player, from the audio actually being heard.
- **Now playing visualiser**: a full-window (or full-screen, `F11`) page with nine retro looks in
  the style of the original Xbox dashboard, Windows Media Player 7–10, the PlayStation's Sound
  Scope and the console light synths: Scope, Rings, Dashboard, Ambient, Kaleido, Polygon,
  Terrain, Tunnel and Cloud. Low-resolution feedback effects and simple 3D, reacting to bass,
  mid, treble and beats; they change by themselves on a beat (or with `↑`/`↓`). The song's cover
  and title fade in on track changes, Xbox style. On pause the picture fades out (the Dashboard
  into a starfield) and drawing stops. Open it with the bar's visualiser button or `V`; `A`
  toggles the automatic changes, a double-click or `F11` toggles full screen, `Esc` leaves.
  Settings → Now playing visualiser picks the look, change interval, resolution, sharp or smooth pixels,
  dithering and frame rate. The picture is scaled straight into the window, so full screen costs
  about a quarter of one CPU core.
- **Ride**: an Audiosurf-style game on the song that is playing, over the visualiser. Press `G`
  (or the Ride button) on the Now playing page at any point in a song; the music never stops.
  - The song is analysed in the background (about a second): onsets in three bands, sustained
    sounds, a steady beat grid, loudness and sections. The **Beat This!** AI model (ISMIR 2024, built into the exe,
    runs on the CPU) then refines the beats in a few more seconds.
  - The analysis becomes a road that climbs through calm parts and drops fast through loud ones,
    turning at section changes, with notes on the beats in five lanes. Bass, mids and highs each
    get a home lane per section and walk around it, and every other phrase mirrors the layout.
    Notes are coloured cool to hot by intensity; grey blocks are to be avoided. **Hold notes** come
    from sustained sounds (held chords, synths, sung notes): a glowing ribbon down the lane. Catch
    its head and stay on it to earn points as you go, plus a bonus for riding it to the end;
    steering off early breaks it, but not your combo. Where the music moves on over the held
    sound, the ribbon slides into the neighbouring lanes and you follow it (more bends on harder
    levels).
  - Steer with the mouse (its position across the view) or `←`/`→`. Catch notes, avoid greys and
    build a combo for a ×2–×4 multiplier. A catch flashes, throws sparks and a shockwave ring and
    pops up its points; a grey flashes red and shakes the camera; missed notes fly past. You can't
    fail: every ride goes to the end of the song.
  - The ride starts wherever the song is (with a 2 s run-up) and carries on into the next song,
    whose track is built while you ride. Each song ends with a results card (score, accuracy,
    best combo, personal best).
  - Restarting the song (Previous, or seeking to the start) rides it again. `Esc` stops.
  - Tracks and scores are kept in the library database. Settings → Storage shows the saved tracks
    and can clear them; your scores stay.
  - Settings → Ride: difficulty (Casual, Pro, Elite; scores per level), hover ship, car or plane,
    30 or 60 fps, an audio delay for wireless headphones, and the AI on or off.
- **Zoom**: 80–150 % (`Ctrl+=`, `Ctrl+−`, `Ctrl+0`, or Settings), remembered.
- **Windows integration**: a themed title bar (drag, double-click to maximise, resize from any
  edge, live redraw while resizing), tray icon with menu, minimise/close to tray, media keys
  (`RegisterHotKey`, plus the System Media Transport Controls media overlay with metadata,
  status and seek timeline), start with Windows, a compact always-on-top mini-player (no
  title bar; drag it anywhere), windows that come back on screen by themselves when a
  resolution change, an unplugged monitor or waking from sleep strands them, and keyboard
  shortcuts (`Space`, arrows, `Ctrl+←/→`,
  `Ctrl+↑/↓`, `Ctrl+M`, `Ctrl+F`).
- **Settings**, as cards of rows with switches: streaming quality (original or a bitrate cap
  via the transcoder), crossfade, normalisation, autoplay, the equaliser, window behaviour,
  Discord, zoom, visualiser, downloads, image and audio cache sizes with usage and clear
  buttons, account/sign-out. Two columns on wide windows, one on narrow ones.

### Equaliser

Ten peaking filters (31 Hz – 16 kHz, one octave apart, ±12 dB) on the final mix, with presets
(Flat, Bass boost/reducer, Treble boost, Vocal, Rock, Pop, Jazz, Classical, Electronic,
Loudness). The editor draws the filters' real combined frequency response; drag a handle to
change a band, double-click it to reset it. Changes are heard immediately and saved when you
let go. To avoid clipping, the mix is pre-cut by half of the largest boost, then limited.
With the equaliser off or flat it is bypassed completely (bit-exact output).

### Discord setup

Plexdio can show what you are listening to on your Discord profile ("Listening to Plexdio",
with the song, artist, album and a progress bar). Discord needs an *application* for that,
which you create once for free; it takes about two minutes.

1. Open the [Discord Developer Portal](https://discord.com/developers/applications) and sign in
   with your Discord account.
2. Click **New Application**, name it **Plexdio** (this name is what your status shows after
   "Listening to"), accept the terms and click **Create**.
3. On the **General Information** page, click **Copy** under **Application ID**.
4. *Optional, for the Plexdio logo:* open **Rich Presence → Art Assets**, click
   **Add Image(s)**, upload [`assets/discord-icon.png`](assets/discord-icon.png) (or any square
   image of at least 512×512), set its name to `plexdio` exactly, and click **Save Changes**.
   New images can take a few minutes to show up. The logo appears as a small badge on the
   album cover, or on its own when no cover is found.
5. In Plexdio open **Settings → Discord**, paste the ID into **Application ID**, and switch on
   **Show what I'm listening to**.
6. Keep the Discord desktop app running and play a song; your status appears within a few
   seconds. Pausing or stopping clears it.

If nothing shows up, check that *Discord → User Settings → Activity Privacy → Share your
detected activities with others* is on, and that you use the desktop app (the browser version
cannot see other apps).

**Album covers.** Discord can only show pictures from the public internet, and your Plex
artwork lives on your own server behind your Plex token, which Plexdio never shares. So the
cover is looked up by artist and album name on [Deezer](https://www.deezer.com) and, if that
finds nothing, Apple's iTunes Search; only an exact artist and album match is used, so you see
the logo rather than a wrong cover. This sends the artist and album names (nothing else) to
those services; turn off **Settings → Discord → Album covers** to keep them local.

The setting is off by default because it shares your listening activity. Plexdio talks to the
Discord desktop app over its local IPC pipe, retries every 15 s while Discord is closed, and
stays within Discord's rate limit (5 updates per 20 s). For a build with a built-in ID, put
it into `CLIENT_ID` in `crates/plexdio/src/discord.rs`; `PLEXDIO_DISCORD_CLIENT_ID` also
works for testing. An ID entered in Settings always wins.

## Status

Tested against **mockplex**, the mock Plex server in this repository, under Wine and on a
real Windows 11 VM. **It has never talked to a real Plex server or plex.tv.**

| Area | Verified |
|---|---|
| Cross-build, PE32+ x86-64, system-DLL-only imports | ✅ every build (`./build.sh`) |
| PIN sign-in, token storage, server/library selection | ✅ mock server, Wine + Windows 11 |
| Full/delta sync, offline start from cache, 100 000 tracks | ✅ mock server, Wine + Windows 11 |
| Browsing, artwork, virtualised scrolling, search | ✅ Wine + Windows 11 screenshots |
| Gapless, crossfade, gain, seek, prefetch | ✅ sample-accurate unit/integration tests |
| Equaliser (response, bypass, presets, editor) | ✅ unit tests + Wine (editor, saved settings) |
| Download reconnect after a dropped connection | ⚠️ code review only (no fault-injection test yet) |
| Queue, shuffle/repeat, resume, timeline/scrobble/rating | ✅ tests + mock request log |
| Radio page, autoplay, Home mixes, Fans also like | ✅ mock server (sonic on and off) + Wine |
| Playlist editing (create/add/move/remove/rename/delete) | ✅ integration tests + Linux build against the mock |
| Multi-select, library sorts/filters/jump bar, search keys | ✅ unit tests + Linux build against the mock |
| Lyrics (LRC/TXT/JSON parsing, synced panel, cache) | ✅ unit tests + mock lyric streams |
| Downloads (pins, resume, offline play, pending scrobbles) | ✅ integration tests + Linux build offline; download on Windows 11 VM |
| Cast / multi-cast (discovery, play queue, commands, timeline) | ✅ against the mock's fake players only |
| Visualiser, zoom, track info | ✅ unit tests + Linux build screenshots |
| Ride (analysis, AI beats, maps, steering, scores, carry-on, settings) | ✅ unit tests on synthetic drums, preview tool on real songs, Linux build + Windows 11 VM against the mock (60 fps; Beat This! ~2 % of song length) |
| Now playing visualiser (9 looks, full screen, pause fade) | ✅ unit tests, preview tool, Linux build, Windows 11 VM full screen (~26 fps, ~24 % of a core) |
| Discord Rich Presence protocol | ✅ test against a fake Discord socket; Wine (no Discord → retries quietly) |
| Themed title bar, mini-player, settings layout | ✅ Wine screenshots |
| Off-screen window rescue (display change, sleep) | ✅ unit tests + Windows 11 VM (window pushed to 5000,5000 came back in 3 s) |
| Settings persistence, tray, media keys, shortcuts | ✅ Wine |
| SMTC overlay (metadata, status, timeline, buttons) | ✅ Windows 11 VM |
| Real audio output | ❌ no audio device in Wine or the VM (null sink path) |
| Real Discord client, Windows live-resize, title bar drag/snap on Windows | ❌ not checked on a real Windows desktop yet |
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

Release profile: `opt-level="z"`, fat LTO, one codegen unit, stripped, static C runtime.

## Running against the mock server

```sh
cargo run -p mockplex -- --port 32500           # fixture library: 5 artists, 7 albums, 25 tracks
cargo run -p mockplex -- --port 32500 --synthetic 100000   # large-library stress test
./dist/plexdio.exe --plex-tv-url http://127.0.0.1:32500
```

The endpoint can also be set with `PLEXDIO_PLEX_TV_URL`. The mock auto-approves the PIN after
a couple of polls and serves a JSON Plex API on both the plex.tv and the PMS paths, including
`/related` hubs (similar artists) and `/nearest` (sonic radio). Other switches:
`--manual-approve`, `--no-sonic` (exercise the radio fallback), `--quiet`.

`mockplex.exe` is also cross-compiled (`cargo build --release -p mockplex --target
x86_64-pc-windows-gnu`) so the player can be tested on a Windows machine without a network.

Environment variables: `PLEXDIO_HOME` (put config and caches in one folder, for side-by-side
test runs), `PLEXDIO_LOG` (`error`…`trace`), `PLEXDIO_PLEX_TV_URL`,
`PLEXDIO_DISCORD_CLIENT_ID`.

## Architecture

```
crates/plexdio
  main.rs            single instance, logging, config, launch, Plexify migration
  config.rs          %APPDATA%\Plexdio\config.json
  secure_store.rs    Credential Manager -> DPAPI file fallback
  logging.rs         rotating file log (1 MB x 3)
  discord.rs         Discord Rich Presence over the local IPC pipe
  cover_art.rs       public album-cover lookup (Deezer, iTunes) for Rich Presence
  plex/              HTTP client, PIN auth, resource/connection racing, models, endpoints,
                     remote control of other players (cast)
  cache/             SQLite schema/db (+ migrations), full+delta sync, artwork disk cache,
                     offline downloads (pins, download worker)
  library_model.rs   compact in-memory indexes (interned Arc<str>) for lists and search
  audio/             source (progressive download, range resume/seek), symphonia decoder,
                     mixer (gapless/crossfade), gain, 10-band equaliser, sinks (cpal / null),
                     sample tap for the visualiser
  player/            mixes, radio, similar artists, reporting, session resume, lyrics
                     parser, cast sessions
  ui/                CPU-rendered egui: runner, app state, views, title bar, EQ editor,
                     icons, artwork cache, visualiser (FFT), retrovis/ (Now playing
                     visualiser: canvas, feedback warps, palettes, analysis, 3D, presets),
                     app/ride.rs (Ride mode on the Now playing page)
  game/              Ride: song analysis (onsets, beat grid, sections), Beat This! AI tier,
                     background build, map generator, song clock, ride rules, 3D renderer
  win/               tray + media keys, SMTC, autostart, single instance, Wine workarounds
crates/mockplex      mock plex.tv + Plex Media Server (tiny_http), including playlist writes,
                     lyric streams and fake remote-controllable players
assets/models        Ride's beat model (ONNX, embedded; see its README)
```

Notable choices:

- **CPU-rendered egui** (winit + softbuffer + a software egui renderer) instead of an OpenGL
  backend: the test VM only exposes OpenGL 1.1, and software painting is plenty for a UI at
  four frames a second. It also keeps the import table system-only.
- **On-demand painting.** The UI only redraws for input, for the four-per-second seek-bar
  tick while playing, when a worker reports progress, and on every size change (Windows'
  modal resize loop would otherwise hold the old frame until the mouse is released).
- **Own title bar.** The native frame is off (`with_decorations(false)`); `ui/titlebar.rs`
  draws the bar and handles move, maximise and edge resizing through winit.
- **Plain threads, no async runtime.** HTTP, sync, decoding, artwork, radio/similar lookups
  and Discord run on worker threads and talk to the UI over channels; this keeps RAM and
  binary size down.
- **No XML.** Every request asks for `application/json`.

Data locations: `%APPDATA%\Plexdio\config.json` (settings, resume queue),
`%LOCALAPPDATA%\Plexdio\db\*.sqlite` (library cache, plus Ride tracks and scores), `…\images`
(artwork, LRU), `…\audio` (audio cache, LRU), `…\models` (Ride's beat model, unpacked from the
exe), `…\logs\plexdio.log`.

## Footprint

Release exe: **25 905 152 bytes (24.7 MiB)**, PE32+ x86-64, system DLLs only. About 11 MB of
that is Ride's beat model and 3 MB its runtime (rten); the player itself is about 10 MB. The model
is unpacked once into `…\models` on first use.

| Environment | Library | Idle working set | Idle CPU | Playing CPU |
|---|---|---|---|---|
| Windows 11 VM¹ | 25 tracks | 69.8 MB (private 44.6 MB) | 0.05 % of a core | 4.5 % of a core |
| Windows 11 VM¹ | 100 000 tracks | 113.6 MB (private 84.8 MB) | 0.05 % of a core | — |
| Wine on Linux | 25 tracks | 84 MB (includes Wine) | 0.10 % of a core | 2.6 % of a core (mini-player, EQ on) |

¹ Measured on the Plexify build before the equaliser, Discord and the new UI; not re-measured
yet. The small visualiser redraws at 30 frames a second while on and playing (it is off by
default); its CPU cost has not been measured on the VM yet.

The **Now playing visualiser** costs about 0.5–2.5 ms per frame for the effect at 320 px wide
(3–4 ms on the VM). Its picture is scaled straight into the window buffer (`VideoFrame` in
`ui/runner.rs`) under egui's shapes, not drawn as an egui image, so the software renderer only
redraws the overlay. On the Windows 11 VM in full screen (1280×800) that is about 26 fps at
**~24 % of one core** (it was ~70 % through egui). It only runs while the page is open and music
plays; after a pause fades out it uses nothing. The log reports its frame rate every 10 s while
the page is open, and `PLEXDIO_PROFILE=1` logs where each frame's time goes (ui, tessellate,
fill, egui render, video, present).

**Ride** draws its game picture at 480 px wide in about 0.4 ms per frame (2.7 ms on the VM), over
the visualiser, which stays at its own frame rate. On the Windows 11 VM in full screen it holds
60 fps at roughly 50 % of one core. That figure is inflated by the mock server's 8-second songs,
which keep fading the song overlay in; 30 fps (Settings → Ride) roughly halves it. Analysing a
song takes about a second (decode and analysis). The Beat This! pass takes about 2 % of the song's
length (about 5 s for a 4-minute song on the VM), at below-normal priority, while the music plays.

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

With Wine's default settings the X11 window manager still draws its own frame over the top
of the window, covering Plexdio's title bar. To see the app as it looks on Windows (the
screenshots above were taken this way), turn that off:

```sh
wine reg add "HKCU\Software\Wine\X11 Driver" /v Decorated /t REG_SZ /d N /f
```

Wine has no tray area, no audio device and no Discord, so the tray icon/menu, real sound and
Rich Presence cannot be checked there. `tools/wine-app.sh stats` measures the app's CPU and
memory under Wine.

## Testing

```sh
cargo test --workspace                                  # 185 tests: parsing, sync, audio, EQ, queue, playlists, downloads, cast, Ride, ...
cargo test --workspace --target x86_64-pc-windows-gnu   # same suite under Wine
semgrep scan --config p/default --metrics off crates     # 0 findings
cargo run --release -p plexdio --example retrovis_preview # renders every Now playing look to target/retrovis/*.png
cargo run --release -p plexdio --example ride_preview -- --ai --frames --clicks SONG...  # Ride analysis timelines, ride shots, beat-click WAVs
cargo clippy --workspace --all-targets -- -D warnings
```

The integration tests start an in-process mock server and cover the PIN flow, connection
racing with a dead connection, paging, full and delta sync with add/update/delete, offline
reads, the queue and shuffle rules, radio with and without sonic data, similar artists from
the related hubs, and the audio pipeline (gapless sample counts, crossfade length,
ReplayGain application) with generated fixtures, playlist editing, downloads (full, resumed
and failing) and casting to the mock's fake players. Unit tests cover the equaliser (bit-exact
bypass, band gain, drawn response), the Discord IPC exchange against a fake Discord socket,
the LRC parser, the visualiser's FFT, the cache schema migration, selection and the library
filters.
