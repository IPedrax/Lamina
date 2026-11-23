//! Mesh repair, shared by import and the booleans. Downloaded meshes often have holes, flipped faces and doubled triangles, and the
//! BSP behind the booleans leaves cracks; both are mended by the same passes. Every pass iterates in a fixed order, so the same mesh is
//! repaired the same way every run. (These passes were written for `lamina-csg` first and moved here.)

use std::collections::{HashMap, HashSet};

use crate::mesh::Mesh;

/// What a repair did, and whether the mesh is closed after it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Triangles removed: doubled faces, skins of no thickness, fins, slivers collapsed by snapping.
    pub dropped: usize,
    /// Faces turned to agree with their neighbours, and whole shells turned right side out.
    pub flipped: usize,
    pub holes_filled: usize,
    pub closed: bool,
}

impl Report {
    pub fn changed(&self) -> bool {
        self.dropped + self.flipped + self.holes_filled > 0
    }
}

/// What is wrong with a mesh, without changing it: what `info` reports and the window warns about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Problems {
    /// Edges only one face uses: the border of a hole.
    pub open_edges: usize,
    /// Edges three or more faces use.
    pub nonmanifold_edges: usize,
    /// Faces wound against their neighbours (counting a whole shell turned inside out as its faces).
    pub flipped_faces: usize,
}

impl Problems {
    pub fn any(&self) -> bool {
        self.open_edges + self.nonmanifold_edges + self.flipped_faces > 0
    }

    /// The problems in words, for a tooltip or a model.
    pub fn describe(&self) -> String {
        let mut out = Vec::new();
        for (n, what) in [
            (self.open_edges, "open edges"),
            (self.nonmanifold_edges, "edges shared by more than two faces"),
            (self.flipped_faces, "flipped faces"),
        ] {
            if n > 0 {
                out.push(format!("{n} {what}"));
            }
        }
        if out.is_empty() { "closed and consistent".into() } else { out.join(", ") }
    }
}

/// How far apart (mm) a vertex may lie from an edge and still be on it, and how flat a hole must be to be filled whatever its size.
pub const TOLERANCE: f64 = 5e-3;

/// A mesh as it came from a file, mended: doubled faces and skins dropped, every face wound like its neighbours and each shell
/// outward, cracks closed and small holes filled. A mesh with nothing wrong comes back as it was.
pub fn repair(m: &Mesh) -> Result<(Mesh, Report), String> {
    let mut m = m.clone();
    let mut r = Report { closed: m.is_watertight(), ..Report::default() };
    if r.closed && m.volume() >= 0.0 {
        return Ok((m, r));
    }
    r.dropped += drop_skins(&mut m);
    r.flipped += orient(&mut m);
    let (mut m, closed) = close_with(m, 4, TOLERANCE)?;
    r.dropped += closed.dropped;
    r.holes_filled += closed.holes_filled;
    // A filled hole is wound against its border, but a pass may have exposed a face that disagrees.
    r.flipped += orient(&mut m);
    r.closed = m.is_watertight();
    Ok((m, r))
}

/// What is wrong with `m`, counted on a copy.
pub fn problems(m: &Mesh) -> Problems {
    let uses = undirected(m);
    let open_edges = uses.values().filter(|&&n| n == 1).count();
    let nonmanifold_edges = uses.values().filter(|&&n| n > 2).count();
    let mut copy = m.clone();
    let flipped_faces = orient(&mut copy);
    Problems { open_edges, nonmanifold_edges, flipped_faces }
}

