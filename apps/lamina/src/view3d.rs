//! The 3D view: the plate and the objects on it, drawn on the graphics card (`gpu.rs`) or on the processor into a picture egui shows.
//! The picture is redrawn only when the scene, the selection, the camera or the size changes, and each pixel keeps the id of the
//! object drawn there, which is the pick.
//! A primary drag orbits, a secondary or middle drag pans, the wheel zooms, a click picks. The picked objects carry a gizmo: an arrow
//! for each axis and a square to slide them on the plate, a ring for each axis to turn them, and a corner square to scale them; a drag
//! that starts on one changes them (seen as it goes, snapped unless Shift is held) and, on release, says how, which the window runs as
//! one transaction of `move`, `rotate` or `scale` commands.
//!
//! The processor's rasterizer is enough for scenes of tens of thousands of triangles; the card draws the same picture for large
//! imported meshes, and the processor draws when there is no card, the card fails, or the user asks for it.

use std::sync::Arc;

use egui::{
    Color32, ColorImage, FontId, Key, PointerButton, Pos2, Rect, Sense, Shape, Stroke, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};
use lamina_core::{Paint, Scene};

type P3 = [f64; 3];

pub use lamina_core::render::{Camera, FOV, Picture, draw};
use lamina_core::render::{add, dot, scale, sub};

/// The picture as egui shows it.
pub fn image(p: &Picture) -> ColorImage {
    ColorImage::new(p.size, p.pixels.iter().map(|c| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])).collect())
}

/// What a frame of the view drew, so the next draws again only if one of these changed.
#[derive(Clone, PartialEq)]
struct Drawn {
    scene: usize,
    camera: Camera,
    selected: Vec<u32>,
    bed: [f64; 2],
    size: [usize; 2],
    moving: Option<(Vec<u32>, Change)>,
    cpu: bool,
}

/// What a click in the view landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Nothing,
    Object(u32),
}

/// What a click in the view does: pick an object (and move it with the gizmo), lay the face clicked flat, or measure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Move,
    LayFlat,
    Measure,
    Paint,
}

/// What the paint brush lays on the faces it touches (`Paint::None` rubs paint out), and how far round the point it reaches, mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brush {
    pub kind: Paint,
    pub radius: f64,
}

impl Default for Brush {
    fn default() -> Brush {
        Brush { kind: Paint::Seam, radius: 3.0 }
    }
}

impl Brush {
    /// The `kind` the `paint` command takes.
    pub fn command_kind(&self) -> &'static str {
        match self.kind {
            Paint::Seam => "seam",
            Paint::Support => "support",
            Paint::Block => "block",
            Paint::None => "none",
        }
    }
}

/// The most points one stroke of the brush keeps: each is a `paint` command when it ends.
pub const MAX_STROKE: usize = 500;

/// A part of the gizmo: an axis arrow, the square that slides the object on the plate, a ring that turns it about an axis, or the
/// corner square that scales it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    X,
    Y,
    Z,
    Plate,
    /// A turn about axis 0, 1 or 2.
    Ring(usize),
    Scale,
}

/// What a drag on the gizmo does to the picked objects: moves them by mm, turns each about its own middle by degrees about an axis, or
/// scales each about its own middle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    Move(P3),
    Turn(usize, f64),
    Scale(f64),
    /// A scale along one axis alone (Blender's S then X, Y or Z).
    Stretch(usize, f64),
}

impl Change {
    fn is_nothing(&self) -> bool {
        match *self {
            Change::Move(by) => by.iter().all(|c| *c == 0.0),
            Change::Turn(_, d) => d == 0.0,
            Change::Scale(f) | Change::Stretch(_, f) => f == 1.0,
        }
    }

    /// The same, a turn about one of `axes`.
    pub fn about_in(&self, c: P3, axes: &Axes) -> lamina_core::Transform {
        use lamina_core::Transform;
        let around = |t: Transform| Transform::translate(c.map(|v| -v)).then(&t).then(&Transform::translate(c));
        match *self {
            Change::Move(by) => Transform::translate(by),
            Change::Turn(axis, degrees) if *axes == PLATE_AXES => around(Transform::rotate(axis, degrees)),
            Change::Turn(axis, degrees) => around(Transform::rotate_axis(axes[axis.min(2)], degrees)),
            Change::Scale(f) => around(Transform::scale([f; 3])),
            Change::Stretch(axis, f) => {
                let mut by = [1.0; 3];
                by[axis.min(2)] = f;
                around(Transform::scale(by))
            }
        }
    }
}

/// Three unit vectors: the axes moves and turns go along, the plate's or an object's own.
pub type Axes = [P3; 3];
pub const PLATE_AXES: Axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// What the picked objects turn and scale about: each its own middle, or the middle of them all (Blender's Individual Origins and
/// Median Point).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pivot {
    #[default]
    Each,
    Middle,
}

/// Which axes the gizmo and the X, Y and Z keys go along: the plate's, or the picked object's own (Blender's Global and Local).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    #[default]
    Plate,
    Own,
}

