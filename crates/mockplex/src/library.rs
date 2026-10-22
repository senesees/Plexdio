//! In-memory music library served by the mock.

/// Embedded audio fixtures: (file name, bytes, container, codec, duration ms).
pub struct AudioFile {
    pub name: &'static str,
    pub bytes: &'static [u8],
    pub container: &'static str,
    pub codec: &'static str,
    pub duration_ms: u64,
    /// Plex loudness analysis gain (dB) for this file.
    pub gain_db: f64,
}

macro_rules! audio {
    ($name:literal, $container:literal, $codec:literal, $dur:expr, $gain:expr) => {
        AudioFile {
            name: $name,
            bytes: include_bytes!(concat!("../fixtures/audio/", $name)),
            container: $container,
            codec: $codec,
            duration_ms: $dur,
            gain_db: $gain,
        }
    };
}

pub static AUDIO: &[AudioFile] = &[
    audio!("t01.mp3", "mp3", "mp3", 8000, -2.0),
    audio!("t02.flac", "flac", "flac", 8000, -2.0),
    audio!("t03.m4a", "mp4", "aac", 8000, -2.0),
    audio!("t04.ogg", "ogg", "vorbis", 8000, -2.0),
    audio!("t05.opus", "ogg", "opus", 8000, -2.0),
    audio!("t06.wav", "wav", "pcm", 8000, -2.0),
    audio!("t07_alac.m4a", "mp4", "alac", 8000, -2.0),
    audio!("t08_loud.flac", "flac", "flac", 8000, -8.5),
    audio!("t09_quiet.mp3", "mp3", "mp3", 8000, 13.9),
    audio!("gapless_a.flac", "flac", "flac", 3000, 0.0),
    audio!("gapless_b.flac", "flac", "flac", 3000, 0.0),
];

pub fn audio_by_name(name: &str) -> Option<&'static AudioFile> {
    AUDIO.iter().find(|a| a.name == name)
}

