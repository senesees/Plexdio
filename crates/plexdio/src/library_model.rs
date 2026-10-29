//! Compact in-memory view of the cached library used by the UI (sorting, filtering, search
//! and virtualised lists). Loaded from SQLite in one pass; repeated strings are interned.

use crate::cache::db::{Kind, LibraryDb};
use std::collections::HashMap;
use std::sync::Arc;

pub type Id = i64;

#[derive(Debug, Clone)]
pub struct Artist {
    pub id: Id,
    pub title: Arc<str>,
    pub sort_key: Arc<str>,
    pub thumb: Option<Arc<str>>,
    pub genre: Option<Arc<str>>,
    pub summary: Option<Box<str>>,
    pub added_at: i64,
}

#[derive(Debug, Clone)]
pub struct Album {
    pub id: Id,
    pub artist_id: Id,
    pub title: Arc<str>,
    pub artist: Arc<str>,
    pub year: i32,
    pub thumb: Option<Arc<str>>,
    pub genre: Option<Arc<str>>,
    pub leaf_count: u32,
    pub added_at: i64,
    /// Album review/description from the server.
    pub summary: Option<Box<str>>,
}

#[derive(Debug, Clone)]
pub struct Track {
    pub id: Id,
    pub album_id: Id,
    pub artist_id: Id,
    pub title: Arc<str>,
    pub album: Arc<str>,
    pub artist: Arc<str>,
    /// Per-track artist when it differs from the album artist (compilations, features).
    pub track_artist: Option<Arc<str>>,
    pub index: u16,
    pub disc: u16,
    pub year: i32,
    pub duration_ms: u32,
    pub part_key: Option<Arc<str>>,
    pub codec: Option<Arc<str>>,
    pub container: Option<Arc<str>>,
    pub thumb: Option<Arc<str>>,
    pub genre: Option<Arc<str>>,
    pub view_count: u32,
    pub last_viewed_at: i64,
    /// 0..=10, 0 = unrated.
    pub user_rating: f32,
    pub gain: Option<f32>,
    pub album_gain: Option<f32>,
    pub added_at: i64,
    /// Bitrate of the file as the server reports it (0 when unknown).
    pub bitrate_kbps: u32,
}

impl Track {
    pub fn display_artist(&self) -> &str {
        self.track_artist.as_deref().unwrap_or(&self.artist)
    }
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: Id,
    pub title: Arc<str>,
    pub smart: bool,
    pub leaf_count: u32,
    pub duration_ms: u64,
    pub composite: Option<Arc<str>>,
    pub items: Vec<Id>,
    /// Server `playlistItemID` per entry of `items` (needed to remove and move entries); `None`
    /// for entries added locally that the server has not confirmed yet.
    pub item_ids: Vec<Option<i64>>,
}

#[derive(Debug, Default)]
pub struct LibraryModel {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
    pub artist_idx: HashMap<Id, usize>,
    pub album_idx: HashMap<Id, usize>,
    pub track_idx: HashMap<Id, usize>,
    /// Album -> track indices in disc/track order.
    pub album_tracks: HashMap<Id, Vec<usize>>,
    /// Artist -> album indices, newest first.
    pub artist_albums: HashMap<Id, Vec<usize>>,
    /// Genre -> number of albums, sorted by name.
    pub genres: Vec<(Arc<str>, usize)>,
    pub last_sync_at: Option<i64>,
}

#[derive(Default)]
struct Interner(HashMap<Box<str>, Arc<str>>);

impl Interner {
    fn get(&mut self, s: &str) -> Arc<str> {
        if let Some(a) = self.0.get(s) {
            return a.clone();
        }
        let a: Arc<str> = Arc::from(s);
        self.0.insert(s.into(), a.clone());
        a
    }
    fn opt(&mut self, s: Option<String>) -> Option<Arc<str>> {
        s.filter(|s| !s.is_empty()).map(|s| self.get(&s))
    }
}

/// Sort key: lowercase, leading "the " dropped.
/// Sort key for names and titles: lowercase, common accents folded ("Ça Va" sorts with C, not
/// after Z) and a leading "The " ignored.
pub fn sort_key(s: &str) -> String {
    let l: String = s.trim().chars().map(fold).collect();
    l.strip_prefix("the ").map(str::to_string).unwrap_or(l)
}

/// Lowercase and fold common Latin accents so "deja" finds "Déjà".
pub fn fold(c: char) -> char {
    match c {
        'A'..='Z' => c.to_ascii_lowercase(),
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'ē' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' | 'ń' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        'ś' | 'š' => 's',
        'ź' | 'ż' | 'ž' => 'z',
        'ł' => 'l',
        _ => {
            let lower = c.to_lowercase().next().unwrap_or(c);
            if lower == c {
                c
            } else {
                fold(lower)
            }
        }
    }
}