/// The commands that make `change` to the objects `ids` of `scene`, about `pivot`, along `axes`, with the undo step's label: a
/// move, turn or scale of each (a turn about a tilted axis given as a vector), and about the middle of them all a move of each
/// as well, so they turn or spread together. The z of that move is left to the turn or scale, which sets each object on the plate.
pub fn commands_for(
    change: Change,
    ids: &[u32],
    scene: &Scene,
    pivot: Pivot,
    axes: &Axes,
) -> (&'static str, Vec<(&'static str, serde_json::Value)>) {
    use serde_json::json;
    let middle = |id: u32| {
        scene.get(id).ok().and_then(|o| o.bounds()).map(|(lo, hi)| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0])
    };
    let all = ids.iter().filter_map(|&id| scene.get(id).ok().and_then(|o| o.bounds())).reduce(|(alo, ahi), (blo, bhi)| {
        ([alo[0].min(blo[0]), alo[1].min(blo[1]), alo[2].min(blo[2])], [ahi[0].max(bhi[0]), ahi[1].max(bhi[1]), ahi[2].max(bhi[2])])
    });
    let g = all.map_or([0.0; 3], |(lo, hi)| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0]);
    let mut out = Vec::new();
    let label = match change {
        Change::Move(_) => "move",
        Change::Turn(..) => "rotate",
        Change::Scale(_) | Change::Stretch(..) => "scale",
    };
    for &id in ids {
        match change {
            Change::Move(by) => out.push(("move", json!({ "id": id, "by": by }))),
            Change::Turn(k, degrees) => {
                let axis = if *axes == PLATE_AXES { json!(["x", "y", "z"][k.min(2)]) } else { json!(axes[k.min(2)]) };
                out.push(("rotate", json!({ "id": id, "axis": axis, "degrees": degrees })));
            }
            Change::Scale(f) => out.push(("scale", json!({ "id": id, "factor": f }))),
            Change::Stretch(k, f) => {
                let mut by = [1.0; 3];
                by[k.min(2)] = f;
                out.push(("scale", json!({ "id": id, "by": by })));
            }
        }
        // About the middle of them all, each object's own middle goes where the change takes it about that middle.
        if pivot == Pivot::Middle
            && !matches!(change, Change::Move(_))
            && let Some(c) = middle(id)
        {
            let to = change.about_in(g, axes).apply(c);
            let by = [to[0] - c[0], to[1] - c[1], 0.0];
            if by.iter().any(|v| v.abs() > 1e-9) {
                out.push(("move", json!({ "id": id, "by": by })));
            }
        }
    }
    (label, out)
}

/// What a transform key started: G moves, R turns, S scales.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyed {
    Move,
    Turn,
    Scale,
}

/// A transform started from the keyboard, as Blender's G, R and S are: it follows the pointer from where it was when the key went
/// down, keeps to `axis` once X, Y or Z is pressed (again to free it), takes a typed amount over the pointer (mm, degrees, a
/// factor), and ends with Enter or a click, which applies it as one step, or Escape or a right click, which puts it back. Ctrl
/// snaps as the gizmo does.
#[derive(Clone, Debug, PartialEq)]
pub struct Modal {
    pub kind: Keyed,
    pub axis: Option<usize>,
    pub typed: String,
    pub change: Change,
    ids: Vec<u32>,
    start: Pos2,
    at: P3,
}

impl Modal {
    /// The change the pointer at `now` asks for, or the typed amount when one is typed.
    fn change(&self, camera: &Camera, rect: Rect, now: Pos2, snap: bool, axes: &Axes) -> Change {
        let typed = crate::lang::parse_number(crate::lang::get(), &self.typed);
        match (self.kind, self.axis, typed) {
            (Keyed::Move, axis, Some(v)) => Change::Move(axes[axis.unwrap_or(0).min(2)].map(|c| c * v)),
            (Keyed::Turn, axis, Some(v)) => Change::Turn(axis.unwrap_or(2), v),
            (Keyed::Scale, None, Some(v)) => Change::Scale(v.clamp(0.001, 1000.0)),
            (Keyed::Scale, Some(k), Some(v)) => Change::Stretch(k, v.clamp(0.001, 1000.0)),
            (kind, axis, None) => {
                let handle = match (kind, axis) {
                    (Keyed::Move, None) => Handle::Plate,
                    (Keyed::Move, Some(k)) => [Handle::X, Handle::Y, Handle::Z][k.min(2)],
                    (Keyed::Turn, k) => Handle::Ring(k.unwrap_or(2)),
                    (Keyed::Scale, _) => Handle::Scale,
                };
                match (drag_change_in(camera, rect, self.at, handle, self.start, now, !snap, axes), axis) {
                    (Change::Scale(f), Some(k)) => Change::Stretch(k, f),
                    (c, _) => c,
                }
            }
        }
    }

    /// What it does, for the label beside the pointer: the amount and the axis.
    fn describe(&self) -> String {
        let axis = |k: usize| ["x", "y", "z"][k.min(2)];
        let n = |v: f64| crate::lang::mark(format!("{v:.2}"));
        let typed = if self.typed.is_empty() { String::new() } else { format!("  [{}]", self.typed) };
        let what = match self.change {
            Change::Move(by) => match self.axis {
                Some(k) => format!("{} {} mm", axis(k), n(by[k])),
                None => {
                    let sep = if crate::lang::get().decimal_comma() { "; " } else { ", " };
                    format!("x {} mm{sep}y {} mm", n(by[0]), n(by[1]))
                }
            },
            Change::Turn(k, d) => crate::lang::tf("{} deg about {}", &[&n(d), axis(k)]),
            Change::Scale(f) => format!("x {}", n(f)),
            Change::Stretch(k, f) => format!("{} x {}", axis(k), n(f)),
        };
        format!("{what}{typed}")
    }
}

