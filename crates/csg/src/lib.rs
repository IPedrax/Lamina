//! Mesh booleans for Lamina. This is the only crate that knows csgrs (a pure-Rust BSP library): the rest of the app calls [`boolean`]
//! with meshes and gets a mesh, so another engine can replace it without touching a command.
//!
//! What csgrs gives back is right in volume but not closed: splitting polygons leaves T-junctions, where a vertex of one face sits
//! in the middle of its neighbour's edge. A slicer and `Mesh::is_watertight` both read topology, so [`close`] welds the vertices and
//! splits triangles at those vertices until every edge is shared by two faces.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod child;
mod commands;

use std::collections::{HashMap, HashSet};

use csgrs::mesh::{bsp::Node, polygon::Polygon, vertex::Vertex};
use lamina_core::repair::{self, SNAP};
use lamina_core::{Mesh, Transform};
use nalgebra::{Point3, Vector3};

pub use child::{CHILD_ARG, CHILD_TIME, child_main, in_child, isolated, run_in_child};
pub use commands::COMMANDS;

/// The most triangles that may go into one operation. BSP trees grow faster than the input, so this keeps one command to seconds.
pub const MAX_INPUT_TRIANGLES: usize = 30_000;
/// Vertices closer than this (mm) are one vertex; a vertex within a few times this of an edge is on it.
const WELD: f64 = 1e-3;
/// BSP trees recurse as deep as the input is tangled, so the work runs on a thread with a big stack.
const STACK: usize = 256 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Union,
    Difference,
    Intersect,
}

/// A mesh in its own frame and the transform that places it, as an object on the plate holds them.
pub type Placed<'a> = (&'a Mesh, Transform);

/// `a` combined with `b` (meshes where they stand), or an error saying why not. See [`boolean_placed`].
pub fn boolean(op: Op, a: &Mesh, b: &Mesh) -> Result<Mesh, String> {
    boolean_placed(op, (a, Transform::IDENTITY), (b, Transform::IDENTITY))
}

/// `a` combined with `b`, each a mesh in its own frame and its transform, or an error saying why not. Flat patches are found in each
/// mesh's own frame, where a cylinder's cap is exactly flat however the object is turned, and only then moved where they stand, in
/// f64. A result is returned only if it is a closed solid with a volume and bounds that are possible for these shapes ([`sound`]):
/// the BSP can leave cracks or lose faces where curved surfaces cut each other, and a broken mesh in the scene would slice into a
/// broken print.
pub fn boolean_placed(op: Op, a: Placed, b: Placed) -> Result<Mesh, String> {
    for (name, (m, xf)) in [("first", &a), ("second", &b)] {
        if !m.is_watertight() {
            return Err(format!("the {name} shape is not a closed mesh, so a boolean cannot tell its inside from its outside"));
        }
        if !xf.is_finite() || xf.det().abs() <= 1e-12 {
            return Err(format!("the {name} shape's transform flattens it"));
        }
    }
    let total = a.0.triangles.len() + b.0.triangles.len();
    if total > MAX_INPUT_TRIANGLES {
        return Err(format!("{total} triangles is more than the {MAX_INPUT_TRIANGLES} one boolean takes: use fewer segments"));
    }
    let result = engine(op, a, b)?;
    sound(op, &a.0.transformed(&a.1), &b.0.transformed(&b.1), &result)
        .map_err(|why| format!("the boolean did not give a clean solid ({why}). {HARD}"))?;
    Ok(result)
}

/// The BSP and the repair, with no judgement of the result.
fn engine(op: Op, a: Placed, b: Placed) -> Result<Mesh, String> {
    let (pa, pb) = (to_csg(a.0, &a.1), to_csg(b.0, &b.1));
    let run = move || from_csg(&combine(op, &pa, &pb));
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(run)
        .map_err(|e| format!("could not start the boolean: {e}"))?
        .join()
        .map_err(|_| HARD.to_string())?
}

/// What the error says to do, because the model or the user reading it has to act on it.
const HARD: &str = "Curved shapes that meet at a glancing angle, and booleans of earlier booleans' results, are the hardest cases for this \
                    engine. Change a size or a position by a hundredth of a millimetre, use fewer segments, or use another shape, and try again";

/// Whether `r` can be `op` of `a` and `b`: closed, with a volume between what inclusion and exclusion allow, and inside the box the
/// operation allows. An empty result is the caller's to explain.
fn sound(op: Op, a: &Mesh, b: &Mesh, r: &Mesh) -> Result<(), String> {
    let (Some((alo, ahi)), Some((blo, bhi))) = (a.bounds(), b.bounds()) else { return Err("a shape is empty".into()) };
    let Some((rlo, rhi)) = r.bounds() else { return Ok(()) };
    if !r.is_watertight() {
        return Err("the result has cracks or doubled faces".into());
    }
    let (va, vb, v) = (a.volume(), b.volume(), r.volume());
    let slack = 1.0 + 1e-3 * (va + vb);
    let volume_ok = match op {
        Op::Union => v >= va.max(vb) - slack && v <= va + vb + slack,
        Op::Difference => v >= (va - vb).max(0.0) - slack && v <= va + slack,
        Op::Intersect => v >= -slack && v <= va.min(vb) + slack,
    };
    if !volume_ok {
        return Err(format!("its volume, {v:.1} mm3, is impossible for shapes of {va:.1} and {vb:.1} mm3"));
    }
    // The repair may move a vertex by up to SNAP; more than that is a real difference.
    let tol = SNAP + WELD;
    let inside = |lo: [f64; 3], hi: [f64; 3]| (0..3).all(|k| rlo[k] >= lo[k] - tol && rhi[k] <= hi[k] + tol);
    let bounds_ok = match op {
        Op::Union => (0..3).all(|k| (rlo[k] - alo[k].min(blo[k])).abs() <= tol && (rhi[k] - ahi[k].max(bhi[k])).abs() <= tol),
        Op::Difference => inside(alo, ahi),
        Op::Intersect => inside(
            [alo[0].max(blo[0]), alo[1].max(blo[1]), alo[2].max(blo[2])],
            [ahi[0].min(bhi[0]), ahi[1].min(bhi[1]), ahi[2].min(bhi[2])],
        ),
    };
    if bounds_ok { Ok(()) } else { Err("it reaches somewhere the shapes do not".into()) }
}

