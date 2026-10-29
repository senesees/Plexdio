# Feature plan

Plan for the next batch of features. Each item lists what it does, the Plex API it needs, where it
lands in the code, what mockplex must learn so it can be tested, and the risks. Sizes are rough:
S ≈ half a day, M ≈ 1–2 days, L ≈ 3+ days.

Ground rules for all of them:
- Server writes go through `PmsClient` (`plex/server.rs`) on a worker thread (`ui/jobs.rs`), never on the UI thread.
- Change the local cache first (optimistic), then call the server, then re-sync just that object; on
  failure roll back and show the existing banner.
- Everything that needs the server is disabled offline (`self.offline`) with a tooltip saying why.
- Every new endpoint gets a mockplex route and an integration test in `crates/plexdio/tests/`.
- New settings go into `config.rs` with serde defaults, so old config files still load.

---

## Phase 1 — editing and browsing basics

### 1. Playlist editing (L)

**User-facing**
- Playlists page: a **New playlist** button that opens a name dialog.
- Playlist page: a ⋯ menu with **Rename** and **Delete** (Delete asks for confirmation). Per row: **Remove
  from playlist**, plus drag-to-reorder through a handle on hover. None of this is shown for smart playlists.
- Track right-click menu (and the multi-select menu, #4): **Add to playlist ▸** lists the regular
  playlists, with **New playlist…** at the top. Album, artist and mix pages get the same item in the button row.
- When the song is already in the playlist, ask "Add anyway / Skip".

**Plex API** (taken from python-plexapi `playlist.py`)
- Create: `POST /playlists?type=audio&title=…&smart=0&uri=server://{machineId}/com.plexapp.plugins.library/library/metadata/{id,id,…}`
- Add: `PUT /playlists/{id}/items?uri=…` (same uri form)
- Remove: `DELETE /playlists/{id}/items/{playlistItemID}`
- Move: `PUT /playlists/{id}/items/{playlistItemID}/move?after={playlistItemID}` (leave out `after` to move to the top)
- Rename: `PUT /playlists/{id}?title=…` — **check on a real server**. The fallback is
  `PUT /library/metadata/{id}?title.value=…&title.locked=1`.
- Delete: `DELETE /playlists/{id}`
- `machineId` comes from `identity()`: store it once when the server opens.

**Code**
- `plex/server.rs`: `create_playlist`, `add_to_playlist`, `remove_playlist_item`, `move_playlist_item`,
  `rename_playlist`, `delete_playlist`. `Http::send` already covers PUT and DELETE.
- `library_model::Playlist`: keep `item_ids: Vec<Option<i64>>` next to `items`. The DB already stores
  `playlist_items.item_id`; it just isn't loaded. Remove and move need these ids, not the track ids.
- `cache/db.rs`: local edits (insert, delete, reorder rows) plus a `refresh_playlist(id)` that re-fetches
  one playlist's items through the existing `set_playlist_items`.
- New `UiAction`s: `CreatePlaylist{title, ids}`, `AddToPlaylist{playlist, ids}`, `RemoveFromPlaylist{playlist, rows}`,
  `MovePlaylistItem{playlist, from, to}`, `RenamePlaylist`, `DeletePlaylist`. Handled in `app.rs` `apply_action`.
- Reordering reuses the drag logic of the queue panel in `app.rs`.

**mockplex**: keep playlists mutable (`Mutex`) and implement the six routes. **Tests**: create → add →
move → remove → rename → delete, then compare the cache with the mock's state.

**Risks**: the rename parameter; playlists on other servers or libraries (only show playlists whose items
are in this library); `composite` art goes stale (re-fetch the list after edits).

### 3. Library sorting, filters and the A–Z jump bar (M)

Some of this exists already: Albums sort by Title/Artist/Year/Added, Artists by Name/Added, and Tracks by
seven keys with a direction toggle (`ui/views/library.rs`).

**Add**
- Sorts: Albums by *Most played* and *Rating*, Artists by *Most played* and *Album count*, and a ↑/↓
  direction toggle on every tab. Replace the row of selectable labels with one "Sort ▾" combo so the toolbar fits narrow windows.
- Filter chips next to the filter box: **Unplayed**, **Rated** (any rating / ≥3★ / 5★), **Decade ▾**,
  **Genre ▾**, and later **Downloaded** (#5). They combine with the text filter.
- A–Z jump bar: a thin column of letters (`#`, A–Z) on the right of Artists, Albums and Tracks, shown only
  when sorting by name or title. A click scrolls the virtualised list or grid to the first match, using
  `ScrollArea::vertical_scroll_offset(row * row_h)`. Letters with no entries are dimmed. Use the same
  `sort_key` folding as the sort (accents removed, "The " ignored).
- Remember the sort, direction and filters in `config.rs` per tab.

**Code**: `BrowseState` gets `filters` and the memo fingerprint includes them; small helpers in `library.rs`; album plays
and rating are summed from tracks once per `library_ver`. **Tests**: unit tests for the sort/filter
functions and for the letter → index lookup.

### 4. Multi-select in track lists (M)

- Click selects, Ctrl+click toggles, Shift+click selects a range, Ctrl+A selects all, Esc clears. Double-click
  and the hover play button keep working.
- Right-clicking inside the selection opens a menu for all of it: Play, Play next, Add to queue, Add to
  playlist ▸, Remove from playlist (on playlist pages), Rate.
- With two or more rows selected, a small bar appears over the list: "N selected · Play · Add to queue · Add to playlist · ✕".

**Code**: a `Selection { list: salt, rows: BTreeSet<usize>, anchor }` on the app, passed in through `ViewCtx`.
`track_row` draws the selected state and handles the modifiers, and `track_menu` gets an `ids: &[Id]` target. The selection clears on navigation or when the list changes
(library_ver or playlist edit). The queue panel can use it later.

---

## Phase 2 — information

### 8. Track info dialog (S–M)

The right-click menu gets **Track info**, which opens a modal with:
- Title, artist, album, track/disc number, year, genre, duration.
- Format (container/codec), bitrate, sample rate, bit depth, channels, file size, file path on the server.
- Plays (server and local), last played, rating, date added, ReplayGain track/album values.
- For the playing track: **how it plays**, either "Original file (direct)" or "Converted by server to N kbps",
  plus the decoder's output rate → the sink's rate.

**Data**: the cache keeps codec, container and bitrate (the `tracks.bitrate` column isn't loaded into
`Track` yet). Sample rate, bit depth, channels, size and file path come from `GET /library/metadata/{id}`
(`PmsClient::metadata`, already there) when the dialog opens; offline, show only the cached fields.
Add `sampling_rate`, `bit_depth` and `channels` to `models::Stream`. The playback side comes from the
engine: add `direct: bool`, `kbps`, `source_rate` and `sink_rate` to `TrackInfo`/`PlaybackStatus` (`audio/engine.rs`).

### 12. Artist bio (S)

The artist summary is already synced (`artists.summary` → `Artist.summary`). Show it under the hero on the
artist page as three lines with a fade and **Read more**, which expands it in place. Hide it when empty.
While here, sync `albums.summary` too (schema migration: `ALTER TABLE … ADD COLUMN`, then a full sync)
and show it the same way on album pages.

### 2. Lyrics (M–L)

**Plex API**: lyrics are a `Stream` with `streamType=4` under Media → Part in `GET /library/metadata/{id}`.
Its `key` (`/library/streams/{id}`), `format` (`lrc` or `txt`) and `timed` (sometimes the string "1") are what we need.
The content is fetched from that `key`. **Check on a real server** whether it comes back as plain LRC text
or needs `?format=…`. The sources are sidecar `.lrc`/`.txt` files, and LyricFind for Plex Pass users.
- Add `key`, `format` and `timed` (lenient bool) to `models::Stream`, and treat streamType 2 as the only audio stream.
- New `player/lyrics.rs`: an LRC parser (`[mm:ss.xx]` lines, several stamps per line, `[offset:]`)
  returning `Vec<(ms, String)>`, with plain text as the fallback. Unit-test it heavily.
- Cache fetched lyrics in SQLite (`lyrics(track_id, timed, body, fetched_at)`) so they work offline and
  aren't fetched again. Remember "no lyrics" too, with a TTL.

**UI**: a lyrics button in the now-playing bar (next to the queue button) opens a right-hand panel, like the queue
panel. For timed lyrics the current line is highlighted and auto-scrolled to the middle; clicking a line seeks to it,
and scrolling by hand pauses auto-scroll for a few seconds. Plain lyrics just scroll. The panel shows
"No lyrics for this song" when there are none. The UI already repaints 4×/s while playing; lyrics need
about 10×/s while the panel is open (see the repaint note under #visualiser).

**mockplex**: add a lyric stream to two fixture tracks (one LRC, one TXT).

### 13. Smarter search (M)

- More sections in `search_view`: **Genres** (from `lib.genres`, opening `Route::Genre`) and **Playlists**
  (from `lib.playlists`, opening `Route::Playlist`), in the same folded prefix/substring match as `LocalResults`.
- A **Top result** card first: the best-scoring artist, album or track, with a play button.
- Keyboard: ↓/↑ move a highlight through all results in order (top result → artists → albums → tracks → …)
  while focus stays in the box; Enter opens (or plays a track), Shift+Enter adds to the queue, Esc
  clears and then leaves. The highlighted item is scrolled into view (`scroll_to_rect`).
- Code: `SearchState` gets `cursor: Option<usize>`, and the results are flattened into a `Vec<Hit>` once per query.

---

## Phase 3 — offline and display

### 5. Downloads for offline listening (L)

**User-facing**
- A **Download** toggle on album, playlist and mix pages (and in the right-click menu for one song). While
  downloading it shows a progress ring; when done, a green arrow.
- A small arrow on downloaded rows in track lists, and a **Downloaded** filter in the Library (#3).
- Settings → Downloads: quality (Original or the streaming cap), folder, space used, "Remove all downloads",
  and "Only download on Wi-Fi/LAN" (uses `Http::is_lan_ip_url` on the active connection).
- Downloaded playlists follow server changes: after each sync, new items are downloaded and removed items deleted.

**Code**
- New `cache/offline.rs`: a pinned store `%LOCALAPPDATA%\Plexdio\offline\{server}\{track}.{ext}` that the
  `AudioCache` LRU never touches, plus `offline_items(kind, id)` and `offline_tracks(track_id, path, bytes,
  quality)` tables in the library DB.
- A download worker: one queue, two at a time, resuming with HTTP Range (as `ProgressiveSource` does),
  writing to `.part` and then renaming. Progress goes to the UI over the usual channel.
- Playback: when building a `TrackSpec`, prefer `SourceSpec::Local(path)` if the track is downloaded (online too: faster).
- **Offline playback**: today an offline start doesn't restore the queue and playback assumes a server
  (README: "playback needs the server"). Downloaded tracks must play with `pms = None`: timeline and
  scrobble go into a small "pending reports" table and are sent once a connection is back.
- Artwork: pin the thumbs of downloaded albums in the image cache (or copy them into the offline folder) so the LRU can't evict them.

**Risks**: disk space (check free space before starting, and stop cleanly when it runs out); transcoded
downloads (use the universal transcoder URL with a download profile, or allow "Original" only at first);
the server id changing after it is re-added.

### Zoom / text size (S)

- Settings → Window: **Zoom** 80–150 % in 10 % steps. Ctrl+= / Ctrl+− / Ctrl+0 everywhere, with a brief toast
  "Zoom 120 %".
- Use egui's `ctx.set_zoom_factor` (`ui/runner.rs` already passes `pixels_per_point` through). Persist it in config.
- Check: the title bar hit-testing and edge resize (they work in points, which should be fine); the 760×480
  minimum window size (scale it with zoom); the mini-player keeps 100 % (its window size is fixed).
  Cost: the software renderer cost follows pixels, not zoom, so there's no impact.

### Visualiser in the now-playing bar and mini-player (M)

- A compact spectrum (about 24 bars) in the now-playing bar behind or next to the seek bar, and along the
  bottom of the mini-player. Settings: Off / Bars / Wave, **off by default**.
- Audio side: the mixer copies the last 1024 post-EQ mono samples into a lock-free buffer shared with the UI
  (`Arc<[AtomicU32]>` plus a write index, no locks on the audio thread). The UI runs a small hand-written
  radix-2 FFT (no new crate; about 40 lines), log-spaced bands, and attack/decay smoothing.
- **Repaint cost**: the UI only paints on demand (4×/s while playing). A visualiser needs about 30 fps, so
  repaint at 30 fps only while it is on, visible, playing and the window isn't minimised. Measure CPU on the
  VM before and after (README footprint table). If it's above ~5 % of a core, drop to 20 fps or repaint
  only the visualiser's region.
- Bars use `theme::ACCENT` and fall back to flat when paused.

### Cast to other Plex players (L, highest risk)

**User-facing**: a **Cast** button in the now-playing bar lists the Plex players that are online (Plexamp
headless and mobile, Plex for Windows/TV, …), with "This computer" at the top. Picking one moves the current
queue and position there and stops local playback. The now-playing bar then mirrors the remote player and
its controls send remote commands. "This computer" takes playback back at the remote position.

**Plex API** (from python-plexapi `client.py`)
- Discovery: plex.tv resources with `provides` containing `player`, and the server's `GET /clients`.
- Queue: `POST /playQueues?type=audio&uri=server://{machineId}/…/library/metadata/{ids}&key=…&shuffle=0` → `playQueueID`.
- Commands, sent through the server so NAT/firewalls don't matter: `GET /player/playback/playMedia?…`
  with the header `X-Plex-Target-Client-Identifier: {clientId}` and an increasing `commandID`. The parameters are
  `containerKey=/playQueues/{id}?window=100&own=1`, `key`, `machineIdentifier`, `address`, `port`,
  `protocol`, `offset`, `type=music` and `token`. After that: `playback/play|pause|skipNext|skipPrevious|seekTo?offset=`
  and `playback/setParameters?volume=`.
- State: `GET /player/timeline/poll?wait=1&commandID=…` (long poll) for position, state and the current track.

**Code**: a new `player/remote.rs` with a `RemotePlayer` that offers the same command set as `Engine`.
`app.rs` switches which one the transport controls talk to. While casting: local audio stops, SMTC and Discord
mirror the remote state, and reporting is left to the remote player.

**mockplex**: a fake player in `/clients` that accepts the commands and serves the timeline poll.
**Risks**: player support for the companion protocol varies, and Plexamp desktop may not show up as a
target. Long-poll timeouts and players dropping off need retries. This can only really be checked
against real devices, so build it last and behind an "experimental" label.

---

## Order and dependencies

1. **Phase 1**: Playlist editing → Multi-select (reuses its "Add to playlist") → Library sort/filters/jump bar.
2. **Phase 2**: Artist bio → Track info → Smarter search → Lyrics.
3. **Phase 3**: Zoom → Visualiser → Downloads (needs the offline-playback groundwork) → Cast (experimental).

Cross-cutting changes made along the way:
- `models::Stream`: `key`, `format`, `timed`, `sampling_rate`, `bit_depth`, `channels`; streamType filtering.
- Schema changes as `ALTER TABLE` steps guarded by a `meta` schema version (none exists yet; add it with the first change).
- Repaint scheduling: one "wanted frame rate" picked from the active features (base 4 fps, lyrics 10, visualiser 30).
- README: feature list, Status table and the manual checklist for a real account updated per feature.

## Decisions

- Downloads: original quality only (no transcoded downloads).
- Cast: list every castable player found; support casting to several players at once if the protocol allows
  (one play queue, `playMedia` sent to each selected player, the first one is the "leader" mirrored in the bar).
- Visualiser: in both the now-playing bar and the mini-player.
