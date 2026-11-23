//! The 3D view: the plate and the objects on it, drawn on the processor into a picture egui shows. The picture is redrawn only when
//! the scene, the selection, the camera or the size changes, and each pixel keeps the id of the object drawn there, which is the pick.
//! A primary drag orbits, a secondary or middle drag pans, the wheel zooms, a click picks. One picked object carries a move gizmo: an
//! arrow for each axis and a square to slide it on the plate; a drag that starts on one moves the object (seen as it goes, snapped to
//! half a millimetre unless Shift is held) and, on release, says how far, which the window runs as one `move` command.
//!
//! shortcut: a software rasterizer, enough for scenes of tens of thousands of triangles; a GPU pass like BIMCraft's
//! (`crates/ui-egui/src/gpu.rs`) is the upgrade for large imported meshes.

use std::sync::Arc;

use egui::{
    Color32, ColorImage, FontId, PointerButton, Pos2, Rect, Sense, Shape, Stroke, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};
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
    moving: Option<(u32, P3)>,
}

/// What a click in the view landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Nothing,
    Object(u32),
}

/// A part of the move gizmo: an axis arrow, or the square that slides the object on the plate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    X,
    Y,
    Z,
    Plate,
}

/// How long the arrows are on screen, points, however far the camera is; how near the pointer must be to one; the snap, mm.
const ARROW: f64 = 70.0;
const NEAR_ARROW: f32 = 9.0;
const STEP: f64 = 0.5;

/// The gizmo as it lands on screen: its middle, each arrow's tip, the plate square's corners, and the millimetres an arrow stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gizmo {
    pub origin: Pos2,
    pub tips: [Pos2; 3],
    pub plate: [Pos2; 4],
    pub length: f64,
}

impl Gizmo {
    /// The gizmo at `at`, seen through `camera` in `rect`; none when it is behind the eye.
    pub fn at(camera: &Camera, rect: Rect, at: P3) -> Option<Gizmo> {
        let (w, h) = (f64::from(rect.width()), f64::from(rect.height()));
        let to = |p: [f64; 3]| pos2(rect.min.x + p[0] as f32, rect.min.y + p[1] as f32);
        let o = camera.project(at, w, h)?;
        let f = (h / 2.0) / (FOV.to_radians() / 2.0).tan();
        // An arrow of `length` mm at that depth is ARROW points long on screen.
        let length = ARROW / (f * o[2]);
        let tip = |k: usize| {
            let mut p = at;
            p[k] += length;
            camera.project(p, w, h).map(to)
        };
        let s = length * 0.35;
        let corner = |dx: f64, dy: f64| camera.project([at[0] + dx, at[1] + dy, at[2]], w, h).map(to);
        Some(Gizmo {
            origin: to(o),
            tips: [tip(0)?, tip(1)?, tip(2)?],
            plate: [corner(0.0, 0.0)?, corner(s, 0.0)?, corner(s, s)?, corner(0.0, s)?],
            length,
        })
    }

    /// The handle under `p`: the plate square first, then the nearest arrow within reach.
    pub fn handle_at(&self, p: Pos2) -> Option<Handle> {
        if inside(&self.plate, p) {
            return Some(Handle::Plate);
        }
        [Handle::X, Handle::Y, Handle::Z]
            .into_iter()
            .zip(self.tips)
            .map(|(h, tip)| (h, distance_to_segment(p, self.origin, tip)))
            .filter(|&(_, d)| d <= NEAR_ARROW)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h)
    }

    /// Where on screen a test grabs `handle`: an arrow's tip, or the middle of the plate square.
    pub fn grip(&self, handle: Handle) -> Pos2 {
        match handle {
            Handle::X => self.tips[0],
            Handle::Y => self.tips[1],
            Handle::Z => self.tips[2],
            Handle::Plate => {
                let sum = self.plate.iter().fold(Vec2::ZERO, |s, p| s + p.to_vec2());
                (sum / 4.0).to_pos2()
            }
        }
    }
}

/// How far a drag on `handle` from `start` to `now` (screen points in `rect`) moves the object standing at `at`: along the arrow's
/// line on screen for an axis, across the level plane through `at` for the plate square. Snapped to [`STEP`] unless `free`.
pub fn drag_offset(camera: &Camera, rect: Rect, at: P3, handle: Handle, start: Pos2, now: Pos2, free: bool) -> P3 {
    let snap = |v: f64| if free { v } else { (v / STEP).round() * STEP };
    let Some(g) = Gizmo::at(camera, rect, at) else { return [0.0; 3] };
    let axis = match handle {
        Handle::X => 0,
        Handle::Y => 1,
        Handle::Z => 2,
        Handle::Plate => {
            let (w, h) = (f64::from(rect.width()), f64::from(rect.height()));
            let hit = |p: Pos2| camera.on_level(f64::from(p.x - rect.min.x), f64::from(p.y - rect.min.y), w, h, at[2]);
            return match (hit(start), hit(now)) {
                (Some(a), Some(b)) => [snap(b[0] - a[0]), snap(b[1] - a[1]), 0.0],
                _ => [0.0; 3],
            };
        }
    };
    let along = g.tips[axis] - g.origin;
    if along.length_sq() < 1.0 {
        return [0.0; 3]; // the axis points at the eye: it cannot be dragged along from here
    }
    let mut by = [0.0; 3];
    by[axis] = snap(f64::from((now - start).dot(along) / along.length_sq()) * g.length);
    by
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() < 1e-6 { 0.0 } else { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) };
    (a + ab * t - p).length()
}

