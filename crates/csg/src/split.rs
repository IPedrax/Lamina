//! Cutting an object into pieces along any axis, with connectors so the printed pieces go back together, and splitting an object too
//! big for the printer into pieces that fit: the `cut` and `split_to_fit` commands.
//!
//! A cut is two booleans with a box bigger than the object on one side of the plane. The connectors go on the cut face, which is the
//! object's section at the plane as the slicer finds it (`lamina_slicer::slice`), intersected with its sections down to the bottom of
//! each hole and a wall beyond (so no hole breaks through a side or the far end), then shrunk by the hole's radius and a wall
//! (`geom::inset`). The centres are spread over what is left by farthest-point sampling on a grid and its corners: two or more, so
//! the pieces cannot turn on them, or one square key where only one fits. Dowels are holes in both pieces and loose pins to print;
//! pins are pegs joined to the piece below the plane and sockets cut in the piece above.

use std::f64::consts::{PI, TAU};

use lamina_core::{Mesh, Scene, Session, Transform};
use lamina_slicer::geom::{self, Region};
use lamina_slicer::slice;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Op, Placed, isolated};

/// Material kept round a connector and behind the bottom of its hole, mm.
const WALL: f64 = 2.0;
/// The gap a side between a pin and its hole, mm: a snug fit, as the design-for-fdm skill says.
const CLEARANCE: f64 = 0.2;
/// The largest and smallest connector chosen on its own, mm across.
const MAX_AUTO: f64 = 8.0;
const MIN_AUTO: f64 = 3.0;
/// The deepest hole chosen on its own, and the shallowest hole or peg made at all, mm.
const MAX_DEPTH: f64 = 15.0;
const MIN_DEPTH: f64 = 2.0;
/// How much shorter a dowel is than its two holes together, and a peg than its socket, mm: the pieces close on their faces.
const GAP: f64 = 0.4;
/// The sides of a round connector, as the modelling skill advises for a cutting tool.
const SEGMENTS: u32 = 32;
/// The most pieces `split_to_fit` makes: each cut is four booleans.
const MAX_PIECES: usize = 32;
const AXES: [&str; 3] = ["x", "y", "z"];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Dowel,
    Pin,
}

/// The `connectors` parameter of `cut` and `split_to_fit`.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Connectors {
    kind: Option<String>,
    shape: Option<String>,
    count: Option<u32>,
    diameter: Option<f64>,
    depth: Option<f64>,
    clearance: Option<f64>,
}

/// The connectors asked for, checked. `kind` none is no connectors; `said` is whether the kind was asked for by name, so a face too
/// small is an error rather than a note.
struct Ask {
    kind: Option<Kind>,
    said: bool,
    /// Square connectors asked for; otherwise round, and a lone one square anyway, so the pieces still cannot turn.
    square: bool,
    count: Option<usize>,
    diameter: Option<f64>,
    depth: Option<f64>,
    clearance: f64,
}

fn ask(c: Option<Connectors>) -> Result<Ask, String> {
    let c = c.unwrap_or_default();
    let (kind, said) = match c.kind.as_deref() {
        // Pegs on one piece and sockets in the other by default: nothing loose to print, lose or glue.
        None => (Some(Kind::Pin), false),
        Some("dowel") => (Some(Kind::Dowel), true),
        Some("pin") => (Some(Kind::Pin), true),
        Some("none") => (None, true),
        Some(other) => return Err(format!("`connectors.kind` is dowel, pin or none, not {other}")),
    };
    let square = match c.shape.as_deref() {
        None | Some("round") => false,
        Some("square") => true,
        Some(other) => return Err(format!("`connectors.shape` is round or square, not {other}")),
    };
    let within = |what: &str, x: Option<f64>, lo: f64, hi: f64| match x {
        Some(v) if !(v.is_finite() && (lo..=hi).contains(&v)) => Err(format!("`connectors.{what}` is from {lo} to {hi} mm, not {v}")),
        _ => Ok(x),
    };
    let count = match c.count {
        None => None,
        Some(n) if (1..=8).contains(&n) => Some(n as usize),
        Some(n) => return Err(format!("`connectors.count` is from 1 to 8, not {n}")),
    };
    let clearance = within("clearance", c.clearance, 0.0, 1.0)?.unwrap_or(CLEARANCE);
    let diameter = within("diameter", c.diameter, 2.0, 30.0)?;
    if diameter.is_some_and(|d| d - 2.0 * clearance < 1.0) {
        return Err("`connectors.diameter` must be at least 1 mm more than twice the clearance".into());
    }
    Ok(Ask { kind, said, square, count, diameter, depth: within("depth", c.depth, MIN_DEPTH, 50.0)?, clearance })
}

/// The plane square to axis `k` (0 x, 1 y, 2 z) at `at` mm. Its own frame is (u, v, w): u and v the next two axes in turn (y and z
/// for x, z and x for y, x and y for z) and w along `k` from the plane. That is a turn, so a mesh keeps its winding in it.
#[derive(Clone, Copy, Debug)]
struct Plane {
    k: usize,
    at: f64,
}

impl Plane {
    /// The transform from the plane's frame, moved to (`u`, `v`) on it, to the plate's.
    fn frame(&self, u: f64, v: f64) -> Transform {
        let (k, ku, kv) = (self.k, (self.k + 1) % 3, (self.k + 2) % 3);
        let mut m = [[0.0; 3]; 3];
        (m[ku][0], m[kv][1], m[k][2]) = (1.0, 1.0, 1.0);
        let mut t = [0.0; 3];
        (t[ku], t[kv], t[k]) = (u, v, self.at);
        Transform { m, t }
    }

    /// A mesh on the plate in the plane's frame, lifted `lift` mm along w: the slicer only cuts above 0.
    fn local(&self, m: &Mesh, lift: f64) -> Mesh {
        let (k, ku, kv) = (self.k, (self.k + 1) % 3, (self.k + 2) % 3);
        let vertices = m.vertices.iter().map(|p| [p[ku], p[kv], (f64::from(p[k]) - self.at + lift) as f32]).collect();
        Mesh { vertices, triangles: m.triangles.clone() }
    }
}

/// Where a mesh in a plane's frame, lifted by `lift`, meets the plane `off` mm along w: a region in (u, v).
fn section(local: &Mesh, lift: f64, off: f64) -> Region {
    let first = slice::layers_at(local, &[2.0 * (lift + off)]).ok().and_then(|l| l.into_iter().next());
    first.map_or_else(Vec::new, |l| geom::region(&l.loops))
}

/// The area of a mesh's section by the plane square to `k` at `at`, mm2.
fn area_at(m: &Mesh, k: usize, at: f64) -> f64 {
    let Some((lo, _)) = m.bounds() else { return 0.0 };
    let lift = at - lo[k] + 1.0;
    geom::area(&section(&Plane { k, at }.local(m, lift), lift, 0.0))
}

