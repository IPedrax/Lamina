//! The 3D view: the plate and the objects on it, drawn on the processor into a picture egui shows. The picture is redrawn only when
//! the scene, the selection, the camera or the size changes, and each pixel keeps the id of the object drawn there, which is the pick.
//! A primary drag orbits, a secondary or middle drag pans, the wheel zooms, a click picks.
//!
//! shortcut: a software rasterizer, enough for scenes of tens of thousands of triangles; a GPU pass like BIMCraft's
//! (`crates/ui-egui/src/gpu.rs`) is the upgrade for large imported meshes.

use std::sync::Arc;

use egui::{Color32, ColorImage, PointerButton, Rect, Sense, TextureHandle, TextureOptions, Ui, pos2};
use lamina_core::Scene;

type P3 = [f64; 3];

/// Vertical field of view, degrees, and the nearest the eye sees, mm.
const FOV: f64 = 35.0;
const NEAR: f64 = 1.0;

const BACKGROUND: Color32 = Color32::from_rgb(38, 41, 46);
const PLATE: Color32 = Color32::from_rgb(62, 66, 72);
const GRID: Color32 = Color32::from_rgb(86, 91, 98);
const MODEL: Color32 = Color32::from_rgb(110, 165, 225);
const SELECTED: Color32 = Color32::from_rgb(245, 165, 60);

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

    /// Looks at `bounds` from where the camera looks now, from far enough to see all of them.
    pub fn frame_all(&mut self, (lo, hi): (P3, P3)) {
        self.target = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
        let radius = (dot(sub(hi, lo), sub(hi, lo))).sqrt() / 2.0;
        self.distance = (radius / (FOV.to_radians() / 2.0).sin() * 1.15).clamp(20.0, 5000.0);
    }
}

/// A picture of the plate, and the object drawn at each pixel (0 for none).
pub struct Picture {
    pub image: ColorImage,
    ids: Vec<u32>,
}

impl Picture {
    pub fn id_at(&self, x: usize, y: usize) -> Option<u32> {
        let [w, h] = self.image.size;
        (x < w && y < h).then(|| self.ids[y * w + x]).filter(|&id| id != 0)
    }
}

struct Raster {
    w: usize,
    h: usize,
    pix: Vec<Color32>,
    depth: Vec<f32>,
    ids: Vec<u32>,
    light: P3,
}

/// The scene seen through `camera`, `size` pixels, with the objects in `selected` drawn in the highlight colour.
pub fn draw(scene: &Scene, selected: &[u32], camera: &Camera, bed: [f64; 2], size: [usize; 2]) -> Picture {
    let (w, h) = (size[0].max(1), size[1].max(1));
    let (_, _, _, forward) = camera.frame();
    // A light from over the viewer's shoulder, so faces turned to the eye are bright and the top reads from the sides.
    let light = normalize(add(scale(forward, -1.0), [0.25, 0.15, 0.7]));
    let mut r = Raster { w, h, pix: vec![BACKGROUND; w * h], depth: vec![0.0; w * h], ids: vec![0; w * h], light };
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
                r.shaded(camera, t, base, o.id);
            }
        }
    }
    Picture { image: ColorImage::new([w, h], r.pix), ids: r.ids }
}

impl Raster {
    fn shaded(&mut self, camera: &Camera, t: [P3; 3], base: Color32, id: u32) {
        let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
        let len = dot(n, n).sqrt();
        if len < 1e-12 {
            return;
        }
        // Both sides lit alike: an open mesh still reads.
        let k = 0.35 + 0.65 * (dot(n, self.light) / len).abs();
        let tint = |c: u8| (f64::from(c) * k).round().clamp(0.0, 255.0) as u8;
        self.fill(camera, t, Color32::from_rgb(tint(base.r()), tint(base.g()), tint(base.b())), id);
    }