/// Whether `p` is inside the convex quad `q`, wound either way.
fn inside(q: &[Pos2; 4], p: Pos2) -> bool {
    let side = |a: Pos2, b: Pos2| (b - a).x * (p - a).y - (b - a).y * (p - a).x;
    let s: Vec<f32> = (0..4).map(|k| side(q[k], q[(k + 1) % 4])).collect();
    s.iter().all(|&v| v >= 0.0) || s.iter().all(|&v| v <= 0.0)
}

/// A drag on the gizmo under way: whose, which handle, where it began on screen and in the model, and how far it has gone.
#[derive(Clone, Copy, Debug)]
struct Moving {
    id: u32,
    handle: Handle,
    start: Pos2,
    at: P3,
    by: P3,
}

/// What a frame of the view did: a click's pick, and a gizmo drag that ended, as the object and how far it went.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shown {
    pub pick: Option<Pick>,
    pub moved: Option<(u32, P3)>,
}

#[derive(Default)]
pub struct View {
    pub camera: Camera,
    texture: Option<TextureHandle>,
    picture: Option<Picture>,
    drawn: Option<Drawn>,
    moving: Option<Moving>,
    /// The gizmo as last drawn, for a test to find its handles.
    pub gizmo: Option<Gizmo>,
}

impl View {
    /// Draws the view in the space left in `ui`, moves the camera or the gizmo with the pointer, and says what a click picked and how
    /// far a gizmo drag moved its object.
    pub fn show(&mut self, ui: &mut Ui, scene: &Arc<Scene>, selected: &[u32], bed: [f64; 2]) -> Shown {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let mut shown = Shown::default();
        // The gizmo stands at the middle of the one object picked.
        let one = match selected {
            &[id] => scene
                .get(id)
                .ok()
                .and_then(|o| o.bounds())
                .map(|(lo, hi)| (id, [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0])),
            _ => None,
        };
        let press = ui.input(|i| i.pointer.press_origin());
        if response.drag_started_by(PointerButton::Primary)
            && let (Some((id, at)), Some(start)) = (one, press)
            && let Some(handle) = Gizmo::at(&self.camera, rect, at).and_then(|g| g.handle_at(start))
        {
            self.moving = Some(Moving { id, handle, start, at, by: [0.0; 3] });
        }
        let d = response.drag_delta();
        if let Some(m) = &mut self.moving {
            if let Some(now) = response.interact_pointer_pos() {
                let free = ui.input(|i| i.modifiers.shift);
                m.by = drag_offset(&self.camera, rect, m.at, m.handle, m.start, now, free);
            }
            if response.drag_stopped() || !response.dragged() {
                shown.moved = Some((m.id, m.by)).filter(|(_, by)| by.iter().any(|c| *c != 0.0));
                self.moving = None;
            }
        } else if response.dragged_by(PointerButton::Primary) {
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
        // While a drag is under way the object is drawn where it would go; the scene itself changes only when the window runs the move.
        let moving = self.moving.map(|m| (m.id, m.by));
        let ppp = ui.ctx().pixels_per_point();
        let size = [(rect.width() * ppp).round().max(1.0) as usize, (rect.height() * ppp).round().max(1.0) as usize];
        let now = Drawn { scene: Arc::as_ptr(scene) as usize, camera: self.camera, selected: selected.to_vec(), bed, size, moving };
        if self.drawn.as_ref() != Some(&now) {
            let picture = match moving {
                Some((id, by)) => {
                    let mut seen = Scene::clone(scene);
                    if let Ok(o) = seen.get_mut(id) {
                        o.xf = o.xf.then(&lamina_core::Transform::translate(by));
                    }
                    draw(&seen, selected, &self.camera, bed, size)
                }
                None => draw(scene, selected, &self.camera, bed, size),
            };
            match &mut self.texture {
                Some(t) => t.set(picture.image.clone(), TextureOptions::LINEAR),
                None => self.texture = Some(ui.ctx().load_texture("lamina-3d", picture.image.clone(), TextureOptions::LINEAR)),
            }
            (self.picture, self.drawn) = (Some(picture), Some(now));
        }
        let painter = ui.painter_at(rect);
        if let Some(t) = &self.texture {
            painter.image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        self.gizmo = one.and_then(|(_, at)| Gizmo::at(&self.camera, rect, add(at, moving.map_or([0.0; 3], |(_, by)| by))));
        if let Some(g) = self.gizmo {
            let active = self.moving.map(|m| m.handle).or_else(|| response.hover_pos().and_then(|p| g.handle_at(p)));
            paint_gizmo(&painter, &g, active);
            if let (Some(m), Some(p)) = (self.moving, response.interact_pointer_pos()) {
                let text = format!("{:+.1}, {:+.1}, {:+.1} mm", m.by[0], m.by[1], m.by[2]);
                painter.text(p + vec2(16.0, -16.0), egui::Align2::LEFT_BOTTOM, text, FontId::proportional(13.0), Color32::WHITE);
            }
        }
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
            && self.gizmo.and_then(|g| g.handle_at(p)).is_none()
        {
            let (x, y) = (((p.x - rect.min.x) * ppp).max(0.0) as usize, ((p.y - rect.min.y) * ppp).max(0.0) as usize);
            shown.pick = Some(self.picture.as_ref().and_then(|pic| pic.id_at(x, y)).map_or(Pick::Nothing, Pick::Object));
        }
        shown
    }
}

fn paint_gizmo(painter: &egui::Painter, g: &Gizmo, active: Option<Handle>) {
    let lit = |h: Handle, c: Color32| if active == Some(h) { Color32::from_rgb(255, 230, 90) } else { c };
    // White, so it shows on a picked object, which is drawn in the highlight orange.
    let plate = if active == Some(Handle::Plate) {
        Color32::from_rgba_unmultiplied(255, 230, 90, 150)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 70)
    };
    painter.add(Shape::convex_polygon(g.plate.to_vec(), plate, Stroke::new(1.5, Color32::from_rgb(240, 240, 240))));
    let colours = [Color32::from_rgb(230, 85, 80), Color32::from_rgb(95, 200, 95), Color32::from_rgb(90, 145, 245)];
    for ((h, tip), c) in [Handle::X, Handle::Y, Handle::Z].into_iter().zip(g.tips).zip(colours) {
        let c = lit(h, c);
        let v = tip - g.origin;
        if v.length_sq() < 1.0 {
            continue;
        }
        let (v, n) = (v.normalized(), v.normalized().rot90());
        painter.line_segment([g.origin, tip - v * 8.0], Stroke::new(if active == Some(h) { 4.0 } else { 3.0 }, c));
        painter.add(Shape::convex_polygon(vec![tip, tip - v * 13.0 + n * 6.0, tip - v * 13.0 - n * 6.0], c, Stroke::NONE));
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

    fn rect() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 600.0))
    }

    #[test]
    fn each_handle_is_found_where_it_is_drawn() {
        let cam = Camera { target: [0.0, 0.0, 10.0], ..Camera::default() };
        let g = Gizmo::at(&cam, rect(), [0.0, 0.0, 10.0]).unwrap();
        for h in [Handle::X, Handle::Y, Handle::Z, Handle::Plate] {
            assert_eq!(g.handle_at(g.grip(h)), Some(h), "{h:?}");
        }
        assert_eq!(g.handle_at(pos2(105.0, 55.0)), None, "a corner of the view is no handle");
        // The arrows stay the same size on screen as the camera backs away.
        let far = Gizmo::at(&Camera { distance: 1500.0, ..cam }, rect(), [0.0, 0.0, 10.0]).unwrap();
        assert!(((far.tips[0] - far.origin).length() - (g.tips[0] - g.origin).length()).abs() < 3.0);
    }

    #[test]
    fn dragging_an_arrow_moves_along_its_axis_by_what_it_shows() {
        let cam = Camera { target: [0.0, 0.0, 10.0], ..Camera::default() };
        let at = [0.0, 0.0, 10.0];
        let g = Gizmo::at(&cam, rect(), at).unwrap();
        // Dragging the x arrow by its own length moves the object by the length it stands for, snapped to half a millimetre.
        let by = drag_offset(&cam, rect(), at, Handle::X, g.origin, g.tips[0], false);
        assert!((by[0] - (g.length / STEP).round() * STEP).abs() < 1e-9 && by[1] == 0.0 && by[2] == 0.0, "{by:?} {}", g.length);
        let free = drag_offset(&cam, rect(), at, Handle::Z, g.origin, g.tips[2], true);
        assert!((free[2] - g.length).abs() < 0.05 * g.length && free[0] == 0.0);
        // Sliding on the plate follows the pointer on the level plane and never changes the height.
        let start = g.grip(Handle::Plate);
        let a = cam.on_level(f64::from(start.x - 100.0), f64::from(start.y - 50.0), 800.0, 600.0, 10.0).unwrap();
        let end = start + vec2(60.0, -25.0);
        let b = cam.on_level(f64::from(end.x - 100.0), f64::from(end.y - 50.0), 800.0, 600.0, 10.0).unwrap();
        let slid = drag_offset(&cam, rect(), at, Handle::Plate, start, end, true);
        assert!((slid[0] - (b[0] - a[0])).abs() < 1e-6 && (slid[1] - (b[1] - a[1])).abs() < 1e-6 && slid[2] == 0.0);
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
