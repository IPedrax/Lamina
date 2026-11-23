//! Pictures of the plate, drawn on the processor: an orbit camera and a small rasterizer with a depth buffer that keeps, at each pixel,
//! the object and the triangle drawn there. The desktop's 3D view shows them, the slicer puts a small one in the G-code as a thumbnail,
//! and the assistant can look at one. No UI toolkit: pixels are plain RGBA, and `png` writes them as a PNG file (deflated by
//! `miniz_oxide`, from the PNG specification).

use crate::mesh::P3;
use crate::scene::Scene;

/// A pixel: red, green, blue and opacity.
pub type Rgba = [u8; 4];

/// Vertical field of view, degrees, and the nearest the eye sees, mm.
pub const FOV: f64 = 35.0;
const NEAR: f64 = 1.0;

pub const BACKGROUND: Rgba = [38, 41, 46, 255];
pub const PLATE: Rgba = [62, 66, 72, 255];
pub const GRID: Rgba = [86, 91, 98, 255];
pub const MODEL: Rgba = [110, 165, 225, 255];
pub const SELECTED: Rgba = [245, 165, 60, 255];

/// An orbit camera round `target`: `azimuth` degrees counter-clockwise from east, `elevation` degrees up, `distance` mm away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub target: P3,
    pub azimuth: f64,
    pub elevation: f64,
    pub distance: f64,
}

impl Default for Camera {
    fn default() -> Camera {
        Camera { target: [0.0, 0.0, 10.0], azimuth: -60.0, elevation: 30.0, distance: 380.0 }
    }
}

impl Camera {
    /// The eye, and the view's right, up and forward axes.
    pub fn frame(&self) -> (P3, P3, P3, P3) {
        let (az, el) = (self.azimuth.to_radians(), self.elevation.to_radians());
        let back = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
        let eye = add(self.target, scale(back, self.distance));
        let forward = scale(back, -1.0);
        let right = normalize(cross(forward, [0.0, 0.0, 1.0]));
        (eye, right, cross(right, forward), forward)
    }

    /// Where `p` lands in a picture `w` by `h` pixels: x, y, and one over its depth (larger is nearer); none behind the eye.
    pub fn project(&self, p: P3, w: f64, h: f64) -> Option<[f64; 3]> {
        let (eye, right, up, forward) = self.frame();
        let d = sub(p, eye);
        let z = dot(d, forward);
        if z < NEAR {
            return None;
        }
        let f = (h / 2.0) / (FOV.to_radians() / 2.0).tan();
        Some([w / 2.0 + f * dot(d, right) / z, h / 2.0 - f * dot(d, up) / z, 1.0 / z])
    }

    /// The ray from the eye through the point (`px`, `py`) of a picture `w` by `h`: the eye and a unit direction.
    pub fn ray(&self, px: f64, py: f64, w: f64, h: f64) -> (P3, P3) {
        let (eye, right, up, forward) = self.frame();
        let f = (h / 2.0) / (FOV.to_radians() / 2.0).tan();
        (eye, normalize(add(forward, add(scale(right, (px - w / 2.0) / f), scale(up, (h / 2.0 - py) / f)))))
    }

    /// Where that ray meets the level plane at height `z`, if it does in front of the eye.
    pub fn on_level(&self, px: f64, py: f64, w: f64, h: f64, z: f64) -> Option<P3> {
        let (eye, dir) = self.ray(px, py, w, h);
        if dir[2].abs() < 1e-9 {
            return None;
        }
        let t = (z - eye[2]) / dir[2];
        (t > 0.0).then(|| add(eye, scale(dir, t)))
    }

    /// Looks at `bounds` from where the camera looks now, from far enough to see all of them.
    pub fn frame_all(&mut self, (lo, hi): (P3, P3)) {
        self.target = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
        let radius = (dot(sub(hi, lo), sub(hi, lo))).sqrt() / 2.0;
        self.distance = (radius / (FOV.to_radians() / 2.0).sin() * 1.15).clamp(20.0, 5000.0);
    }
}

/// A picture of the plate, and at each pixel the object drawn there (0 for none), its triangle, and one over its depth.
pub struct Picture {
    pub size: [usize; 2],
    pub pixels: Vec<Rgba>,
    ids: Vec<u32>,
    tris: Vec<u32>,
    depth: Vec<f32>,
}

impl Picture {
    pub fn id_at(&self, x: usize, y: usize) -> Option<u32> {
        let [w, h] = self.size;
        (x < w && y < h).then(|| self.ids[y * w + x]).filter(|&id| id != 0)
    }

    /// The object and its triangle at a pixel: what a "lay flat" click lands on.
    pub fn face_at(&self, x: usize, y: usize) -> Option<(u32, usize)> {
        let w = self.size[0];
        self.id_at(x, y).map(|id| (id, self.tris[y * w + x] as usize))
    }