/// The classic BSP booleans over the whole of both shapes. csgrs' own `union` and `difference` first set aside the faces whose
/// boxes miss the other shape's box, which is quick, but a shape lying wholly inside the other then has nothing to be clipped
/// against: a union of nested boxes keeps both, and a box cut by a bigger box comes back whole. So both trees are built from every
/// polygon ([`build`]), and only the clipping ([`clip`]) passes over a polygon far from the other shape. The last argument of each
/// `clip` says whether the tree it clips against has been inverted.
fn combine(op: Op, pa: &[Polygon<()>], pb: &[Polygon<()>]) -> Vec<Polygon<()>> {
    let (near_a, near_b) = (reach(pa), reach(pb));
    let (mut a, mut b) = (Node::new(), Node::new());
    build(&mut a, pa.to_vec());
    build(&mut b, pb.to_vec());
    match op {
        Op::Union => {
            clip(&mut a, &b, &near_b, false);
            clip(&mut b, &a, &near_a, false);
            b.invert();
            clip(&mut b, &a, &near_a, false);
            b.invert();
            a.build(&b.all_polygons());
        }
        Op::Difference => {
            a.invert();
            clip(&mut a, &b, &near_b, false);
            clip(&mut b, &a, &near_a, true);
            b.invert();
            clip(&mut b, &a, &near_a, true);
            b.invert();
            a.build(&b.all_polygons());
            a.invert();
        }
        Op::Intersect => {
            a.invert();
            clip(&mut b, &a, &near_a, true);
            b.invert();
            clip(&mut a, &b, &near_b, true);
            clip(&mut b, &a, &near_a, true);
            a.build(&b.all_polygons());
            a.invert();
        }
    }
    a.all_polygons()
}

/// The box a shape's polygons fill, widened by ten times [`WELD`]: a polygon outside it cannot touch the shape.
type Reach = ([f64; 3], [f64; 3]);

fn reach(polys: &[Polygon<()>]) -> Reach {
    let mut r = ([f64::MAX; 3], [f64::MIN; 3]);
    for v in polys.iter().flat_map(|p| &p.vertices) {
        for k in 0..3 {
            r.0[k] = r.0[k].min(v.pos[k] - 10.0 * WELD);
            r.1[k] = r.1[k].max(v.pos[k] + 10.0 * WELD);
        }
    }
    r
}

/// `node.clip_to(by)`, except that a polygon outside `by`'s reach is not split by its planes. csgrs splits every polygon by every plane
/// of the other tree, and its planes reach across the whole part: a peg's top cut the walls of a hole at the far end into slivers,
/// T-junctions and fans, which the next boolean then stumbled on. A polygon that misses the other shape's box is wholly outside that
/// shape, so it is kept whole, or dropped whole when `by` has been inverted (its inside is then the shape's outside). The tree that
/// decides is still built from every polygon, so a shape inside the other is clipped as before.
fn clip(node: &mut Node<()>, by: &Node<()>, near: &Reach, inverted: bool) {
    let (n, far): (Vec<_>, Vec<_>) = std::mem::take(&mut node.polygons).into_iter().partition(|p| {
        (0..3).all(|k| p.vertices.iter().any(|v| v.pos[k] >= near.0[k]) && p.vertices.iter().any(|v| v.pos[k] <= near.1[k]))
    });
    node.polygons = by.clip_polygons(&n);
    if !inverted {
        node.polygons.extend(far);
    }
    if let Some(f) = node.front.as_deref_mut() {
        clip(f, by, near, inverted);
    }
    if let Some(b) = node.back.as_deref_mut() {
        clip(b, by, near, inverted);
    }
}

/// Twice the area of the triangle that spans a polygon's plane: csgrs takes a point as on the plane when the determinant over that
/// triangle and the point is under 1e-8, so the plane takes in points up to 1e-8 / this many millimetres off it.
fn span(p: &Polygon<()>) -> f64 {
    let q = &p.plane;
    (q.point_b - q.point_a).cross(&(q.point_c - q.point_a)).norm()
}

/// Below this [`span`] a polygon's plane takes in points more than a tenth of a micron off it: a splitting plane only when nothing
/// better is left. (Never one, dropping such polygons instead, measured worse.)
const THIN: f64 = 1e-4;
/// Below this csgrs gives the plane no normal at all (its own `EPSILON`): such a polygon is dropped.
const FLAT: f64 = 1e-8;

/// csgrs' `Node::build`, except that a polygon whose corners span no plane is dropped and a sliver's plane is never the splitting
/// plane while a better one is left. A sliver's plane takes in everything for millimetres round it ([`span`]), so csgrs' own choice,
/// which scores a plane by the polygons it splits and how evenly it parts the rest, likes it best of all: it splits nothing. A node on
/// it then holds every polygon that reaches it as coplanar, and a cell below it comes out inside or outside by chance. Splitting
/// leaves such slivers wherever a plane passes near a corner, the more the more corners a mesh has: an earlier boolean's result.
/// (Measured on a pocket cut into a block that already had two holes: a node 43 deep on a fragment of 7e-11 mm2 held 107 polygons
/// as coplanar, and a corner of the pocket's floor came out outside the block.)
fn build(node: &mut Node<()>, polygons: Vec<Polygon<()>>) {
    let (mut list, thin): (Vec<_>, Vec<_>) = polygons.into_iter().filter(|p| span(p) > FLAT).partition(|p| span(p) >= THIN);
    let good = list.len();
    list.extend(thin);
    if list.is_empty() {
        return;
    }
    let plane = if good > 0 {
        node.pick_best_splitting_plane(&list[..good])
    } else {
        let Some(widest) = list.iter().max_by(|x, y| span(x).total_cmp(&span(y))) else { return };
        widest.plane.clone()
    };
    node.plane = Some(plane.clone());
    let (mut front, mut back) = (Vec::new(), Vec::new());
    for p in &list {
        let (on_front, on_back, f, b) = plane.split_polygon(p);
        node.polygons.extend(on_front);
        node.polygons.extend(on_back);
        front.extend(f);
        back.extend(b);
    }
    if !front.is_empty() {
        build(node.front.get_or_insert_with(|| Box::new(Node::new())), front);
    }
    if !back.is_empty() {
        build(node.back.get_or_insert_with(|| Box::new(Node::new())), back);
    }
}

