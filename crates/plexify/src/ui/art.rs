//! UI side of artwork: texture LRU fed by `cache::images::ImageService`, plus generated
//! placeholders for items without art (or while art loads).

use crate::cache::images::{ArtKey, ImageService};
use egui::{Color32, Rect, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet};

use super::icons::{self, Icon};

/// Maximum textures kept alive (each is at most 400x400 RGBA).
const MAX_TEXTURES: usize = 96;

pub struct ArtCache {
    pub svc: ImageService,
    textures: HashMap<ArtKey, (TextureHandle, u64)>,
    pending: HashSet<ArtKey>,
    failed: HashMap<ArtKey, f64>,
    frame: u64,
    now: f64,
    offline: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Rounded(u8),
    Circle,
}

/// Snap requested sizes to a few buckets so the same art isn't fetched at many sizes.
pub fn bucket(px: f32) -> u32 {
    if px <= 72.0 {
        64
    } else if px <= 200.0 {
        200
    } else {
        400
    }
}

fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Muted colour pair derived from a title, for placeholders.
pub fn placeholder_colors(seed: &str) -> (Color32, Color32) {
    let h = hash(seed);
    let hue = (h % 360) as f32 / 360.0;
    let c1 = egui::ecolor::Hsva::new(hue, 0.45, 0.42, 1.0);
    let c2 = egui::ecolor::Hsva::new((hue + 0.08) % 1.0, 0.55, 0.22, 1.0);
    (c1.into(), c2.into())
}

impl ArtCache {
    pub fn new(svc: ImageService) -> Self {
        Self {
            svc,
            textures: HashMap::new(),
            pending: HashSet::new(),
            failed: HashMap::new(),
            frame: 0,
            now: 0.0,
            offline: false,
        }
    }

    pub fn set_offline(&mut self, offline: bool) {
        if self.offline != offline {
            // Things that failed offline may load now.
            self.failed.clear();
        }
        self.offline = offline;
    }

    /// Drop all textures and pending state (library switch).
    pub fn reset(&mut self) {
        self.textures.clear();
        self.pending.clear();
        self.failed.clear();
        self.svc.clear_queue();
    }

    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        self.now = ctx.input(|i| i.time);
        for (key, res) in self.svc.drain() {
            self.pending.remove(&key);
            match res {
                Ok(img) => {
                    let ci = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.rgba);
                    let tex = ctx.load_texture(format!("art:{}:{}", key.thumb, key.size), ci, TextureOptions::LINEAR);
                    self.textures.insert(key, (tex, self.frame));
                }
                Err(e) => {
                    log::debug!("art {} failed: {e}", key.thumb);
                    self.failed.insert(key, self.now);
                }
            }
        }
        if self.textures.len() > MAX_TEXTURES {
            let mut v: Vec<(ArtKey, u64)> = self.textures.iter().map(|(k, (_, f))| (k.clone(), *f)).collect();
            v.sort_by_key(|x| x.1);
            for (k, _) in v.into_iter().take(self.textures.len() - MAX_TEXTURES * 3 / 4) {
                self.textures.remove(&k);
            }
        }
    }

    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Texture for `thumb` at about `px` physical pixels, requesting it if needed.
    pub fn get(&mut self, thumb: &str, px: f32) -> Option<egui::TextureId> {
        let size = bucket(px);
        let key = ArtKey { thumb: thumb.to_string(), size };
        if let Some((tex, used)) = self.textures.get_mut(&key) {
            *used = self.frame;
            return Some(tex.id());
        }
        // A bigger or smaller version is better than a placeholder while loading.
        let alt = [64, 200, 400]
            .into_iter()
            .filter(|s| *s != size)
            .find_map(|s| self.textures.get(&ArtKey { thumb: thumb.to_string(), size: s }).map(|(t, _)| t.id()));
        let retry_after = if self.offline { 30.0 } else { 20.0 };
        let failed_recently = self.failed.get(&key).is_some_and(|t| self.now - t < retry_after);
        if !failed_recently && self.pending.insert(key.clone()) {
            self.svc.request(key);
        }
        alt
    }

    /// Paint art (or a placeholder) into `rect`.
    pub fn paint(&mut self, ui: &egui::Ui, rect: Rect, thumb: Option<&str>, seed: &str, shape: Shape) {
        let ppp = ui.ctx().pixels_per_point();
        let tex = thumb.filter(|t| !t.is_empty()).and_then(|t| self.get(t, rect.width().max(rect.height()) * ppp));
        let p = ui.painter_at(rect.expand(1.0));
        match tex {
            Some(id) => match shape {
                Shape::Rounded(r) => {
                    p.add(egui::Shape::Rect(
                        egui::epaint::RectShape::filled(rect, r, Color32::WHITE)
                            .with_texture(id, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))),
                    ));
                }
                Shape::Circle => {
                    // Approximate a circular crop with a textured polygon fan.
                    let c = rect.center();
                    let r = rect.width().min(rect.height()) / 2.0;
                    let mut mesh = egui::Mesh::with_texture(id);
                    let n = 48;
                    mesh.colored_vertex(c, Color32::WHITE);
                    mesh.vertices[0].uv = egui::pos2(0.5, 0.5);
                    for i in 0..=n {
                        let a = i as f32 / n as f32 * std::f32::consts::TAU;
                        let d = egui::vec2(a.cos(), a.sin());
                        let idx = mesh.vertices.len() as u32;
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos: c + d * r,
                            uv: egui::pos2(0.5 + d.x * 0.5, 0.5 + d.y * 0.5),
                            color: Color32::WHITE,
                        });
                        if i > 0 {
                            mesh.add_triangle(0, idx - 1, idx);
                        }
                    }
                    p.add(egui::Shape::mesh(mesh));
                }
            },
            None => {
                let (c1, c2) = placeholder_colors(seed);
                match shape {
                    Shape::Rounded(r) => {
                        p.rect_filled(rect, r, c2);
                        let inner = rect.shrink(rect.width() * 0.08);
                        p.rect_filled(Rect::from_min_max(inner.min, egui::pos2(inner.max.x, inner.center().y + inner.height() * 0.2)), r, c1.linear_multiply(0.35));
                    }
                    Shape::Circle => {
                        p.circle_filled(rect.center(), rect.width().min(rect.height()) / 2.0, c2);
                    }
                }
                let icon = Rect::from_center_size(rect.center(), rect.size() * 0.36);
                icons::paint(&p, icon, if shape == Shape::Circle { Icon::Radio } else { Icon::Library }, c1.linear_multiply(1.6));
            }
        }
    }
}