/// Turns snap to this many degrees, and scales to this step.
const TURN_STEP: f64 = 5.0;
const SCALE_STEP: f64 = 0.05;

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
    /// Each ring's points on screen, a circle about the middle square to its axis; and the scale square's corners.
    pub rings: [[Pos2; RING]; 3],
    pub scale: [Pos2; 4],
    /// Whether each axis points toward the eye, which says which way a turn on screen goes.
    pub facing: [bool; 3],
}

/// The points of a ring.
pub const RING: usize = 48;

impl Gizmo {
    /// The gizmo at `at`, seen through `camera` in `rect`; none when it is behind the eye.
    pub fn at(camera: &Camera, rect: Rect, at: P3) -> Option<Gizmo> {
        Gizmo::at_in(camera, rect, at, &PLATE_AXES)
    }

    /// The same, its arrows and rings along `axes` (the plate's, or an object's own).
    pub fn at_in(camera: &Camera, rect: Rect, at: P3, axes: &Axes) -> Option<Gizmo> {
        let (w, h) = (f64::from(rect.width()), f64::from(rect.height()));
        let to = |p: [f64; 3]| pos2(rect.min.x + p[0] as f32, rect.min.y + p[1] as f32);
        let o = camera.project(at, w, h)?;
        let f = (h / 2.0) / (FOV.to_radians() / 2.0).tan();
        // An arrow of `length` mm at that depth is ARROW points long on screen.
        let length = ARROW / (f * o[2]);
        let tip = |k: usize| camera.project(add(at, axes[k].map(|v| v * length)), w, h).map(to);
        let s = length * 0.35;
        let corner = |dx: f64, dy: f64| camera.project([at[0] + dx, at[1] + dy, at[2]], w, h).map(to);
        let ring = |axis: usize| -> Option<[Pos2; RING]> {
            let (u, v) = (axes[(axis + 1) % 3], axes[(axis + 2) % 3]);
            let mut out = [Pos2::ZERO; RING];
            for (i, slot) in out.iter_mut().enumerate() {
                let a = std::f64::consts::TAU * i as f64 / RING as f64;
                let (cu, cv) = (length * 0.8 * a.cos(), length * 0.8 * a.sin());
                let p = [at[0] + u[0] * cu + v[0] * cv, at[1] + u[1] * cu + v[1] * cv, at[2] + u[2] * cu + v[2] * cv];
                *slot = camera.project(p, w, h).map(to)?;
            }
            Some(out)
        };
        let origin = to(o);
        // The scale square sits below and to the right of the middle on screen, the same size however far the camera is.
        let k = (ARROW * 0.62) as f32;
        let q = 7.0;
        let scale = [origin + vec2(k - q, k - q), origin + vec2(k + q, k - q), origin + vec2(k + q, k + q), origin + vec2(k - q, k + q)];
        let forward = camera.frame().3;
        Some(Gizmo {
            origin,
            tips: [tip(0)?, tip(1)?, tip(2)?],
            plate: [corner(0.0, 0.0)?, corner(s, 0.0)?, corner(s, s)?, corner(0.0, s)?],
            length,
            rings: [ring(0)?, ring(1)?, ring(2)?],
            scale,
            facing: axes.map(|a| forward[0] * a[0] + forward[1] * a[1] + forward[2] * a[2] < 0.0),
        })
    }

    /// The handle under `p`: the scale square, the plate square, then the nearest arrow or ring within reach.
    pub fn handle_at(&self, p: Pos2) -> Option<Handle> {
        if inside(&self.scale, p) {
            return Some(Handle::Scale);
        }
        if inside(&self.plate, p) {
            return Some(Handle::Plate);
        }
        let arrows =
            [Handle::X, Handle::Y, Handle::Z].into_iter().zip(self.tips).map(|(h, tip)| (h, distance_to_segment(p, self.origin, tip)));
        let rings = (0..3).map(|k| {
            let r = &self.rings[k];
            (Handle::Ring(k), (0..RING).map(|i| distance_to_segment(p, r[i], r[(i + 1) % RING])).fold(f32::MAX, f32::min))
        });
        arrows.chain(rings).filter(|&(_, d)| d <= NEAR_ARROW).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(h, _)| h)
    }

    /// Where on screen a test grabs `handle`: an arrow's tip, the middle of a square, or the ring's point farthest from the middle
    /// on screen (where no arrow is near).
    pub fn grip(&self, handle: Handle) -> Pos2 {
        let middle = |q: &[Pos2; 4]| (q.iter().fold(Vec2::ZERO, |s, p| s + p.to_vec2()) / 4.0).to_pos2();
        match handle {
            Handle::X => self.tips[0],
            Handle::Y => self.tips[1],
            Handle::Z => self.tips[2],
            Handle::Plate => middle(&self.plate),
            Handle::Scale => middle(&self.scale),
            Handle::Ring(k) => {
                let r = &self.rings[k.min(2)];
                let square = middle(&self.scale);
                let clear = |p: &Pos2| {
                    self.tips.iter().all(|t| distance_to_segment(*p, self.origin, *t) > 2.0 * NEAR_ARROW)
                        && (*p - square).length() > 3.0 * NEAR_ARROW
                        && !inside(&self.plate, *p)
                };
                r.iter()
                    .copied()
                    .filter(clear)
                    .max_by(|a, b| (*a - self.origin).length().total_cmp(&(*b - self.origin).length()))
                    .unwrap_or(r[0])
            }
        }
    }
}

