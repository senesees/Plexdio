//! Render every Now playing visualiser preset from synthetic music into a PNG contact sheet, to
//! review the looks without starting the app:
//!
//!     cargo run --release -p plexdio --example retrovis_preview [-- WIDTH]
//!
//! Writes `target/retrovis/sheet.png` (one row per preset, frames at 1.5 s, 3.5 s and 5.5 s),
//! `target/retrovis/paused.png` (the Dashboard fading into its starfield) and prints how long a
//! frame of each preset takes.

use plexdio::ui::retrovis::{RetroPrefs, RetroVis};
use std::time::Instant;

const RATE: u32 = 48_000;
const FPS: f32 = 30.0;

/// A few seconds of "music": kick and snare at 124 bpm, a bass line, chords and hi-hats.
fn music(seconds: f32) -> Vec<f32> {
    let n = (seconds * RATE as f32) as usize;
    let beat = 60.0 / 124.0;
    let mut noise = 0x1234_5678u32;
    let mut rnd = move || {
        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        (noise >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    };
    let tau = std::f32::consts::TAU;
    let bass = [55.0, 55.0, 65.4, 49.0];
    (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let bt = t % beat;
            let bar = ((t / (beat * 4.0)) as usize) % 4;
            let kick = (tau * (50.0 + 60.0 * (-bt * 30.0).exp()) * bt).sin() * (-bt * 9.0).exp();
            let st = (t + beat) % (beat * 2.0);
            let snare = rnd() * (-st * 22.0).exp() * 0.5;
            let hat = rnd() * (-(t % (beat / 2.0)) * 60.0).exp() * 0.25;
            let b = (tau * bass[bar] * t).sin() * 0.35;
            let chord: f32 = [1.0, 1.26, 1.5].iter().map(|k| (tau * 220.0 * k * t).sin()).sum::<f32>() * 0.08 * (0.6 + 0.4 * (t * 0.5).sin());
            (kick * 0.9 + snare + hat + b + chord) * 0.6
        })
        .collect()
}

fn main() {
    let width: u32 = std::env::args().nth(1).and_then(|w| w.parse().ok()).unwrap_or(320);
    let out_dir = std::path::Path::new("target/retrovis");
    std::fs::create_dir_all(out_dir).expect("create target/retrovis");
    let audio = music(8.0);
    let names = RetroVis::new(RetroPrefs::default()).names();
    let shots = [1.5f32, 3.5, 5.5];
    let aspect = 16.0 / 9.0;
    let h = (width as f32 / aspect).round() as u32;
    let mut sheet = image::RgbImage::new(width * shots.len() as u32, h * names.len() as u32);
    for (row, name) in names.iter().enumerate() {
        let mut v = RetroVis::new(RetroPrefs { preset: Some(name.to_string()), cycle_secs: 0, width, ..Default::default() });
        let dt = 1.0 / FPS;
        let mut spent = 0.0;
        let mut frames = 0;
        let mut col = 0;
        let mut t = 0.0f32;
        while col < shots.len() {
            t += dt;
            let end = ((t * RATE as f32) as usize).min(audio.len());
            let start = end.saturating_sub(4096);
            let clock = Instant::now();
            v.frame(Some((&audio[start..end], RATE)), true, dt, aspect);
            spent += clock.elapsed().as_secs_f64();
            frames += 1;
            if t >= shots[col] {
                blit(&mut sheet, &mut v, col as u32 * width, row as u32 * h);
                col += 1;
            }
        }
        println!("{name:<10} {:.2} ms/frame at {width}×{h}", spent * 1000.0 / frames as f64);
    }
    sheet.save(out_dir.join("sheet.png")).expect("write sheet");

    // The Dashboard when the music pauses: the picture fades into a starfield.
    let mut v = RetroVis::new(RetroPrefs { preset: Some("Dashboard".into()), cycle_secs: 0, width, ..Default::default() });
    let mut paused = image::RgbImage::new(width * 3, h);
    let dt = 1.0 / FPS;
    for i in 0..(FPS as usize * 4) {
        let end = (i + 1) * (RATE as usize / FPS as usize);
        v.frame(Some((&audio[end.saturating_sub(4096)..end], RATE)), true, dt, aspect);
    }
    for (k, frames) in [(0u32, 15usize), (1, 45), (2, 60)] {
        for _ in 0..frames {
            v.frame(None, false, dt, aspect);
        }
        blit(&mut paused, &mut v, k * width, 0);
    }
    paused.save(out_dir.join("paused.png")).expect("write paused");
    println!("wrote {}/sheet.png and paused.png", out_dir.display());
}

fn blit(dst: &mut image::RgbImage, v: &mut RetroVis, x0: u32, y0: u32) {
    let img = v.image();
    for (i, p) in img.pixels.iter().enumerate() {
        let (x, y) = ((i % img.size[0]) as u32, (i / img.size[0]) as u32);
        if x0 + x < dst.width() && y0 + y < dst.height() {
            dst.put_pixel(x0 + x, y0 + y, image::Rgb([p.r(), p.g(), p.b()]));
        }
    }
}
