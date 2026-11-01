//! Analyse songs and build their Ride maps without starting the app, to review the beat grid,
//! sections and note placement:
//!
//!     cargo run --release -p plexdio --example ride_preview [-- [--clicks] FILE...]
//!
//! Without files it uses a synthetic song. For each song it prints the analysis and writes
//! `target/ride/<name>.png`, a timeline in rows of 30 s:
//! - intensity (filled, cool to hot), the road's slope (yellow line) and section starts (white);
//! - onsets: bass (red), mids (green), highs (blue), taller when stronger;
//! - the beat grid (grey) and downbeats (white);
//! - the notes for Casual, Pro and Elite (three lanes each; grey blocks in grey).
//!
//! It also writes the beat times to `target/ride/<name>.beats.txt`. `--clicks` also writes `target/ride/<name>-clicks.wav`: the song with a click on every beat
//! (higher on downbeats), to check the grid by ear.

use plexdio::game::analysis::{analyse, decode_mono, Analysis, RATE};
use plexdio::game::mapgen::{generate, seed_for, Difficulty, Map, NoteKind, LANES};
use plexdio::game::play::{lane_x, Input, Ride};
use plexdio::game::render::{draw, RoadGeom};
use plexdio::ui::retrovis::canvas::Canvas;
use plexdio::ui::retrovis::{RetroPrefs, RetroVis};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

const PX_PER_S: f32 = 40.0;
const ROW_S: f32 = 30.0;
const ROW_H: u32 = 150;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let clicks = args.iter().any(|a| a == "--clicks");
    let frames = args.iter().any(|a| a == "--frames");
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let out = Path::new("target/ride");
    std::fs::create_dir_all(out).expect("create target/ride");
    let songs: Vec<(String, Vec<f32>, f64)> = if files.is_empty() {
        vec![("synthetic".into(), synthetic(), 0.0)]
    } else {
        files
            .iter()
            .filter_map(|f| {
                let p = Path::new(f.as_str());
                let file = std::fs::File::open(p).map_err(|e| eprintln!("{f}: {e}")).ok()?;
                let ext = p.extension().and_then(|e| e.to_str()).map(str::to_lowercase);
                let t = Instant::now();
                let samples = decode_mono(Box::new(file), ext.as_deref(), &AtomicBool::new(false), None).map_err(|e| eprintln!("{f}: {e}")).ok()?;
                let name = p.file_stem().map_or("song".into(), |s| s.to_string_lossy().into_owned());
                Some((name, samples, t.elapsed().as_secs_f64()))
            })
            .collect()
    };
    for (name, samples, decode_s) in songs {
        let t = Instant::now();
        let a = analyse(&samples, None);
        let analyse_s = t.elapsed().as_secs_f64();
        let maps: Vec<Map> = Difficulty::ALL.iter().map(|&d| generate(&a, d, seed_for(&name))).collect();
        println!(
            "{name}: {:.1} s long, decoded in {decode_s:.2} s, analysed in {analyse_s:.2} s; {:.1} bpm, {} beats, {} downbeats, onsets {}/{}/{} (bass/mid/high), {} sections",
            a.duration,
            a.bpm,
            a.beats.len(),
            a.downbeats.len(),
            a.onsets.iter().filter(|o| o.band == 0).count(),
            a.onsets.iter().filter(|o| o.band == 1).count(),
            a.onsets.iter().filter(|o| o.band == 2).count(),
            a.sections.len()
        );
        for s in &a.sections {
            println!("    section {:6.1}–{:6.1} s, intensity {:.2}", s.start, s.end, s.intensity);
        }
        for m in &maps {
            let greys = m.notes.len() - m.colour_notes();
            println!("    {:6}: {} notes ({:.1}/s), {} greys, {:.0} m", m.difficulty.label(), m.colour_notes(), m.colour_notes() as f32 / a.duration.max(1.0), greys, m.distance_at(a.duration));
        }
        let png = out.join(format!("{name}.png"));
        timeline(&a, &maps).save(&png).expect("write png");
        println!("    → {}", png.display());
        let beats_txt = out.join(format!("{name}.beats.txt"));
        std::fs::write(&beats_txt, a.beats.iter().map(|b| format!("{b:.3}\n")).collect::<String>()).expect("write beats");
        if frames {
            let png = out.join(format!("{name}-ride.png"));
            let (img, stats) = ride_frames(&samples, maps[1].clone());
            img.save(&png).expect("write png");
            println!("    → {} ({stats})", png.display());
        }
        if clicks {
            let wav = out.join(format!("{name}-clicks.wav"));
            write_wav(&wav, &with_clicks(&samples, &a)).expect("write wav");
            println!("    → {}", wav.display());
        }
    }
}