/// Connectors placed on one cut.
#[derive(Clone, Debug)]
struct Plan {
    kind: Kind,
    /// The centres, in the plane's (u, v).
    centres: Vec<[f64; 2]>,
    /// The hole's diameter (dowels) or the peg's (pins), mm; a square key's across its corners.
    diameter: f64,
    /// How deep into the piece below the plane and into the piece above, mm. For pins, the peg's root and the socket.
    depth: [f64; 2],
    clearance: f64,
    /// One square key, where only one connector fits, so the pieces still cannot turn.
    square: bool,
}

impl Plan {
    fn sides(&self) -> u32 {
        if self.square { 4 } else { SEGMENTS }
    }

    /// A corner radius `r` of the connector's outline with its sides moved out by `by` mm (in, when negative).
    fn widen(&self, r: f64, by: f64) -> f64 {
        r + by / (PI / f64::from(self.sides())).cos()
    }

    /// The tool cut from (dowels) or joined to (pins) the piece below the plane, the tool cut from the piece above, both in the
    /// plane's frame at the origin, and a loose dowel in the same frame where it sits when the pieces are together.
    fn shapes(&self) -> (Mesh, Mesh, Option<Mesh>) {
        let (r, [down, up], n) = (self.diameter / 2.0, self.depth, self.sides());
        match self.kind {
            Kind::Dowel => {
                let pin = self.widen(r, -self.clearance);
                let ch = (pin / 4.0).min(0.4);
                let (z0, z1) = (-down + GAP / 2.0, up - GAP / 2.0);
                let dowel = rings(&[(pin - ch, z0), (pin, z0 + ch), (pin, z1 - ch), (pin - ch, z1)], n);
                (rings(&[(r, -down), (r, 1.0)], n), rings(&[(r, -1.0), (r, up)], n), Some(dowel))
            }
            Kind::Pin => {
                let (length, ch) = (up - GAP, (r / 4.0).min(0.4));
                let socket = self.widen(r, self.clearance);
                (rings(&[(r, -down), (r, length - ch), (r - ch, length)], n), rings(&[(socket, -1.0), (socket, up)], n), None)
            }
        }
    }

    /// What an agent reads of it: sizes, and the centres on the plate where the pieces stood when cut.
    fn describe(&self, plane: Plane) -> Value {
        let at: Vec<[f64; 3]> = self.centres.iter().map(|&[u, v]| plane.frame(u, v).t).collect();
        let shape = if self.square { "square" } else { "round" };
        let (d, c, [down, up]) = (self.diameter, self.clearance, self.depth);
        match self.kind {
            Kind::Dowel => json!({"kind": "dowel", "shape": shape, "count": at.len(), "hole_diameter": d, "hole_depth": [down, up],
                "dowel_diameter": d - 2.0 * c, "dowel_length": down + up - GAP, "clearance": c, "at": at}),
            Kind::Pin => json!({"kind": "pin", "shape": shape, "count": at.len(), "peg_diameter": d, "peg_length": up - GAP,
                "socket_diameter": d + 2.0 * c, "socket_depth": up, "clearance": c, "at": at}),
        }
    }
}

/// A solid of revolution about z with flat ends: `profile` is (radius, z) from the bottom up, each a ring of `sides` corners (4 makes
/// a square key). A plain cylinder is two rings, a pin chamfered at both ends four.
fn rings(profile: &[(f64, f64)], sides: u32) -> Mesh {
    let n = sides.max(3);
    let (Some(&(_, bottom_z)), Some(&(_, top_z))) = (profile.first(), profile.last()) else { return Mesh::default() };
    let mut vertices = Vec::new();
    for &(r, z) in profile {
        for i in 0..n {
            let a = TAU * f64::from(i) / f64::from(n);
            vertices.push([(r * a.cos()) as f32, (r * a.sin()) as f32, z as f32]);
        }
    }
    let count = profile.len() as u32;
    let (bottom, top, last) = (count * n, count * n + 1, (count - 1) * n);
    vertices.push([0.0, 0.0, bottom_z as f32]);
    vertices.push([0.0, 0.0, top_z as f32]);
    let mut triangles = Vec::new();
    for i in 0..n {
        let j = (i + 1) % n;
        triangles.push([bottom, j, i]);
        triangles.push([top, last + i, last + j]);
        for ring in 0..count - 1 {
            let (a, b) = (ring * n, (ring + 1) * n);
            triangles.push([a + i, a + j, b + j]);
            triangles.push([a + i, b + j, b + i]);
        }
    }
    Mesh { vertices, triangles }
}

/// Where connectors go on the cut of `world` (on the plate) by `plane`, or none when not one fits. `avoid` are lines of the plane,
/// (0, c) for u = c and (1, c) for v = c, that no connector may come near: where later cuts pass.
fn place(world: &Mesh, plane: Plane, ask: &Ask, avoid: &[(usize, f64)]) -> Result<Option<Plan>, String> {
    let Some(kind) = ask.kind else { return Ok(None) };
    // The booleans would refuse it anyway; the sections below would take long first.
    if world.triangles.len() > crate::MAX_INPUT_TRIANGLES {
        return Err(format!("{} triangles is more than a cut takes: `simplify` it first", world.triangles.len()));
    }
    let (lo, hi) = world.bounds().ok_or("the object is empty")?;
    // The material below and above the plane, at most.
    let room = [plane.at - lo[plane.k], hi[plane.k] - plane.at];
    let lift = room[0] + 1.0;
    let local = plane.local(world, lift);
    let auto = ((geom::area(&section(&local, lift, 0.0)).sqrt() / 6.0 * 2.0).floor() / 2.0).clamp(MIN_AUTO, MAX_AUTO);
    let sizes: Vec<f64> = match ask.diameter {
        Some(d) => vec![d],
        None => (0..).map(|i| auto - 0.5 * f64::from(i)).take_while(|&d| d >= MIN_AUTO - 1e-9).collect(),
    };
    let want = ask.count.unwrap_or(2);
    let (mut single, mut most) = (None, 0);
    for d in sizes {
        let c = ask.clearance;
        let deep = ask.depth.unwrap_or((1.5 * d).clamp(4.0, MAX_DEPTH));
        let (hole, depth) = match kind {
            Kind::Dowel => (d / 2.0, [deep.min(room[0] - WALL), deep.min(room[1] - WALL)]),
            Kind::Pin => (d / 2.0 + c, [1.0, (deep + GAP).min(room[1] - WALL)]),
        };
        let short = match kind {
            Kind::Dowel => depth.iter().any(|&x| x < MIN_DEPTH),
            Kind::Pin => room[0] < 1.0 + WALL || depth[1] - GAP < MIN_DEPTH,
        };
        if short {
            continue;
        }
        // Every section from a wall beyond the bottom of the hole below to a wall beyond the bottom of the hole above.
        let mut core = section(&local, lift, -0.05);
        for off in offsets((depth[0] + WALL).min(room[0] - 0.05), (depth[1] + WALL).min(room[1] - 0.05)) {
            core = geom::intersect(&core, &section(&local, lift, off));
        }
        let free = geom::inset(&core, hole + WALL);
        let found = spread(&free, want, 2.0 * hole + WALL, avoid, hole + WALL);
        let plan =
            |centres: Vec<[f64; 2]>| Plan { kind, square: ask.square || centres.len() == 1, centres, diameter: d, depth, clearance: c };
        most = most.max(found.len());
        if found.len() >= want {
            return Ok(Some(plan(found)));
        }
        if found.len() == 1 && single.is_none() {
            single = Some(plan(found));
        }
    }
    match (single, ask.count) {
        (Some(p), None) => Ok(Some(p)),
        (_, Some(_)) if most > 0 => Err(format!(
            "only {most} connectors fit the cut at {} = {:.2}, not {want}: ask for fewer (one is a square key) or a smaller `diameter`",
            AXES[plane.k], plane.at
        )),
        _ => Ok(None),
    }
}