impl LibraryModel {
    pub fn load(db: &LibraryDb) -> rusqlite::Result<Self> {
        let started = std::time::Instant::now();
        let mut i = Interner::default();
        let mut m = LibraryModel { last_sync_at: db.meta_i64("last_sync_at"), ..Default::default() };
        // Reserve exact-ish capacity: `Track` is large, so letting a Vec of 100k grow by doubling
        // costs several MB (and a full copy) at the last reallocation.
        m.artists.reserve(db.count(Kind::Artist)?);
        m.albums.reserve(db.count(Kind::Album)?);
        m.tracks.reserve(db.count(Kind::Track)?);

        let mut st = db.conn.prepare("SELECT id,title,title_sort,thumb,genre,summary,added_at FROM artists")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let title: String = r.get(1)?;
            let sort: Option<String> = r.get(2)?;
            m.artists.push(Artist {
                id: r.get(0)?,
                sort_key: i.get(&sort_key(sort.as_deref().unwrap_or(&title))),
                title: i.get(&title),
                thumb: i.opt(r.get(3)?),
                genre: i.opt(r.get(4)?),
                summary: r.get::<_, Option<String>>(5)?.map(String::into_boxed_str),
                added_at: r.get(6)?,
            });
        }
        drop(rows);
        m.artists.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));

        let mut st = db.conn.prepare("SELECT id,artist_id,title,artist_title,year,thumb,genre,leaf_count,added_at,summary FROM albums")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            m.albums.push(Album {
                id: r.get(0)?,
                artist_id: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                title: i.get(&r.get::<_, String>(2)?),
                artist: i.get(&r.get::<_, Option<String>>(3)?.unwrap_or_else(|| "Unknown artist".into())),
                year: r.get::<_, Option<i64>>(4)?.unwrap_or(0) as i32,
                thumb: i.opt(r.get(5)?),
                genre: i.opt(r.get(6)?),
                leaf_count: r.get::<_, Option<i64>>(7)?.unwrap_or(0) as u32,
                added_at: r.get(8)?,
                summary: r.get::<_, Option<String>>(9)?.map(String::into_boxed_str),
            });
        }
        drop(rows);
        m.albums.sort_by_cached_key(|a| sort_key(&a.title));

        let mut st = db.conn.prepare(
            "SELECT id,album_id,artist_id,title,album_title,artist_title,track_artist,idx,disc,year,duration_ms,part_key,
                    codec,container,thumb,genre,view_count,last_viewed_at,user_rating,gain,album_gain,added_at,bitrate FROM tracks",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let artist = i.get(&r.get::<_, Option<String>>(5)?.unwrap_or_else(|| "Unknown artist".into()));
            let track_artist = i.opt(r.get(6)?).filter(|t| **t != *artist);
            m.tracks.push(Track {
                id: r.get(0)?,
                album_id: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                artist_id: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                title: i.get(&r.get::<_, String>(3)?),
                album: i.get(&r.get::<_, Option<String>>(4)?.unwrap_or_else(|| "Unknown album".into())),
                artist,
                track_artist,
                index: r.get::<_, Option<i64>>(7)?.unwrap_or(0).clamp(0, u16::MAX as i64) as u16,
                disc: r.get::<_, Option<i64>>(8)?.unwrap_or(1).clamp(0, u16::MAX as i64) as u16,
                year: r.get::<_, Option<i64>>(9)?.unwrap_or(0) as i32,
                duration_ms: r.get::<_, Option<i64>>(10)?.unwrap_or(0).clamp(0, u32::MAX as i64) as u32,
                part_key: i.opt(r.get(11)?),
                codec: i.opt(r.get(12)?),
                container: i.opt(r.get(13)?),
                thumb: i.opt(r.get(14)?),
                genre: i.opt(r.get(15)?),
                view_count: r.get::<_, i64>(16)?.max(0) as u32,
                last_viewed_at: r.get::<_, Option<i64>>(17)?.unwrap_or(0),
                user_rating: r.get::<_, Option<f64>>(18)?.unwrap_or(0.0) as f32,
                gain: r.get::<_, Option<f64>>(19)?.map(|g| g as f32),
                album_gain: r.get::<_, Option<f64>>(20)?.map(|g| g as f32),
                added_at: r.get(21)?,
                bitrate_kbps: r.get::<_, Option<i64>>(22)?.unwrap_or(0).clamp(0, u32::MAX as i64) as u32,
            });
        }
        drop(rows);
        m.tracks.sort_by_cached_key(|t| sort_key(&t.title));

        m.playlists = Self::load_playlists(db)?;

        m.reindex();
        log::info!(
            "library model loaded in {:?}: {} artists, {} albums, {} tracks, {} playlists",
            started.elapsed(),
            m.artists.len(),
            m.albums.len(),
            m.tracks.len(),
            m.playlists.len()
        );
        Ok(m)
    }

    /// Playlists with their items, by title (also used to refresh them after an edit).
    pub fn load_playlists(db: &LibraryDb) -> rusqlite::Result<Vec<Playlist>> {
        let mut i = Interner::default();
        let mut out = Vec::new();
        let mut st = db.conn.prepare("SELECT id,title,smart,leaf_count,duration_ms,composite FROM playlists ORDER BY title COLLATE NOCASE")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            out.push(Playlist {
                id: r.get(0)?,
                title: i.get(&r.get::<_, String>(1)?),
                smart: r.get::<_, i64>(2)? != 0,
                leaf_count: r.get::<_, Option<i64>>(3)?.unwrap_or(0) as u32,
                duration_ms: r.get::<_, Option<i64>>(4)?.unwrap_or(0) as u64,
                composite: i.opt(r.get(5)?),
                items: Vec::new(),
                item_ids: Vec::new(),
            });
        }
        drop(rows);
        let mut st = db.conn.prepare("SELECT playlist_id, track_id, item_id FROM playlist_items ORDER BY playlist_id, pos")?;
        let mut rows = st.query([])?;
        let pl_idx: HashMap<Id, usize> = out.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
        while let Some(r) = rows.next()? {
            if let Some(&pi) = pl_idx.get(&r.get::<_, i64>(0)?) {
                out[pi].items.push(r.get(1)?);
                out[pi].item_ids.push(r.get(2)?);
            }
        }
        Ok(out)
    }

    /// Positions in `p.items` whose track is in the library, in order. Playlist pages list only
    /// these, so table row `r` is playlist position `playlist_rows(p)[r]`.
    pub fn playlist_rows(&self, p: &Playlist) -> Vec<usize> {
        (0..p.items.len()).filter(|&i| self.track(p.items[i]).is_some()).collect()
    }

    pub fn playlist_mut(&mut self, id: Id) -> Option<&mut Playlist> {
        self.playlists.iter_mut().find(|p| p.id == id)
    }

    pub fn reindex(&mut self) {
        self.artist_idx = self.artists.iter().enumerate().map(|(i, a)| (a.id, i)).collect();
        self.album_idx = self.albums.iter().enumerate().map(|(i, a)| (a.id, i)).collect();
        self.track_idx = self.tracks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
        self.album_tracks.clear();
        for (i, t) in self.tracks.iter().enumerate() {
            self.album_tracks.entry(t.album_id).or_default().push(i);
        }
        for v in self.album_tracks.values_mut() {
            v.sort_by_key(|&i| (self.tracks[i].disc, self.tracks[i].index));
        }
        self.artist_albums.clear();
        for (i, a) in self.albums.iter().enumerate() {
            self.artist_albums.entry(a.artist_id).or_default().push(i);
        }
        for v in self.artist_albums.values_mut() {
            v.sort_by_key(|&i| std::cmp::Reverse(self.albums[i].year));
        }
        let mut genres: HashMap<Arc<str>, usize> = HashMap::new();
        for a in &self.albums {
            if let Some(g) = &a.genre {
                *genres.entry(g.clone()).or_default() += 1;
            }
        }
        let mut g: Vec<_> = genres.into_iter().collect();
        g.sort_by(|a, b| a.0.cmp(&b.0));
        self.genres = g;
    }

    pub fn is_empty(&self) -> bool {
        self.artists.is_empty() && self.albums.is_empty() && self.tracks.is_empty()
    }

    pub fn track(&self, id: Id) -> Option<&Track> {
        self.track_idx.get(&id).map(|&i| &self.tracks[i])
    }
    pub fn album(&self, id: Id) -> Option<&Album> {
        self.album_idx.get(&id).map(|&i| &self.albums[i])
    }
    pub fn artist(&self, id: Id) -> Option<&Artist> {
        self.artist_idx.get(&id).map(|&i| &self.artists[i])
    }
    pub fn playlist(&self, id: Id) -> Option<&Playlist> {
        self.playlists.iter().find(|p| p.id == id)
    }

    /// Tracks of an album in play order.
    pub fn album_track_ids(&self, album_id: Id) -> Vec<Id> {
        self.album_tracks.get(&album_id).map(|v| v.iter().map(|&i| self.tracks[i].id).collect()).unwrap_or_default()
    }

    /// All tracks of an artist, album by album (newest album first).
    pub fn artist_track_ids(&self, artist_id: Id) -> Vec<Id> {
        let mut out = Vec::new();
        for &ai in self.artist_albums.get(&artist_id).map(Vec::as_slice).unwrap_or(&[]) {
            out.extend(self.album_track_ids(self.albums[ai].id));
        }
        out
    }

    /// Album indices, most recently added first.
    pub fn recently_added_albums(&self, n: usize) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.albums.len()).collect();
        v.sort_by_key(|&i| std::cmp::Reverse(self.albums[i].added_at));
        v.truncate(n);
        v
    }

    pub fn apply_play(&mut self, track_id: Id, at: i64) {
        if let Some(&i) = self.track_idx.get(&track_id) {
            self.tracks[i].view_count += 1;
            self.tracks[i].last_viewed_at = at;
        }
    }

    pub fn apply_rating(&mut self, track_id: Id, rating: f32) {
        if let Some(&i) = self.track_idx.get(&track_id) {
            self.tracks[i].user_rating = rating;
        }
    }
}