    fn fill(&mut self, camera: &Camera, t: [P3; 3], color: Color32, id: u32) {
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
                    (self.depth[k], self.pix[k], self.ids[k]) = (inv, color, id);
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

/// What a frame of the view drew, so the next draws again only if one of these changed.
#[derive(Clone, PartialEq)]
struct Drawn {
    scene: usize,
    camera: Camera,
    selected: Vec<u32>,
    bed: [f64; 2],
    size: [usize; 2],
}

/// What a click in the view landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Nothing,
    Object(u32),
}

#[derive(Default)]
pub struct View {
    pub camera: Camera,
    texture: Option<TextureHandle>,
    picture: Option<Picture>,
    drawn: Option<Drawn>,
}

impl View {
    /// Draws the view in the space left in `ui`, moves the camera with the pointer, and says what a click picked.
    pub fn show(&mut self, ui: &mut Ui, scene: &Arc<Scene>, selected: &[u32], bed: [f64; 2]) -> Option<Pick> {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let d = response.drag_delta();
        if response.dragged_by(PointerButton::Primary) {
            self.camera.azimuth -= f64::from(d.x) * 0.4;
            self.camera.elevation = (self.camera.elevation + f64::from(d.y) * 0.4).clamp(-89.0, 89.0);
        }
        if response.dragged_by(PointerButton::Secondary) || response.dragged_by(PointerButton::Middle) {
            let (_, right, up, _) = self.camera.frame();
            let k = self.camera.distance * 0.0015;
            self.camera.target = add(self.camera.target, add(scale(right, -f64::from(d.x) * k), scale(up, f64::from(d.y) * k)));
        }
        if response.hovered() {
            let wheel = f64::from(ui.input(|i| i.smooth_scroll_delta.y));
            if wheel != 0.0 {
                self.camera.distance = (self.camera.distance * (1.0 - wheel * 0.002).clamp(0.5, 1.5)).clamp(20.0, 5000.0);
            }
        }
        let ppp = ui.ctx().pixels_per_point();
        let size = [(rect.width() * ppp).round().max(1.0) as usize, (rect.height() * ppp).round().max(1.0) as usize];
        let now = Drawn { scene: Arc::as_ptr(scene) as usize, camera: self.camera, selected: selected.to_vec(), bed, size };
        if self.drawn.as_ref() != Some(&now) {
            let picture = draw(scene, selected, &self.camera, bed, size);
            match &mut self.texture {
                Some(t) => t.set(picture.image.clone(), TextureOptions::LINEAR),
                None => self.texture = Some(ui.ctx().load_texture("lamina-3d", picture.image.clone(), TextureOptions::LINEAR)),
            }
            (self.picture, self.drawn) = (Some(picture), Some(now));
        }
        if let Some(t) = &self.texture {
            ui.painter_at(rect).image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        if !response.clicked() {
            return None;
        }
        let p = response.interact_pointer_pos()?;
        let (x, y) = (((p.x - rect.min.x) * ppp).max(0.0) as usize, ((p.y - rect.min.y) * ppp).max(0.0) as usize);
        Some(self.picture.as_ref().and_then(|pic| pic.id_at(x, y)).map_or(Pick::Nothing, Pick::Object))
    }
}

fn add(a: P3, b: P3) -> P3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: P3, k: f64) -> P3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: P3, b: P3) -> P3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize(a: P3) -> P3 {
    let len = dot(a, a).sqrt();
    if len < 1e-12 { [0.0, 0.0, 1.0] } else { scale(a, 1.0 / len) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lamina_core::Mesh;

    fn scene_with_box() -> Scene {
        let mut s = Scene::default();
        s.add("box", Mesh::cuboid([40.0, 40.0, 40.0])).unwrap();
        s
    }

    #[test]
    fn the_object_is_drawn_where_the_camera_looks_and_can_be_picked() {
        let s = scene_with_box();
        let cam = Camera { target: [0.0, 0.0, 20.0], ..Camera::default() };
        let pic = draw(&s, &[], &cam, [220.0, 220.0], [200, 150]);
        assert_eq!(pic.id_at(100, 75), Some(1), "the middle of the picture is the box");
        assert_eq!(pic.id_at(2, 2), None, "a corner is background");
        let mid = pic.image.pixels[75 * 200 + 100];
        assert!(mid.b() > mid.r(), "drawn in the model colour: {mid:?}");
        let chosen = draw(&s, &[1], &cam, [220.0, 220.0], [200, 150]).image.pixels[75 * 200 + 100];
        assert!(chosen.r() > chosen.b(), "a selected object is drawn in the highlight: {chosen:?}");
    }

    #[test]
    fn the_nearer_face_wins_and_nothing_behind_the_eye_is_drawn() {
        let mut s = scene_with_box();
        // A second box straight behind the first, seen from the front: the first hides it.
        let id = s.add("behind", Mesh::cuboid([40.0, 40.0, 40.0])).unwrap();
        s.get_mut(id).unwrap().xf = lamina_core::Transform::translate([0.0, 200.0, 0.0]);
        let front = Camera { target: [0.0, 0.0, 20.0], azimuth: -90.0, elevation: 0.0, distance: 300.0 };
        assert_eq!(draw(&s, &[], &front, [400.0, 400.0], [100, 100]).id_at(50, 50), Some(1));
        let inside = Camera { distance: 5.0, ..front };
        let _ = draw(&s, &[], &inside, [400.0, 400.0], [64, 64]); // must not panic with geometry around and behind the eye
    }

    #[test]
    fn framing_puts_the_whole_scene_in_view() {
        let mut cam = Camera::default();
        cam.frame_all(([-100.0, -100.0, 0.0], [100.0, 100.0, 50.0]));
        for corner in [[-100.0, -100.0, 0.0], [100.0, 100.0, 50.0], [100.0, -100.0, 0.0]] {
            let p = cam.project(corner, 800.0, 600.0).unwrap();
            assert!((0.0..=800.0).contains(&p[0]) && (0.0..=600.0).contains(&p[1]), "{corner:?} -> {p:?}");
        }
    }
}
