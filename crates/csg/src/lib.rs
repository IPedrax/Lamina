//! Mesh booleans for Lamina. This is the only crate that knows csgrs (a pure-Rust BSP library): the rest of the app calls [`boolean`]
//! with meshes and gets a mesh, so another engine can replace it without touching a command.
//!
//! What csgrs gives back is right in volume but not closed: splitting polygons leaves T-junctions, where a vertex of one face sits
//! in the middle of its neighbour's edge. A slicer and `Mesh::is_watertight` both read topology, so [`close`] welds the vertices and
//! splits triangles at those vertices until every edge is shared by two faces.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod commands;

use std::collections::{HashMap, HashSet};

use csgrs::mesh::{bsp::Node, polygon::Polygon, vertex::Vertex};
use lamina_core::Mesh;
use nalgebra::{Point3, Vector3};

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

/// `a` combined with `b`, or an error saying why not. A result is returned only if it is a closed solid with a volume and bounds that
/// are possible for these shapes ([`sound`]): the BSP can leave cracks or lose faces where curved surfaces cut each other, and a
/// broken mesh in the scene would slice into a broken print.
pub fn boolean(op: Op, a: &Mesh, b: &Mesh) -> Result<Mesh, String> {
    for (name, m) in [("first", a), ("second", b)] {
        if !m.is_watertight() {
            return Err(format!("the {name} shape is not a closed mesh, so a boolean cannot tell its inside from its outside"));
        }
    }
    let total = a.triangles.len() + b.triangles.len();
    if total > MAX_INPUT_TRIANGLES {
        return Err(format!("{total} triangles is more than the {MAX_INPUT_TRIANGLES} one boolean takes: use fewer segments"));
    }
    let (pa, pb) = (to_csg(a), to_csg(b));
    let run = move || from_csg(&combine(op, &pa, &pb));
    let result = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(run)
        .map_err(|e| format!("could not start the boolean: {e}"))?
        .join()
        .map_err(|_| HARD.to_string())??;
    sound(op, a, b, &result).map_err(|why| format!("the boolean did not give a clean solid ({why}). {HARD}"))?;
    Ok(result)
}

/// What the error says to do, because the model or the user reading it has to act on it.
const HARD: &str = "Curved shapes that cut through each other (spheres, crossing cylinders, tilted cylinders) are the hard cases for this engine; \
                    boxes and upright cylinders work best. Change a size or a position by a hundredth of a millimetre, use fewer segments, or \
                    use another shape, and try again";

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
    // The welds move a vertex by a micron; a hundredth of a millimetre is a real difference.
    let tol = 0.01;
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
/// against: a union of nested boxes keeps both, and a box cut by a bigger box comes back whole. Using its `Node` directly has no such gap.
fn combine(op: Op, a: &[Polygon<()>], b: &[Polygon<()>]) -> Vec<Polygon<()>> {
    let (mut a, mut b) = (Node::from_polygons(a), Node::from_polygons(b));
    match op {
        Op::Union => {
            a.clip_to(&b);
            b.clip_to(&a);
            b.invert();
            b.clip_to(&a);
            b.invert();
            a.build(&b.all_polygons());
        }
        Op::Difference => {
            a.invert();
            a.clip_to(&b);
            b.clip_to(&a);
            b.invert();
            b.clip_to(&a);
            b.invert();
            a.build(&b.all_polygons());
            a.invert();
        }
        Op::Intersect => {
            a.invert();
            b.clip_to(&a);
            b.invert();
            a.clip_to(&b);
            b.clip_to(&a);
            a.build(&b.all_polygons());
            a.invert();
        }
    }
    a.all_polygons()
}

fn to_csg(m: &Mesh) -> Vec<Polygon<()>> {
    (0..m.triangles.len())
        .filter_map(|i| m.tri(i))
        .filter_map(|t| {
            let u = Vector3::new(t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]);
            let v = Vector3::new(t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]);
            let n = u.cross(&v);
            // A flat triangle has no normal and no volume to give; leave it out.
            (n.norm() > 1e-12).then(|| {
                let n = n.normalize();
                Polygon::new(t.iter().map(|p| Vertex::new(Point3::new(p[0], p[1], p[2]), n)).collect(), None)
            })
        })
        .collect()
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

