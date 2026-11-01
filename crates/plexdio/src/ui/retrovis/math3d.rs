//! Just enough 3D for the polygon, terrain, tunnel and point-cloud presets: vectors, rotation
//! and a perspective projection (with optional PS1-style snapping to whole pixels).

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

pub const fn v3(x: f32, y: f32, z: f32) -> V3 {
    V3 { x, y, z }
}

// Plain methods rather than operator traits keep the preset code short and explicit.
#[allow(clippy::should_implement_trait)]
impl V3 {
    pub fn add(self, o: V3) -> V3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    pub fn sub(self, o: V3) -> V3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    pub fn mul(self, k: f32) -> V3 {
        v3(self.x * k, self.y * k, self.z * k)
    }
    pub fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: V3) -> V3 {
        v3(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    pub fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn norm(self) -> V3 {
        let l = self.len();
        if l > 1e-6 {
            self.mul(1.0 / l)
        } else {
            self
        }
    }
    pub fn lerp(self, o: V3, t: f32) -> V3 {
        self.add(o.sub(self).mul(t))
    }
    /// Rotate around X, then Y, then Z.
    pub fn rot(self, ax: f32, ay: f32, az: f32) -> V3 {
        let (s, c) = ax.sin_cos();
        let p = v3(self.x, self.y * c - self.z * s, self.y * s + self.z * c);
        let (s, c) = ay.sin_cos();
        let p = v3(p.x * c + p.z * s, p.y, -p.x * s + p.z * c);
        let (s, c) = az.sin_cos();
        v3(p.x * c - p.y * s, p.x * s + p.y * c, p.z)
    }
}

/// A camera at the origin looking down +Z.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// Focal length in half-heights (about 1.2 is a natural field of view).
    pub focal: f32,
    pub w: f32,
    pub h: f32,
    /// Snap projected points to whole pixels (the PS1's wobbly polygons).
    pub snap: bool,
}

impl Camera {
    pub fn new(w: usize, h: usize) -> Self {
        Self { focal: 1.2, w: w as f32, h: h as f32, snap: false }
    }

    /// Screen position and depth, or `None` behind (or too close to) the camera.
    pub fn project(&self, p: V3) -> Option<(f32, f32, f32)> {
        if p.z < 0.05 {
            return None;
        }
        let half = self.h / 2.0;
        let k = self.focal * half / p.z;
        let (mut x, mut y) = (self.w / 2.0 + p.x * k, self.h / 2.0 - p.y * k);
        if self.snap {
            x = x.round();
            y = y.round();
        }
        Some((x, y, p.z))
    }
}

/// An icosphere (radius 1) with `sub` subdivisions: vertices and triangles.
pub fn icosphere(sub: u32) -> (Vec<V3>, Vec<[usize; 3]>) {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut v: Vec<V3> = [
        (-1.0, t, 0.0), (1.0, t, 0.0), (-1.0, -t, 0.0), (1.0, -t, 0.0),
        (0.0, -1.0, t), (0.0, 1.0, t), (0.0, -1.0, -t), (0.0, 1.0, -t),
        (t, 0.0, -1.0), (t, 0.0, 1.0), (-t, 0.0, -1.0), (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| v3(x, y, z).norm())
    .collect();
    let mut f: Vec<[usize; 3]> = vec![
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11], [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9], [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    for _ in 0..sub {
        let mut cache = std::collections::HashMap::new();
        let mut mid = |a: usize, b: usize, v: &mut Vec<V3>| -> usize {
            let key = (a.min(b), a.max(b));
            *cache.entry(key).or_insert_with(|| {
                v.push(v[a].add(v[b]).mul(0.5).norm());
                v.len() - 1
            })
        };
        let mut nf = Vec::with_capacity(f.len() * 4);
        for [a, b, c] in f {
            let (ab, bc, ca) = (mid(a, b, &mut v), mid(b, c, &mut v), mid(c, a, &mut v));
            nf.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        f = nf;
    }
    (v, f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_centres_and_rejects_behind() {
        let cam = Camera::new(320, 180);
        let (x, y, _) = cam.project(v3(0.0, 0.0, 5.0)).unwrap();
        assert_eq!((x, y), (160.0, 90.0));
        assert!(cam.project(v3(0.0, 0.0, -1.0)).is_none());
        // Up is up on screen, right is right.
        let (x, y, _) = cam.project(v3(1.0, 1.0, 5.0)).unwrap();
        assert!(x > 160.0 && y < 90.0);
    }

    #[test]
    fn rotation_keeps_length_and_icosphere_is_closed() {
        let p = v3(1.0, 2.0, 3.0);
        assert!((p.rot(0.3, 1.1, -0.7).len() - p.len()).abs() < 1e-5);
        let (v, f) = icosphere(1);
        assert_eq!((v.len(), f.len()), (42, 80));
        assert!(v.iter().all(|p| (p.len() - 1.0).abs() < 1e-5));
    }
}