    /// The point of the model (or the plate) seen at a pixel, from its depth: where a measuring click lands.
    pub fn point_at(&self, camera: &Camera, x: usize, y: usize) -> Option<P3> {
        let [w, h] = self.size;
        if x >= w || y >= h {
            return None;
        }
        let inv = f64::from(self.depth[y * w + x]);
        if inv <= 0.0 {
            return None;
        }
        let (eye, dir) = camera.ray(x as f64 + 0.5, y as f64 + 0.5, w as f64, h as f64);
        let forward = camera.frame().3;
        let t = (1.0 / inv) / dot(dir, forward);
        Some(add(eye, scale(dir, t)))
    }
}

struct Raster {
    w: usize,
    h: usize,
    pix: Vec<Rgba>,
    depth: Vec<f32>,
    ids: Vec<u32>,
    tris: Vec<u32>,
    /// The triangle being filled, for `tris`.
    tri: u32,
    light: P3,
}

/// The scene seen through `camera`, `size` pixels, with the objects in `selected` drawn in the highlight colour.
pub fn draw(scene: &Scene, selected: &[u32], camera: &Camera, bed: [f64; 2], size: [usize; 2]) -> Picture {
    let (w, h) = (size[0].max(1), size[1].max(1));
    let (_, _, _, forward) = camera.frame();
    // A light from over the viewer's shoulder, so faces turned to the eye are bright and the top reads from the sides.
    let light = normalize(add(scale(forward, -1.0), [0.25, 0.15, 0.7]));
    let mut r =
        Raster { w, h, pix: vec![BACKGROUND; w * h], depth: vec![0.0; w * h], ids: vec![0; w * h], tris: vec![0; w * h], tri: 0, light };
    let (hx, hy) = (bed[0] / 2.0, bed[1] / 2.0);
    let c = [[-hx, -hy, 0.0], [hx, -hy, 0.0], [hx, hy, 0.0], [-hx, hy, 0.0]];
    r.fill(camera, [c[0], c[1], c[2]], PLATE, 0);
    r.fill(camera, [c[0], c[2], c[3]], PLATE, 0);
    // A line every 10 mm, a hair above the plate so it wins the depth test against it and loses to anything standing on it.
    let mut x = -hx;
    while x <= hx + 1e-9 {
        r.line(camera, [x, -hy, 0.01], [x, hy, 0.01]);
        x += 10.0;
    }
    let mut y = -hy;
    while y <= hy + 1e-9 {
        r.line(camera, [-hx, y, 0.01], [hx, y, 0.01]);
        y += 10.0;
    }
    for o in &scene.objects {
        let base = if selected.contains(&o.id) { SELECTED } else { MODEL };
        let world = o.world();
        for i in 0..world.triangles.len() {
            if let Some(t) = world.tri(i) {
                r.tri = i as u32;
                r.shaded(camera, t, base, o.id);
            }
        }
    }
    Picture { size: [w, h], pixels: r.pix, ids: r.ids, tris: r.tris, depth: r.depth }
}

impl Raster {
    fn shaded(&mut self, camera: &Camera, t: [P3; 3], base: Rgba, id: u32) {
        let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
        let len = dot(n, n).sqrt();
        if len < 1e-12 {
            return;
        }
        // Both sides lit alike: an open mesh still reads.
        let k = 0.35 + 0.65 * (dot(n, self.light) / len).abs();
        let tint = |c: u8| (f64::from(c) * k).round().clamp(0.0, 255.0) as u8;
        self.fill(camera, t, [tint(base[0]), tint(base[1]), tint(base[2]), base[3]], id);
    }

    fn fill(&mut self, camera: &Camera, t: [P3; 3], color: Rgba, id: u32) {
        let (w, h) = (self.w as f64, self.h as f64);
        let (Some(a), Some(b), Some(c)) = (camera.project(t[0], w, h), camera.project(t[1], w, h), camera.project(t[2], w, h)) else {
            return;
        };
        let area = edge(a, b, [c[0], c[1]]);
        if area.abs() < 1e-12 {
            return;
        }
        let (x0, x1) = (a[0].min(b[0]).min(c[0]).floor(), a[0].max(b[0]).max(c[0]).ceil());
        let (y0, y1) = (a[1].min(b[1]).min(c[1]).floor(), a[1].max(b[1]).max(c[1]).ceil());
        if x1 < 0.0 || y1 < 0.0 || x0 > w - 1.0 || y0 > h - 1.0 {
            return;
        }
        let (x0, x1, y0, y1) = (x0.max(0.0) as usize, x1.min(w - 1.0) as usize, y0.max(0.0) as usize, y1.min(h - 1.0) as usize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = [x as f64 + 0.5, y as f64 + 0.5];
                let (l0, l1, l2) = (edge(b, c, p) / area, edge(c, a, p) / area, edge(a, b, p) / area);
                if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
                    continue;
                }
                let inv = (l0 * a[2] + l1 * b[2] + l2 * c[2]) as f32;
                let k = y * self.w + x;
                if inv > self.depth[k] {
                    (self.depth[k], self.pix[k], self.ids[k], self.tris[k]) = (inv, color, id, self.tri);
                }
            }
        }
    }

    fn line(&mut self, camera: &Camera, p: P3, q: P3) {
        let (w, h) = (self.w as f64, self.h as f64);
        let (Some(a), Some(b)) = (camera.project(p, w, h), camera.project(q, w, h)) else { return };
        let steps = (b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil().clamp(1.0, 20_000.0) as usize;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let (x, y) = (a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]));
            if x < 0.0 || y < 0.0 || x >= w || y >= h {
                continue;
            }
            let k = y as usize * self.w + x as usize;
            let inv = (a[2] + t * (b[2] - a[2])) as f32;
            if inv >= self.depth[k] {
                (self.depth[k], self.pix[k]) = (inv, GRID);
            }
        }
    }
}