/// The mesh as BSP polygons. Triangles that share an edge and a plane become one polygon when together they make an exactly flat,
/// convex outline (a box's face, an upright cylinder's cap), because every extra edge inside a face is one more place for the BSP
/// to split, and a cap fanned from its centre is a ring of slivers.
fn to_csg(m: &Mesh, xf: &Transform) -> Vec<Polygon<()>> {
    let mut normals: Vec<Option<Vector3<f64>>> = (0..m.triangles.len())
        .map(|i| {
            let t = m.tri(i)?;
            let n = Vector3::new(t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]).cross(&Vector3::new(
                t[2][0] - t[0][0],
                t[2][1] - t[0][1],
                t[2][2] - t[0][2],
            ));
            // A flat triangle has no normal and no volume to give; leave it out.
            (n.norm() > 1e-12).then(|| n.normalize())
        })
        .collect();
    // Union-find over triangles that share an edge and lie in one plane.
    let mut parent: Vec<usize> = (0..m.triangles.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, t) in m.triangles.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            by_edge.entry((a.min(b), a.max(b))).or_default().push(i);
        }
    }
    let mut pairs: Vec<(usize, usize)> = by_edge.values().filter(|v| v.len() == 2).map(|v| (v[0], v[1])).collect();
    pairs.sort_unstable();
    // A fold: two triangles on one edge that face opposite ways, where a repair turned a sliver over. The mesh is still closed, but
    // csgrs trusts each polygon's facing across its whole plane: a fold of a few square microns in a part's top, high in the tree,
    // made everything above the part its inside. The smaller of the two is left out; the gap it leaves is that small.
    for &(i, j) in &pairs {
        if let (Some(ni), Some(nj)) = (normals[i], normals[j])
            && ni.dot(&nj) < -0.99
        {
            normals[if repair::triangle_area(m, i) < repair::triangle_area(m, j) { i } else { j }] = None;
        }
    }
    for (i, j) in pairs {
        let (Some(ni), Some(nj)) = (normals[i], normals[j]) else { continue };
        let pi = m.vertices[m.triangles[i][0] as usize].map(f64::from);
        let pj = m.vertices[m.triangles[j][0] as usize].map(f64::from);
        let off = ni.dot(&Vector3::new(pj[0] - pi[0], pj[1] - pi[1], pj[2] - pi[2]));
        if ni.dot(&nj) > 1.0 - 1e-9 && off.abs() < WELD {
            let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
            parent[ri] = rj;
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
    for i in 0..m.triangles.len() {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    // Where each corner stands, worked out in f64 from the mesh's own frame: a patch flat there stays flat to f64's rounding.
    let point = |i: u32| {
        let p = xf.apply(m.vertices[i as usize].map(f64::from));
        Point3::new(p[0], p[1], p[2])
    };
    // A normal moves by the matrix's cofactors (its inverse transposed, scaled), which keeps it square to the face under any scale.
    let c = &xf.m;
    let cof = [
        [c[1][1] * c[2][2] - c[1][2] * c[2][1], c[1][2] * c[2][0] - c[1][0] * c[2][2], c[1][0] * c[2][1] - c[1][1] * c[2][0]],
        [c[0][2] * c[2][1] - c[0][1] * c[2][2], c[0][0] * c[2][2] - c[0][2] * c[2][0], c[0][1] * c[2][0] - c[0][0] * c[2][1]],
        [c[0][1] * c[1][2] - c[0][2] * c[1][1], c[0][2] * c[1][0] - c[0][0] * c[1][2], c[0][0] * c[1][1] - c[0][1] * c[1][0]],
    ];
    // A mirror turns faces inside out: their corners go the other way round, and the cofactors point the normal inward.
    let flip = xf.det() < 0.0;
    let sign = if flip { -1.0 } else { 1.0 };
    let corners = |ring: &[u32]| -> Vec<u32> { if flip { ring.iter().rev().copied().collect() } else { ring.to_vec() } };
    let turn = |n: Vector3<f64>| {
        let w = sign
            * Vector3::new(
                cof[0][0] * n.x + cof[0][1] * n.y + cof[0][2] * n.z,
                cof[1][0] * n.x + cof[1][1] * n.y + cof[1][2] * n.z,
                cof[2][0] * n.x + cof[2][1] * n.y + cof[2][2] * n.z,
            );
        if w.norm() > 1e-12 { w.normalize() } else { n }
    };
    let mut out = Vec::new();
    for tris in groups.values() {
        let Some(n) = normals[tris[0]].map(turn) else { continue };
        if tris.len() > 1
            && let Some(ring) = outline(m, tris).filter(|r| exactly_flat(m, r) && repair::flat_and_convex(m, r, 5.0 * WELD))
        {
            out.push(Polygon::new(corners(&ring).iter().map(|&i| Vertex::new(point(i), n)).collect(), None));
            continue;
        }
        for &i in tris {
            if let Some(n) = normals[i].map(turn) {
                out.push(Polygon::new(corners(&m.triangles[i]).iter().map(|&v| Vertex::new(point(v), n)).collect(), None));
            }
        }
    }
    out
}

/// Whether every corner has the very same coordinate on one axis. csgrs takes a polygon off its own plane by more than 1e-8 as
/// spanning it, and splits it by that plane again and again until the stack overflows, which aborts the process: no error can be
/// returned. A corner stored as f32 is only within a hundredth of a micron of a tilted plane, so only a patch on a plane square to an
/// axis of the object's own frame (a box's face, a cylinder's cap, however the object is turned: the turn is applied afterwards, in
/// f64) is merged; triangles, flat by definition, carry everything else. (Moving tilted
/// corners onto their plane was tried: it leaves them a few nanometres off their neighbours, and measured worse, 131 of 160 against 144.)
fn exactly_flat(m: &Mesh, ring: &[u32]) -> bool {
    (0..3).any(|k| {
        let first = m.vertices[ring[0] as usize][k];
        ring.iter().all(|&i| m.vertices[i as usize][k].to_bits() == first.to_bits())
    })
}

/// The outline of a patch of triangles as one loop, in their winding, or none when it has holes or touches itself.
fn outline(m: &Mesh, tris: &[usize]) -> Option<Vec<u32>> {
    let mut directed: HashSet<(u32, u32)> = HashSet::new();
    for &i in tris {
        let t = m.triangles[i];
        for k in 0..3 {
            directed.insert((t[k], t[(k + 1) % 3]));
        }
    }
    let mut next: HashMap<u32, u32> = HashMap::new();
    for &(a, b) in &directed {
        if !directed.contains(&(b, a)) && next.insert(a, b).is_some() {
            return None; // two ways on from one corner: the outline touches itself
        }
    }
    let &start = next.keys().min()?;
    let (mut ring, mut at) = (vec![start], *next.get(&start)?);
    while at != start {
        if ring.len() > next.len() {
            return None;
        }
        ring.push(at);
        at = *next.get(&at)?;
    }
    // Every outline edge must be on this one loop, or the patch has a hole.
    (ring.len() == next.len()).then_some(ring)
}

/// Vertices of a mesh being built, where a point within [`WELD`] of one already there is that one. A grid alone would split two
/// points that are a hair apart across a cell boundary, and the crack between them is the kind this module exists to close.
#[derive(Default)]
struct Welder {
    cells: HashMap<[i64; 3], Vec<u32>>,
    at: Vec<[f64; 3]>,
}

impl Welder {
    fn id(&mut self, p: [f64; 3]) -> u32 {
        let c = p.map(|x| (x / WELD).floor() as i64);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &i in self.cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]).into_iter().flatten() {
                        let q = self.at[i as usize];
                        if (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2) <= WELD * WELD {
                            return i;
                        }
                    }
                }
            }
        }
        self.at.push(p);
        self.cells.entry(c).or_default().push(self.at.len() as u32 - 1);
        self.at.len() as u32 - 1
    }
}

