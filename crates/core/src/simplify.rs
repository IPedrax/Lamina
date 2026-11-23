//! Fewer triangles for a heavy mesh (a scan of millions, say), keeping its shape: quadric edge collapse, written from Garland and
//! Heckbert's public paper ("Surface Simplification Using Quadric Error Metrics", 1997). Each vertex carries the sum of the squared
//! distances to the planes of the faces round it, as a 4 by 4 quadric; collapsing an edge puts the two ends at the point that
//! makes their summed quadric least, and the edges are collapsed cheapest first until the mesh has the triangles asked for.
//!
//! Two checks keep the result a sound surface, which the paper leaves to the implementer: an edge is collapsed only when its ends
//! share no neighbour but the corners of the faces on that edge (the link condition, so the surface stays manifold), and only when no
//! face round it would turn over or shrink to nothing. An open border is held in place by a plane square to each border face, weighted
//! heavily, so holes do not grow. A closed mesh stays closed.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::mesh::{MAX_TRIANGLES, Mesh};

type V3 = [f64; 3];

/// A symmetric 4 by 4 quadric, its ten distinct entries: a a, a b, a c, a d, b b, b c, b d, c c, c d, d d.
#[derive(Clone, Copy, Debug, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    /// The squared distance to the plane a x + b y + c z + d = 0 (a, b, c a unit normal), times `w`.
    fn plane(n: V3, d: f64, w: f64) -> Quadric {
        let [a, b, c] = n;
        Quadric([a * a, a * b, a * c, a * d, b * b, b * c, b * d, c * c, c * d, d * d].map(|x| x * w))
    }

    fn add(&mut self, o: &Quadric) {
        for k in 0..10 {
            self.0[k] += o.0[k];
        }
    }

    fn sum(a: &Quadric, b: &Quadric) -> Quadric {
        let mut q = *a;
        q.add(b);
        q
    }

    /// The error at `p`: p^T Q p.
    fn error(&self, p: V3) -> f64 {
        let [aa, ab, ac, ad, bb, bc, bd, cc, cd, dd] = self.0;
        let [x, y, z] = p;
        aa * x * x
            + 2.0 * ab * x * y
            + 2.0 * ac * x * z
            + 2.0 * ad * x
            + bb * y * y
            + 2.0 * bc * y * z
            + 2.0 * bd * y
            + cc * z * z
            + 2.0 * cd * z
            + dd
    }

    /// The point where the error is least, when the system is well conditioned.
    fn best(&self) -> Option<V3> {
        let [aa, ab, ac, ad, bb, bc, bd, cc, cd, _] = self.0;
        let det = aa * (bb * cc - bc * bc) - ab * (ab * cc - bc * ac) + ac * (ab * bc - bb * ac);
        let scale = (aa.abs() + bb.abs() + cc.abs()).max(1e-30);
        if det.abs() < 1e-9 * scale * scale * scale {
            return None;
        }
        // Cramer's rule on [[aa ab ac] [ab bb bc] [ac bc cc]] p = -[ad bd cd].
        let (r0, r1, r2) = (-ad, -bd, -cd);
        let x = (r0 * (bb * cc - bc * bc) - ab * (r1 * cc - bc * r2) + ac * (r1 * bc - bb * r2)) / det;
        let y = (aa * (r1 * cc - bc * r2) - r0 * (ab * cc - bc * ac) + ac * (ab * r2 - r1 * ac)) / det;
        let z = (aa * (bb * r2 - r1 * bc) - ab * (ab * r2 - r1 * ac) + r0 * (ab * bc - bb * ac)) / det;
        [x, y, z].iter().all(|c| c.is_finite()).then_some([x, y, z])
    }
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// A candidate collapse in the queue: cheapest first. `stamp` is the two ends' versions when it was made; a collapse made
/// before either end changed is stale.
struct Candidate {
    cost: f64,
    a: u32,
    b: u32,
    at: V3,
    stamp: (u32, u32),
}