/// Repairs what the BSP leaves: triangles are split at the vertices that lie on their edges (T-junctions), thin fins on an edge three or
/// more faces share are dropped, and small holes where the triangulator lost a triangle are filled. Each pass can expose the next, so it
/// repeats until the mesh is closed or a pass changes nothing.
fn close(m: Mesh) -> Result<Mesh, String> {
    close_with(m, 4, 5.0 * WELD)
}

fn close_with(mut m: Mesh, passes: usize, tol: f64) -> Result<Mesh, String> {
    for _ in 0..passes {
        if m.is_watertight() {
            break;
        }
        let before = (m.triangles.len(), m.vertices.len());
        split_t_junctions(&mut m, tol);
        drop_fins(&mut m);
        fill_small_holes(&mut m);
        if (m.triangles.len(), m.vertices.len()) == before {
            break;
        }
    }
    Mesh::new(m.vertices, m.triangles)
}

fn undirected(m: &Mesh) -> HashMap<(u32, u32), u32> {
    let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
    for t in &m.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *uses.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    uses
}

fn split_t_junctions(m: &mut Mesh, tol: f64) {
    let open: Vec<(u32, u32)> = undirected(m).into_iter().filter(|&(_, n)| n == 1).map(|(e, _)| e).collect();
    if open.is_empty() {
        return;
    }
    // Only vertices on an open edge can be the T of a T-junction, which keeps the search small.
    let touching: HashSet<u32> = open.iter().flat_map(|&(a, b)| [a, b]).collect();
    let pos: Vec<[f64; 3]> = m.vertices.iter().map(|v| v.map(f64::from)).collect();
    let at = |i: u32| pos[i as usize];
    let mut splits: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for &(a, b) in &open {
        let (pa, pb) = (at(a), at(b));
        let mut on: Vec<(f64, u32)> =
            touching.iter().filter(|&&p| p != a && p != b).filter_map(|&p| along(pa, pb, at(p), tol).map(|t| (t, p))).collect();
        on.sort_by(|x, y| x.0.total_cmp(&y.0));
        if !on.is_empty() {
            splits.insert((a, b), on.into_iter().map(|(_, p)| p).collect());
        }
    }
    if splits.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(m.triangles.len());
    for t in &m.triangles {
        // The triangle's outline with each split edge's vertices put in, in its own direction.
        let mut ring = Vec::new();
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            ring.push(a);
            if let Some(on) = splits.get(&(a.min(b), a.max(b))) {
                if a < b { ring.extend(on) } else { ring.extend(on.iter().rev()) }
            }
        }
        if ring.len() == 3 {
            out.push(*t);
            continue;
        }
        // A new vertex in the middle joins each stretch of the outline, so no sliver of zero width is made.
        let centre = m.vertices.len() as u32;
        let mid = t.map(at).iter().fold([0.0; 3], |s, p| [s[0] + p[0] / 3.0, s[1] + p[1] / 3.0, s[2] + p[2] / 3.0]);
        m.vertices.push(mid.map(|c| c as f32));
        for k in 0..ring.len() {
            out.push([centre, ring[k], ring[(k + 1) % ring.len()]]);
        }
    }
    m.triangles = out;
}

/// A triangle this small (mm2) on an edge that three or more faces share is a fin the BSP left, not a surface.
const FIN: f64 = 1e-4;

fn drop_fins(m: &mut Mesh) {
    let uses = undirected(m);
    if uses.values().all(|&n| n <= 2) {
        return;
    }
    let keep: Vec<bool> = (0..m.triangles.len())
        .map(|i| {
            let t = m.triangles[i];
            let shared = (0..3).any(|k| uses.get(&(t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3]))).is_some_and(|&n| n > 2));
            !(shared && triangle_area(m, i) < FIN)
        })
        .collect();
    let mut keep = keep.into_iter();
    m.triangles.retain(|_| keep.next().unwrap_or(true));
}

fn triangle_area(m: &Mesh, i: usize) -> f64 {
    let Some(t) = m.tri(i) else { return 0.0 };
    let (u, v) = ([t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]], [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]]);
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()
}

/// The most edges of a hole that is filled. A bigger gap is a real one, and filling it would invent a surface.
const SMALL_HOLE: usize = 16;