/// csgrs' polygons as a welded triangle mesh with its cracks closed.
fn from_csg(polys: &[Polygon<()>]) -> Result<Mesh, String> {
    let mut weld = Welder::default();
    let mut triangles = Vec::new();
    for poly in polys {
        for tri in poly.triangulate() {
            let t = tri.map(|v| weld.id([v.pos.x, v.pos.y, v.pos.z]));
            if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                triangles.push(t);
            }
        }
    }
    close(Mesh::new(weld.at.iter().map(|p| p.map(|c| c as f32)).collect(), triangles)?)
}

/// Repairs what the BSP leaves (`lamina_core::repair::close_with`): T-junctions split, skins and fins dropped, slivers snapped,
/// small holes filled, in passes until the mesh is closed or a pass changes nothing. A vertex within two welds of an edge is on it:
/// the weld moves a point by up to one. Five, as before, took the corners of a curved rim into the chord beside them, and the split
/// turned slivers over (folds, see [`to_csg`]): `chain_score` gave 51 clean chains of 120 with five, 73 with two.
fn close(m: Mesh) -> Result<Mesh, String> {
    repair::close_with(m, 4, 2.0 * WELD).map(|(m, _)| m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(m: Mesh, x: f64, y: f64, z: f64) -> Mesh {
        m.transformed(&Transform::translate([x, y, z]))
    }

    /// The area of a regular `n`-gon of radius `r`, which is what a cylinder of `n` segments really is.
    fn polygon_area(n: f64, r: f64) -> f64 {
        0.5 * n * r * r * (std::f64::consts::TAU / n).sin()
    }

    /// Welding moves vertices by up to a micron, so volumes agree to a part in ten thousand, not to the last digit.
    fn near(got: f64, want: f64) {
        assert!((got - want).abs() <= 1e-4 * want, "{got} is not {want}");
    }

    fn closed(m: &Mesh) {
        assert!(m.is_watertight(), "{} triangles, not closed", m.triangles.len());
    }

    #[test]
    fn a_hole_through_a_box_is_closed_and_has_the_right_volume() {
        let cube = Mesh::cuboid([20.0, 20.0, 10.0]);
        let tool = at(Mesh::cylinder(5.0, 14.0, 48), 0.0, 0.0, -2.0);
        let m = boolean(Op::Difference, &cube, &tool).unwrap();
        closed(&m);
        near(m.volume(), 4000.0 - polygon_area(48.0, 5.0) * 10.0);
    }

    #[test]
    fn a_tool_flush_with_the_faces_works_too() {
        let cube = Mesh::cuboid([20.0, 20.0, 10.0]);
        let m = boolean(Op::Difference, &cube, &Mesh::cylinder(5.0, 10.0, 48)).unwrap();
        closed(&m);
        near(m.volume(), 4000.0 - polygon_area(48.0, 5.0) * 10.0);
    }

    #[test]
    fn union_and_intersection_follow_inclusion_exclusion() {
        let a = Mesh::cuboid([20.0, 20.0, 10.0]);
        let b = at(Mesh::cuboid([10.0, 10.0, 10.0]), 8.0, 0.0, 3.0);
        let (u, i) = (boolean(Op::Union, &a, &b).unwrap(), boolean(Op::Intersect, &a, &b).unwrap());
        closed(&u);
        closed(&i);
        near(i.volume(), 490.0);
        near(u.volume(), 4510.0);
    }

    #[test]
    fn a_round_cut_and_a_swept_cut_stay_closed() {
        let a = Mesh::cuboid([20.0, 20.0, 10.0]);
        let ball = at(Mesh::sphere(8.0, 32), 0.0, 0.0, 5.0);
        let dented = boolean(Op::Difference, &a, &ball).unwrap();
        closed(&dented);
        // The ball (centre 13 mm up, radius 8) dips 5 mm into the top: a cap of pi * 5^2 * (3 * 8 - 5) / 3 = 497 mm3, a little less for 32 sides.
        assert!((dented.volume() - (4000.0 - 497.4)).abs() < 20.0, "{}", dented.volume());
        let slot = at(Mesh::cuboid([4.0, 30.0, 4.0]), 0.0, 0.0, 8.0);
        let grooved = boolean(Op::Difference, &a, &slot).unwrap();
        closed(&grooved);
        near(grooved.volume(), 4000.0 - 20.0 * 4.0 * 2.0);
    }

    #[test]
    fn a_mirrored_object_takes_part_in_a_boolean_the_right_way_out() {
        let cube = Mesh::cuboid([20.0, 20.0, 10.0]);
        // A peg at x = 6, mirrored across x: it cuts at x = -6.
        let peg = Mesh::cylinder(3.0, 14.0, 24);
        let mirrored = Transform::translate([6.0, 0.0, -2.0]).then(&Transform::scale([-1.0, 1.0, 1.0]));
        let m = boolean_placed(Op::Difference, (&cube, Transform::IDENTITY), (&peg, mirrored)).unwrap();
        closed(&m);
        near(m.volume(), 4000.0 - polygon_area(24.0, 3.0) * 10.0);
        let hole_x = m.vertices.iter().filter(|v| v[2] > 9.9 && (v[1].abs() < 0.01)).map(|v| f64::from(v[0])).fold(0.0, f64::min);
        assert!(hole_x < -8.0, "the hole is on the mirrored side: {hole_x}");
    }

    #[test]
    fn a_result_can_be_cut_again() {
        let a = Mesh::cuboid([30.0, 30.0, 10.0]);
        let once = boolean(Op::Difference, &a, &at(Mesh::cylinder(4.0, 14.0, 32), -8.0, 0.0, -2.0)).unwrap();
        let twice = boolean(Op::Difference, &once, &at(Mesh::cylinder(4.0, 14.0, 32), 8.0, 0.0, -2.0)).unwrap();
        closed(&twice);
        near(twice.volume(), 9000.0 - 2.0 * polygon_area(32.0, 4.0) * 10.0);
    }

    /// The cases BSP booleans find hard. Each of the first group must work and be right; the second may be refused, never wrong.
    #[test]
    fn awkward_cases_work_or_are_refused_never_wrong() {
        let cube = |s: f64| Mesh::cuboid([s, s, s]);
        let tilted = Mesh::cylinder(3.0, 30.0, 32).transformed(&Transform::rotate(0, 30.0));
        let must: Vec<(&str, Op, Mesh, Mesh, f64)> = vec![
            ("boxes sharing a face", Op::Union, cube(10.0), at(cube(10.0), 10.0, 0.0, 0.0), 2000.0),
            ("a box with itself", Op::Union, cube(10.0), cube(10.0), 1000.0),
            ("a box cut by itself", Op::Intersect, cube(10.0), cube(10.0), 1000.0),
            ("a box swallowing a box", Op::Union, cube(10.0), at(cube(4.0), 0.0, 0.0, 3.0), 1000.0),
            ("a cavity", Op::Difference, cube(10.0), at(cube(4.0), 0.0, 0.0, 3.0), 1000.0 - 64.0),
            ("half a box", Op::Difference, cube(10.0), at(cube(10.0), 5.0, 0.0, 0.0), 500.0),
        ];
        for (what, op, a, b, want) in must {
            let m = boolean(op, &a, &b).unwrap_or_else(|e| panic!("{what}: {e}"));
            closed(&m);
            near(m.volume(), want);
        }
        let may: Vec<(&str, Op, Mesh, Mesh)> = vec![
            ("boxes meeting at an edge", Op::Union, cube(10.0), at(cube(10.0), 10.0, 10.0, 0.0)),
            ("two balls", Op::Union, Mesh::sphere(6.0, 32), at(Mesh::sphere(6.0, 32), 5.0, 0.0, 0.0)),
            ("a cut at 30 degrees", Op::Difference, Mesh::cuboid([20.0, 20.0, 20.0]), at(tilted, 0.0, 0.0, 10.0)),
        ];
        for (what, op, a, b) in may {
            match boolean(op, &a, &b) {
                Ok(m) => closed(&m),
                Err(e) => assert!(e.contains("hundredth of a millimetre"), "{what}: a refusal says what to try: {e}"),
            }
        }
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (hi - lo) * self.next()
        }
        fn pick<T: Copy>(&mut self, v: &[T]) -> T {
            v[(self.next() * v.len() as f64) as usize % v.len()]
        }
    }

    /// A box or an upright cylinder (the shapes a block with holes and bosses is made of), at round or random positions.
    fn prism(r: &mut Rng) -> Mesh {
        let m = if r.next() < 0.5 {
            Mesh::cuboid([r.range(5.0, 25.0), r.range(5.0, 25.0), r.range(5.0, 25.0)])
        } else {
            Mesh::cylinder(r.range(2.0, 8.0), r.range(5.0, 25.0), r.pick(&[16, 24, 32, 48]))
        };
        let (x, y, z) = (r.range(-8.0, 8.0), r.range(-8.0, 8.0), r.range(-4.0, 10.0));
        at(m, r.pick(&[0.0, 2.0, 2.5, 5.0, x]), r.pick(&[0.0, 3.0, y]), r.pick(&[0.0, 4.0, z]))
    }

    /// Measured: box with box, box with cylinder and cylinder with cylinder, 100 percent upright. The floor is 95, and every result that
    /// is returned has already passed `sound`, so what fails is refused, never wrong.
    #[test]
    fn boxes_and_upright_cylinders_almost_always_work() {
        let mut r = Rng(0x2545_F491_4F6C_DD1D);
        let (mut worked, n) = (0, 120);
        for _ in 0..n {
            let (a, b) = (prism(&mut r), prism(&mut r));
            let op = r.pick(&[Op::Union, Op::Difference, Op::Intersect]);
            if let Ok(m) = boolean(op, &a, &b) {
                // An empty result (nothing shared, nothing left) is a result.
                if !m.triangles.is_empty() {
                    closed(&m);
                }
                worked += 1;
            }
        }
        assert!(worked * 100 >= 95 * n, "only {worked} of {n} worked");
    }

    /// Upright curved pairs (spheres, cylinders, boxes): measured 199 of 200 clean after the repair, the geodesic sphere and flat-face
    /// merging; the floor here is 90 percent, so a change that undoes one of those fails this test, not a user's print.
    #[test]
    fn upright_curved_shapes_almost_always_work() {
        let mut r = Rng(0x5DEE_CE66_D1CE_4E5B);
        let (mut worked, n) = (0, 40);
        for i in 0..n {
            let shape = |r: &mut Rng, k: u32| match k % 3 {
                0 => Mesh::sphere(r.range(3.0, 9.0), r.pick(&[16, 24, 32])),
                1 => Mesh::cylinder(r.range(2.0, 8.0), r.range(5.0, 25.0), r.pick(&[16, 24, 32])),
                _ => Mesh::cuboid([r.range(5.0, 25.0), r.range(5.0, 25.0), r.range(5.0, 25.0)]),
            };
            let (a, b) = (shape(&mut r, i), shape(&mut r, i / 3 + 1));
            let (x, y) = (r.range(-6.0, 6.0), r.range(-6.0, 6.0));
            let b = at(b, r.pick(&[0.0, 2.5, x]), r.pick(&[0.0, 3.0, y]), r.pick(&[0.0, 4.0]));
            let op = r.pick(&[Op::Union, Op::Difference, Op::Intersect]);
            if let Ok(m) = boolean(op, &a, &b) {
                if !m.triangles.is_empty() {
                    closed(&m);
                }
                worked += 1;
            }
        }
        assert!(worked * 100 >= 90 * n, "only {worked} of {n} worked");
    }

    #[test]
    fn what_cannot_be_done_is_an_error() {
        let big = Mesh::sphere(10.0, 200);
        assert!(boolean(Op::Union, &big, &big).unwrap_err().contains("fewer segments"));
    }
}

