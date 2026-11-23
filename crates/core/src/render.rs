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
pub const NEAR: f64 = 1.0;
/// Where a cut at the near plane puts the new corner: a hair in front of it, so rounding never puts it behind.
const CLIP: f64 = NEAR * 1.01;

pub const BACKGROUND: Rgba = [38, 41, 46, 255];
pub const PLATE: Rgba = [62, 66, 72, 255];
pub const GRID: Rgba = [86, 91, 98, 255];
pub const MODEL: Rgba = [110, 165, 225, 255];
pub const SELECTED: Rgba = [245, 165, 60, 255];
/// The plane given to what is not a part's face (the plate): level, through the origin.
const FLAT: [f32; 4] = [0.0, 0.0, 1.0, 0.0];
/// How much an edge darkens the pixel it is drawn on: where a part's faces meet at a crease or a step, and round each part.
pub const EDGE_SHADE: f64 = 0.42;
/// Faces whose normals differ by more than this, degrees, meet at a crease; a curved surface's facets turn by less, and stay smooth.
pub const CREASE: f64 = 35.0;
/// Parallel faces further apart than this, mm, are a step (a pocket's floor beside the top it is cut into, raised lettering).
pub const STEP: f32 = 0.05;

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

    /// The light: from over the viewer's shoulder, so faces turned to the eye are bright and the top reads from the sides.
    pub fn light(&self) -> P3 {
        normalize(add(scale(self.frame().3, -1.0), [0.25, 0.15, 0.7]))
    }

    /// The point seen at pixel (`x`, `y`) of a picture `size` pixels, at one over depth `inv` (as [`Picture`] keeps it); none where
    /// nothing is (`inv` not above 0).
    pub fn at_depth(&self, x: usize, y: usize, size: [usize; 2], inv: f32) -> Option<P3> {
        let inv = f64::from(inv);
        if inv <= 0.0 {
            return None;
        }
        let (eye, dir) = self.ray(x as f64 + 0.5, y as f64 + 0.5, size[0] as f64, size[1] as f64);
        let t = (1.0 / inv) / dot(dir, self.frame().3);
        Some(add(eye, scale(dir, t)))
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
        camera.at_depth(x, y, self.size, self.depth[y * w + x])
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
    /// At each pixel the plane drawn there: its unit normal, turned to the eye, and its distance from the origin. And the plane of the
    /// triangle being filled.
    planes: Vec<[f32; 4]>,
    plane: [f32; 4],
    light: P3,
}

/// The scene seen through `camera`, `size` pixels, with the objects in `selected` drawn in the highlight colour.
pub fn draw(scene: &Scene, selected: &[u32], camera: &Camera, bed: [f64; 2], size: [usize; 2]) -> Picture {
    let mut r = Raster::new(camera, size);
    r.plate(camera, bed);
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
    r.outline();
    r.picture()
}

/// The most segments [`draw_paths`] draws; past it, the rest are left out (a slice of a large plate has a few million).
pub const MAX_PATHS: usize = 4_000_000;

/// A printed line: its two ends, its colour, and how wide it prints, mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Path {
    pub from: P3,
    pub to: P3,
    pub color: Rgba,
    pub width: f64,
}

/// The plate and `paths` (a slice's extrusions) seen through `camera`, `size` pixels: each line as wide as it prints and shaded
/// round, so it reads as a bead of plastic, nearer lines in front. The G-code preview in 3D.
pub fn draw_paths(paths: &[Path], camera: &Camera, bed: [f64; 2], size: [usize; 2]) -> Picture {
    let mut r = Raster::new(camera, size);
    r.plate(camera, bed);
    for p in paths.iter().take(MAX_PATHS) {
        r.bead(camera, p);
    }
    r.picture()
}

impl Raster {
    fn new(camera: &Camera, size: [usize; 2]) -> Raster {
        let (w, h) = (size[0].clamp(1, 8192), size[1].clamp(1, 8192));
        let light = camera.light();
        let n = w * h;
        Raster {
            w,
            h,
            pix: vec![BACKGROUND; n],
            depth: vec![0.0; n],
            ids: vec![0; n],
            tris: vec![0; n],
            tri: 0,
            planes: vec![[0.0; 4]; n],
            plane: FLAT,
            light,
        }
    }