/// Makes the winding consistent: walks each connected shell across edges that two faces share and turns any face that walks the
/// shared edge the same way as its neighbour, keeping the larger side of each disagreement. Then a shell that encloses negative volume
/// is turned right side out, unless it lies inside another shell (a void, wound inward on purpose). Returns the faces turned.
pub fn orient(m: &mut Mesh) -> usize {
    let n = m.triangles.len();
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, t) in m.triangles.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            by_edge.entry((a.min(b), a.max(b))).or_default().push(i);
        }
    }
    // Whether triangle i walks the edge (a, b) from a to b.
    let forward = |t: &[u32; 3], a: u32, b: u32| (0..3).any(|k| t[k] == a && t[(k + 1) % 3] == b);
    let mut shell = vec![usize::MAX; n];
    let mut turn = vec![false; n];
    let mut shells: Vec<Vec<usize>> = Vec::new();
    for seed in 0..n {
        if shell[seed] != usize::MAX {
            continue;
        }
        let id = shells.len();
        let mut members = vec![seed];
        shell[seed] = id;
        let mut at = 0;
        while at < members.len() {
            let i = members[at];
            at += 1;
            let t = m.triangles[i];
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let Some(list) = by_edge.get(&(a.min(b), a.max(b))) else { continue };
                if list.len() != 2 {
                    continue; // an open or crowded edge says nothing about which way is out
                }
                let j = if list[0] == i { list[1] } else { list[0] };
                if shell[j] != usize::MAX {
                    continue;
                }
                // i walks a to b as it will be wound; j must walk b to a.
                let i_forward = forward(&t, a, b) != turn[i];
                let j_forward = forward(&m.triangles[j], a, b);
                turn[j] = j_forward == i_forward;
                shell[j] = id;
                members.push(j);
            }
        }
        // Keep the side most faces were already on, so one flipped face is the one turned.
        let turned = members.iter().filter(|&&i| turn[i]).count();
        if turned * 2 > members.len() {
            for &i in &members {
                turn[i] = !turn[i];
            }
        }
        shells.push(members);
    }
    for (i, t) in m.triangles.iter_mut().enumerate() {
        if turn[i] {
            t.swap(1, 2);
        }
    }
    let mut count = turn.iter().filter(|&&t| t).count();
    // Shells enclosing negative volume: turned out unless inside another shell. Above a few dozen shells only the whole mesh is judged.
    let volume = |tris: &[usize], m: &Mesh| -> f64 {
        tris.iter()
            .filter_map(|&i| m.tri(i))
            .map(|[a, b, c]| a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0]))
            .sum::<f64>()
            / 6.0
    };
    if shells.len() > 64 {
        if m.volume() < 0.0 {
            m.triangles.iter_mut().for_each(|t| t.swap(1, 2));
            count = n - count;
        }
        return count;
    }
    let negative: Vec<usize> = (0..shells.len()).filter(|&s| volume(&shells[s], m) < 0.0).collect();
    for s in negative {
        let Some(p) = shells[s].first().and_then(|&i| m.tri(i)).map(|t| t[0]) else { continue };
        let inside = (0..shells.len()).filter(|&o| o != s).any(|o| crossings(m, &shells[o], p) % 2 == 1);
        if !inside {
            for &i in &shells[s] {
                if let Some(t) = m.triangles.get_mut(i) {
                    t.swap(1, 2);
                }
                count = if turn[i] { count - 1 } else { count + 1 };
            }
        }
    }
    count
}