/// What a drag on `handle` from `start` to `now` does: an arrow or the plate square moves ([`drag_offset`]); a ring turns by the angle
/// the pointer swept round the middle on screen, the way the axis is seen; the scale square scales by how much farther from the middle
/// the pointer is. Snapped (to [`STEP`] mm, [`TURN_STEP`] degrees or [`SCALE_STEP`]) unless `free`.
pub fn drag_change(camera: &Camera, rect: Rect, at: P3, handle: Handle, start: Pos2, now: Pos2, free: bool) -> Change {
    drag_change_in(camera, rect, at, handle, start, now, free, &PLATE_AXES)
}

/// The same with the gizmo along `axes`.
#[allow(clippy::too_many_arguments)]
pub fn drag_change_in(camera: &Camera, rect: Rect, at: P3, handle: Handle, start: Pos2, now: Pos2, free: bool, axes: &Axes) -> Change {
    let Some(g) = Gizmo::at_in(camera, rect, at, axes) else { return Change::Move([0.0; 3]) };
    let angle = |p: Pos2| f64::from(-(p.y - g.origin.y)).atan2(f64::from(p.x - g.origin.x));
    match handle {
        Handle::Ring(axis) => {
            let mut swept = (angle(now) - angle(start)).to_degrees();
            swept = (swept + 540.0).rem_euclid(360.0) - 180.0;
            let degrees = if g.facing[axis.min(2)] { swept } else { -swept };
            Change::Turn(axis, if free { degrees } else { (degrees / TURN_STEP).round() * TURN_STEP })
        }
        Handle::Scale => {
            let (a, b) = ((start - g.origin).length(), (now - g.origin).length());
            let f = if a > 1.0 { f64::from(b / a).clamp(0.05, 20.0) } else { 1.0 };
            Change::Scale(if free { f } else { ((f / SCALE_STEP).round() * SCALE_STEP).max(SCALE_STEP) })
        }
        _ => Change::Move(drag_offset_in(camera, rect, at, handle, start, now, free, axes)),
    }
}

/// How far a drag on `handle` from `start` to `now` (screen points in `rect`) moves the object standing at `at`: along the arrow's
/// line on screen for an axis, across the level plane through `at` for the plate square. Snapped to [`STEP`] unless `free`.
pub fn drag_offset(camera: &Camera, rect: Rect, at: P3, handle: Handle, start: Pos2, now: Pos2, free: bool) -> P3 {
    drag_offset_in(camera, rect, at, handle, start, now, free, &PLATE_AXES)
}