    /// Darkens the pixels where two visible surfaces meet: the edge of a part against the plate or another part, a crease between faces,
    /// and a step between parallel faces. Without them a pocket's floor and the top it is cut into, both facing up, are one flat colour.
    /// Of two pixels either side of an edge, the nearer is darkened, so an outline belongs to what is in front.
    fn outline(&mut self) {
        let crease = CREASE.to_radians().cos() as f32;
        let differs = |a: usize, b: usize| {
            if self.ids[a] != self.ids[b] {
                return true;
            }
            if self.ids[a] == 0 {
                return false; // the plate and the background
            }
            let (p, q) = (self.planes[a], self.planes[b]);
            let along = p[0] * q[0] + p[1] * q[1] + p[2] * q[2];
            along < crease || (along > 0.999 && (p[3] - q[3]).abs() > STEP)
        };
        let mut edge = vec![false; self.pix.len()];
        for y in 0..self.h {
            for x in 0..self.w {
                let a = y * self.w + x;
                for b in [(x + 1 < self.w).then_some(a + 1), (y + 1 < self.h).then_some(a + self.w)].into_iter().flatten() {
                    if differs(a, b) {
                        edge[if self.depth[a] >= self.depth[b] { a } else { b }] = true;
                    }
                }
            }
        }
        for (px, on) in self.pix.iter_mut().zip(edge) {
            if on {
                for c in &mut px[..3] {
                    *c = (f64::from(*c) * EDGE_SHADE).round() as u8;
                }
            }
        }
    }

    fn picture(self) -> Picture {
        Picture { size: [self.w, self.h], pixels: self.pix, ids: self.ids, tris: self.tris, depth: self.depth }
    }

    /// The plate, `bed` mm, centred on the origin, with a line every 10 mm.
    fn plate(&mut self, camera: &Camera, bed: [f64; 2]) {
        let (hx, hy) = (bed[0] / 2.0, bed[1] / 2.0);
        let c = [[-hx, -hy, 0.0], [hx, -hy, 0.0], [hx, hy, 0.0], [-hx, hy, 0.0]];
        self.fill(camera, [c[0], c[1], c[2]], PLATE, 0);
        self.fill(camera, [c[0], c[2], c[3]], PLATE, 0);
        // A hair above the plate so the lines win the depth test against it and lose to anything standing on it.
        let mut x = -hx;
        while x <= hx + 1e-9 {
            self.line(camera, [x, -hy, 0.01], [x, hy, 0.01]);
            x += 10.0;
        }
        let mut y = -hy;
        while y <= hy + 1e-9 {
            self.line(camera, [-hx, y, 0.01], [hx, y, 0.01]);
            y += 10.0;
        }
    }