/// How many of the triangles `tris` a ray from `p` along a slightly skewed +x direction crosses.
fn crossings(m: &Mesh, tris: &[usize], p: [f64; 3]) -> usize {
    // Skewed so it does not run exactly along an edge or through a vertex of an axis-square mesh.
    let d = [1.0, 0.000_123_7, 0.000_071_3];
    tris.iter()
        .filter_map(|&i| m.tri(i))
        .filter(|[a, b, c]| {
            // Moller and Trumbore's ray and triangle test.
            let (e1, e2) = (sub(*b, *a), sub(*c, *a));
            let h = cross(d, e2);
            let det = dot(e1, h);
            if det.abs() < 1e-12 {
                return false;
            }
            let s = sub(p, *a);
            let u = dot(s, h) / det;
            if !(0.0..=1.0).contains(&u) {
                return false;
            }
            let q = cross(s, e1);
            let v = dot(d, q) / det;
            v >= 0.0 && u + v <= 1.0 && dot(e2, q) / det > 1e-9
        })
        .count()
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Closes what can be closed, in up to `passes` passes until the mesh is closed or a pass changes nothing: triangles are split at
/// vertices lying on their edges within `tol` (T-junctions), skins of no thickness and thin fins are dropped, vertices on open edges
/// within [`SNAP`] are snapped together, and holes are filled where a fan is surely the missing face. What it did is counted.
pub fn close_with(mut m: Mesh, passes: usize, tol: f64) -> Result<(Mesh, Report), String> {
    let mut r = Report::default();
    for _ in 0..passes {
        if m.is_watertight() {
            break;
        }
        let before = (m.triangles.len(), m.vertices.len());
        split_t_junctions(&mut m, tol);
        r.dropped += drop_skins(&mut m);
        r.dropped += drop_fins(&mut m);
        if !m.is_watertight() {
            r.dropped += snap_open(&mut m);
            r.dropped += drop_skins(&mut m);
        }
        r.holes_filled += fill_holes(&mut m, tol);
        if (m.triangles.len(), m.vertices.len()) == before {
            break;
        }
    }
    let m = Mesh::new(m.vertices, m.triangles)?;
    r.closed = m.is_watertight();
    Ok((m, r))
}

pub fn undirected(m: &Mesh) -> HashMap<(u32, u32), u32> {
    let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
    for t in &m.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *uses.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    uses
}

pub fn split_t_junctions(m: &mut Mesh, tol: f64) {
    let mut open: Vec<(u32, u32)> = undirected(m).into_iter().filter(|&(_, n)| n == 1).map(|(e, _)| e).collect();
    if open.is_empty() {
        return;
    }
    // Sorted, here and below, so a mesh is repaired the same way every time: hash maps iterate in a different order each run.
    open.sort_unstable();
    // Only vertices on an open edge can be the T of a T-junction, which keeps the search small.
    let touching: std::collections::BTreeSet<u32> = open.iter().flat_map(|&(a, b)| [a, b]).collect();
    let pos: Vec<[f64; 3]> = m.vertices.iter().map(|v| v.map(f64::from)).collect();
    let at = |i: u32| pos[i as usize];
    let mut splits: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for &(a, b) in &open {
        let (pa, pb) = (at(a), at(b));
        let mut on: Vec<(f64, u32)> =
            touching.iter().filter(|&&p| p != a && p != b).filter_map(|&p| along(pa, pb, at(p), tol).map(|t| (t, p))).collect();
        on.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
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

/// Removes triangles that share their three vertices with another. Two facing opposite ways are a skin of no thickness the BSP leaves
/// where the shapes' surfaces coincide, so both go; two facing the same way are one face drawn twice, so one goes.
pub fn drop_skins(m: &mut Mesh) -> usize {
    let mut by_corners: HashMap<[u32; 3], Vec<(usize, bool)>> = HashMap::new();
    for (i, t) in m.triangles.iter().enumerate() {
        let mut k = *t;
        k.sort_unstable();
        let even = (0..3).any(|r| [t[r], t[(r + 1) % 3], t[(r + 2) % 3]] == k);
        by_corners.entry(k).or_default().push((i, even));
    }
    let mut gone = vec![false; m.triangles.len()];
    for list in by_corners.values().filter(|l| l.len() > 1) {
        let (mut up, mut down): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
        for &(i, even) in list {
            if even { up.push(i) } else { down.push(i) }
        }
        // Opposite pairs cancel; of what is left, one face stays.
        let pairs = up.len().min(down.len());
        for &i in up.iter().take(pairs).chain(down.iter().take(pairs)) {
            gone[i] = true;
        }
        for &i in up.iter().skip(pairs).skip(1).chain(down.iter().skip(pairs).skip(1)) {
            gone[i] = true;
        }
    }
    let count = gone.iter().filter(|&&g| g).count();
    let mut it = gone.into_iter();
    m.triangles.retain(|_| !it.next().unwrap_or(false));
    count
}

/// A triangle this small (mm2, a tenth of a millimetre square) on an edge that three or more faces share is a fin the BSP left, not a
/// surface; dropping it leaves a small hole that `fill_holes` closes.
pub const FIN: f64 = 1e-2;

pub fn drop_fins(m: &mut Mesh) -> usize {
    let uses = undirected(m);
    if uses.values().all(|&n| n <= 2) {
        return 0;
    }
    let keep: Vec<bool> = (0..m.triangles.len())
        .map(|i| {
            let t = m.triangles[i];
            let shared = (0..3).any(|k| uses.get(&(t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3]))).is_some_and(|&n| n > 2));
            !(shared && triangle_area(m, i) < FIN)
        })
        .collect();
    let before = m.triangles.len();
    let mut keep = keep.into_iter();
    m.triangles.retain(|_| keep.next().unwrap_or(true));
    before - m.triangles.len()
}

pub fn triangle_area(m: &Mesh, i: usize) -> f64 {
    let Some(t) = m.tri(i) else { return 0.0 };
    let (u, v) = ([t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]], [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]]);
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()
}