/// Note colours, cool to hot (as in the game).
const PALETTE: [[u8; 3]; 5] = [[70, 90, 255], [170, 70, 255], [40, 220, 120], [255, 200, 40], [255, 60, 40]];

fn timeline(a: &Analysis, maps: &[Map]) -> image::RgbImage {
    let rows = (a.duration / ROW_S).ceil().max(1.0) as u32;
    let w = (ROW_S * PX_PER_S) as u32;
    let mut img = image::RgbImage::from_pixel(w, rows * ROW_H, image::Rgb([16, 16, 20]));
    let rect = |img: &mut image::RgbImage, x: i32, y: i32, rw: i32, rh: i32, c: [u8; 3]| {
        for yy in y.max(0)..(y + rh).min(img.height() as i32) {
            for xx in x.max(0)..(x + rw).min(img.width() as i32) {
                img.put_pixel(xx as u32, yy as u32, image::Rgb(c));
            }
        }
    };
    let at = |t: f32| -> (i32, i32) {
        let row = (t / ROW_S) as i32;
        (((t - row as f32 * ROW_S) * PX_PER_S) as i32, row * ROW_H as i32)
    };
    // Intensity, filled and coloured by value.
    for x in 0..(a.duration * PX_PER_S) as i32 {
        let t = x as f32 / PX_PER_S;
        let v = a.intensity_at(t);
        let c = PALETTE[((v * 4.99) as usize).min(4)];
        let (px, y0) = at(t);
        let h = (v * 38.0) as i32;
        rect(&mut img, px, y0 + 40 - h, 1, h, [c[0] / 2, c[1] / 2, c[2] / 2]);
        // The road slope (yellow): 0 in the middle, up is uphill.
        let slope = maps[1].road_at(t).slope;
        rect(&mut img, px, y0 + 20 - (slope * 70.0) as i32, 1, 1, [255, 230, 80]);
    }
    for s in a.sections.iter().skip(1) {
        let (x, y0) = at(s.start);
        rect(&mut img, x, y0, 1, ROW_H as i32 - 6, [255, 255, 255]);
    }
    // Onsets per band.
    for o in &a.onsets {
        let (x, y0) = at(o.t);
        let c = [[255, 70, 70], [70, 230, 90], [80, 140, 255]][o.band as usize];
        let h = (2.0 + o.strength * 6.0) as i32;
        rect(&mut img, x, y0 + 44 + o.band as i32 * 9 + 8 - h, 1, h, c);
    }
    // Beat grid.
    for &b in &a.beats {
        let (x, y0) = at(b);
        let down = a.downbeats.iter().any(|d| (d - b).abs() < 1e-3);
        rect(&mut img, x, y0 + 72, 1, 10, if down { [255, 255, 255] } else { [110, 110, 110] });
    }
    // Notes: Casual, Pro, Elite.
    for (k, m) in maps.iter().enumerate() {
        let top = 86 + k as i32 * 20;
        for n in &m.notes {
            let (x, y0) = at(n.t);
            let c = match n.kind {
                NoteKind::Colour(c) => PALETTE[c as usize],
                NoteKind::Grey => [120, 120, 120],
            };
            rect(&mut img, x - 1, y0 + top + n.lane as i32 * (18 / LANES as i32), 3, 5, c);
        }
    }
    img
}

/// The song plus a short click on every beat (1 kHz, 1.6 kHz on downbeats).
fn with_clicks(samples: &[f32], a: &Analysis) -> Vec<f32> {
    let mut out: Vec<f32> = samples.iter().map(|s| s * 0.7).collect();
    let len = (0.03 * RATE as f32) as usize;
    for &b in &a.beats {
        let down = a.downbeats.iter().any(|d| (d - b).abs() < 1e-3);
        let hz = if down { 1600.0 } else { 1000.0 };
        let s0 = (b * RATE as f32) as usize;
        for i in 0..len {
            if let Some(o) = out.get_mut(s0 + i) {
                let t = i as f32 / RATE as f32;
                *o += (std::f32::consts::TAU * hz * t).sin() * (-t * 120.0).exp() * 0.5;
            }
        }
    }
    out
}