    /// A printed line: a run of discs along it, each as wide as the line looks at its depth, darker towards its edges.
    fn bead(&mut self, camera: &Camera, path: &Path) {
        let Some((from, to)) = clip(camera, path.from, path.to) else { return };
        let (w, h) = (self.w as f64, self.h as f64);
        let (Some(a), Some(b)) = (camera.project(from, w, h), camera.project(to, w, h)) else { return };
        if !path.width.is_finite() || path.width <= 0.0 {
            return;
        }
        let f = (h / 2.0) / (FOV.to_radians() / 2.0).tan();
        let steps = (b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil().clamp(1.0, 20_000.0) as usize;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let (x, y, inv) = (a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), a[2] + t * (b[2] - a[2]));
            let r = (path.width / 2.0 * f * inv).clamp(0.5, 12.0);
            let (x0, x1) = ((x - r).floor().max(0.0), (x + r).ceil().min(w - 1.0));
            let (y0, y1) = ((y - r).floor().max(0.0), (y + r).ceil().min(h - 1.0));
            if x1 < x0 || y1 < y0 {
                continue;
            }
            for py in y0 as usize..=y1 as usize {
                for px in x0 as usize..=x1 as usize {
                    let d2 = ((px as f64 + 0.5 - x).powi(2) + (py as f64 + 0.5 - y).powi(2)) / (r * r);
                    if d2 > 1.0 {
                        continue;
                    }
                    // The bead is round: its middle stands a little nearer the eye than its edges, and is lit more.
                    let near = (inv * (1.0 + 0.002 * (1.0 - d2))) as f32;
                    let k = py * self.w + px;
                    if near > self.depth[k] {
                        let shade = 1.0 - 0.45 * d2;
                        let c = path.color.map(|v| (f64::from(v) * shade).round().clamp(0.0, 255.0) as u8);
                        (self.depth[k], self.pix[k], self.ids[k]) = (near, [c[0], c[1], c[2], 255], 0);
                    }
                }
            }
        }
    }

    fn shaded(&mut self, camera: &Camera, t: [P3; 3], base: Rgba, id: u32) {
        let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
        let len = dot(n, n).sqrt();
        if len < 1e-12 {
            return;
        }
        // Both sides lit alike: an open mesh still reads.
        let k = 0.35 + 0.65 * (dot(n, self.light) / len).abs();
        let tint = |c: u8| (f64::from(c) * k).round().clamp(0.0, 255.0) as u8;
        // The plane, its normal turned to the eye, for `outline`.
        let toward = if dot(n, camera.frame().3) > 0.0 { -1.0 / len } else { 1.0 / len };
        let u = scale(n, toward);
        self.plane = [u[0] as f32, u[1] as f32, u[2] as f32, dot(u, t[0]) as f32];
        self.fill(camera, t, [tint(base[0]), tint(base[1]), tint(base[2]), base[3]], id);
        self.plane = FLAT;
    }

    /// A triangle, cut at the near plane first: a plate seen from close by has corners behind the eye, and the part in front must
    /// still be drawn.
    fn fill(&mut self, camera: &Camera, t: [P3; 3], color: Rgba, id: u32) {
        let (eye, _, _, forward) = camera.frame();
        let z = t.map(|p| dot(sub(p, eye), forward));
        if z.iter().all(|&d| d >= NEAR) {
            return self.fill_seen(camera, t, color, id);
        }
        // Clip against z >= NEAR (one plane of Sutherland and Hodgman): three corners become up to four, drawn as a fan.
        let mut kept: Vec<P3> = Vec::with_capacity(4);
        for i in 0..3 {
            let (p, q, zp, zq) = (t[i], t[(i + 1) % 3], z[i], z[(i + 1) % 3]);
            if zp >= NEAR {
                kept.push(p);
            }
            if (zp >= NEAR) != (zq >= NEAR) {
                kept.push(add(p, scale(sub(q, p), (CLIP - zp) / (zq - zp))));
            }
        }
        for k in 1..kept.len().saturating_sub(1) {
            self.fill_seen(camera, [kept[0], kept[k], kept[k + 1]], color, id);
        }
    }

    fn fill_seen(&mut self, camera: &Camera, t: [P3; 3], color: Rgba, id: u32) {
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
                    (self.depth[k], self.pix[k], self.ids[k], self.tris[k], self.planes[k]) = (inv, color, id, self.tri, self.plane);
                }
            }
        }
    }

    fn line(&mut self, camera: &Camera, p: P3, q: P3) {
        let Some((p, q)) = clip(camera, p, q) else { return };
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

/// The part of the segment `p`, `q` in front of the near plane, if any.
fn clip(camera: &Camera, p: P3, q: P3) -> Option<(P3, P3)> {
    let (eye, _, _, forward) = camera.frame();
    let (zp, zq) = (dot(sub(p, eye), forward), dot(sub(q, eye), forward));
    let cut = |a: P3, b: P3, za: f64, zb: f64| add(a, scale(sub(b, a), (CLIP - za) / (zb - za)));
    match (zp >= NEAR, zq >= NEAR) {
        (true, true) => Some((p, q)),
        (false, false) => None,
        (true, false) => Some((p, cut(p, q, zp, zq))),
        (false, true) => Some((cut(q, p, zq, zp), q)),
    }
}

/// Twice the signed area of the triangle `a`, `b`, `p` in the picture.
fn edge(a: [f64; 3], b: [f64; 3], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// Each object of `scene` alone (no plate), seen from the front and above, framed to fill a square of `size` pixels on a clear
/// background: the thumbnail a printer's screen shows.
pub fn thumbnail(scene: &Scene, size: usize) -> Picture {
    let worlds: Vec<crate::Mesh> = scene.objects.iter().map(|o| o.world()).collect();
    thumbnail_of(&worlds, size)
}

/// The same for meshes where they stand.
pub fn thumbnail_of(meshes: &[crate::Mesh], size: usize) -> Picture {
    let bounds = meshes.iter().filter_map(|m| m.bounds()).reduce(|(alo, ahi), (blo, bhi)| {
        ([alo[0].min(blo[0]), alo[1].min(blo[1]), alo[2].min(blo[2])], [ahi[0].max(bhi[0]), ahi[1].max(bhi[1]), ahi[2].max(bhi[2])])
    });
    let mut camera = Camera { azimuth: -60.0, elevation: 30.0, ..Camera::default() };
    if let Some(b) = bounds {
        camera.frame_all(b);
        camera.distance *= 0.8;
    }
    let size = size.clamp(1, 1024);
    let light = camera.light();
    let n = size * size;
    let mut r = Raster {
        w: size,
        h: size,
        pix: vec![[0, 0, 0, 0]; n],
        depth: vec![0.0; n],
        ids: vec![0; n],
        tris: vec![0; n],
        tri: 0,
        planes: vec![[0.0; 4]; n],
        plane: FLAT,
        light,
    };
    for (k, world) in meshes.iter().enumerate() {
        for i in 0..world.triangles.len() {
            if let Some(t) = world.tri(i) {
                r.tri = i as u32;
                r.shaded(&camera, t, MODEL, k as u32 + 1);
            }
        }
    }
    r.outline();
    Picture { size: [size, size], pixels: r.pix, ids: r.ids, tris: r.tris, depth: r.depth }
}

/// The picture as a PNG file: eight bits a channel, RGBA, each row unfiltered, the image data deflated in a zlib stream.
pub fn png(p: &Picture) -> Vec<u8> {
    png_rgba(p.size, &p.pixels)
}

/// Pixels `size` wide and high, row by row, as a PNG file.
pub fn png_rgba(size: [usize; 2], pixels: &[Rgba]) -> Vec<u8> {
    let [w, h] = size;
    let mut raw = Vec::with_capacity(h * (1 + 4 * w));
    for row in pixels.chunks(w.max(1)).take(h) {
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

    /// A part whose faces all face up (a plinth with a block standing on it, as one object) shows its step as an edge: the detail of a
    /// pocket or of raised lettering. The diagonal splitting a flat face into triangles, and a curved surface's facets, show none.
    #[test]
    fn edges_show_steps_and_creases_but_not_a_face_s_own_triangles() {
        let plinth = Mesh::cuboid([60.0, 60.0, 10.0]);
        let block = Mesh::cuboid([20.0, 20.0, 4.0]).transformed(&crate::Transform::translate([0.0, 0.0, 10.0]));
        let mut scene = Scene::default();
        let id = scene.add("stepped", Mesh::merged([&plinth, &block])).unwrap();
        scene.get_mut(id).unwrap().xf = crate::Transform::IDENTITY;
        let camera = Camera { target: [0.0, 0.0, 10.0], azimuth: -90.0, elevation: 89.0, distance: 200.0 };
        let p = draw(&scene, &[], &camera, [220.0, 220.0], [300, 300]);
        let at = |x: f64, y: f64, z: f64| {
            let q = camera.project([x, y, z], 300.0, 300.0).unwrap();
            p.pixels[q[1] as usize * 300 + q[0] as usize]
        };
        let dark = |c: Rgba| u32::from(c[2]) < u32::from(MODEL[2]) * 6 / 10;
        // Straight down, the plinth's top and the block's top are the same colour: only the edge between them tells them apart.
        assert!(!dark(at(-20.0, -20.0, 10.0)) && !dark(at(0.0, 0.0, 14.0)), "faces stay light");
        // An edge is a pixel wide and falls on one side of the exact line or the other: look within a pixel of it.
        let near_edge = |x: f64, y: f64, z: f64| {
            let q = camera.project([x, y, z], 300.0, 300.0).unwrap();
            let (cx, cy) = (q[0] as i64, q[1] as i64);
            (-1..=1).flat_map(|dy| (-1..=1).map(move |dx| (cx + dx, cy + dy))).any(|(x, y)| dark(p.pixels[y as usize * 300 + x as usize]))
        };
        let ring = (0..60).filter(|&k| near_edge(-10.0 + 20.0 * f64::from(k) / 60.0, -10.0, 14.0)).count();
        assert!(ring > 40, "the block's outline is drawn along its edge: {ring} of 60");
        let diagonal = (5..55).filter(|&k| dark(at(-10.0 + 20.0 * f64::from(k) / 60.0, -10.0 + 20.0 * f64::from(k) / 60.0, 14.0))).count();
        assert!(diagonal <= 2, "no line where the block's top is split into triangles: {diagonal}");
        // A sphere's facets meet at a few degrees: it stays smooth, with an outline round it.
        let mut ball = Scene::default();
        ball.add("ball", Mesh::sphere(20.0, 48)).unwrap();
        let q = draw(&ball, &[], &Camera { target: [0.0, 0.0, 20.0], ..camera }, [220.0, 220.0], [300, 300]);
        // An edge is a line a pixel wide, darker than the pixels either side of it; shading only changes slowly across the ball.
        let blue = |x: usize, y: usize| i32::from(q.pixels[y * 300 + x][2]);
        let lines = (130..170)
            .flat_map(|y| (130..170).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let c = blue(x, y);
                (blue(x - 1, y) - c > 25 && blue(x + 1, y) - c > 25) || (blue(x, y - 1) - c > 25 && blue(x, y + 1) - c > 25)
            })
            .count();
        assert!(lines < 20, "a sphere is not drawn as a wireframe: {lines} line pixels in its middle");
    }

    #[test]
    fn printed_lines_are_drawn_as_wide_as_they_print_and_the_nearer_one_in_front() {
        let camera = Camera { target: [0.0, 0.0, 0.0], azimuth: -90.0, elevation: 89.0, distance: 100.0 };
        let red = [220, 40, 40, 255];
        let blue = [40, 40, 220, 255];
        // From above: a red line along x at z 0.2, and a blue one crossing it a layer higher.
        let paths = [
            Path { from: [-20.0, 0.0, 0.2], to: [20.0, 0.0, 0.2], color: red, width: 0.45 },
            Path { from: [0.0, -20.0, 0.4], to: [0.0, 20.0, 0.4], color: blue, width: 0.45 },
        ];
        let p = draw_paths(&paths, &camera, [220.0, 220.0], [200, 200]);
        let at = |x: f64, y: f64| {
            let q = camera.project([x, y, 0.3], 200.0, 200.0).unwrap();
            p.pixels[q[1] as usize * 200 + q[0] as usize]
        };
        let reddish = |c: Rgba| c[0] > 2 * c[2];
        let bluish = |c: Rgba| c[2] > 2 * c[0];
        assert!(reddish(at(10.0, 0.0)) && bluish(at(0.0, 10.0)), "{:?} {:?}", at(10.0, 0.0), at(0.0, 10.0));
        assert!(bluish(at(0.0, 0.0)), "the higher line is in front where they cross");
        assert_eq!(at(5.0, 5.0), PLATE, "the plate between them");
        // A line 0.45 mm wide, 100 mm away in a 200 pixel picture: a few pixels across, not one and not many.
        let q = camera.project([10.0, 0.0, 0.2], 200.0, 200.0).unwrap();
        let across = (0..200).filter(|&y| reddish(p.pixels[y * 200 + q[0] as usize])).count();
        assert!((1..=4).contains(&across), "{across}");
        // Nothing drawn for a line with no width, or behind the eye.
        let none = draw_paths(&[Path { width: f64::NAN, ..paths[0] }], &camera, [220.0, 220.0], [50, 50]);
        assert!(none.pixels.iter().all(|c| !reddish(*c)));
    }

    #[test]
    fn the_plate_seen_from_close_by_is_drawn_up_to_the_eye() {
        // Low over the plate, near its middle: most of its corners are behind the eye, yet all the picture's lower half is plate.
        let camera = Camera { target: [0.0, 0.0, 0.0], azimuth: -90.0, elevation: 10.0, distance: 30.0 };
        let p = draw(&Scene::default(), &[], &camera, [220.0, 220.0], [100, 100]);
        let bottom = &p.pixels[90 * 100..91 * 100];
        assert!(bottom.iter().all(|&c| c == PLATE || c == GRID), "{:?}", &bottom[..4]);
    }

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