/// The most edges of a hole that is filled whatever its shape. A bigger gap is filled only if it is flat and convex, where a fan is
/// exactly the face the BSP lost; otherwise it is a real gap, and filling it would invent a surface.
pub const SMALL_HOLE: usize = 16;
pub const BIG_HOLE: usize = 512;

/// Fills each loop of open edges that [`SMALL_HOLE`] and [`flat_and_convex`] (flat within `tol`) allow with a fan, wound against the
/// faces around it, and says how many it filled.
pub fn fill_holes(m: &mut Mesh, tol: f64) -> usize {
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
    let (mut fill, mut filled) = (Vec::new(), 0);
    let mut starts: Vec<(u32, u32)> = next.iter().flat_map(|(&a, bs)| bs.iter().map(move |&b| (a, b))).collect();
    starts.sort_unstable();
    for outs in next.values_mut() {
        outs.sort_unstable();
    }
    for (a, b) in starts {
        if used.contains(&(a, b)) {
            continue;
        }
        let (mut ring, mut at) = (vec![a], b);
        let closed = loop {
            if ring.len() > BIG_HOLE {
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
        if closed && ring.len() >= 3 && (ring.len() <= SMALL_HOLE || flat_and_convex(m, &ring, tol)) {
            filled += 1;
            for k in 1..ring.len() - 1 {
                fill.push([ring[0], ring[k + 1], ring[k]]);
            }
        }
    }
    m.triangles.extend(fill);
    filled
}

/// Whether the outline lies in one plane (within `tol`) and turns one way all round, so a fan from any corner covers it once.
pub fn flat_and_convex(m: &Mesh, ring: &[u32], tol: f64) -> bool {
    let p: Vec<[f64; 3]> = ring.iter().map(|&i| m.vertices[i as usize].map(f64::from)).collect();
    // Newell's normal: the outline's own, however many corners it has.
    let mut n = [0.0; 3];
    for k in 0..p.len() {
        let (a, b) = (p[k], p[(k + 1) % p.len()]);
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len < 1e-12 {
        return false;
    }
    let n = n.map(|c| c / len);
    let d0 = n[0] * p[0][0] + n[1] * p[0][1] + n[2] * p[0][2];
    if p.iter().any(|q| (n[0] * q[0] + n[1] * q[1] + n[2] * q[2] - d0).abs() > tol) {
        return false;
    }
    let mut sign = 0.0f64;
    for k in 0..p.len() {
        let (a, b, c) = (p[k], p[(k + 1) % p.len()], p[(k + 2) % p.len()]);
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - b[0], c[1] - b[1], c[2] - b[2]]);
        let turn = n[0] * (u[1] * v[2] - u[2] * v[1]) + n[1] * (u[2] * v[0] - u[0] * v[2]) + n[2] * (u[0] * v[1] - u[1] * v[0]);
        if turn.abs() < 1e-9 {
            continue; // corners on a straight stretch
        }
        if sign == 0.0 {
            sign = turn.signum();
        } else if turn.signum() != sign {
            return false;
        }
    }
    true
}

/// Vertices on an open edge closer than this (mm) are snapped together: what the BSP splits along an intersection curve leaves slivers
/// a few microns wide, and collapsing them closes the cracks around them. 20 microns is a tenth of a fine layer, invisible in a print.
pub const SNAP: f64 = 0.05;

/// Snaps each vertex of an open or crowded edge to the first such vertex within [`SNAP`], then drops the triangles that collapse,
/// and says how many those were.
pub fn snap_open(m: &mut Mesh) -> usize {
    let uses = undirected(m);
    let loose: HashSet<u32> = uses.iter().filter(|&(_, &n)| n != 2).flat_map(|(&(a, b), _)| [a, b]).collect();
    if loose.is_empty() {
        return 0;
    }
    let mut ids: Vec<u32> = loose.into_iter().collect();
    ids.sort_unstable(); // the same mesh snaps the same way every time
    let pos = |i: u32| m.vertices[i as usize].map(f64::from);
    let mut cells: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut to: HashMap<u32, u32> = HashMap::new();
    for &i in &ids {
        let p = pos(i);
        let c = p.map(|x| (x / SNAP).floor() as i64);
        let mut target = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &j in cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]).into_iter().flatten() {
                        let q = pos(j);
                        if (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2) <= SNAP * SNAP {
                            target = Some(j);
                            break 'search;
                        }
                    }
                }
            }
        }
        match target {
            Some(j) => {
                to.insert(i, j);
            }
            None => cells.entry(c).or_default().push(i),
        }
    }
    if to.is_empty() {
        return 0;
    }
    for t in &mut m.triangles {
        for v in t.iter_mut() {
            if let Some(&j) = to.get(v) {
                *v = j;
            }
        }
    }
    let before = m.triangles.len();
    m.triangles.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
    before - m.triangles.len()
}

