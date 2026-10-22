//! Artist, album, playlist and genre pages, plus Home.

use super::common::*;
use crate::library_model::Id;
use crate::ui::art::Shape;
use crate::ui::theme;
use egui::RichText;
use std::sync::Arc;

/// Big header: art on the left, kind/title/subtitle on the right.
fn hero(ui: &mut egui::Ui, ctx: &mut ViewCtx, thumb: Option<&str>, seed: &str, round: bool, kind: &str, title: &str, sub: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(180.0, 180.0), egui::Sense::hover());
        ctx.art.paint(ui, rect, thumb, seed, if round { Shape::Circle } else { Shape::Rounded(8) });
        ui.add_space(18.0);
        ui.vertical(|ui| {
            ui.add_space(40.0);
            ui.label(RichText::new(kind).small().strong().color(theme::TEXT_DIM));
            ui.label(RichText::new(title).size(34.0).strong());
            ui.add_space(4.0);
            ui.label(RichText::new(sub).color(theme::TEXT_DIM));
        });
    });
    ui.add_space(12.0);
}

pub fn album_view(ui: &mut egui::Ui, ctx: &mut ViewCtx, id: Id) {
    let lib = ctx.lib;
    let Some(a) = lib.album(id) else {
        ui.label(RichText::new("This album is no longer in the library.").color(theme::TEXT_DIM));
        return;
    };
    let ids = lib.album_track_ids(id);
    let total: u64 = ids.iter().filter_map(|t| lib.track(*t)).map(|t| t.duration_ms as u64).sum();
    let year = if a.year > 0 { format!("{} · ", a.year) } else { String::new() };
    let sub = format!("{}{}{} tracks · {}", year, a.genre.as_deref().map(|g| format!("{g} · ")).unwrap_or_default(), ids.len(), fmt_long_duration(total));
    hero(ui, ctx, a.thumb.as_deref(), &a.title, false, "ALBUM", &a.title, &sub);
    let artist_id = a.artist_id;
    ui.horizontal(|ui| {
        if lib.artist(artist_id).is_some() {
            if ui.link(RichText::new(&*a.artist).size(15.0).strong()).clicked() {
                ctx.actions.push(UiAction::Navigate(Route::Artist(artist_id)));
            }
        } else {
            ui.label(RichText::new(&*a.artist).size(15.0).strong());
        }
    });
    ui.add_space(8.0);
    play_buttons(ui, ctx, &ids, Some(RadioSeed::Album(id)));
    ui.add_space(8.0);
    track_table(ui, ctx, &ids, TableOpts { album: false, artist: true, track_numbers: true, art: false }, "album-tracks");
}

pub fn artist_view(ui: &mut egui::Ui, ctx: &mut ViewCtx, id: Id) {
    let lib = ctx.lib;
    let Some(a) = lib.artist(id) else {
        ui.label(RichText::new("This artist is no longer in the library.").color(theme::TEXT_DIM));
        return;
    };
    let albums: Vec<usize> = lib.artist_albums.get(&id).cloned().unwrap_or_default();
    let ids = lib.artist_track_ids(id);
    let sub = format!("{}{} albums · {} tracks", a.genre.as_deref().map(|g| format!("{g} · ")).unwrap_or_default(), albums.len(), ids.len());
    hero(ui, ctx, a.thumb.as_deref(), &a.title, true, "ARTIST", &a.title, &sub);
    play_buttons(ui, ctx, &ids, Some(RadioSeed::Artist(id)));
    ui.add_space(10.0);
    // Popular: most played tracks of the artist.
    let mut top: Vec<Id> = ids.clone();
    top.sort_by_key(|t| std::cmp::Reverse(lib.track(*t).map(|t| t.view_count).unwrap_or(0)));
    top.truncate(5);
    ui.label(RichText::new("Popular").size(18.0).strong());
    let h = (top.len() as f32 * ROW_H).max(ROW_H);
    ui.allocate_ui(egui::vec2(ui.available_width(), h + 6.0), |ui| {
        track_table(ui, ctx, &top, TableOpts { album: true, artist: false, track_numbers: false, art: true }, "artist-top");
    });
    ui.add_space(10.0);
    ui.label(RichText::new("Discography").size(18.0).strong());
    ui.add_space(6.0);
    let mut open = None;
    let subs: Vec<String> = albums.iter().map(|&i| if lib.albums[i].year > 0 { lib.albums[i].year.to_string() } else { "Album".into() }).collect();
    card_grid(
        ui,
        ctx.art,
        albums.len(),
        false,
        "artist-albums",
        |i| {
            let al = &lib.albums[albums[i]];
            CardItem { thumb: al.thumb.as_deref(), title: &al.title, subtitle: &subs[i], seed: &al.title }
        },
        |i, r| {
            if r.clicked() {
                open = Some(lib.albums[albums[i]].id);
            }
        },
    );
    if let Some(id) = open {
        ctx.actions.push(UiAction::Navigate(Route::Album(id)));
    }
}

