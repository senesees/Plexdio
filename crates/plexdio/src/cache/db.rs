//! SQLite library cache: one database file per (server, library section).

use crate::plex::models::Metadata;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: &str = "1";

pub struct LibraryDb {
    pub conn: Connection,
    pub path: PathBuf,
}

/// Which table a sync step works on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Artist,
    Album,
    Track,
}

impl Kind {
    pub fn table(self) -> &'static str {
        match self {
            Kind::Artist => "artists",
            Kind::Album => "albums",
            Kind::Track => "tracks",
        }
    }
}

fn key_i64(s: &Option<String>) -> Option<i64> {
    s.as_deref().and_then(|s| s.parse().ok())
}

/// Safe file name for a server/section pair.
pub fn db_file_name(server_id: &str, section_key: &str) -> String {
    let clean = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect::<String>();
    format!("{}_{}.sqlite", clean(server_id), clean(section_key))
}

pub fn db_path(server_id: &str, section_key: &str) -> PathBuf {
    crate::paths::db_dir().join(db_file_name(server_id, section_key))
}

impl LibraryDb {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let conn = Connection::open(path)?;
        // Readers (UI) and the sync writer may overlap; wait instead of failing.
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch(include_str!("schema.sql"))?;
        let db = Self { conn, path: path.to_path_buf() };
        match db.meta("schema_version")? {
            Some(v) if v == SCHEMA_VERSION => {}
            Some(v) => {
                log::warn!("cache schema {v} != {SCHEMA_VERSION}; clearing cache");
                db.clear()?;
            }
            None => db.set_meta("schema_version", SCHEMA_VERSION)?,
        }
        Ok(db)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(include_str!("schema.sql"))?;
        Ok(Self { conn, path: PathBuf::new() })
    }

    pub fn meta(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn meta_i64(&self, key: &str) -> Option<i64> {
        self.meta(key).ok().flatten().and_then(|v| v.parse().ok())
    }

    /// Remove all cached library data (keeps the schema version).
    pub fn clear(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "DELETE FROM artists; DELETE FROM albums; DELETE FROM tracks; DELETE FROM playlists;
             DELETE FROM playlist_items; DELETE FROM play_history; DELETE FROM meta;",
        )?;
        self.set_meta("schema_version", SCHEMA_VERSION)
    }

    pub fn count(&self, kind: Kind) -> rusqlite::Result<usize> {
        self.conn
            .query_row(&format!("SELECT COUNT(*) FROM {}", kind.table()), [], |r| r.get::<_, i64>(0))
            .map(|n| n as usize)
    }

    /// Mark every row of a kind as unseen (start of a full sweep).
    pub fn mark_unseen(&self, kind: Kind) -> rusqlite::Result<()> {
        self.conn.execute(&format!("UPDATE {} SET seen = 0", kind.table()), [])?;
        Ok(())
    }

    /// Delete rows not seen since `mark_unseen`; returns how many were removed.
    pub fn sweep_unseen(&self, kind: Kind) -> rusqlite::Result<usize> {
        self.conn.execute(&format!("DELETE FROM {} WHERE seen = 0", kind.table()), [])
    }

    /// Upsert a batch of items of one kind inside a transaction.
    pub fn upsert(&mut self, kind: Kind, items: &[Metadata]) -> rusqlite::Result<usize> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        {
            match kind {
                Kind::Artist => {
                    let mut st = tx.prepare_cached(
                        "INSERT INTO artists(id,title,title_sort,thumb,art,genre,summary,added_at,updated_at,seen)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1)
                         ON CONFLICT(id) DO UPDATE SET title=excluded.title,title_sort=excluded.title_sort,
                           thumb=excluded.thumb,art=excluded.art,genre=excluded.genre,summary=excluded.summary,
                           added_at=excluded.added_at,updated_at=excluded.updated_at,seen=1",
                    )?;
                    for m in items {
                        let Some(id) = key_i64(&m.rating_key) else { continue };
                        st.execute(params![
                            id,
                            m.title.as_deref().unwrap_or("Unknown artist"),
                            m.title_sort,
                            m.thumb,
                            m.art,
                            m.genre_names().first(),
                            m.summary,
                            m.added_at.unwrap_or(0),
                            m.updated_at.unwrap_or(0)
                        ])?;
                        n += 1;
                    }
                }
                Kind::Album => {
                    let mut st = tx.prepare_cached(
                        "INSERT INTO albums(id,artist_id,title,artist_title,year,thumb,genre,leaf_count,user_rating,added_at,updated_at,seen)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,1)
                         ON CONFLICT(id) DO UPDATE SET artist_id=excluded.artist_id,title=excluded.title,
                           artist_title=excluded.artist_title,year=excluded.year,thumb=excluded.thumb,genre=excluded.genre,
                           leaf_count=excluded.leaf_count,user_rating=excluded.user_rating,added_at=excluded.added_at,
                           updated_at=excluded.updated_at,seen=1",
                    )?;
                    for m in items {
                        let Some(id) = key_i64(&m.rating_key) else { continue };
                        st.execute(params![
                            id,
                            key_i64(&m.parent_rating_key),
                            m.title.as_deref().unwrap_or("Unknown album"),
                            m.parent_title,
                            m.year,
                            m.thumb,
                            m.genre_names().first(),
                            m.leaf_count,
                            m.user_rating,
                            m.added_at.unwrap_or(0),
                            m.updated_at.unwrap_or(0)
                        ])?;
                        n += 1;
                    }
                }
                Kind::Track => {
                    let mut st = tx.prepare_cached(
                        "INSERT INTO tracks(id,album_id,artist_id,title,album_title,artist_title,track_artist,idx,disc,year,
                           duration_ms,part_key,container,codec,bitrate,thumb,genre,view_count,last_viewed_at,user_rating,
                           gain,album_gain,added_at,updated_at,seen)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,1)
                         ON CONFLICT(id) DO UPDATE SET album_id=excluded.album_id,artist_id=excluded.artist_id,
                           title=excluded.title,album_title=excluded.album_title,artist_title=excluded.artist_title,
                           track_artist=excluded.track_artist,idx=excluded.idx,disc=excluded.disc,year=excluded.year,
                           duration_ms=excluded.duration_ms,part_key=excluded.part_key,container=excluded.container,
                           codec=excluded.codec,bitrate=excluded.bitrate,thumb=excluded.thumb,genre=excluded.genre,
                           view_count=excluded.view_count,last_viewed_at=excluded.last_viewed_at,
                           user_rating=excluded.user_rating,
                           gain=COALESCE(excluded.gain, tracks.gain),album_gain=COALESCE(excluded.album_gain, tracks.album_gain),
                           added_at=excluded.added_at,updated_at=excluded.updated_at,seen=1",
                    )?;
                    for m in items {
                        let Some(id) = key_i64(&m.rating_key) else { continue };
                        let media = m.media.first();
                        let part = m.first_part();
                        let stream = m.audio_stream();
                        st.execute(params![
                            id,
                            key_i64(&m.parent_rating_key),
                            key_i64(&m.grandparent_rating_key),
                            m.title.as_deref().unwrap_or("Unknown track"),
                            m.parent_title,
                            m.grandparent_title,
                            m.original_title,
                            m.index,
                            m.parent_index,
                            m.parent_year.or(m.year),
                            m.duration.or(media.and_then(|x| x.duration)),
                            part.and_then(|p| p.key.clone()),
                            part.and_then(|p| p.container.clone()).or(media.and_then(|x| x.container.clone())),
                            media.and_then(|x| x.audio_codec.clone()),
                            media.and_then(|x| x.bitrate),
                            m.parent_thumb.clone().or(m.thumb.clone()),
                            m.genre_names().first(),
                            m.view_count.unwrap_or(0),
                            m.last_viewed_at,
                            m.user_rating,
                            stream.and_then(|s| s.gain),
                            stream.and_then(|s| s.album_gain),
                            m.added_at.unwrap_or(0),
                            m.updated_at.unwrap_or(0)
                        ])?;
                        n += 1;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// Replace the playlist list; returns ids whose items need (re)fetching.
    pub fn upsert_playlists(&mut self, items: &[Metadata]) -> rusqlite::Result<Vec<i64>> {
        let tx = self.conn.transaction()?;
        let mut stale = Vec::new();
        let mut keep = Vec::new();
        for m in items {
            let Some(id) = key_i64(&m.rating_key) else { continue };
            keep.push(id);
            let prev: Option<(i64, Option<i64>, Option<i64>)> = tx
                .query_row("SELECT updated_at, leaf_count, items_synced_at FROM playlists WHERE id=?1", [id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .optional()?;
            let updated = m.updated_at.unwrap_or(0);
            let changed = match prev {
                Some((u, lc, synced)) => u != updated || lc != m.leaf_count || synced.is_none(),
                None => true,
            };
            if changed {
                stale.push(id);
            }
            tx.execute(
                "INSERT INTO playlists(id,title,smart,leaf_count,duration_ms,composite,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(id) DO UPDATE SET title=excluded.title,smart=excluded.smart,leaf_count=excluded.leaf_count,
                   duration_ms=excluded.duration_ms,composite=excluded.composite,updated_at=excluded.updated_at",
                params![id, m.title.as_deref().unwrap_or("Playlist"), m.smart, m.leaf_count, m.duration, m.composite, updated],
            )?;
        }
        // Drop playlists that no longer exist.
        let existing: Vec<i64> = {
            let mut st = tx.prepare("SELECT id FROM playlists")?;
            let rows = st.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
            rows
        };
        for id in existing.into_iter().filter(|id| !keep.contains(id)) {
            tx.execute("DELETE FROM playlists WHERE id=?1", [id])?;
            tx.execute("DELETE FROM playlist_items WHERE playlist_id=?1", [id])?;
        }
        tx.commit()?;
        Ok(stale)
    }

    pub fn set_playlist_items(&mut self, playlist_id: i64, items: &[Metadata], now: i64) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM playlist_items WHERE playlist_id=?1", [playlist_id])?;
        {
            let mut st = tx.prepare_cached("INSERT INTO playlist_items(playlist_id,pos,track_id,item_id) VALUES (?1,?2,?3,?4)")?;
            for (pos, m) in items.iter().enumerate() {
                if let Some(tid) = key_i64(&m.rating_key) {
                    st.execute(params![playlist_id, pos as i64, tid, m.playlist_item_id])?;
                }
            }
        }
        tx.execute("UPDATE playlists SET items_synced_at=?2 WHERE id=?1", params![playlist_id, now])?;
        tx.commit()
    }

    pub fn record_play(&self, track_id: i64, at: i64) -> rusqlite::Result<()> {
        self.conn.execute("INSERT INTO play_history(track_id, played_at) VALUES (?1, ?2)", params![track_id, at])?;
        self.conn.execute(
            "UPDATE tracks SET view_count = view_count + 1, last_viewed_at = ?2 WHERE id = ?1",
            params![track_id, at],
        )?;
        Ok(())
    }

    /// Locally played track ids, newest first (the same track may repeat).
    pub fn recent_play_ids(&self, limit: usize) -> rusqlite::Result<Vec<i64>> {
        let mut st = self.conn.prepare("SELECT track_id FROM play_history ORDER BY played_at DESC LIMIT ?1")?;
        let rows = st.query_map([limit as i64], |r| r.get(0))?;
        rows.collect()
    }

    /// Local play count per track.
    pub fn play_counts(&self) -> rusqlite::Result<Vec<(i64, u64)>> {
        let mut st = self.conn.prepare("SELECT track_id, COUNT(*) FROM play_history GROUP BY track_id")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)))?;
        rows.collect()
    }

    pub fn set_track_rating(&self, track_id: i64, rating: Option<f64>) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE tracks SET user_rating=?2 WHERE id=?1", params![track_id, rating])?;
        Ok(())
    }

    pub fn set_track_gain(&self, track_id: i64, gain: Option<f64>, album_gain: Option<f64>) -> rusqlite::Result<()> {
        self.conn
            .execute("UPDATE tracks SET gain=?2, album_gain=?3 WHERE id=?1", params![track_id, gain, album_gain])?;
        Ok(())
    }

    /// Largest addedAt/updatedAt seen in the cache: the delta-sync watermark (server clock).
    pub fn watermark(&self) -> rusqlite::Result<i64> {
        self.conn.query_row(
            "SELECT MAX(m) FROM (SELECT MAX(updated_at) m FROM artists UNION ALL SELECT MAX(updated_at) FROM albums
             UNION ALL SELECT MAX(updated_at) FROM tracks UNION ALL SELECT MAX(added_at) FROM tracks)",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
        .map(|v| v.unwrap_or(0))
    }
}