/// Fills each loop of open edges of at most [`SMALL_HOLE`] edges with a fan, wound against the faces around it.
fn fill_small_holes(m: &mut Mesh) {
    let mut directed: HashSet<(u32, u32)> = HashSet::new();
    for t in &m.triangles {
        for k in 0..3 {
            directed.insert((t[k], t[(k + 1) % 3]));
        }
    }
    // An open edge is one whose reverse no face walks.
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(a, b) in &directed {
        if !directed.contains(&(b, a)) {
            next.entry(a).or_default().push(b);
        }
    }
    let mut used: HashSet<(u32, u32)> = HashSet::new();
    let mut fill = Vec::new();
    let starts: Vec<(u32, u32)> = next.iter().flat_map(|(&a, bs)| bs.iter().map(move |&b| (a, b))).collect();
    for (a, b) in starts {
        if used.contains(&(a, b)) {
            continue;
        }
        let (mut ring, mut at) = (vec![a], b);
        let closed = loop {
            if ring.len() > SMALL_HOLE {
                break false;
            }
            if at == a {
                break true;
            }
            ring.push(at);
            match next.get(&at).map(Vec::as_slice) {
                Some([only]) => at = *only,
                _ => break false, // the outline branches here: not a simple hole
            }
        };
        for k in 0..ring.len() {
            used.insert((ring[k], ring[(k + 1) % ring.len()]));
        }
        if closed && ring.len() >= 3 {
            for k in 1..ring.len() - 1 {
                fill.push([ring[0], ring[k + 1], ring[k]]);
            }
        }
    }
    m.triangles.extend(fill);
}

/// How far along `a` to `b` the point `p` lies (0 to 1, exclusive) if it is on that segment within five times [`WELD`].
fn along(a: [f64; 3], b: [f64; 3], p: [f64; 3], tol: f64) -> Option<f64> {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    if len2 < 1e-18 {
        return None;
    }
    let t = ((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1] + (p[2] - a[2]) * ab[2]) / len2;
    if t <= 0.0 || t >= 1.0 {
        return None;
    }
    let near = [a[0] + t * ab[0] - p[0], a[1] + t * ab[1] - p[1], a[2] + t * ab[2] - p[2]];
    ((near[0] * near[0] + near[1] * near[1] + near[2] * near[2]).sqrt() <= tol).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lamina_core::Transform;

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

    /// Measured: box with box and box with cylinder, 100 percent; cylinder with cylinder, 73 to 100. The floor is 90, and every result that
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
    use lamina_core::Transform;
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

    fn kind_shape(r: &mut Rng, rotate: bool) -> (Mesh, char) {
        let (m, k) = match (r.next() * 3.0) as u32 {
            0 => (Mesh::cuboid([r.range(5.0, 25.0), r.range(5.0, 25.0), r.range(5.0, 25.0)]), 'B'),
            1 => (Mesh::cylinder(r.range(2.0, 8.0), r.range(5.0, 25.0), r.pick(&[16, 24, 32, 48])), 'C'),
            _ => (Mesh::sphere(r.range(3.0, 9.0), r.pick(&[16, 24, 32])), 'S'),
        };
        let m = if rotate { m.transformed(&Transform::rotate(r.pick(&[0, 1, 2]), r.pick(&[30.0, 45.0, 90.0]))) } else { m };
        let (x, y, z) = (r.range(-8.0, 8.0), r.range(-8.0, 8.0), r.range(-4.0, 10.0));
        let t = [r.pick(&[0.0, 2.0, 2.5, 5.0, x]), r.pick(&[0.0, 3.0, y]), r.pick(&[0.0, 4.0, z])];
        (m.transformed(&Transform::translate(t)), k)
    }

    #[test]
    #[ignore = "slow: 800 booleans, run by hand to compare engines"]
    fn success_matrix() {
        for rotate in [false, true] {
            let mut r = Rng(0x1234_5678_9ABC_DEF1);
            let mut table: BTreeMap<String, (u32, u32)> = BTreeMap::new();
            let (mut all, mut ok_all) = (0, 0);
            for _ in 0..200 {
                let ((a, ka), (b, kb)) = (kind_shape(&mut r, rotate), kind_shape(&mut r, rotate));
                let op = r.pick(&[Op::Union, Op::Difference, Op::Intersect]);
                let mut key = [ka, kb];
                key.sort_unstable();
                let e = table.entry(format!("{}{} {:?}", key[0], key[1], op)).or_default();
                e.0 += 1;
                all += 1;
                if boolean(op, &a, &b).is_ok() {
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
}
