//! Track info dialog contents: what the cache knows at once, the file details the server sends
//! when asked, and for the playing track how it is being played.

use super::common::{fmt_duration, plural};
use crate::audio::engine::StreamInfo;
use crate::library_model::{Id, LibraryModel};
use crate::plex::models::{stream_type, Metadata};
use crate::ui::theme;
use egui::RichText;

/// File details from the server, when they have been fetched.
pub enum Details<'a> {
    Loading,
    /// No server connection: only cached fields are shown.
    Offline,
    Failed(&'a str),
    Ready(&'a Metadata),
}

fn kbps(k: i64) -> String {
    format!("{k} kbps")
}

fn khz(hz: i64) -> String {
    let k = hz as f64 / 1000.0;
    if (k - k.round()).abs() < 0.05 {
        format!("{k:.0} kHz")
    } else {
        format!("{k:.1} kHz")
    }
}

fn size(bytes: i64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{} KB", (bytes / 1024).max(1))
    }
}

fn channels(n: i64) -> String {
    match n {
        1 => "Mono".into(),
        2 => "Stereo".into(),
        n => format!("{n} channels"),
    }
}

/// Two-column rows under a small heading.
fn group(ui: &mut egui::Ui, title: &str, rows: &[(&str, String)]) {
    let rows: Vec<&(&str, String)> = rows.iter().filter(|(_, v)| !v.is_empty()).collect();
    if rows.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.label(RichText::new(title).small().strong().color(theme::TEXT_DIM));
    egui::Grid::new(("track-info", title)).num_columns(2).min_col_width(96.0).spacing([18.0, 4.0]).show(ui, |ui| {
        for (k, v) in rows {
            ui.label(RichText::new(*k).color(theme::TEXT_DIM));
            ui.add(egui::Label::new(v.as_str()).wrap());
            ui.end_row();
        }
    });
}

/// How the playing track is played, in words.
pub fn playback_line(s: &StreamInfo) -> String {
    let how = match (s.local, s.transcode_kbps) {
        (true, _) => "Played from this computer".to_string(),
        (false, Some(k)) => format!("Converted by the server to {k} kbps MP3"),
        (false, None) => "Original file, streamed from the server".to_string(),
    };
    let rates = if s.source_rate == s.out_rate {
        format!("{} output", khz(s.out_rate as i64))
    } else {
        format!("{} resampled to {} for the output device", khz(s.source_rate as i64), khz(s.out_rate as i64))
    };
    format!("{how} · {rates}")
}

/// Draw the dialog body for track `id`. `stream` is set when this track is playing.
pub fn track_info(ui: &mut egui::Ui, lib: &LibraryModel, id: Id, details: Details, stream: Option<&StreamInfo>, local_plays: Option<u64>) {
    let Some(t) = lib.track(id) else {
        ui.label("This track is no longer in the library.");
        return;
    };
    ui.label(RichText::new(&*t.title).size(18.0).strong());
    ui.label(RichText::new(format!("{} · {}", t.display_artist(), t.album)).color(theme::TEXT_DIM));
    let genre = t.genre.as_deref().or_else(|| lib.album(t.album_id).and_then(|a| a.genre.as_deref())).unwrap_or("");
    let year = if t.year > 0 { t.year } else { lib.album(t.album_id).map_or(0, |a| a.year) };
    let number = match (t.disc, t.index) {
        (_, 0) => String::new(),
        (0 | 1, i) => format!("Track {i}"),
        (d, i) => format!("Disc {d} · track {i}"),
    };
    let plays = match local_plays.filter(|n| *n > 0) {
        Some(l) => format!("{} ({} from this app)", plural(t.view_count as usize, "play", "plays"), l),
        None => plural(t.view_count as usize, "play", "plays"),
    };
    let rating = if t.user_rating > 0.0 { format!("{}★", t.user_rating / 2.0) } else { String::new() };
    group(
        ui,
        "TRACK",
        &[
            ("Number", number),
            ("Year", if year > 0 { year.to_string() } else { String::new() }),
            ("Genre", genre.to_string()),
            ("Length", fmt_duration(t.duration_ms as u64)),
            ("Plays", plays),
            ("Last played", if t.last_viewed_at > 0 { crate::logging::date(t.last_viewed_at) } else { "Never".into() }),
            ("Rating", rating),
            ("Added", if t.added_at > 0 { crate::logging::date(t.added_at) } else { String::new() }),
        ],
    );

    // File: cached basics, completed by the server's details.
    let format = match (t.container.as_deref(), t.codec.as_deref()) {
        (Some(c), Some(k)) if !c.eq_ignore_ascii_case(k) => format!("{} ({})", c.to_uppercase(), k.to_uppercase()),
        (Some(c), _) => c.to_uppercase(),
        (None, Some(k)) => k.to_uppercase(),
        (None, None) => String::new(),
    };
    let mut file: Vec<(&str, String)> = vec![("Format", format), ("Bitrate", if t.bitrate_kbps > 0 { kbps(t.bitrate_kbps as i64) } else { String::new() })];
    let gain = |g: Option<f32>| g.map(|g| format!("{g:+.1} dB")).unwrap_or_default();
    let mut note = None;
    match details {
        Details::Ready(m) => {
            let media = m.media.first();
            let part = m.first_part();
            let audio = part.and_then(|p| p.streams.iter().find(|s| s.stream_type == Some(stream_type::AUDIO)));
            if let Some(b) = media.and_then(|x| x.bitrate).or(audio.and_then(|a| a.bitrate)) {
                file[1].1 = kbps(b);
            }
            let rate = audio.and_then(|a| a.sampling_rate).map(khz).unwrap_or_default();
            let depth = audio.and_then(|a| a.bit_depth).map(|d| format!("{d}-bit")).unwrap_or_default();
            let quality = [rate, depth].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            file.push(("Quality", quality));
            file.push(("Channels", audio.and_then(|a| a.channels).or(media.and_then(|x| x.audio_channels)).map(channels).unwrap_or_default()));
            file.push(("Size", part.and_then(|p| p.size).map(size).unwrap_or_default()));
            file.push(("File", part.and_then(|p| p.file.clone()).unwrap_or_default()));
        }
        Details::Loading => note = Some("Asking the server for file details…".to_string()),
        Details::Offline => note = Some("Connect to your server to see the file details.".to_string()),
        Details::Failed(e) => note = Some(format!("Couldn't get the file details: {e}")),
    }
    file.push(("Track gain", gain(t.gain)));
    file.push(("Album gain", gain(t.album_gain)));
    group(ui, "FILE", &file);
    if let Some(n) = note {
        ui.label(RichText::new(n).small().color(theme::TEXT_DIM));
    }
    if let Some(s) = stream {
        group(ui, "PLAYING NOW", &[("How", playback_line(s))]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_line_describes_direct_transcoded_and_local() {
        let mut s = StreamInfo { transcode_kbps: None, local: false, source_rate: 44_100, out_rate: 48_000, format: Some("flac".into()) };
        assert_eq!(playback_line(&s), "Original file, streamed from the server · 44.1 kHz resampled to 48 kHz for the output device");
        s.transcode_kbps = Some(192);
        s.source_rate = 48_000;
        assert_eq!(playback_line(&s), "Converted by the server to 192 kbps MP3 · 48 kHz output");
        s.local = true;
        assert!(playback_line(&s).starts_with("Played from this computer"));
    }

    #[test]
    fn units() {
        assert_eq!(khz(96_000), "96 kHz");
        assert_eq!(size(5_452_595), "5.2 MB");
        assert_eq!(channels(6), "6 channels");
    }
}
