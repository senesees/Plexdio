-- Plexify library cache (one database per server + library section).
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artists (
    id         INTEGER PRIMARY KEY,
    title      TEXT NOT NULL,
    title_sort TEXT,
    thumb      TEXT,
    art        TEXT,
    genre      TEXT,
    summary    TEXT,
    added_at   INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0,
    seen       INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS albums (
    id           INTEGER PRIMARY KEY,
    artist_id    INTEGER,
    title        TEXT NOT NULL,
    artist_title TEXT,
    year         INTEGER,
    thumb        TEXT,
    genre        TEXT,
    leaf_count   INTEGER,
    user_rating  REAL,
    added_at     INTEGER NOT NULL DEFAULT 0,
    updated_at   INTEGER NOT NULL DEFAULT 0,
    seen         INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS albums_artist ON albums(artist_id);

CREATE TABLE IF NOT EXISTS tracks (
    id             INTEGER PRIMARY KEY,
    album_id       INTEGER,
    artist_id      INTEGER,
    title          TEXT NOT NULL,
    album_title    TEXT,
    artist_title   TEXT,
    track_artist   TEXT,
    idx            INTEGER,
    disc           INTEGER,
    year           INTEGER,
    duration_ms    INTEGER,
    part_key       TEXT,
    container      TEXT,
    codec          TEXT,
    bitrate        INTEGER,
    thumb          TEXT,
    genre          TEXT,
    view_count     INTEGER NOT NULL DEFAULT 0,
    last_viewed_at INTEGER,
    user_rating    REAL,
    gain           REAL,
    album_gain     REAL,
    added_at       INTEGER NOT NULL DEFAULT 0,
    updated_at     INTEGER NOT NULL DEFAULT 0,
    seen           INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS tracks_album ON tracks(album_id);
CREATE INDEX IF NOT EXISTS tracks_artist ON tracks(artist_id);

CREATE TABLE IF NOT EXISTS playlists (
    id          INTEGER PRIMARY KEY,
    title       TEXT NOT NULL,
    smart       INTEGER NOT NULL DEFAULT 0,
    leaf_count  INTEGER,
    duration_ms INTEGER,
    composite   TEXT,
    updated_at  INTEGER NOT NULL DEFAULT 0,
    items_synced_at INTEGER
);

CREATE TABLE IF NOT EXISTS playlist_items (
    playlist_id INTEGER NOT NULL,
    pos         INTEGER NOT NULL,
    track_id    INTEGER NOT NULL,
    item_id     INTEGER,
    PRIMARY KEY (playlist_id, pos)
);

-- Plays made from this app (used for "Recently played" / "Most played" mixes even when the
-- server's counters lag behind).
CREATE TABLE IF NOT EXISTS play_history (
    track_id  INTEGER NOT NULL,
    played_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS play_history_track ON play_history(track_id);