/// The same with the gizmo's arrows along `axes`: the move is along the arrow dragged, in the plate's frame.
#[allow(clippy::too_many_arguments)]
pub fn drag_offset_in(camera: &Camera, rect: Rect, at: P3, handle: Handle, start: Pos2, now: Pos2, free: bool, axes: &Axes) -> P3 {
    let snap = |v: f64| if free { v } else { (v / STEP).round() * STEP };
    let Some(g) = Gizmo::at_in(camera, rect, at, axes) else { return [0.0; 3] };
    let axis = match handle {
        Handle::X => 0,
        Handle::Y => 1,
        Handle::Z => 2,
        Handle::Ring(_) | Handle::Scale => return [0.0; 3],
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
    let amount = snap(f64::from((now - start).dot(along) / along.length_sq()) * g.length);
    axes[axis].map(|v| v * amount)
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

/// A drag on the gizmo under way: whose (every object picked), which handle, where it began on screen and in the model, and how far
/// it has gone.
#[derive(Clone, Debug)]
struct Moving {
    ids: Vec<u32>,
    handle: Handle,
    start: Pos2,
    at: P3,
    change: Change,
}

/// What a frame of the view did: a click's pick, a gizmo drag that ended (the object and how far it went), and with the other
/// tools, the face or the point a click landed on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub pick: Option<Pick>,
    pub moved: Option<(Vec<u32>, Change)>,
    pub face: Option<(u32, usize)>,
    pub point: Option<P3>,
    /// What a right click landed on, for the menu it opens.
    pub context: Option<Pick>,
    /// A stroke of the paint brush that ended: the object and the points on it, each painted round.
    pub painted: Option<(u32, Vec<P3>)>,
}

/// Moves `camera` with the pointer over `response`: a primary drag orbits (when `orbit`), a secondary or middle drag pans, the wheel
/// zooms. The 3D view and the 3D G-code preview steer alike.
pub fn steer(camera: &mut Camera, response: &egui::Response, ui: &Ui, orbit: bool) {
    let d = response.drag_delta();
    if orbit && response.dragged_by(PointerButton::Primary) {
        camera.azimuth -= f64::from(d.x) * 0.4;
        camera.elevation = (camera.elevation + f64::from(d.y) * 0.4).clamp(-89.0, 89.0);
    }
    if response.dragged_by(PointerButton::Secondary) || response.dragged_by(PointerButton::Middle) {
        let (_, right, up, _) = camera.frame();
        let k = camera.distance * 0.0015;
        camera.target = add(camera.target, add(scale(right, -f64::from(d.x) * k), scale(up, f64::from(d.y) * k)));
    }
    if response.hovered() {
        let wheel = f64::from(ui.input(|i| i.smooth_scroll_delta.y));
        if wheel != 0.0 {
            camera.distance = (camera.distance * (1.0 - wheel * 0.002).clamp(0.5, 1.5)).clamp(20.0, 5000.0);
        }
    }
}

#[derive(Default)]
pub struct View {
    pub camera: Camera,
    texture: Option<TextureHandle>,
    /// The picture shown: the processor's (`texture`, and `picture` to pick in) or the card's.
    shown: Option<egui::TextureId>,
    pub(crate) picture: Option<Picture>,
    drawn: Option<Drawn>,
    moving: Option<Moving>,
    /// A transform from the keyboard (G, R, S), while it lasts.
    pub modal: Option<Modal>,
    /// What turns and scales go about, and along which axes the gizmo and the keys go; `axes` is worked out each frame.
    pub pivot: Pivot,
    pub orientation: Orientation,
    pub axes: Axes,
    /// The gizmo as last drawn, for a test to find its handles.
    pub gizmo: Option<Gizmo>,
    pub tool: Tool,
    pub brush: Brush,
    /// A stroke of the brush under way: the object it started on, and the points on it so far.
    stroke: Option<(u32, Vec<P3>)>,
    /// Points measured, drawn joined over the picture.
    pub marks: Vec<P3>,
    /// The corners of a cut's plane, drawn translucent over the picture while the window shows the Cut tools; set each frame.
    pub plane: Option<[P3; 4]>,
    /// Where the view was last drawn in the window, and its response, for the right-click menu.
    pub rect: Option<Rect>,
    pub response: Option<egui::Response>,
    /// The graphics card, when the window has one and may draw on it.
    pub card: crate::gpu::Card,
}

impl View {
    /// Draws the view in the space left in `ui`, moves the camera or the gizmo with the pointer, and says what a click picked and how
    /// far a gizmo drag moved its object.
    /// Starts a transform from the keyboard of the objects `ids`, whose middle is `at`, from the pointer at `start`.
    pub fn start_keyed(&mut self, kind: Keyed, ids: Vec<u32>, at: P3, start: Pos2) {
        let change = match kind {
            Keyed::Move => Change::Move([0.0; 3]),
            Keyed::Turn => Change::Turn(2, 0.0),
            Keyed::Scale => Change::Scale(1.0),
        };
        self.modal = Some(Modal { kind, axis: None, typed: String::new(), change, ids, start, at });
    }

    /// The keys and the pointer while a keyed transform lasts: what it becomes, and whether it ended (applied or put back).
    fn keyed(&mut self, ui: &Ui, rect: Rect, response: &egui::Response, shown: &mut Shown) {
        let Some(m) = self.modal.as_mut() else { return };
        let (keys, text, back, enter, escape, ctrl, pointer) = ui.input(|i| {
            let keys = [Key::X, Key::Y, Key::Z].map(|k| i.key_pressed(k));
            let text: String = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect();
            (
                keys,
                text,
                i.key_pressed(Key::Backspace),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
                i.modifiers.command,
                i.pointer.hover_pos(),
            )
        });
        for (k, pressed) in keys.into_iter().enumerate() {
            if pressed {
                m.axis = if m.axis == Some(k) { None } else { Some(k) };
            }
        }
        m.typed.extend(text.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-')));
        if back {
            m.typed.pop();
        }
        if let Some(now) = pointer.or(Some(m.start)) {
            m.change = m.change(&self.camera, rect, now, ctrl, &self.axes);
        }
        if escape || response.secondary_clicked() {
            self.modal = None;
        } else if enter || response.clicked() {
            shown.moved = Some((m.ids.clone(), m.change)).filter(|(_, c)| !c.is_nothing());
            self.modal = None;
        }
    }

    pub fn show(&mut self, ui: &mut Ui, scene: &Arc<Scene>, selected: &[u32], bed: [f64; 2]) -> Shown {
        let rect = ui.available_rect_before_wrap();
        self.rect = Some(rect);
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let mut shown = Shown::default();
        // The axes: the plate's, or the first picked object's own (its turn, without its scale).
        self.axes = match (self.orientation, selected.first().and_then(|&id| scene.get(id).ok())) {
            (Orientation::Own, Some(o)) => {
                o.xf.polar().map_or(PLATE_AXES, |(turn, _)| std::array::from_fn(|k| [turn.m[0][k], turn.m[1][k], turn.m[2][k]]))
            }
            _ => PLATE_AXES,
        };
        // The gizmo stands at the middle of what is picked (one object or several), while the move tool is in hand.
        let bounds = selected.iter().filter_map(|&id| scene.get(id).ok().and_then(|o| o.bounds())).reduce(|(alo, ahi), (blo, bhi)| {
            ([alo[0].min(blo[0]), alo[1].min(blo[1]), alo[2].min(blo[2])], [ahi[0].max(bhi[0]), ahi[1].max(bhi[1]), ahi[2].max(bhi[2])])
        });
        let one = match bounds {
            Some((lo, hi)) if self.tool == Tool::Move => Some([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0]),
            _ => None,
        };
        // A transform from the keyboard takes the pointer, the keys and the clicks while it lasts.
        let keyed = self.modal.is_some();
        self.keyed(ui, rect, &response, &mut shown);
        let press = ui.input(|i| i.pointer.press_origin());
        if !keyed
            && response.drag_started_by(PointerButton::Primary)
            && let (Some(at), Some(start)) = (one, press)
            && let Some(handle) = Gizmo::at_in(&self.camera, rect, at, &self.axes).and_then(|g| g.handle_at(start))
        {
            self.moving = Some(Moving { ids: selected.to_vec(), handle, start, at, change: Change::Move([0.0; 3]) });
        }
        // The paint brush: a drag that starts on an object paints along it, as one stroke, and one that starts off it turns the view.
        if !keyed && self.tool == Tool::Paint {
            let (_, ppp) = picture_size(ui, rect);
            let pixel = |p: Pos2| (((p.x - rect.min.x) * ppp).max(0.0) as usize, ((p.y - rect.min.y) * ppp).max(0.0) as usize);
            if response.drag_started_by(PointerButton::Primary)
                && let Some(start) = press
                && let (x, y) = pixel(start)
                && let (Some(id), _, Some(point)) = self.seen(x, y)
            {
                self.stroke = Some((id, vec![point]));
            }
            let under = response.interact_pointer_pos().map(pixel).map(|(x, y)| self.seen(x, y));
            let gap = self.brush.radius / 2.0;
            if let Some((id, points)) = &mut self.stroke {
                if let Some((Some(seen), _, Some(point))) = under
                    && seen == *id
                    && points.len() < MAX_STROKE
                    && points.last().is_none_or(|&last| dot(sub(point, last), sub(point, last)).sqrt() > gap)
                {
                    points.push(point);
                }
                if response.drag_stopped() || !response.dragged() {
                    shown.painted = self.stroke.take();
                }
            }
        } else {
            self.stroke = None;
        }
        if let Some(m) = &mut self.moving {
            if let Some(now) = response.interact_pointer_pos() {
                let free = ui.input(|i| i.modifiers.shift);
                m.change = drag_change_in(&self.camera, rect, m.at, m.handle, m.start, now, free, &self.axes);
            }
            if response.drag_stopped() || !response.dragged() {
                shown.moved = Some((m.ids.clone(), m.change)).filter(|(_, c)| !c.is_nothing());
                self.moving = None;
            }
        } else if !keyed {
            steer(&mut self.camera, &response, ui, self.stroke.is_none());
        }
        if self.moving.is_some() {
            steer(&mut self.camera, &response, ui, false);
        }
        // While a drag is under way the object is drawn where it would go; the scene itself changes only when the window runs the move.
        let moving =
            self.moving.as_ref().map(|m| (m.ids.clone(), m.change)).or_else(|| self.modal.as_ref().map(|m| (m.ids.clone(), m.change)));
        // What each object turns and scales about: its own middle, or the middle of everything picked.
        let middle_of = |lo: P3, hi: P3| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
        let pivot_of = |lo: P3, hi: P3| match (self.pivot, bounds) {
            (Pivot::Middle, Some((glo, ghi))) => middle_of(glo, ghi),
            _ => middle_of(lo, hi),
        };
        let (size, ppp) = picture_size(ui, rect);
        let now = Drawn {
            scene: Arc::as_ptr(scene) as usize,
            camera: self.camera,
            selected: selected.to_vec(),
            bed,
            size,
            moving: moving.clone(),
            cpu: self.card.cpu,
        };
        if self.drawn.as_ref() != Some(&now) {
            // A drag's preview on the card: each object moved about its pivot, as the processor's copy of the scene has it.
            let moved: Vec<(u32, lamina_core::Transform)> = moving
                .iter()
                .flat_map(|(ids, change)| {
                    ids.iter().filter_map(|&id| {
                        let (lo, hi) = scene.get(id).ok()?.bounds()?;
                        let c = pivot_of(lo, hi);
                        Some((id, change.about_in(c, &self.axes)))
                    })
                })
                .collect();
            if let Some(id) = self.card.draw(scene, selected, &moved, &self.camera, bed, size) {
                (self.shown, self.picture, self.drawn) = (Some(id), None, Some(now.clone()));
            }
        }
        if self.drawn.as_ref() != Some(&now) {
            let picture = match &moving {
                Some((ids, change)) => {
                    let mut seen = Scene::clone(scene);
                    for &id in ids {
                        if let Ok(o) = seen.get_mut(id)
                            && let Some((lo, hi)) = o.bounds()
                        {
                            o.xf = o.xf.then(&change.about_in(pivot_of(lo, hi), &self.axes));
                        }
                    }
                    draw(&seen, selected, &self.camera, bed, size)
                }
                None => draw(scene, selected, &self.camera, bed, size),
            };
            match &mut self.texture {
                Some(t) => t.set(image(&picture), TextureOptions::LINEAR),
                None => self.texture = Some(ui.ctx().load_texture("lamina-3d", image(&picture), TextureOptions::LINEAR)),
            }
            self.shown = self.texture.as_ref().map(TextureHandle::id);
            (self.picture, self.drawn) = (Some(picture), Some(now));
        }
        let painter = ui.painter_at(rect);
        if let Some(t) = self.shown {
            painter.image(t, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        let shift = match moving.as_ref().map(|(_, c)| *c) {
            Some(Change::Move(by)) => by,
            _ => [0.0; 3],
        };
        self.gizmo = one.and_then(|at| Gizmo::at_in(&self.camera, rect, add(at, shift), &self.axes));
        if let Some(g) = self.gizmo {
            let active = self.moving.as_ref().map(|m| m.handle).or_else(|| response.hover_pos().and_then(|p| g.handle_at(p)));
            paint_gizmo(&painter, &g, active);
            if let (Some(m), Some(p)) = (self.moving.as_ref(), response.interact_pointer_pos()) {
                let text = match m.change {
                    Change::Stretch(axis, f) => format!("{} x {}", ["x", "y", "z"][axis.min(2)], crate::lang::mark(format!("{f:.2}"))),
                    Change::Move(by) => {
                        // With a decimal comma, the three are kept apart by semicolons, as Portuguese and Spanish write a list of them.
                        let sep = if crate::lang::get().decimal_comma() { "; " } else { ", " };
                        let n = by.map(|v| crate::lang::mark(format!("{v:+.1}")));
                        format!("{}{sep}{}{sep}{} mm", n[0], n[1], n[2])
                    }
                    Change::Turn(axis, d) => crate::lang::tf("{} deg about {}", &[&format!("{d:+.0}"), ["x", "y", "z"][axis.min(2)]]),
                    Change::Scale(f) => format!("x {}", crate::lang::mark(format!("{f:.2}"))),
                };
                painter.text(p + vec2(16.0, -16.0), egui::Align2::LEFT_BOTTOM, text, FontId::proportional(13.0), Color32::WHITE);
            }
        }
        // Painted over the picture, so it shows the same whichever draws it, the card or the processor.
        let corners = self.plane.map(|q| q.map(|p| self.camera.project(p, f64::from(rect.width()), f64::from(rect.height()))));
        if let Some([Some(a), Some(b), Some(c), Some(d)]) = corners {
            let quad = [a, b, c, d].map(|q| pos2(rect.min.x + q[0] as f32, rect.min.y + q[1] as f32)).to_vec();
            let edge = Color32::from_rgb(120, 200, 255);
            painter.add(Shape::convex_polygon(quad, Color32::from_rgba_unmultiplied(120, 200, 255, 60), Stroke::new(1.5, edge)));
        }
        let marks: Vec<Pos2> = self
            .marks
            .iter()
            .filter_map(|&m| self.camera.project(m, f64::from(rect.width()), f64::from(rect.height())))
            .map(|q| pos2(rect.min.x + q[0] as f32, rect.min.y + q[1] as f32))
            .collect();
        for m in &marks {
            painter.circle_filled(*m, 4.0, Color32::from_rgb(255, 230, 90));
        }
        if let [a, b] = marks[..] {
            painter.line_segment([a, b], Stroke::new(2.0, Color32::from_rgb(255, 230, 90)));
        }
        // The brush's stroke so far, until it ends and the paint shows on the faces.
        if let Some((_, points)) = &self.stroke {
            let c = match self.brush.kind {
                Paint::Seam => lamina_core::render::PAINT[0],
                Paint::Support => lamina_core::render::PAINT[1],
                Paint::Block => lamina_core::render::PAINT[2],
                Paint::None => [235, 235, 235, 255],
            };
            for q in points.iter().filter_map(|&m| self.camera.project(m, f64::from(rect.width()), f64::from(rect.height()))) {
                painter.circle_filled(pos2(rect.min.x + q[0] as f32, rect.min.y + q[1] as f32), 3.0, Color32::from_rgb(c[0], c[1], c[2]));
            }
        }
        if let Some(m) = &self.modal
            && let Some(p) = ui.input(|i| i.pointer.hover_pos())
        {
            let font = FontId::proportional(13.0);
            painter.text(p + vec2(16.0, -16.0), egui::Align2::LEFT_BOTTOM, m.describe(), font, Color32::WHITE);
        }
        if !keyed
            && response.clicked()
            && let Some(p) = response.interact_pointer_pos()
            && self.gizmo.and_then(|g| g.handle_at(p)).is_none()
        {
            let (x, y) = (((p.x - rect.min.x) * ppp).max(0.0) as usize, ((p.y - rect.min.y) * ppp).max(0.0) as usize);
            let (id, face, point) = self.seen(x, y);
            match self.tool {
                Tool::Move => shown.pick = Some(id.map_or(Pick::Nothing, Pick::Object)),
                Tool::LayFlat => shown.face = face,
                Tool::Measure => shown.point = point,
                Tool::Paint => shown.painted = id.zip(point).map(|(id, p)| (id, vec![p])),
            }
        }
        if !keyed
            && response.secondary_clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let (x, y) = (((p.x - rect.min.x) * ppp).max(0.0) as usize, ((p.y - rect.min.y) * ppp).max(0.0) as usize);
            shown.context = Some(self.seen(x, y).0.map_or(Pick::Nothing, Pick::Object));
        }
        self.response = Some(response);
        shown
    }

    /// Whether the picture shown is the card's.
    #[cfg(test)]
    pub(crate) fn on_card(&self) -> bool {
        self.picture.is_none() && self.shown.is_some()
    }

    /// What the picture shows at pixel (`x`, `y`): the object (none on the plate or nothing), its triangle, and the point seen there.
    /// Read from the processor's picture, or back from the card's.
    fn seen(&self, x: usize, y: usize) -> (Option<u32>, Option<(u32, usize)>, Option<P3>) {
        if let Some(pic) = &self.picture {
            return (pic.id_at(x, y), pic.face_at(x, y), pic.point_at(&self.camera, x, y));
        }
        let Some(hit) = self.card.at(x, y) else { return (None, None, None) };
        let id = (hit.id != 0).then_some(hit.id);
        (id, id.map(|id| (id, hit.tri as usize)), self.camera.at_depth(x, y, hit.size, hit.inv))
    }
}

/// The picture's size in pixels for `rect`, and the pixels it has per point: the screen's, unless that would make it wider or
/// higher than the graphics card takes a texture (a tall window on a large screen), when it is drawn smaller and stretched.
pub(crate) fn picture_size(ui: &Ui, rect: Rect) -> ([usize; 2], f32) {
    let most = ui.ctx().input(|i| i.max_texture_side).max(1) as f32;
    let ppp = ui.ctx().pixels_per_point();
    let ppp = ppp.min(most / rect.width().max(1.0)).min(most / rect.height().max(1.0));
    let side = |points: f32| ((points * ppp).round() as usize).clamp(1, most as usize);
    ([side(rect.width()), side(rect.height())], ppp)
}

fn paint_gizmo(painter: &egui::Painter, g: &Gizmo, active: Option<Handle>) {
    let lit = |h: Handle, c: Color32| if active == Some(h) { Color32::from_rgb(255, 230, 90) } else { c };
    let colours = [Color32::from_rgb(230, 85, 80), Color32::from_rgb(95, 200, 95), Color32::from_rgb(90, 145, 245)];
    for (k, c) in colours.iter().enumerate() {
        let c = lit(Handle::Ring(k), c.gamma_multiply(0.75));
        let mut ring = g.rings[k].to_vec();
        ring.push(g.rings[k][0]);
        painter.add(Shape::line(ring, Stroke::new(if active == Some(Handle::Ring(k)) { 3.0 } else { 1.5 }, c)));
    }
    let scale = lit(Handle::Scale, Color32::from_rgb(240, 240, 240));
    painter.add(Shape::convex_polygon(g.scale.to_vec(), scale.gamma_multiply(0.5), Stroke::new(1.5, scale)));
    // White, so it shows on a picked object, which is drawn in the highlight orange.
    let plate = if active == Some(Handle::Plate) {
        Color32::from_rgba_unmultiplied(255, 230, 90, 150)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 70)
    };
    painter.add(Shape::convex_polygon(g.plate.to_vec(), plate, Stroke::new(1.5, Color32::from_rgb(240, 240, 240))));
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
        let mid = pic.pixels[75 * 200 + 100];
        assert!(mid[2] > mid[0], "drawn in the model colour: {mid:?}");
        let chosen = draw(&s, &[1], &cam, [220.0, 220.0], [200, 150]).pixels[75 * 200 + 100];
        assert!(chosen[0] > chosen[2], "a selected object is drawn in the highlight: {chosen:?}");
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
    fn a_click_gives_the_face_and_the_point_under_it() {
        let s = scene_with_box();
        // Looking straight down at the 40 mm box: the middle pixel is its top face, 40 mm up, at the middle of the plate.
        let cam = Camera { target: [0.0, 0.0, 20.0], azimuth: 0.0, elevation: 89.0, distance: 300.0 };
        let pic = draw(&s, &[], &cam, [220.0, 220.0], [201, 201]);
        let (id, face) = pic.face_at(100, 100).unwrap();
        let t = s.get(id).unwrap().world().tri(face).unwrap();
        assert!(t.iter().all(|p| (p[2] - 40.0).abs() < 1e-6), "the top face: {t:?}");
        let p = pic.point_at(&cam, 100, 100).unwrap();
        assert!((p[2] - 40.0).abs() < 0.05 && p[0].abs() < 1.0 && p[1].abs() < 1.0, "{p:?}");
        assert_eq!(pic.point_at(&cam, 0, 0).map(|p| p[2].abs() < 0.05), Some(true), "a corner sees the plate");
    }

    #[test]
    fn a_ring_turns_by_the_angle_swept_and_the_corner_square_scales() {
        let cam = Camera { target: [0.0, 0.0, 10.0], azimuth: -60.0, elevation: 35.0, distance: 380.0 };
        let at = [0.0, 0.0, 10.0];
        let g = Gizmo::at(&cam, rect(), at).unwrap();
        for k in 0..3 {
            assert_eq!(g.handle_at(g.grip(Handle::Ring(k))), Some(Handle::Ring(k)), "ring {k}");
        }
        assert_eq!(g.handle_at(g.grip(Handle::Scale)), Some(Handle::Scale));
        // A quarter turn of the pointer round the middle is a quarter turn about z, seen from above (z faces the eye).
        let r = 100.0;
        let (start, quarter) = (g.origin + vec2(r, 0.0), g.origin + vec2(0.0, -r));
        assert_eq!(drag_change(&cam, rect(), at, Handle::Ring(2), start, quarter, false), Change::Turn(2, 90.0));
        let nudged = g.origin + vec2(r, -r * 0.035);
        assert_eq!(drag_change(&cam, rect(), at, Handle::Ring(2), start, nudged, false), Change::Turn(2, 0.0), "snapped to 5 degrees");
        // Twice as far from the middle is twice the size.
        let s = g.grip(Handle::Scale);
        let twice = g.origin + (s - g.origin) * 2.0;
        assert_eq!(drag_change(&cam, rect(), at, Handle::Scale, s, twice, false), Change::Scale(2.0));
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