/// Offsets along w, mm, from the plane to `below` under it and `above` over it: a millimetre apart and both ends.
fn offsets(below: f64, above: f64) -> Vec<f64> {
    let mut out = vec![0.05];
    let mut x = 1.0;
    while x < below {
        out.push(-x);
        x += 1.0;
    }
    out.push(-below);
    x = 1.0;
    while x < above {
        out.push(x);
        x += 1.0;
    }
    out.push(above);
    out
}

/// Up to `want` points of `free`, each at least `apart` from the others and `clear` from the lines `avoid`, spread as far as they
/// go: farthest-point sampling over a grid and the region's corners, the first point the farthest from their middle.
fn spread(free: &Region, want: usize, apart: f64, avoid: &[(usize, f64)], clear: f64) -> Vec<[f64; 2]> {
    let mut points: Vec<[f64; 2]> = free.iter().flatten().flatten().copied().collect();
    let Some(&first) = points.first() else { return Vec::new() };
    let (lo, hi) =
        points.iter().fold((first, first), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
    const N: u32 = 48;
    let step = ((hi[0] - lo[0]).max(hi[1] - lo[1]) / f64::from(N)).max(0.05);
    for i in 0..=N {
        for j in 0..=N {
            let p = [lo[0] + step * f64::from(i), lo[1] + step * f64::from(j)];
            if p[0] <= hi[0] && p[1] <= hi[1] && geom::contains(free, p) {
                points.push(p);
            }
        }
    }
    points.retain(|p| avoid.iter().all(|&(a, c)| (p[a.min(1)] - c).abs() >= clear));
    if points.is_empty() {
        return Vec::new();
    }
    let n = points.len() as f64;
    let mid = points.iter().fold([0.0, 0.0], |m, p| [m[0] + p[0] / n, m[1] + p[1] / n]);
    let dist = |a: &[f64; 2], b: &[f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let mut chosen: Vec<[f64; 2]> = points.iter().max_by(|a, b| dist(a, &mid).total_cmp(&dist(b, &mid))).into_iter().copied().collect();
    while chosen.len() < want {
        let nearest = |p: &[f64; 2]| chosen.iter().map(|q| dist(p, q)).fold(f64::MAX, f64::min);
        let Some(best) = points.iter().max_by(|a, b| nearest(a).total_cmp(&nearest(b))) else { break };
        if nearest(best) < apart {
            break;
        }
        chosen.push(*best);
    }
    chosen
}

/// One cut, done: the piece below the plane and the piece above, on the plate, and the loose dowels in the plane's frame with
/// where each sits when the pieces are put together.
struct Parted {
    below: Mesh,
    above: Mesh,
    dowels: Vec<(Mesh, Transform)>,
}

/// `placed` parted by `plane`, with the holes, pegs and sockets of `plan`.
fn part(placed: Placed, plane: Plane, plan: Option<&Plan>) -> Result<Parted, String> {
    lamina_core::cancel::check()?;
    let (lo, hi) = placed.0.transformed(&placed.1).bounds().ok_or("the object is empty")?;
    // A box larger than the object on the far side of the plane: what it shares with the object is the piece above, what it cuts
    // away the piece below.
    let margin = 10.0;
    let (mut from, mut to) = (lo.map(|x| x - margin), hi.map(|x| x + margin));
    from[plane.k] = plane.at;
    to[plane.k] = hi[plane.k] + margin;
    let size = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let half = Mesh::cuboid(size).transformed(&Transform::translate([(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0, from[2]]));
    let mut below = isolated(Op::Difference, placed, (&half, Transform::IDENTITY))?;
    let mut above = isolated(Op::Intersect, placed, (&half, Transform::IDENTITY))?;
    if below.triangles.is_empty() || above.triangles.is_empty() {
        return Err("the cut left a piece with nothing in it".into());
    }
    let mut dowels = Vec::new();
    if let Some(plan) = plan {
        let (down, up, dowel) = plan.shapes();
        let at = |m: &Mesh| Mesh::merged(&plan.centres.iter().map(|&[u, v]| m.transformed(&plane.frame(u, v))).collect::<Vec<_>>());
        let op = if plan.kind == Kind::Pin { Op::Union } else { Op::Difference };
        below = isolated(op, (&below, Transform::IDENTITY), (&at(&down), Transform::IDENTITY))?;
        above = isolated(Op::Difference, (&above, Transform::IDENTITY), (&at(&up), Transform::IDENTITY))?;
        if let Some(d) = dowel {
            dowels = plan.centres.iter().map(|&[u, v]| (d.clone(), plane.frame(u, v))).collect();
        }
    }
    Ok(Parted { below, above, dowels })
}

/// Why a cut has no connectors, or the error when they were asked for by name.
fn none_fit(plane: Plane, ask: &Ask) -> Result<String, String> {
    let why = format!(
        "the cut face at {} = {:.2} has no room for a {} mm connector with {WALL} mm of wall round it and {MIN_DEPTH} mm of material \
         either side",
        AXES[plane.k],
        plane.at,
        ask.diameter.unwrap_or(MIN_AUTO)
    );
    if ask.said {
        Err(format!("no connector fits: {why}. Choose a smaller `diameter` or `depth`, or `kind: \"none\"`"))
    } else {
        Ok(format!("no connectors: {why}"))
    }
}

/// The turn that sets a piece on the plate to print. A piece with pegs (`pegs`: the axis and the side they point to) stands pegs up.
/// Otherwise it is turned onto one of its cut faces (each the axis and the side it faces) when that leaves it no taller and the face
/// is no smaller than what it stands on now, so half a ball lies on its flat; else it stays as it is. A turn that would not fit
/// within `limit` is not taken.
fn turn_for_print(piece: &Mesh, faces: &[(usize, f64)], pegs: Option<(usize, f64)>, limit: Option<[f64; 3]>) -> Transform {
    let size = |t: &Transform| piece.transformed(t).bounds().map(|(lo, hi)| [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]);
    let fits = |s: [f64; 3]| limit.is_none_or(|l| (0..3).all(|k| s[k] <= l[k] + 1e-6));
    if let Some((k, side)) = pegs {
        let t = towards(k, side, 1.0);
        return if size(&t).is_some_and(fits) { t } else { Transform::IDENTITY };
    }
    let (Some(now), Some((lo, hi))) = (size(&Transform::IDENTITY), piece.bounds()) else { return Transform::IDENTITY };
    let standing = area_at(piece, 2, lo[2] + 0.05);
    for &(k, side) in faces {
        let t = towards(k, side, -1.0);
        let face = area_at(piece, k, if side > 0.0 { hi[k] - 0.05 } else { lo[k] + 0.05 });
        if size(&t).is_some_and(|s| s[2] <= now[2] + 1e-6 && fits(s)) && face >= 0.99 * standing {
            return t;
        }
    }
    Transform::IDENTITY
}

/// The turn that points the side `side` (+1 or -1) of axis `k` up (`to` +1) or down (-1).
fn towards(k: usize, side: f64, to: f64) -> Transform {
    let mut n = [0.0; 3];
    n[k.min(2)] = side;
    match n[2] * to {
        d if d > 0.5 => Transform::IDENTITY,
        d if d < -0.5 => Transform::rotate(0, 180.0),
        // About the axis square to both, a quarter turn takes n to (0, 0, to).
        _ => Transform::rotate_axis([n[1] * to, -n[0] * to, 0.0], 90.0),
    }
}

/// Adds `mesh` turned by `turn` and set on the plate, and returns its id.
fn put(scene: &mut Scene, name: &str, mesh: Mesh, turn: Transform) -> Result<u32, String> {
    let (lo, hi) = mesh.transformed(&turn).bounds().ok_or("a piece is empty")?;
    scene.add_transformed(name, mesh, turn.then(&Transform::translate([-(lo[0] + hi[0]) / 2.0, -(lo[1] + hi[1]) / 2.0, -lo[2]])))
}

fn axis(name: &str) -> Result<usize, String> {
    AXES.iter().position(|a| *a == name).ok_or_else(|| format!("`axis` is x, y or z, not {name}"))
}

fn gap(g: Option<f64>) -> Result<f64, String> {
    match g {
        None => Ok(5.0),
        Some(g) if g.is_finite() && (0.0..=100.0).contains(&g) => Ok(g),
        Some(g) => Err(format!("`gap` is from 0 to 100 mm, not {g}")),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CutParams {
    id: u32,
    z: Option<f64>,
    axis: Option<String>,
    at: Option<f64>,
    keep: Option<String>,
    connectors: Option<Connectors>,
    lay_out: Option<bool>,
    name: Option<String>,
}

pub(crate) fn cut(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = CutParams::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    let (k, at) = match (a.z, a.axis.as_deref(), a.at) {
        (Some(z), None | Some("z"), None) => (2, z),
        (None, name, Some(at)) => (axis(name.unwrap_or("z"))?, at),
        (None, _, None) => return Err("give `at`, where along `axis` to cut (mm)".into()),
        _ => return Err("give `at` with `axis`, or the older `z` alone, not both".into()),
    };
    let (lower, upper) = match a.keep.as_deref().unwrap_or("both") {
        "both" => (true, true),
        "lower" => (true, false),
        "upper" => (false, true),
        other => return Err(format!("`keep` is both, lower or upper, not {other}")),
    };
    let mut ask = ask(a.connectors)?;
    if !(lower && upper) {
        if ask.said && ask.kind.is_some() {
            return Err("connectors join two pieces: keep both, or give `connectors: {kind: \"none\"}`".into());
        }
        ask.kind = None;
    }
    let o = s.scene.get(a.id)?.clone();
    let (lo, hi) = o.bounds().ok_or("the object is empty")?;
    if !at.is_finite() || at <= lo[k] + 1e-3 || at >= hi[k] - 1e-3 {
        return Err(format!("`at` must be between the object's least {0} ({1:.3}) and greatest {0} ({2:.3}), mm", AXES[k], lo[k], hi[k]));
    }
    let plane = Plane { k, at };
    let plan = place(&o.world(), plane, &ask, &[])?;
    let note = if plan.is_none() && ask.kind.is_some() { Some(none_fit(plane, &ask)?) } else { None };
    let parted = part((&o.mesh, o.xf), plane, plan.as_ref())?;
    let lay_out = a.lay_out.unwrap_or(false);
    let name = a.name.unwrap_or_else(|| o.name.clone());
    let pegs = plan.as_ref().filter(|p| p.kind == Kind::Pin).map(|_| (k, 1.0));
    let scene = s.scene_mut();
    scene.objects.retain(|x| x.id != a.id);
    let mut ids = Vec::new();
    for (keep, n, piece, face, pegs) in [(lower, 1, parted.below, (k, 1.0), pegs), (upper, 2, parted.above, (k, -1.0), None)] {
        if keep {
            let name = format!("{name} {n}");
            ids.push(if lay_out {
                let turn = turn_for_print(&piece, &[face], pegs, None);
                put(scene, &name, piece, turn)?
            } else {
                scene.add_placed(&name, piece)?
            });
        }
    }
    let mut dowels = Vec::new();
    for (m, xf) in parted.dowels {
        let name = format!("{name} dowel");
        dowels.push(if lay_out { put(scene, &name, m, Transform::IDENTITY)? } else { scene.add_transformed(&name, m, xf)? });
    }
    let mut out = json!({"ids": ids, "dowels": dowels, "connectors": plan.map(|p| p.describe(plane))});
    if lay_out {
        let all: Vec<u32> = s.scene.objects.iter().map(|o| o.id).collect();
        let bed = lamina_core::commands::plate(s);
        out["fits"] = json!(lamina_core::commands::lay_out(s, &all, 5.0, bed)?);
    }
    if let Some(n) = note {
        out["note"] = json!(n);
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FitParams {
    id: u32,
    bed: Option<[f64; 2]>,
    height: Option<f64>,
    connectors: Option<Connectors>,
    gap: Option<f64>,
}

/// A piece of an object being split: its mesh and transform, its cut faces (axis and the side each faces) and where its pegs point.
struct Piece {
    mesh: Mesh,
    xf: Transform,
    faces: Vec<(usize, f64)>,
    pegs: Option<(usize, f64)>,
}

pub(crate) fn split_to_fit(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = FitParams::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    let positive = |what: &str, x: f64| {
        if x.is_finite() && x > 0.0 && x <= 10_000.0 {
            Ok(x)
        } else {
            Err(format!("`{what}` must be above 0 and at most 10000 mm, not {x}"))
        }
    };
    let bed = match a.bed {
        None => lamina_core::commands::plate(s),
        Some([x, y]) => [positive("bed", x)?, positive("bed", y)?],
    };
    let default_height = lamina_slicer::profile::Profile::default().max_height;
    let height = positive("height", a.height.unwrap_or_else(|| s.print["profile"]["max_height"].as_f64().unwrap_or(default_height)))?;
    let (gap, ask) = (gap(a.gap)?, ask(a.connectors)?);
    let o = s.scene.get(a.id)?.clone();
    let (lo, hi) = o.bounds().ok_or("the object is empty")?;
    let size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let limit = [bed[0], bed[1], height];
    // Across the longest side that does not fit first; each such side in as few equal pieces as fit, shorter by a peg with pins.
    let mut axes: Vec<usize> = (0..3).filter(|&k| size[k] > limit[k] + 1e-6).collect();
    if axes.is_empty() {
        return Ok(
            json!({"ids": [a.id], "dowels": [], "cuts": [], "fits": true, "each_fits": true, "note": "it already fits the printer: nothing was cut"}),
        );
    }
    axes.sort_by(|&x, &y| size[y].total_cmp(&size[x]));
    let extra = if ask.kind == Some(Kind::Pin) { ask.depth.unwrap_or(MAX_DEPTH) } else { 0.0 };
    let mut planes = Vec::new();
    let mut count = 1usize;
    for &k in &axes {
        let room = limit[k] - extra;
        if room < 1.0 {
            return Err(format!("pieces cannot be made to fit {:.1} mm along {} with pegs that long", limit[k], AXES[k]));
        }
        let n = (size[k] / room).ceil().min(MAX_PIECES as f64 + 1.0) as usize;
        count = count.saturating_mul(n);
        planes.extend((1..n).map(|i| Plane { k, at: lo[k] + size[k] * i as f64 / n as f64 }));
    }
    if count > MAX_PIECES {
        return Err(format!(
            "that would take {count} pieces, more than the {MAX_PIECES} this makes: scale the object down or use a larger `bed`"
        ));
    }
    let mut pieces = vec![Piece { mesh: (*o.mesh).clone(), xf: o.xf, faces: Vec::new(), pegs: None }];
    let (mut dowels, mut cuts, mut notes) = (Vec::new(), Vec::new(), Vec::new());
    for &k in &axes {
        let here: Vec<Plane> = planes.iter().filter(|q| q.k == k).copied().collect();
        // Later and earlier cuts across this one: no connector near where they pass.
        let avoid: Vec<(usize, f64)> = planes.iter().filter(|q| q.k != k).map(|q| (usize::from(q.k != (k + 1) % 3), q.at)).collect();
        let mut next = Vec::new();
        for piece in pieces {
            let mut rest = piece;
            for &plane in &here {
                let world = rest.mesh.transformed(&rest.xf);
                let plan = place(&world, plane, &ask, &avoid)?;
                if plan.is_none() && ask.kind.is_some() {
                    notes.push(none_fit(plane, &ask)?);
                }
                let parted = part((&rest.mesh, rest.xf), plane, plan.as_ref())?;
                cuts.push(json!({"axis": AXES[k], "at": plane.at, "connectors": plan.as_ref().map(|p| p.describe(plane))}));
                dowels.extend(parted.dowels.into_iter().map(|(m, _)| m));
                let pegs = if plan.as_ref().is_some_and(|p| p.kind == Kind::Pin) { Some((k, 1.0)) } else { rest.pegs };
                let mut faces = rest.faces.clone();
                faces.push((k, 1.0));
                next.push(Piece { mesh: parted.below, xf: Transform::IDENTITY, faces, pegs });
                rest.faces.push((k, -1.0));
                rest = Piece { mesh: parted.above, xf: Transform::IDENTITY, ..rest };
            }
            next.push(rest);
        }
        pieces = next;
    }
    let scene = s.scene_mut();
    scene.objects.retain(|x| x.id != a.id);
    let (mut ids, mut each_fits) = (Vec::new(), true);
    for (i, piece) in pieces.into_iter().enumerate() {
        let world = piece.mesh.transformed(&piece.xf);
        let turn = turn_for_print(&world, &piece.faces, piece.pegs, Some(limit));
        let id = put(scene, &format!("{} {}", o.name, i + 1), world, turn)?;
        let (plo, phi) = scene.get(id)?.bounds().ok_or("a piece is empty")?;
        each_fits &= (0..3).all(|k| phi[k] - plo[k] <= limit[k] + 1e-6);
        ids.push(id);
    }
    let dowels: Vec<u32> =
        dowels.into_iter().map(|m| put(scene, &format!("{} dowel", o.name), m, Transform::IDENTITY)).collect::<Result<_, _>>()?;
    let all: Vec<u32> = s.scene.objects.iter().map(|o| o.id).collect();
    let fits = lamina_core::commands::lay_out(s, &all, gap, bed)?;
    if !fits {
        notes.push("the pieces do not all fit on the plate at once: print them in batches".into());
    }
    let mut out = json!({"ids": ids, "dowels": dowels, "cuts": cuts, "fits": fits, "each_fits": each_fits});
    if !notes.is_empty() {
        out["note"] = json!(notes.join("; "));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::new();
        s.register(crate::COMMANDS);
        s
    }

    fn ids(v: &Value) -> Vec<u32> {
        v.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect()
    }

    fn world(s: &Session, id: u32) -> Mesh {
        s.scene.get(id).unwrap().world()
    }

    /// The area of a regular polygon of `n` corners `r` from its middle.
    fn polygon(r: f64, n: u32) -> f64 {
        f64::from(n) / 2.0 * r * r * (TAU / f64::from(n)).sin()
    }

    fn sorted(mut v: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
        v.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
        v
    }

    /// The middles of the contours of a mesh's section square to `k` at `at`, in that plane's (u, v): with `holes`, only the holes
    /// in its shapes, else every shape's outline.
    fn middles(m: &Mesh, k: usize, at: f64, holes: bool) -> Vec<[f64; 2]> {
        let lift = at - m.bounds().unwrap().0[k] + 1.0;
        let region = section(&Plane { k, at }.local(m, lift), lift, 0.0);
        let contours = region.iter().flat_map(|shape| if holes { shape[1..].to_vec() } else { shape[..1].to_vec() });
        // Each contour's centroid by area: a boolean leaves its corners unevenly spread.
        let centroid = |c: Vec<[f64; 2]>| {
            let (mut a, mut x, mut y) = (0.0, 0.0, 0.0);
            for (p, q) in c.iter().zip(c.iter().cycle().skip(1)) {
                let w = p[0] * q[1] - q[0] * p[1];
                (a, x, y) = (a + w, x + (p[0] + q[0]) * w, y + (p[1] + q[1]) * w);
            }
            [x / (3.0 * a), y / (3.0 * a)]
        };
        sorted(contours.map(centroid).collect())
    }

    /// The centres a cut reports, in the plane's (u, v).
    fn centres(r: &Value, k: usize) -> Vec<[f64; 2]> {
        let at = r["connectors"]["at"].as_array().unwrap();
        sorted(at.iter().map(|p| [p[(k + 1) % 3].as_f64().unwrap(), p[(k + 2) % 3].as_f64().unwrap()]).collect())
    }

    fn same(a: &[[f64; 2]], b: &[[f64; 2]]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(p, q)| (p[0] - q[0]).hypot(p[1] - q[1]) < 0.02)
    }

    #[test]
    fn a_box_cut_with_dowels_gives_closed_pieces_with_holes_that_line_up_and_a_dowel_for_each() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [40, 40, 60]})).unwrap();
        // The older form, no axis, with dowels asked for by name.
        let r = s.execute("cut", &json!({"id": 1, "z": 30, "connectors": {"kind": "dowel"}})).unwrap();
        let (pieces, dowels, c) = (ids(&r["ids"]), ids(&r["dowels"]), &r["connectors"]);
        assert_eq!((pieces.len(), dowels.len(), c["kind"].as_str(), c["count"].as_u64()), (2, 2, Some("dowel"), Some(2)), "{r}");
        let names: Vec<&str> = s.scene.objects.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["box 1", "box 2", "box dowel", "box dowel"]);
        let d = c["hole_diameter"].as_f64().unwrap();
        let depth = [c["hole_depth"][0].as_f64().unwrap(), c["hole_depth"][1].as_f64().unwrap()];
        assert!((3.0..=8.0).contains(&d) && depth.iter().all(|&x| (4.0..=15.0).contains(&x)), "{c}");
        let hole = polygon(d / 2.0, SEGMENTS);
        for (id, deep) in [(pieces[0], depth[0]), (pieces[1], depth[1])] {
            let m = world(&s, id);
            assert!(m.is_watertight(), "piece {id} is closed");
            assert!((m.volume() - (48_000.0 - 2.0 * hole * deep)).abs() < 1.0, "piece {id}: {} mm3", m.volume());
        }
        // Each dowel is a closed pin the clearance smaller than its holes and a little shorter than both together.
        let pin = d - 2.0 * c["clearance"].as_f64().unwrap();
        assert!((c["dowel_diameter"].as_f64().unwrap() - pin).abs() < 1e-9);
        for &id in &dowels {
            let m = world(&s, id);
            let (lo, hi) = m.bounds().unwrap();
            assert!(m.is_watertight());
            assert!((hi[0] - lo[0] - pin).abs() < 0.01 && (hi[2] - lo[2] - (depth[0] + depth[1] - GAP)).abs() < 1e-3, "{lo:?} {hi:?}");
        }
        // Put together as cut: the holes below the plane and above it are where the cut says, and each dowel stands in one.
        let at = centres(&r, 2);
        let below = middles(&world(&s, pieces[0]), 2, 29.5, true);
        assert!(same(&below, &at), "{below:?} {at:?}");
        assert!(same(&middles(&world(&s, pieces[1]), 2, 30.5, true), &at));
        let standing = sorted(dowels.iter().map(|&id| s.scene.get(id).unwrap().xf.t).map(|t| [t[0], t[1]]).collect());
        assert!(same(&standing, &at));
        // Spread across the face, so the pieces cannot turn on them.
        assert!((at[0][0] - at[1][0]).hypot(at[0][1] - at[1][1]) > 30.0, "{at:?}");
        s.undo().unwrap();
        assert_eq!(s.scene.objects.len(), 1, "one undo puts the box back");
    }

    #[test]
    fn pins_add_pegs_to_one_piece_and_sockets_with_clearance_to_the_other_in_line() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [40, 40, 60]})).unwrap();
        let r = s.execute("cut", &json!({"id": 1, "axis": "z", "at": 30, "connectors": {"kind": "pin"}})).unwrap();
        let (pieces, c) = (ids(&r["ids"]), &r["connectors"]);
        assert_eq!((r["dowels"].as_array().unwrap().len(), c["count"].as_u64()), (0, Some(2)), "{r}");
        let [d, length, socket, deep] = ["peg_diameter", "peg_length", "socket_diameter", "socket_depth"].map(|k| c[k].as_f64().unwrap());
        assert!((socket - d - 0.4).abs() < 1e-9 && (deep - length - GAP).abs() < 1e-9, "{c}");
        // A peg above the plane is its whole solid but the millimetre of root inside piece 1.
        let (r1, ch) = (d / 2.0, (d / 8.0).min(0.4));
        let peg = rings(&[(r1, -1.0), (r1, length - ch), (r1 - ch, length)], SEGMENTS).volume() - polygon(r1, SEGMENTS);
        let rs = r1 + 0.2 / (PI / f64::from(SEGMENTS)).cos();
        let (low, high) = (world(&s, pieces[0]), world(&s, pieces[1]));
        assert!(low.is_watertight() && high.is_watertight());
        assert!((low.volume() - (48_000.0 + 2.0 * peg)).abs() < 1.0, "{} {}", low.volume(), 48_000.0 + 2.0 * peg);
        assert!((high.volume() - (48_000.0 - 2.0 * polygon(rs, SEGMENTS) * deep)).abs() < 1.0, "{}", high.volume());
        // Put together: half a millimetre over the plane, piece 1 is only its pegs and piece 2 has its sockets round them.
        let at = centres(&r, 2);
        let pegs = middles(&low, 2, 30.5, false);
        assert!(same(&pegs, &at), "{pegs:?} {at:?}");
        assert!(same(&middles(&high, 2, 30.5, true), &at));
        assert!((low.bounds().unwrap().1[2] - (30.0 + length)).abs() < 1e-3);
    }

    /// One piece carries the connectors and the other the holes, unless something else is asked for: nothing loose to print.
    #[test]
    fn by_default_one_piece_has_the_pegs_and_the_other_the_sockets() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [40, 40, 60]})).unwrap();
        let r = s.execute("cut", &json!({"id": 1, "z": 30})).unwrap();
        let c = &r["connectors"];
        assert_eq!((c["kind"].as_str(), c["count"].as_u64(), r["dowels"].as_array().map(Vec::len)), (Some("pin"), Some(2), Some(0)), "{r}");
        assert_eq!(s.scene.objects.len(), 2, "nothing loose");
        let p = ids(&r["ids"]);
        let (pegs, sockets) = (world(&s, p[0]), world(&s, p[1]));
        assert!(pegs.is_watertight() && sockets.is_watertight());
        assert!(pegs.volume() > 48_000.0 && sockets.volume() < 48_000.0, "{} {}", pegs.volume(), sockets.volume());
        assert!(pegs.bounds().unwrap().1[2] > 30.0, "the pegs stand above the cut");
    }

    /// What the user chose is what is made: the kind, the shape, how many and how big.
    #[test]
    fn the_connectors_are_what_was_chosen() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [60, 60, 40]})).unwrap();
        let ask = json!({"kind": "dowel", "shape": "square", "count": 3, "diameter": 6});
        let r = s.execute("cut", &json!({"id": 1, "z": 20, "connectors": ask})).unwrap();
        let c = &r["connectors"];
        assert_eq!(
            (c["kind"].as_str(), c["shape"].as_str(), c["count"].as_u64(), c["hole_diameter"].as_f64()),
            (Some("dowel"), Some("square"), Some(3), Some(6.0)),
            "{r}"
        );
        assert_eq!(ids(&r["dowels"]).len(), 3);
        assert!(ids(&r["ids"]).iter().chain(ids(&r["dowels"]).iter()).all(|&id| world(&s, id).is_watertight()));
        assert!(s.execute("cut", &json!({"id": 2, "z": 10, "connectors": {"shape": "star"}})).unwrap_err().contains("round or square"));
    }

    #[test]
    fn cuts_along_x_and_y_part_where_asked_and_line_up() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [60, 30, 20]})).unwrap();
        let r = s.execute("cut", &json!({"id": 1, "axis": "x", "at": 10, "connectors": {"kind": "dowel"}})).unwrap();
        let pieces = ids(&r["ids"]);
        let (a, b) = (world(&s, pieces[0]), world(&s, pieces[1]));
        assert!(a.is_watertight() && b.is_watertight());
        assert!((a.bounds().unwrap().1[0] - 10.0).abs() < 1e-4 && (b.bounds().unwrap().0[0] - 10.0).abs() < 1e-4);
        let c = &r["connectors"];
        let holes = 2.0 * polygon(c["hole_diameter"].as_f64().unwrap() / 2.0, SEGMENTS);
        let depth: f64 = c["hole_depth"].as_array().unwrap().iter().filter_map(Value::as_f64).sum();
        assert!((a.volume() + b.volume() - (36_000.0 - holes * depth)).abs() < 1.0, "{} {}", a.volume(), b.volume());
        let at = centres(&r, 0);
        assert!(same(&middles(&a, 0, 9.5, true), &at) && same(&middles(&b, 0, 10.5, true), &at), "{at:?}");
        // Along y, with pins: the pegs stand out of piece 1 toward plus y, into piece 2's sockets.
        let r = s.execute("cut", &json!({"id": pieces[1], "axis": "y", "at": -3, "connectors": {"kind": "pin"}})).unwrap();
        let p = ids(&r["ids"]);
        let (low, high) = (world(&s, p[0]), world(&s, p[1]));
        assert!(low.is_watertight() && high.is_watertight());
        let at = centres(&r, 1);
        assert_eq!(at.len(), 2, "{r}");
        assert!(same(&middles(&low, 1, -2.5, false), &at) && same(&middles(&high, 1, -2.5, true), &at), "{at:?}");
    }

    #[test]
    fn a_face_with_room_for_one_gets_a_square_key_and_a_face_too_small_none_said() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [9, 9, 20]})).unwrap();
        let r = s.execute("cut", &json!({"id": 1, "z": 10, "connectors": {"kind": "dowel"}})).unwrap();
        assert_eq!((r["connectors"]["shape"].as_str(), r["connectors"]["count"].as_u64()), (Some("square"), Some(1)), "{r}");
        let key = world(&s, ids(&r["dowels"])[0]);
        let (lo, hi) = key.bounds().unwrap();
        assert!(key.is_watertight() && (hi[0] - lo[0]) < 3.0 - 0.4, "a square across its corners: {lo:?} {hi:?}");
        s.undo().unwrap();
        s.execute("add_primitive", &json!({"kind": "box", "size": [6, 6, 20]})).unwrap();
        let r = s.execute("cut", &json!({"id": 2, "at": 10})).unwrap();
        assert!(r["connectors"].is_null() && r["dowels"].as_array().unwrap().is_empty(), "{r}");
        assert!(r["note"].as_str().unwrap().starts_with("no connectors"), "{r}");
        let total: f64 = ids(&r["ids"]).iter().map(|&id| world(&s, id).volume()).sum();
        assert!((total - 720.0).abs() < 1e-3);
        s.undo().unwrap();
        // Asked for by name, they are refused instead, and nothing changes.
        let before = std::sync::Arc::clone(&s.scene);
        let err = s.execute("cut", &json!({"id": 2, "at": 10, "connectors": {"kind": "dowel"}})).unwrap_err();
        assert!(err.contains("no connector fits"), "{err}");
        assert!(std::sync::Arc::ptr_eq(&before, &s.scene));
    }

    #[test]
    fn laid_out_the_peg_piece_stands_pegs_up_and_the_other_on_its_cut_face() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [40, 30, 60]})).unwrap();
        let r = s.execute("cut", &json!({"id": 1, "axis": "x", "at": 0, "connectors": {"kind": "pin"}, "lay_out": true})).unwrap();
        assert_eq!(r["fits"], true);
        let p = ids(&r["ids"]);
        let length = r["connectors"]["peg_length"].as_f64().unwrap();
        let (llo, lhi) = s.scene.get(p[0]).unwrap().bounds().unwrap();
        let (hlo, hhi) = s.scene.get(p[1]).unwrap().bounds().unwrap();
        assert!(llo[2].abs() < 1e-6 && (lhi[2] - (20.0 + length)).abs() < 1e-3, "pegs up: {llo:?} {lhi:?}");
        assert!(hlo[2].abs() < 1e-6 && (hhi[2] - 20.0).abs() < 1e-3, "on its cut face: {hlo:?} {hhi:?}");
        assert!(llo[0] > hhi[0] || hlo[0] > lhi[0] || llo[1] > hhi[1] || hlo[1] > lhi[1], "side by side, not touching");
    }

    #[test]
    fn a_cylinder_too_tall_is_split_into_pieces_that_fit_with_its_dowels_on_the_plate() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "cylinder", "radius": 30, "height": 300})).unwrap();
        let whole = world(&s, 1).volume();
        let r = s.execute("split_to_fit", &json!({"id": 1, "bed": [220, 220], "connectors": {"kind": "dowel"}})).unwrap();
        let (pieces, dowels) = (ids(&r["ids"]), ids(&r["dowels"]));
        assert_eq!((pieces.len(), r["fits"].as_bool(), r["each_fits"].as_bool()), (2, Some(true), Some(true)), "{r}");
        let cut = &r["cuts"][0];
        assert_eq!((cut["axis"].as_str(), cut["at"].as_f64()), (Some("z"), Some(150.0)));
        let c = &cut["connectors"];
        assert_eq!(dowels.len() as u64, c["count"].as_u64().unwrap());
        let hole = polygon(c["hole_diameter"].as_f64().unwrap() / 2.0, SEGMENTS);
        let depth: f64 = c["hole_depth"].as_array().unwrap().iter().filter_map(Value::as_f64).sum();
        let mut total = 0.0;
        for &id in pieces.iter().chain(&dowels) {
            let o = s.scene.get(id).unwrap();
            let (lo, hi) = o.bounds().unwrap();
            assert!(o.world().is_watertight(), "{}", o.name);
            let on_plate = lo[2].abs() < 1e-6 && hi[2] <= 250.0 && lo[0] >= -110.0 && hi[0] <= 110.0 && lo[1] >= -110.0 && hi[1] <= 110.0;
            assert!(on_plate, "{} {lo:?} {hi:?}", o.name);
            if pieces.contains(&id) {
                total += o.world().volume();
            }
        }
        let want = whole - dowels.len() as f64 * hole * depth;
        assert!((total - want).abs() < 2.0, "{total} {want}");
    }

    #[test]
    fn a_wide_plate_is_split_both_ways_keeping_connectors_off_the_other_cuts() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [300, 260, 12]})).unwrap();
        let r = s.execute("split_to_fit", &json!({"id": 1, "bed": [220, 220], "gap": 3, "connectors": {"kind": "dowel"}})).unwrap();
        let pieces = ids(&r["ids"]);
        assert_eq!((pieces.len(), r["each_fits"].as_bool()), (4, Some(true)), "{r}");
        assert_eq!(r["cuts"].as_array().unwrap().len(), 3, "x first, the longer side, then y through both halves: {r}");
        assert_eq!(r["cuts"][0]["axis"], "x");
        for cut in r["cuts"].as_array().unwrap() {
            let other = if cut["axis"] == "x" { 1 } else { 0 };
            for p in cut["connectors"]["at"].as_array().unwrap() {
                assert!(p[other].as_f64().unwrap().abs() > 3.0, "a connector on the other cut: {cut}");
            }
        }
        let holes: f64 = r["cuts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cut| {
                let c = &cut["connectors"];
                let depth: f64 = c["hole_depth"].as_array().unwrap().iter().filter_map(Value::as_f64).sum();
                c["count"].as_f64().unwrap() * polygon(c["hole_diameter"].as_f64().unwrap() / 2.0, SEGMENTS) * depth
            })
            .sum();
        let total: f64 = pieces.iter().map(|&id| world(&s, id).volume()).sum();
        assert!((total - (300.0 * 260.0 * 12.0 - holes)).abs() < 2.0, "{total} {holes}");
        assert!(pieces.iter().all(|&id| world(&s, id).is_watertight()));
    }

    #[test]
    fn hostile_requests_are_refused_and_change_nothing() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [40, 40, 60]})).unwrap();
        let before = std::sync::Arc::clone(&s.scene);
        for (command, p) in [
            ("cut", json!({"id": 1, "at": 70})),
            ("cut", json!({"id": 1, "axis": "x", "at": -20})),
            ("cut", json!({"id": 1, "axis": "w", "at": 0})),
            ("cut", json!({"id": 1, "z": 30, "at": 30})),
            ("cut", json!({"id": 1, "z": 30, "axis": "x"})),
            ("cut", json!({"id": 1})),
            ("cut", json!({"id": 9, "at": 30})),
            ("cut", json!({"id": 1, "at": 30, "connectors": {"kind": "glue"}})),
            ("cut", json!({"id": 1, "at": 30, "connectors": {"count": 0}})),
            ("cut", json!({"id": 1, "at": 30, "connectors": {"count": 9}})),
            ("cut", json!({"id": 1, "at": 30, "connectors": {"diameter": 100}})),
            ("cut", json!({"id": 1, "at": 30, "connectors": {"diameter": 2, "clearance": 1}})),
            ("cut", json!({"id": 1, "at": 30, "connectors": {"depth": -1}})),
            ("cut", json!({"id": 1, "at": 30, "connectors": {"clearance": 2}})),
            ("cut", json!({"id": 1, "at": 30, "connectors": {"kinds": "pin"}})),
            ("cut", json!({"id": 1, "at": 30, "connectors": {"count": 8, "diameter": 30}})),
            ("cut", json!({"id": 1, "at": 30, "keep": "lower", "connectors": {"kind": "pin"}})),
            ("cut", json!({"id": 1, "at": 30, "lay_out": "yes"})),
            ("split_to_fit", json!({"id": 1, "bed": [0, 220]})),
            ("split_to_fit", json!({"id": 1, "bed": [-1, 5]})),
            ("split_to_fit", json!({"id": 1, "height": 0})),
            ("split_to_fit", json!({"id": 1, "gap": 200})),
            ("split_to_fit", json!({"id": 1, "bed": [5, 5]})),
            ("split_to_fit", json!({"id": 1, "bed": [30, 30], "connectors": {"kind": "pin", "depth": 40}})),
            ("split_to_fit", json!({"id": 9})),
            ("split_to_fit", json!({"id": 1, "plate": [100, 100]})),
        ] {
            assert!(s.execute(command, &p).is_err(), "{command} {p}");
            assert!(std::sync::Arc::ptr_eq(&before, &s.scene), "{command} {p} changed the plate");
        }
        // An object that already fits is left as it is, and that is said.
        let r = s.execute("split_to_fit", &json!({"id": 1})).unwrap();
        assert_eq!(ids(&r["ids"]), [1]);
        assert!(std::sync::Arc::ptr_eq(&before, &s.scene));
    }

    #[test]
    fn a_chamfered_pin_is_closed_with_the_volume_of_its_frustums_and_faces_turn_where_asked() {
        let (r, c, h) = (3.0, 0.4, 20.0);
        let m = rings(&[(r - c, 0.0), (r, c), (r, h - c), (r - c, h)], 64);
        let area = |r: f64| polygon(r, 64);
        let frustum = |a: f64, b: f64, h: f64| h / 3.0 * (area(a) + area(b) + (area(a) * area(b)).sqrt());
        assert!(m.is_watertight());
        assert!((m.volume() - (2.0 * frustum(r - c, r, c) + area(r) * (h - 2.0 * c))).abs() < 1e-3, "{}", m.volume());
        let near = |p: [f64; 3], q: [f64; 3]| (0..3).all(|k| (p[k] - q[k]).abs() < 1e-12);
        assert!(near(towards(0, 1.0, -1.0).apply([1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]));
        assert!(near(towards(1, -1.0, 1.0).apply([0.0, -1.0, 0.0]), [0.0, 0.0, 1.0]));
        assert!(near(towards(2, 1.0, -1.0).apply([0.0, 0.0, 1.0]), [0.0, 0.0, -1.0]));
    }
}