pub fn playlist_view(ui: &mut egui::Ui, ctx: &mut ViewCtx, id: Id) {
    let lib = ctx.lib;
    let Some(p) = lib.playlist(id) else {
        ui.label(RichText::new("This playlist no longer exists.").color(theme::TEXT_DIM));
        return;
    };
    let ids: Vec<Id> = p.items.iter().copied().filter(|t| lib.track(*t).is_some()).collect();
    let total: u64 = ids.iter().filter_map(|t| lib.track(*t)).map(|t| t.duration_ms as u64).sum();
    let sub = format!("{}{} tracks · {}", if p.smart { "Smart playlist · " } else { "" }, ids.len(), fmt_long_duration(total));
    hero(ui, ctx, p.composite.as_deref(), &p.title, false, "PLAYLIST", &p.title, &sub);
    play_buttons(ui, ctx, &ids, None);
    ui.add_space(8.0);
    track_table(ui, ctx, &ids, TableOpts { album: true, artist: true, track_numbers: false, art: true }, "playlist-tracks");
}

pub fn genre_view(ui: &mut egui::Ui, ctx: &mut ViewCtx, genre: &Arc<str>) {
    let lib = ctx.lib;
    let albums: Vec<usize> = (0..lib.albums.len()).filter(|&i| lib.albums[i].genre.as_deref() == Some(&**genre)).collect();
    let ids: Vec<Id> = albums.iter().flat_map(|&i| lib.album_track_ids(lib.albums[i].id)).collect();
    ui.label(RichText::new("GENRE").small().strong().color(theme::TEXT_DIM));
    ui.label(RichText::new(&**genre).size(34.0).strong());
    ui.label(RichText::new(format!("{} albums · {} tracks", albums.len(), ids.len())).color(theme::TEXT_DIM));
    ui.add_space(8.0);
    play_buttons(ui, ctx, &ids, None);
    ui.add_space(8.0);
    let mut open = None;
    card_grid(
        ui,
        ctx.art,
        albums.len(),
        false,
        "genre-albums",
        |i| {
            let a = &lib.albums[albums[i]];
            CardItem { thumb: a.thumb.as_deref(), title: &a.title, subtitle: &a.artist, seed: &a.title }
        },
        |i, r| {
            if r.clicked() {
                open = Some(lib.albums[albums[i]].id);
            }
        },
    );
    if let Some(id) = open {
        ctx.actions.push(UiAction::Navigate(Route::Album(id)));
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title).size(20.0).strong());
    ui.add_space(6.0);
}

pub fn home_view(ui: &mut egui::Ui, ctx: &mut ViewCtx, greeting: &str) {
    let lib = ctx.lib;
    egui::ScrollArea::vertical().id_salt("home").auto_shrink([false, false]).show(ui, |ui| {
        ui.label(RichText::new(greeting).size(28.0).strong());
        ui.label(
            RichText::new(format!(
                "{} artists · {} albums · {} tracks · {} playlists",
                lib.artists.len(),
                lib.albums.len(),
                lib.tracks.len(),
                lib.playlists.len()
            ))
            .color(theme::TEXT_DIM),
        );

        section(ui, "Recently added");
        let recent = lib.recently_added_albums(12);
        let mut open = None;
        card_row(
            ui,
            ctx.art,
            recent.len(),
            false,
            "home-recent",
            |i| {
                let a = &lib.albums[recent[i]];
                CardItem { thumb: a.thumb.as_deref(), title: &a.title, subtitle: &a.artist, seed: &a.title }
            },
            |i, r| {
                if r.clicked() {
                    open = Some(Route::Album(lib.albums[recent[i]].id));
                }
            },
        );

        if !lib.playlists.is_empty() {
            section(ui, "Your playlists");
            card_row(
                ui,
                ctx.art,
                lib.playlists.len().min(12),
                false,
                "home-playlists",
                |i| {
                    let p = &lib.playlists[i];
                    CardItem { thumb: p.composite.as_deref(), title: &p.title, subtitle: "Playlist", seed: &p.title }
                },
                |i, r| {
                    if r.clicked() {
                        open = Some(Route::Playlist(lib.playlists[i].id));
                    }
                },
            );
        }

        // Artists with the most plays.
        let mut plays: std::collections::HashMap<Id, u64> = std::collections::HashMap::new();
        for t in &lib.tracks {
            *plays.entry(t.artist_id).or_default() += t.view_count as u64;
        }
        let mut top: Vec<usize> = (0..lib.artists.len()).collect();
        top.sort_by_key(|&i| std::cmp::Reverse(plays.get(&lib.artists[i].id).copied().unwrap_or(0)));
        top.truncate(10);
        if !top.is_empty() {
            section(ui, "Your top artists");
            card_row(
                ui,
                ctx.art,
                top.len(),
                true,
                "home-artists",
                |i| {
                    let a = &lib.artists[top[i]];
                    CardItem { thumb: a.thumb.as_deref(), title: &a.title, subtitle: "Artist", seed: &a.title }
                },
                |i, r| {
                    if r.clicked() {
                        open = Some(Route::Artist(lib.artists[top[i]].id));
                    }
                },
            );
        }
        if let Some(r) = open {
            ctx.actions.push(UiAction::Navigate(r));
        }
    });
}