/// Twice the signed area of the triangle `a`, `b`, `p` in the picture.
fn edge(a: [f64; 3], b: [f64; 3], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// Each object of `scene` alone (no plate), seen from the front and above, framed to fill a square of `size` pixels on a clear
/// background: the thumbnail a printer's screen shows.
pub fn thumbnail(scene: &Scene, size: usize) -> Picture {
    let bounds = scene.objects.iter().filter_map(|o| o.bounds()).reduce(|(alo, ahi), (blo, bhi)| {
        ([alo[0].min(blo[0]), alo[1].min(blo[1]), alo[2].min(blo[2])], [ahi[0].max(bhi[0]), ahi[1].max(bhi[1]), ahi[2].max(bhi[2])])
    });
    let mut camera = Camera { azimuth: -60.0, elevation: 30.0, ..Camera::default() };
    if let Some(b) = bounds {
        camera.frame_all(b);
        camera.distance *= 0.8;
    }
    let size = size.clamp(1, 1024);
    let (_, _, _, forward) = camera.frame();
    let light = normalize(add(scale(forward, -1.0), [0.25, 0.15, 0.7]));
    let n = size * size;
    let mut r =
        Raster { w: size, h: size, pix: vec![[0, 0, 0, 0]; n], depth: vec![0.0; n], ids: vec![0; n], tris: vec![0; n], tri: 0, light };
    for o in &scene.objects {
        let world = o.world();
        for i in 0..world.triangles.len() {
            if let Some(t) = world.tri(i) {
                r.tri = i as u32;
                r.shaded(&camera, t, MODEL, o.id);
            }
        }
    }
    Picture { size: [size, size], pixels: r.pix, ids: r.ids, tris: r.tris, depth: r.depth }
}

/// The picture as a PNG file: eight bits a channel, RGBA, each row unfiltered, the image data deflated in a zlib stream.
pub fn png(p: &Picture) -> Vec<u8> {
    let [w, h] = p.size;
    let mut raw = Vec::with_capacity(h * (1 + 4 * w));
    for row in p.pixels.chunks(w.max(1)).take(h) {
        raw.push(0); // filter: none
        raw.extend(row.iter().flatten());
    }
    let chunk = |out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut typed = kind.to_vec();
        typed.extend_from_slice(data);
        out.extend_from_slice(&typed);
        out.extend_from_slice(&crate::zip::crc32(&typed).to_be_bytes());
    };
    let mut out = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits, RGBA, deflate, adaptive filtering, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
    chunk(&mut out, b"IEND", &[]);
    out
}

pub fn add(a: P3, b: P3) -> P3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: P3, k: f64) -> P3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: P3, b: P3) -> P3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn normalize(a: P3) -> P3 {
    let len = dot(a, a).sqrt();
    if len < 1e-12 { [0.0, 0.0, 1.0] } else { scale(a, 1.0 / len) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mesh;

    #[test]
    fn a_thumbnail_shows_the_part_on_a_clear_background_and_is_a_png() {
        let mut s = Scene::default();
        s.add("box", Mesh::cuboid([20.0, 10.0, 15.0])).unwrap();
        let t = thumbnail(&s, 64);
        assert_eq!(t.size, [64, 64]);
        assert_eq!(t.pixels[0][3], 0, "the corner is clear");
        assert_eq!(t.id_at(32, 32), Some(1), "the part fills the middle");
        let png = png(&t);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes([png[16], png[17], png[18], png[19]]), 64);
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        // The image data comes back as written.
        let at = png.windows(4).position(|w| w == b"IDAT").unwrap();
        let len = u32::from_be_bytes([png[at - 4], png[at - 3], png[at - 2], png[at - 1]]) as usize;
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&png[at + 4..at + 4 + len]).unwrap();
        assert_eq!(raw.len(), 64 * (1 + 64 * 4));
    }
}