#[cfg(test)]
/// The measurement that decided how far csgrs can be trusted, kept so a change (or another engine behind `boolean`) can be compared:
/// `cargo test -p lamina-csg --release -- --ignored success_matrix --nocapture`. About ten minutes in a debug build.
mod measure {
    use super::*;
    use std::collections::BTreeMap;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (hi - lo) * self.next()
        }
        fn pick<T: Copy>(&mut self, v: &[T]) -> T {
            v[(self.next() * v.len() as f64) as usize % v.len()]
        }
    }

    /// A random shape: its mesh in its own frame, the transform that places it (turned when `rotate`), and its kind.
    fn kind_shape(r: &mut Rng, rotate: bool) -> (Mesh, Transform, char) {
        let (m, k) = match (r.next() * 3.0) as u32 {
            0 => (Mesh::cuboid([r.range(5.0, 25.0), r.range(5.0, 25.0), r.range(5.0, 25.0)]), 'B'),
            1 => (Mesh::cylinder(r.range(2.0, 8.0), r.range(5.0, 25.0), r.pick(&[16, 24, 32, 48])), 'C'),
            _ => (Mesh::sphere(r.range(3.0, 9.0), r.pick(&[16, 24, 32])), 'S'),
        };
        let turn = if rotate { Transform::rotate(r.pick(&[0, 1, 2]), r.pick(&[30.0, 45.0, 90.0])) } else { Transform::IDENTITY };
        let (x, y, z) = (r.range(-8.0, 8.0), r.range(-8.0, 8.0), r.range(-4.0, 10.0));
        let t = [r.pick(&[0.0, 2.0, 2.5, 5.0, x]), r.pick(&[0.0, 3.0, y]), r.pick(&[0.0, 4.0, z])];
        (m, turn.then(&Transform::translate(t)), k)
    }

    /// Curved pairs only (the weak cases), union and difference, upright and tilted: one number to compare a change by.
    /// `cargo test -p lamina-csg --release -- --ignored curved_score --nocapture`
    #[test]
    #[ignore = "slow: run by hand when changing the engine or the repair"]
    fn curved_score() {
        let mut r = Rng(0xC0FF_EE00_D15E_A5E5);
        let (mut n, mut ok) = (0, 0);
        let mut why: BTreeMap<String, u32> = BTreeMap::new();
        while n < 160 {
            let rotate = n % 2 == 1;
            let ((a, xa, ka), (b, xb, kb)) = (kind_shape(&mut r, rotate), kind_shape(&mut r, rotate));
            if ka == 'B' && kb == 'B' {
                continue;
            }
            if (ka == 'B' || kb == 'B') && ka != 'S' && kb != 'S' {
                continue; // box with cylinder is already right
            }
            let op = r.pick(&[Op::Union, Op::Difference]);
            n += 1;
            match boolean_placed(op, (&a, xa), (&b, xb)) {
                Ok(_) => ok += 1,
                Err(e) => {
                    let detail =
                        engine(op, (&a, xa), (&b, xb)).map(|m| shape_of_failure(&m)).unwrap_or_else(|e| e.chars().take(40).collect());
                    let reason = e.split('(').nth(1).and_then(|x| x.split(')').next()).unwrap_or("other").to_string();
                    *why.entry(format!("{reason}: {detail}")).or_default() += 1;
                }
            }
        }
        println!("curved score: {ok} of {n}");
        for (k, c) in why.iter().rev().take(25) {
            println!("  {c:3} x {k}");
        }
    }

    /// A short label for what is wrong with a mesh the gate refused: open edges in loops of what size, and edges with three or more faces.
    fn shape_of_failure(m: &Mesh) -> String {
        let mut directed: std::collections::HashSet<(u32, u32)> = Default::default();
        let mut uses: std::collections::HashMap<(u32, u32), u32> = Default::default();
        for t in &m.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                directed.insert((a, b));
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let open = uses.values().filter(|&&c| c == 1).count();
        let fat = uses.values().filter(|&&c| c > 2).count();
        let flipped = uses
            .iter()
            .filter(|&(&(a, b), &c)| c == 2 && directed.contains(&(a, b)) == directed.contains(&(b, a)) && !directed.contains(&(b, a)))
            .count();
        let bucket = |x: usize| match x {
            0 => "0",
            1..=4 => "1-4",
            5..=16 => "5-16",
            17..=64 => "17-64",
            _ => "65+",
        };
        // Triangles on the same three vertices: the same way round (a doubled sheet) or opposite (a skin of no thickness).
        let mut sets: std::collections::HashMap<[u32; 3], Vec<bool>> = Default::default();
        for t in &m.triangles {
            let mut k = *t;
            k.sort_unstable();
            let even = (0..3).any(|r| [t[r], t[(r + 1) % 3], t[(r + 2) % 3]] == k);
            sets.entry(k).or_default().push(even);
        }
        let same = sets.values().filter(|v| v.len() > 1 && v.iter().all(|&e| e == v[0])).count();
        let opposite = sets.values().filter(|v| v.len() > 1 && v.iter().any(|&e| e != v[0])).count();
        let tiny_fat = uses
            .iter()
            .filter(|&(_, &c)| c > 2)
            .filter(|&(&(a, b), _)| {
                m.triangles.iter().enumerate().any(|(i, t)| t.contains(&a) && t.contains(&b) && repair::triangle_area(m, i) < 0.01)
            })
            .count();
        // The holes: loops of open edges, how many, the longest, whether an outline branches, and the area a fan over each would cover.
        let mut next: std::collections::HashMap<u32, Vec<u32>> = Default::default();
        for &(a, b) in &directed {
            if !directed.contains(&(b, a)) {
                next.entry(a).or_default().push(b);
            }
        }
        let branching = next.values().filter(|v| v.len() > 1).count();
        let (mut loops, mut longest, mut biggest_area, mut worst_flat, mut convex) = (0, 0, 0.0f64, 0.0f64, false);
        let mut seen: std::collections::HashSet<(u32, u32)> = Default::default();
        let p = |i: u32| m.vertices[i as usize].map(f64::from);
        for (&a, bs) in &next {
            for &b in bs {
                if seen.contains(&(a, b)) {
                    continue;
                }
                let (mut ring, mut at) = (vec![a], b);
                seen.insert((a, b));
                while at != a && ring.len() < 5000 {
                    ring.push(at);
                    let Some(&n) = next.get(&at).and_then(|v| v.first()) else { break };
                    seen.insert((at, n));
                    at = n;
                }
                loops += 1;
                longest = longest.max(ring.len());
                let o = p(ring[0]);
                let mut area = [0.0; 3];
                for k in 1..ring.len().saturating_sub(1) {
                    let (u, v) = (p(ring[k]), p(ring[k + 1]));
                    let (u, v) = ([u[0] - o[0], u[1] - o[1], u[2] - o[2]], [v[0] - o[0], v[1] - o[1], v[2] - o[2]]);
                    area[0] += u[1] * v[2] - u[2] * v[1];
                    area[1] += u[2] * v[0] - u[0] * v[2];
                    area[2] += u[0] * v[1] - u[1] * v[0];
                }
                let a2 = 0.5 * (area[0] * area[0] + area[1] * area[1] + area[2] * area[2]).sqrt();
                if a2 > biggest_area {
                    biggest_area = a2;
                    let n = area.map(|c| c / (2.0 * a2).max(1e-12));
                    let d: Vec<f64> = ring
                        .iter()
                        .map(|&i| {
                            let q = p(i);
                            n[0] * q[0] + n[1] * q[1] + n[2] * q[2]
                        })
                        .collect();
                    let mean = d.iter().sum::<f64>() / d.len() as f64;
                    worst_flat = d.iter().map(|x| (x - mean).abs()).fold(0.0, f64::max);
                    convex = repair::flat_and_convex(m, &ring, 5.0 * WELD);
                }
            }
        }
        let holes = if loops == 0 {
            String::new()
        } else {
            format!(
                " holes {loops} longest {longest} branching {branching} biggest {biggest_area:.3}mm2 off-flat {worst_flat:.3}mm flat-convex {convex}"
            )
        };
        format!(
            "{holes} open {} fat {} (tiny face {}) dup-same {} dup-opposite {} wrong-way {}",
            bucket(open),
            bucket(fat),
            bucket(tiny_fat),
            bucket(same),
            bucket(opposite),
            bucket(flipped)
        )
    }

    #[test]
    #[ignore = "slow: 800 booleans, run by hand to compare engines"]
    fn success_matrix() {
        for rotate in [false, true] {
            let mut r = Rng(0x1234_5678_9ABC_DEF1);
            let mut table: BTreeMap<String, (u32, u32)> = BTreeMap::new();
            let (mut all, mut ok_all) = (0, 0);
            for _ in 0..200 {
                let ((a, xa, ka), (b, xb, kb)) = (kind_shape(&mut r, rotate), kind_shape(&mut r, rotate));
                let op = r.pick(&[Op::Union, Op::Difference, Op::Intersect]);
                let mut key = [ka, kb];
                key.sort_unstable();
                let e = table.entry(format!("{}{} {:?}", key[0], key[1], op)).or_default();
                e.0 += 1;
                all += 1;
                if boolean_placed(op, (&a, xa), (&b, xb)).is_ok() {
                    e.1 += 1;
                    ok_all += 1;
                }
            }
            println!("rotated {rotate}: {ok_all} of {all} give a clean solid");
            for (k, (n, good)) in &table {
                println!("  {k:18} n {n:3} ok {good:3} ({:3.0}%)", 100.0 * f64::from(*good) / f64::from(*n));
            }
        }
    }

    /// A mesh as the commands put it on the plate: its box's middle at (`x`, `y`), its bottom at `z`.
    fn placed(m: Mesh, x: f64, y: f64, z: f64) -> (Mesh, Transform) {
        let Some((lo, hi)) = m.bounds() else { return (m, Transform::IDENTITY) };
        let xf = Transform::translate([x - (lo[0] + hi[0]) / 2.0, y - (lo[1] + hi[1]) / 2.0, z - lo[2]]);
        (m, xf)
    }

    /// A part's own shape: a box, an upright cylinder, a rounded plate (with a round hole, sometimes) or a word, `h` mm tall.
    fn chain_shape(r: &mut Rng, size: f64, h: f64) -> (Mesh, char) {
        use lamina_core::sketch;
        let (w, d) = (r.range(0.3, 1.0) * size, r.range(0.3, 1.0) * size);
        match (r.next() * 4.0) as u32 {
            0 => (Mesh::cuboid([w, d, h]), 'B'),
            1 => (Mesh::cylinder(w.min(d) / 2.0, h, r.pick(&[16, 24, 32, 48])), 'C'),
            2 => {
                let outline = sketch::rounded_rect(w, d, r.range(0.0, 0.45) * w.min(d), 8);
                let holes = if r.next() < 0.3 { vec![sketch::circle(0.0, 0.0, w.min(d) / 5.0, 32)] } else { vec![] };
                (sketch::extrude(&outline, &holes, h).unwrap_or_else(|_| Mesh::cuboid([w, d, h])), 'E')
            }
            _ => {
                let word = r.pick(&["LAMINA", "Thor", "ÅB 12", "rune", "x7", "MOULD"]);
                let c = lamina_core::text::contours(word, (0.3 * size).clamp(3.0, 8.0), 1.0).unwrap_or_default();
                (sketch::extrude_contours(&c, h).unwrap_or_else(|_| Mesh::cuboid([w, d, h])), 'T')
            }
        }
    }

    /// Chains of 4 to 6 booleans, each taking the last one's result: a part cut with pockets, holes and lettering and given bosses,
    /// as a design is built, the tools at round or random positions. A step is clean when it gives a closed solid whose volume agrees
    /// with inclusion and exclusion (checked against an intersection of the same two shapes), so a closed but wrong result counts
    /// as a failure too. `cargo test -p lamina-csg --release -- --ignored chain_score --nocapture`
    #[test]
    #[ignore = "slow: run by hand when changing the engine or the repair"]
    fn chain_score() {
        let mut r = Rng(0x0BAD_5EED_CAFE_F00D);
        let (chains, mut clean, mut steps, mut good) = (120, 0, 0, 0);
        let mut why: BTreeMap<String, u32> = BTreeMap::new();
        for _ in 0..chains {
            let h = r.range(6.0, 14.0);
            let (m, xf) = {
                let (m, _) = chain_shape(&mut r, 60.0, h);
                placed(m, 0.0, 0.0, 0.0)
            };
            let mut part = m.transformed(&xf);
            let mut ok = true;
            for n in 0..r.pick(&[4, 5, 6]) {
                let Some((lo, hi)) = part.bounds() else { break };
                let op = r.pick(&[Op::Difference, Op::Difference, Op::Union]);
                // Holes go through; pockets and lettering sink into the top; bosses stand on it, overlapping by a hair or more.
                let through = op == Op::Difference && r.next() < 0.3;
                let th = if through { hi[2] - lo[2] + 2.0 } else { r.range(1.0, 8.0) };
                let (tool, kind) = chain_shape(&mut r, 20.0, th);
                let (x, y) = (r.range(lo[0], hi[0]), r.range(lo[1], hi[1]));
                let (x, y) = (r.pick(&[x, x.round(), (2.0 * x).round() / 2.0]), r.pick(&[y, y.round()]));
                let z = match op {
                    _ if through => lo[2] - 1.0,
                    Op::Difference => hi[2] - r.pick(&[0.5, 0.7, 1.0, th * 0.6]),
                    _ => hi[2] - r.pick(&[0.05, 0.3, 1.0]),
                };
                let (tool, txf) = placed(tool, x, y, z);
                steps += 1;
                let step = boolean_placed(op, (&part, Transform::IDENTITY), (&tool, txf));
                let both = boolean_placed(Op::Intersect, (&part, Transform::IDENTITY), (&tool, txf));
                let (va, vb) = (part.volume(), tool.transformed(&txf).volume());
                let verdict = match (&step, &both) {
                    (Err(e), _) => {
                        Err(format!("{kind} {op:?} refused: {}", e.split('(').nth(1).and_then(|x| x.split(')').next()).unwrap_or("other")))
                    }
                    (Ok(_), Err(_)) => Err("intersection refused, so unchecked".to_string()),
                    (Ok(m), Ok(i)) => {
                        let want = if op == Op::Union { va + vb - i.volume() } else { va - i.volume() };
                        if (m.volume() - want).abs() <= 1e-4 * (va + vb) + 0.01 {
                            Ok(())
                        } else {
                            Err(format!("{kind} {op:?} closed but wrong"))
                        }
                    }
                };
                match verdict {
                    Ok(()) => good += 1,
                    Err(e) => {
                        *why.entry(format!("step {}: {e}", n + 1)).or_default() += 1;
                        ok = false;
                        break;
                    }
                }
                part = step.unwrap_or_default();
            }
            clean += usize::from(ok);
        }
        println!("chain score: {clean} of {chains} chains clean, {good} of {steps} steps");
        for (k, c) in &why {
            println!("  {c:3} x {k}");
        }
    }
}