/// How far along `a` to `b` the point `p` lies (0 to 1, exclusive) if it is on that segment within `tol`.
pub fn along(a: [f64; 3], b: [f64; 3], p: [f64; 3], tol: f64) -> Option<f64> {
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

    #[test]
    fn a_cube_missing_a_face_is_closed() {
        let mut cube = Mesh::cuboid([10.0, 10.0, 10.0]);
        cube.triangles.truncate(10); // the left face's two triangles are gone
        assert_eq!(problems(&cube).open_edges, 4);
        let (fixed, r) = repair(&cube).unwrap();
        assert!(fixed.is_watertight() && r.closed);
        assert_eq!(r.holes_filled, 1);
        assert!((fixed.volume() - 1000.0).abs() < 1e-6, "{}", fixed.volume());
    }

    #[test]
    fn a_flipped_face_is_turned_back() {
        let mut cube = Mesh::cuboid([10.0, 10.0, 10.0]);
        cube.triangles[3].swap(1, 2);
        assert!(!cube.is_watertight());
        assert_eq!(problems(&cube).flipped_faces, 1);
        let (fixed, r) = repair(&cube).unwrap();
        assert_eq!((r.flipped, r.holes_filled, r.dropped), (1, 0, 0));
        assert!(fixed.is_watertight() && (fixed.volume() - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn a_doubled_face_is_dropped() {
        let mut cube = Mesh::cuboid([10.0, 10.0, 10.0]);
        cube.triangles.push(cube.triangles[0]);
        assert_eq!(problems(&cube).nonmanifold_edges, 3);
        let (fixed, r) = repair(&cube).unwrap();
        assert_eq!(r.dropped, 1);
        assert!(fixed.is_watertight() && (fixed.volume() - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn a_shell_inside_out_is_turned_but_a_void_is_kept() {
        let mut inside_out = Mesh::cuboid([10.0, 10.0, 10.0]);
        inside_out.triangles.iter_mut().for_each(|t| t.swap(1, 2));
        assert_eq!(problems(&inside_out).flipped_faces, 12);
        let (fixed, r) = repair(&inside_out).unwrap();
        assert_eq!(r.flipped, 12);
        assert!((fixed.volume() - 1000.0).abs() < 1e-6);
        // A box with a cavity: the inner box wound inward is a void, not a mistake.
        let mut cavity = Mesh::cuboid([4.0, 4.0, 4.0]).transformed(&crate::Transform::translate([0.0, 0.0, 3.0]));
        cavity.triangles.iter_mut().for_each(|t| t.swap(1, 2));
        let hollow = Mesh::merged([&Mesh::cuboid([10.0, 10.0, 10.0]), &cavity]);
        assert!(!problems(&hollow).any());
        let (same, r) = repair(&hollow).unwrap();
        assert!(!r.changed());
        assert!((same.volume() - 936.0).abs() < 1e-6);
    }

    #[test]
    fn a_sound_mesh_comes_back_unchanged() {
        let ball = Mesh::sphere(5.0, 48);
        let (same, r) = repair(&ball).unwrap();
        assert_eq!((same, r), (ball.clone(), Report { closed: true, ..Report::default() }));
        assert_eq!(problems(&ball), Problems::default());
        assert_eq!(problems(&ball).describe(), "closed and consistent");
    }
}