impl PartialEq for Candidate {
    fn eq(&self, o: &Self) -> bool {
        self.cost.total_cmp(&o.cost) == Ordering::Equal
    }
}
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Candidate {
    /// Reversed, so the heap gives the cheapest.
    fn cmp(&self, o: &Self) -> Ordering {
        o.cost.total_cmp(&self.cost)
    }
}

struct Work {
    pos: Vec<V3>,
    q: Vec<Quadric>,
    faces: Vec<[u32; 3]>,
    alive: Vec<bool>,
    /// The faces round each vertex (some dead), and each vertex's version, bumped when it changes.
    round: Vec<Vec<u32>>,
    version: Vec<u32>,
    gone: Vec<bool>,
}

impl Work {
    fn neighbours(&self, v: u32) -> Vec<u32> {
        let mut out: Vec<u32> = self.round[v as usize]
            .iter()
            .filter(|&&f| self.alive[f as usize])
            .flat_map(|&f| self.faces[f as usize])
            .filter(|&w| w != v)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The cheapest place for the collapse of `a` and `b`, and its error.
    fn candidate(&self, a: u32, b: u32) -> Candidate {
        let q = Quadric::sum(&self.q[a as usize], &self.q[b as usize]);
        let (pa, pb) = (self.pos[a as usize], self.pos[b as usize]);
        let mid = [(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0, (pa[2] + pb[2]) / 2.0];
        // The best point, unless it runs far from the edge (a flat region's system is near singular): then the best of the ends and
        // the middle.
        let reach = len(sub(pa, pb)).max(1e-9);
        let best = q.best().filter(|p| len(sub(*p, mid)) <= 2.0 * reach);
        let at = best.unwrap_or_else(|| [pa, pb, mid].into_iter().min_by(|x, y| q.error(*x).total_cmp(&q.error(*y))).unwrap_or(mid));
        Candidate { cost: q.error(at).max(0.0), a, b, at, stamp: (self.version[a as usize], self.version[b as usize]) }
    }

    /// Whether collapsing `a` into `b`'s place at `at` keeps the surface sound: the link condition, and no face turned or crushed.
    fn may(&self, a: u32, b: u32, at: V3) -> bool {
        let shared: Vec<u32> =
            self.round[a as usize].iter().copied().filter(|&f| self.alive[f as usize] && self.faces[f as usize].contains(&b)).collect();
        if shared.is_empty() || shared.len() > 2 {
            return false;
        }
        // The common neighbours of the two ends must be the third corners of the faces they share.
        let (na, nb) = (self.neighbours(a), self.neighbours(b));
        let common = na.iter().filter(|w| nb.binary_search(w).is_ok()).count();
        if common != shared.len() {
            return false;
        }
        for v in [a, b] {
            for &f in &self.round[v as usize] {
                if !self.alive[f as usize] || shared.contains(&f) {
                    continue;
                }
                let t = self.faces[f as usize];
                let p = t.map(|i| self.pos[i as usize]);
                let moved = t.map(|i| if i == a || i == b { at } else { self.pos[i as usize] });
                let before = cross(sub(p[1], p[0]), sub(p[2], p[0]));
                let after = cross(sub(moved[1], moved[0]), sub(moved[2], moved[0]));
                let (lb, la) = (len(before), len(after));
                if la < 1e-12 || la < 1e-3 * lb || dot(before, after) < 0.2 * lb * la {
                    return false;
                }
            }
        }
        true
    }

    /// Collapses `b` into `a`, placed at `at`. Returns how many faces went.
    fn collapse(&mut self, a: u32, b: u32, at: V3) -> usize {
        let mut removed = 0;
        self.pos[a as usize] = at;
        let qb = self.q[b as usize];
        self.q[a as usize].add(&qb);
        let from_b = std::mem::take(&mut self.round[b as usize]);
        for &f in &from_b {
            if !self.alive[f as usize] {
                continue;
            }
            let t = &mut self.faces[f as usize];
            if t.contains(&a) {
                self.alive[f as usize] = false;
                removed += 1;
            } else {
                for i in t.iter_mut() {
                    if *i == b {
                        *i = a;
                    }
                }
                self.round[a as usize].push(f);
            }
        }
        let alive = &self.alive;
        self.round[a as usize].retain(|&f| alive[f as usize]);
        self.gone[b as usize] = true;
        self.version[a as usize] += 1;
        self.version[b as usize] += 1;
        removed
    }
}

/// `mesh` with at most `target` triangles (at least 4), its shape kept as well as quadric collapse can. A mesh at or under the target
/// comes back as it was.
pub fn simplify(mesh: &Mesh, target: usize) -> Result<Mesh, String> {
    if mesh.triangles.len() > MAX_TRIANGLES {
        return Err(format!("more than the {MAX_TRIANGLES} triangles a mesh may hold"));
    }
    let target = target.max(4);
    if mesh.triangles.len() <= target {
        return Ok(mesh.clone());
    }
    let pos: Vec<V3> = mesh.vertices.iter().map(|v| v.map(f64::from)).collect();
    let n = pos.len();
    let mut w = Work {
        q: vec![Quadric::default(); n],
        faces: mesh.triangles.clone(),
        alive: vec![true; mesh.triangles.len()],
        round: vec![Vec::new(); n],
        version: vec![0; n],
        gone: vec![false; n],
        pos,
    };
    // Each face's plane, weighted by its area, added to its corners; and how many faces each edge has, to find the borders.
    let mut edges: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
    for (f, t) in mesh.triangles.iter().enumerate() {
        if t.iter().any(|&i| i as usize >= n) || t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            w.alive[f] = false;
            continue;
        }
        let p = t.map(|i| w.pos[i as usize]);
        let normal = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let area2 = len(normal);
        if area2 > 1e-18 {
            let u = normal.map(|c| c / area2);
            let plane = Quadric::plane(u, -dot(u, p[0]), area2 / 2.0);
            for &i in t {
                w.q[i as usize].add(&plane);
            }
        }
        for &i in t {
            w.round[i as usize].push(f as u32);
        }
        for (x, y) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *edges.entry((x.min(y), x.max(y))).or_default() += 1;
        }
    }
    // A border edge: a heavy plane square to its face, through the edge, so the border stays where it is.
    for (f, t) in mesh.triangles.iter().enumerate() {
        if !w.alive[f] {
            continue;
        }
        let p = t.map(|i| w.pos[i as usize]);
        let normal = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        for (k, (x, y)) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])].into_iter().enumerate() {
            if edges.get(&(x.min(y), x.max(y))) != Some(&1) {
                continue;
            }
            let (px, py) = (p[k], p[(k + 1) % 3]);
            let side = cross(sub(py, px), normal);
            let l = len(side);
            if l > 1e-18 {
                let u = side.map(|c| c / l);
                let weight = 1000.0 * dot(sub(py, px), sub(py, px));
                let plane = Quadric::plane(u, -dot(u, px), weight);
                w.q[x as usize].add(&plane);
                w.q[y as usize].add(&plane);
            }
        }
    }
    let mut heap: BinaryHeap<Candidate> = edges.keys().map(|&(a, b)| w.candidate(a, b)).collect();
    let mut faces = w.alive.iter().filter(|&&a| a).count();
    while faces > target {
        let Some(c) = heap.pop() else { break };
        let (a, b) = (c.a as usize, c.b as usize);
        if w.gone[a] || w.gone[b] || c.stamp != (w.version[a], w.version[b]) {
            continue;
        }
        if !w.may(c.a, c.b, c.at) {
            continue;
        }
        faces -= w.collapse(c.a, c.b, c.at);
        for nb in w.neighbours(c.a) {
            heap.push(w.candidate(c.a.min(nb), c.a.max(nb)));
        }
    }
    // The vertices still used, numbered again.
    let mut map = vec![u32::MAX; n];
    let mut vertices = Vec::new();
    let mut triangles = Vec::with_capacity(faces);
    for (f, t) in w.faces.iter().enumerate() {
        if !w.alive[f] {
            continue;
        }
        let mut out = [0u32; 3];
        for (k, &i) in t.iter().enumerate() {
            if map[i as usize] == u32::MAX {
                map[i as usize] = vertices.len() as u32;
                vertices.push(w.pos[i as usize].map(|c| c as f32));
            }
            out[k] = map[i as usize];
        }
        triangles.push(out);
    }
    Mesh::new(vertices, triangles)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long a heavy mesh takes (`cargo test -p lamina-core --release -- --ignored --nocapture heavy`).
    #[test]
    #[ignore]
    fn heavy() {
        let parts: Vec<Mesh> =
            (0..8).map(|k| Mesh::sphere(10.0, 256).transformed(&crate::Transform::translate([f64::from(k) * 25.0, 0.0, 0.0]))).collect();
        let m = Mesh::merged(&parts);
        let start = std::time::Instant::now();
        let out = simplify(&m, m.triangles.len() / 20).unwrap();
        println!("{} triangles to {} in {:?}", m.triangles.len(), out.triangles.len(), start.elapsed());
        assert!(out.is_watertight());
    }

    #[test]
    fn a_sphere_keeps_its_shape_and_stays_closed_with_a_tenth_of_its_triangles() {
        let s = Mesh::sphere(20.0, 96);
        let before = s.triangles.len();
        let out = simplify(&s, before / 10).unwrap();
        assert!(out.triangles.len() <= before / 10, "{} of {before}", out.triangles.len());
        assert!(out.triangles.len() > before / 20, "it stops near the target: {}", out.triangles.len());
        assert!(out.is_watertight());
        assert!((out.volume() - s.volume()).abs() < 0.03 * s.volume(), "{} vs {}", out.volume(), s.volume());
        let ((lo, hi), (a, b)) = (s.bounds().unwrap(), out.bounds().unwrap());
        for k in 0..3 {
            assert!((lo[k] - a[k]).abs() < 1.0 && (hi[k] - b[k]).abs() < 1.0, "{k}: {lo:?} {hi:?} vs {a:?} {b:?}");
        }
    }

    #[test]
    fn flat_faces_and_sharp_edges_are_kept() {
        // A cylinder of many sides: its flat caps lose their fans first, and its rims stay where they are.
        let c = Mesh::cylinder(10.0, 20.0, 256);
        let out = simplify(&c, 300).unwrap();
        assert!(out.triangles.len() <= 300 && out.is_watertight());
        let (lo, hi) = out.bounds().unwrap();
        assert!(lo[2].abs() < 1e-4 && (hi[2] - 20.0).abs() < 1e-4, "{lo:?} {hi:?}");
        assert!((out.volume() - c.volume()).abs() < 0.02 * c.volume(), "{} vs {}", out.volume(), c.volume());
        // Asked for very few, it goes as far as asked and stays a closed solid (a box toward a tetrahedron).
        let out = simplify(&Mesh::cuboid([10.0, 10.0, 10.0]), 4).unwrap();
        assert!(out.triangles.len() <= 8 && out.is_watertight() && out.volume() > 0.0, "{} {}", out.triangles.len(), out.volume());
    }

    #[test]
    fn an_open_mesh_keeps_its_border_and_a_mesh_under_the_target_is_kept_as_it_is() {
        let mut open = Mesh::sphere(10.0, 64);
        // Cut a cap off: the top's triangles go, leaving a ring of border edges.
        open.triangles.retain(|t| t.iter().all(|&i| open.vertices[i as usize][2] < 18.0));
        let top = |m: &Mesh| m.triangles.iter().flatten().map(|&i| m.vertices[i as usize][2]).fold(f32::MIN, f32::max);
        let out = simplify(&open, open.triangles.len() / 4).unwrap();
        assert!((top(&open) - top(&out)).abs() < 0.05, "the border stays: {} vs {}", top(&open), top(&out));
        assert!(out.triangles.len() <= open.triangles.len() / 4);
        let small = Mesh::cuboid([1.0, 2.0, 3.0]);
        assert_eq!(simplify(&small, 100).unwrap(), small);
    }
}