fn write_wav(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let data: Vec<u8> = samples.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
    let mut b = Vec::with_capacity(44 + data.len());
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    std::fs::write(path, b)
}

/// A minute of synthetic music at 124 bpm: a quiet intro, a loud part, a breakdown, a loud end.
fn synthetic() -> Vec<f32> {
    let seconds = 64.0;
    let n = (seconds * RATE as f32) as usize;
    let beat = 60.0 / 124.0;
    let tau = std::f32::consts::TAU;
    let mut noise = 0x1234_5678u32;
    let mut last = 0.0f32;
    let bass = [55.0, 55.0, 65.4, 49.0];
    (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let loud = (8.0..32.0).contains(&t) || t >= 40.0;
            let bt = t % beat;
            let bar = ((t / (beat * 4.0)) as usize) % 4;
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let white = (noise >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
            let hp = white - last;
            last = white;
            let kick = (tau * (50.0 + 60.0 * (-bt * 30.0).exp()) * bt).sin() * (-bt * 9.0).exp();
            let st = (t + beat) % (beat * 2.0);
            let snare = white * (-st * 22.0).exp() * 0.4;
            let hat = hp * 0.5 * (-(t % (beat / 2.0)) * 60.0).exp() * 0.3;
            let b = (tau * bass[bar] * t).sin() * 0.3;
            let chord: f32 = [1.0, 1.26, 1.5].iter().map(|k| (tau * 220.0 * k * t).sin()).sum::<f32>() * 0.07;
            if loud {
                (kick * 0.9 + snare + hat + b + chord) * 0.6
            } else {
                (hat * 0.6 + chord) * 0.5
            }
        })
        .collect()
}

/// Ride the whole song with an autopilot and grab six frames (2 × 3 grid of 480×270).
fn ride_frames(samples: &[f32], map: Map) -> (image::RgbImage, String) {
    let (w, h) = (480usize, 270usize);
    let geom = RoadGeom::new(&map);
    let duration = map.duration;
    let mut ride = Ride::new(map, 0.0);
    let mut vis = RetroVis::new(RetroPrefs { preset: Some("Tunnel".into()), cycle_secs: 0, ..Default::default() });
    let mut out = Canvas::new(w, h);
    let mut sheet = image::RgbImage::new(w as u32 * 3, h as u32 * 2);
    let shots: Vec<f32> = (1..=6).map(|k| duration * k as f32 / 7.0).collect();
    let (dt, mut t, mut shot) = (1.0 / 60.0, 0.0f32, 0);
    let mut render_ms = 0.0;
    let mut frames = 0;
    let mut pixels = Vec::new();
    while t < duration && shot < shots.len() {
        t += dt;
        // Autopilot: head for the next coloured note's lane.
        let target = ride.map.notes[ride.next_note()..].iter().find(|n| matches!(n.kind, NoteKind::Colour(_))).map_or(0.0, |n| lane_x(n.lane));
        ride.update(t, Input { pointer: Some(target / 2.4 + 0.5), ..Default::default() }, dt);
        let s0 = ((t * RATE as f32) as usize).min(samples.len());
        let audio = &samples[s0.saturating_sub(2048)..s0];
        vis.frame(Some((audio, RATE)), true, dt, w as f32 / h as f32);
        if t >= shots[shot] {
            let clock = Instant::now();
            draw(&mut out, vis.canvas(), &ride, &geom, t);
            render_ms += clock.elapsed().as_secs_f64() * 1000.0;
            frames += 1;
            out.to_rgb_u32(&mut pixels, false);
            let (ox, oy) = ((shot % 3 * w) as u32, (shot / 3 * h) as u32);
            for (i, p) in pixels.iter().enumerate() {
                sheet.put_pixel(ox + (i % w) as u32, oy + (i / w) as u32, image::Rgb([(p >> 16) as u8, (p >> 8) as u8, *p as u8]));
            }
            shot += 1;
        }
    }
    // Time a few more frames for a steadier figure.
    let clock = Instant::now();
    for _ in 0..60 {
        draw(&mut out, vis.canvas(), &ride, &geom, t);
    }
    let per = clock.elapsed().as_secs_f64() * 1000.0 / 60.0;
    let stats = format!("score {}, caught {}, missed {}, greys {}, best combo {}; draw {:.2} ms/frame ({:.2} for the shots)", ride.score, ride.caught, ride.missed, ride.greys, ride.best_combo, per, render_ms / frames.max(1) as f64);
    (sheet, stats)
}