#[derive(Debug, Clone)]
pub struct Artist {
    pub id: u64,
    pub title: String,
    pub genre: String,
    pub added_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct Album {
    pub id: u64,
    pub artist_id: u64,
    pub title: String,
    pub year: i64,
    pub genre: String,
    pub added_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct Track {
    pub id: u64,
    pub album_id: u64,
    pub title: String,
    pub index: i64,
    pub audio: usize,
    pub view_count: i64,
    pub last_viewed_at: Option<i64>,
    pub user_rating: Option<f64>,
    pub added_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: u64,
    pub title: String,
    pub smart: bool,
    pub items: Vec<u64>,
    pub added_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Library {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
    /// Logical clock for addedAt/updatedAt (unix seconds).
    pub clock: i64,
}

pub const BASE_TIME: i64 = 1_700_000_000;

impl Library {
    pub fn tick(&mut self) -> i64 {
        self.clock += 60;
        self.clock
    }

    pub fn artist(&self, id: u64) -> Option<&Artist> {
        self.artists.iter().find(|a| a.id == id)
    }
    pub fn album(&self, id: u64) -> Option<&Album> {
        self.albums.iter().find(|a| a.id == id)
    }
    pub fn track(&self, id: u64) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }
    pub fn track_mut(&mut self, id: u64) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// Hand-made demo library with every fixture format, a gapless album and playlists.
    pub fn fixture() -> Self {
        let mut lib = Library { clock: BASE_TIME, ..Default::default() };
        let artists = [
            ("Aurora Lines", "Electronic"),
            ("The Copper Foxes", "Rock"),
            ("Mila Varga", "Jazz"),
            ("Northbound Choir", "Classical"),
            ("Déjà Vu Collective", "Electronic"),
        ];
        for (i, (name, genre)) in artists.iter().enumerate() {
            let t = lib.tick();
            lib.artists.push(Artist {
                id: 1000 + i as u64,
                title: name.to_string(),
                genre: genre.to_string(),
                added_at: t,
                updated_at: t,
            });
        }
        // (artist index, album title, year, track titles)
        let albums: &[(usize, &str, i64, &[&str])] = &[
            (0, "Night Signals", 2019, &["Low Orbit", "Satellite Hearts", "Neon Rain", "Afterglow"]),
            (0, "Continuum", 2021, &["Continuum I", "Continuum II"]),
            (1, "Rust & Rivets", 2015, &["Iron Gate", "Burn Slow", "Copper Sky", "Last Train Home", "Dust"]),
            (1, "Live at the Mill", 2017, &["Intro (Live)", "Iron Gate (Live)", "Encore"]),
            (2, "Blue Hours", 2012, &["Smoke Rings", "Late Tram", "Velvet", "Quiet Corner"]),
            (3, "Winter Psalms", 2009, &["Kyrie", "Northern Light", "Vesper"]),
            (4, "Café Électrique", 2023, &["Ça Va", "Déjà Vu", "Über Alles Gut", "日本の夜"]),
        ];
        let mut track_id = 10_000u64;
        for (ai, (artist_idx, title, year, tracks)) in albums.iter().enumerate() {
            let t = lib.tick();
            let album_id = 2000 + ai as u64;
            lib.albums.push(Album {
                id: album_id,
                artist_id: 1000 + *artist_idx as u64,
                title: title.to_string(),
                year: *year,
                genre: artists[*artist_idx].1.to_string(),
                added_at: t,
                updated_at: t,
            });
            for (ti, name) in tracks.iter().enumerate() {
                let audio = if *title == "Continuum" {
                    9 + ti // gapless_a / gapless_b
                } else {
                    (track_id as usize) % 9
                };
                lib.tracks.push(Track {
                    id: track_id,
                    album_id,
                    title: name.to_string(),
                    index: ti as i64 + 1,
                    audio,
                    view_count: (track_id % 7) as i64,
                    last_viewed_at: if track_id.is_multiple_of(3) { None } else { Some(BASE_TIME - (track_id as i64 % 50) * 86_400) },
                    user_rating: if track_id.is_multiple_of(5) { Some(8.0) } else { None },
                    added_at: t,
                    updated_at: t,
                });
                track_id += 1;
            }
        }
        let t = lib.tick();
        lib.playlists.push(Playlist {
            id: 500,
            title: "Evening Mix".into(),
            smart: false,
            items: vec![10_000, 10_008, 10_013, 10_020, 10_003],
            added_at: t,
            updated_at: t,
        });
        lib.playlists.push(Playlist {
            id: 501,
            title: "Gapless Test".into(),
            smart: false,
            items: vec![10_004, 10_005],
            added_at: t,
            updated_at: t,
        });
        lib
    }

    /// Large generated library for scroll/RAM testing.
    pub fn synthetic(n_tracks: usize) -> Self {
        let mut lib = Library { clock: BASE_TIME, ..Default::default() };
        let genres = ["Rock", "Pop", "Jazz", "Electronic", "Classical", "Hip-Hop", "Folk", "Metal"];
        let tracks_per_album = 10;
        let albums_per_artist = 5;
        let n_albums = n_tracks.div_ceil(tracks_per_album);
        let n_artists = n_albums.div_ceil(albums_per_artist);
        for a in 0..n_artists {
            lib.artists.push(Artist {
                id: 100_000 + a as u64,
                title: format!("Artist {a:05}"),
                genre: genres[a % genres.len()].into(),
                added_at: BASE_TIME + a as i64,
                updated_at: BASE_TIME + a as i64,
            });
        }
        for al in 0..n_albums {
            lib.albums.push(Album {
                id: 200_000 + al as u64,
                artist_id: 100_000 + (al / albums_per_artist) as u64,
                title: format!("Album {al:06}"),
                year: 1960 + (al % 64) as i64,
                genre: genres[(al / albums_per_artist) % genres.len()].into(),
                added_at: BASE_TIME + al as i64,
                updated_at: BASE_TIME + al as i64,
            });
        }
        for t in 0..n_tracks {
            lib.tracks.push(Track {
                id: 1_000_000 + t as u64,
                album_id: 200_000 + (t / tracks_per_album) as u64,
                title: format!("Track {t:07}"),
                index: (t % tracks_per_album) as i64 + 1,
                audio: t % 9,
                view_count: (t % 13) as i64,
                last_viewed_at: Some(BASE_TIME - (t as i64 % 400) * 86_400),
                user_rating: None,
                added_at: BASE_TIME + t as i64,
                updated_at: BASE_TIME + t as i64,
            });
        }
        lib.clock = BASE_TIME + n_tracks as i64 + 60;
        lib.playlists.push(Playlist {
            id: 500,
            title: "Big Playlist".into(),
            smart: false,
            items: lib.tracks.iter().step_by(37).take(2000).map(|t| t.id).collect(),
            added_at: lib.clock,
            updated_at: lib.clock,
        });
        lib
    }
}
