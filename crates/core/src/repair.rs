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
    let before_closing = m.clone();
    let (mut m, closed) = close_with(m, 4, TOLERANCE)?;
    // A sheet capped by its own outline encloses nothing: that is no solid, so leave it open and say so.
    if closed.holes_filled > 0 && m.volume().abs() < 1e-3 {
        r.closed = false;
        return Ok((before_closing, r));
    }
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

/// A closed mesh with the vertices taken out that add nothing to its shape, and the holes they leave triangulated again: a vertex
/// whose triangles all lie in one plane (the middle of a fan, a point a split left inside a face), and a vertex on the straight
/// crease between two planes (a point a split left along an edge). Every other vertex stays exactly where it was, so the surface
/// and its volume are the same, but a face an earlier boolean cut into slivers becomes a few wide triangles, which the next boolean
/// splits less. A vertex is taken out only where its ring of neighbours is one simple loop, every triangle round it has an area,
/// and the hole can be cut into ears; `tol` (mm) is how far off a plane, or off the crease's line, a point may be.
pub fn tidy_flat(m: &Mesh, tol: f64) -> Mesh {
    let p: Vec<[f64; 3]> = m.vertices.iter().map(|v| v.map(f64::from)).collect();
    let mut tris: Vec<Option<[u32; 3]>> = m.triangles.iter().map(|&t| Some(t)).collect();
    for _ in 0..64 {
        let mut around: Vec<Vec<usize>> = vec![Vec::new(); p.len()];
        for (i, t) in tris.iter().enumerate() {
            for &v in t.iter().flatten() {
                if let Some(a) = around.get_mut(v as usize) {
                    a.push(i);
                }
            }
        }
        let mut touched = vec![false; p.len()];
        let mut changed = false;
        for v in 0..p.len() {
            if touched[v] || around[v].len() < 3 {
                continue;
            }
            let fan: Vec<[u32; 3]> = around[v].iter().filter_map(|&i| tris[i]).collect();
            let Some(new) = take_out(&p, v as u32, &fan, tol) else { continue };
            for &i in &around[v] {
                if let Some(t) = tris[i] {
                    for &u in &t {
                        touched[u as usize] = true;
                    }
                }
                tris[i] = None;
            }
            // New triangles go in the slots the old ones left, then at the end.
            let mut free = around[v].clone().into_iter();
            for t in new {
                match free.next() {
                    Some(i) => tris[i] = Some(t),
                    None => tris.push(Some(t)),
                }
            }
            changed = true;
        }
        if !changed {
            break;
        }
    }
    // Only the vertices still used, in their order.
    let kept: Vec<[u32; 3]> = tris.into_iter().flatten().collect();
    let mut used = vec![false; p.len()];
    for t in &kept {
        for &v in t {
            used[v as usize] = true;
        }
    }
    let mut map = vec![u32::MAX; p.len()];
    let mut vertices = Vec::new();
    for (i, &u) in used.iter().enumerate() {
        if u {
            map[i] = vertices.len() as u32;
            vertices.push(m.vertices[i]);
        }
    }
    let triangles = kept.iter().map(|t| t.map(|v| map[v as usize])).collect();
    Mesh { vertices, triangles }
}

/// The triangles that replace `fan`, the triangles round vertex `v`, with `v` taken out, or none when it must stay.
fn take_out(p: &[[f64; 3]], v: u32, fan: &[[u32; 3]], tol: f64) -> Option<Vec<[u32; 3]>> {
    // The ring of neighbours: each triangle (v, a, b), turned to start at v, gives the step a to b.
    let mut next: HashMap<u32, (u32, usize)> = HashMap::new();
    for (k, t) in fan.iter().enumerate() {
        let at = t.iter().position(|&u| u == v)?;
        let (a, b) = (t[(at + 1) % 3], t[(at + 2) % 3]);
        if next.insert(a, (b, k)).is_some() {
            return None; // not one simple loop
        }
    }
    let start = *next.keys().min()?;
    let (mut ring, mut order) = (vec![start], Vec::with_capacity(fan.len()));
    let mut at = start;
    loop {
        let &(b, k) = next.get(&at)?;
        order.push(k);
        if b == start {
            break;
        }
        if ring.len() >= fan.len() {
            return None;
        }
        ring.push(b);
        at = b;
    }
    if ring.len() != fan.len() {
        return None;
    }
    // Each triangle's normal (twice its area); one too small to have a sure direction keeps the vertex.
    let normal = |t: &[u32; 3]| {
        let (a, b, c) = (p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]);
        cross(sub(b, a), sub(c, a))
    };
    let unit = |n: [f64; 3]| {
        let l = dot(n, n).sqrt();
        (l > 1e-9).then(|| n.map(|x| x / l))
    };
    let normals: Vec<[f64; 3]> = order.iter().map(|&k| unit(normal(&fan[k]))).collect::<Option<_>>()?;
    let here = p[v as usize];
    // The plane of each triangle, and whether a point is on it.
    let on = |n: [f64; 3], q: [f64; 3]| dot(n, sub(q, here)).abs() <= tol;
    let same = |n: [f64; 3], m: [f64; 3]| dot(n, m) > 1.0 - 1e-6;
    let n0 = normals[0];
    if normals.iter().all(|&n| same(n, n0)) && ring.iter().all(|&u| on(n0, p[u as usize])) {
        // In the middle of a flat face: the hole is the whole ring.
        return ears(p, &ring, n0, tol);
    }
    // On a crease: the triangles in two runs, each in one plane, and v on the line between the two neighbours where they meet.
    let first = (0..normals.len()).find(|&k| !same(normals[k], normals[(k + normals.len() - 1) % normals.len()]))?;
    let mut runs: Vec<(usize, usize)> = Vec::new(); // (start, length), in ring order from `first`
    let n = normals.len();
    let mut k = 0;
    while k < n {
        let s = (first + k) % n;
        let mut len = 1;
        while k + len < n && same(normals[(first + k + len) % n], normals[s]) {
            len += 1;
        }
        runs.push((s, len));
        k += len;
    }
    let [(s1, l1), (s2, l2)] = runs[..] else { return None };
    // Triangle i spans ring[i] to ring[i + 1]: run one goes from ring[s1] to ring[s2], run two back.
    let (u, w) = (ring[s1], ring[s2]);
    let (pu, pw) = (p[u as usize], p[w as usize]);
    let line = sub(pw, pu);
    let len2 = dot(line, line);
    let t = dot(sub(here, pu), line) / len2;
    let off = sub(here, [pu[0] + t * line[0], pu[1] + t * line[1], pu[2] + t * line[2]]);
    if len2 < 1e-18 || !(0.0..=1.0).contains(&t) || dot(off, off).sqrt() > tol {
        return None;
    }
    let arc = |s: usize, l: usize| (0..=l).map(|i| ring[(s + i) % n]).collect::<Vec<u32>>();
    let (one, two) = (arc(s1, l1), arc(s2, l2));
    if one.len() < 3
        || two.len() < 3
        || !one.iter().all(|&q| on(normals[s1], p[q as usize]))
        || !two.iter().all(|&q| on(normals[s2], p[q as usize]))
    {
        return None;
    }
    let mut out = ears(p, &one, normals[s1], tol)?;
    out.extend(ears(p, &two, normals[s2], tol)?);
    Some(out)
}

/// The polygon `ring` (counter-clockwise seen along `n`) cut into triangles by clipping ears, the widest first (the ear whose
/// smallest angle is largest), so no needle is made where a wider cut will do. None when no ear can be cut, as a polygon too thin
/// to tell its turns has.
fn ears(p: &[[f64; 3]], ring: &[u32], n: [f64; 3], tol: f64) -> Option<Vec<[u32; 3]>> {
    // A frame in the plane: (e1, e2, n) right-handed, so counter-clockwise in the plane is counter-clockwise about n.
    let helper = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let e1 = {
        let c = cross(helper, n);
        let l = dot(c, c).sqrt();
        c.map(|x| x / l)
    };
    let e2 = cross(n, e1);
    let flat = |i: u32| {
        let q = p[i as usize];
        [dot(q, e1), dot(q, e2)]
    };
    let mut left: Vec<(u32, [f64; 2])> = ring.iter().map(|&i| (i, flat(i))).collect();
    let turn = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    // Inside or on an edge: a point on the cut would be left in the middle of a triangle's side, a crack.
    let inside =
        |a: [f64; 2], b: [f64; 2], c: [f64; 2], q: [f64; 2]| turn(a, b, q) >= -1e-12 && turn(b, c, q) >= -1e-12 && turn(c, a, q) >= -1e-12;
    let angle = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        let (u, v) = ([a[0] - b[0], a[1] - b[1]], [c[0] - b[0], c[1] - b[1]]);
        let l = (u[0].hypot(u[1]) * v[0].hypot(v[1])).max(1e-300);
        ((u[0] * v[0] + u[1] * v[1]) / l).clamp(-1.0, 1.0).acos()
    };
    let mut out = Vec::with_capacity(ring.len().saturating_sub(2));
    // A corner on a straight stretch (the crease's own) is no ear, but stays a corner of the triangles beside it.
    let straight = tol * tol;
    while left.len() > 3 {
        let m = left.len();
        let mut best: Option<(usize, f64)> = None;
        for i in 0..m {
            let (a, b, c) = (left[(i + m - 1) % m].1, left[i].1, left[(i + 1) % m].1);
            if turn(a, b, c) <= straight {
                continue;
            }
            if (0..m).filter(|&j| j != i && j != (i + 1) % m && j != (i + m - 1) % m).any(|j| inside(a, b, c, left[j].1)) {
                continue;
            }
            let worst = angle(c, a, b).min(angle(a, b, c)).min(angle(b, c, a));
            if best.is_none_or(|(_, w)| worst > w) {
                best = Some((i, worst));
            }
        }
        let (i, _) = best?;
        let m = left.len();
        out.push([left[(i + m - 1) % m].0, left[i].0, left[(i + 1) % m].0]);
        left.remove(i);
    }
    let [a, b, c] = [left[0], left[1], left[2]];
    if turn(a.1, b.1, c.1) <= straight {
        return None;
    }
    out.push([a.0, b.0, c.0]);
    Some(out)
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
    fn a_sheet_is_not_closed_into_a_solid_of_no_volume() {
        let sheet =
            Mesh::new(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], vec![[0, 1, 2], [0, 2, 3]]).unwrap();
        let (same, r) = repair(&sheet).unwrap();
        assert_eq!((same.triangles.len(), r.closed, r.holes_filled), (2, false, 0));
    }

    #[test]
    fn a_sound_mesh_comes_back_unchanged() {
        let ball = Mesh::sphere(5.0, 48);
        let (same, r) = repair(&ball).unwrap();
        assert_eq!((same, r), (ball.clone(), Report { closed: true, ..Report::default() }));
        assert_eq!(problems(&ball), Problems::default());
        assert_eq!(problems(&ball).describe(), "closed and consistent");
    }

    /// Splits the triangles on edge (`a`, `b`) at its middle, a new vertex.
    fn split_edge(m: &mut Mesh, a: u32, b: u32) {
        let (pa, pb) = (m.vertices[a as usize], m.vertices[b as usize]);
        m.vertices.push([(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0, (pa[2] + pb[2]) / 2.0]);
        let mid = m.vertices.len() as u32 - 1;
        let mut out = Vec::new();
        for &t in &m.triangles {
            match (0..3).find(|&k| (t[k], t[(k + 1) % 3]) == (a, b) || (t[k], t[(k + 1) % 3]) == (b, a)) {
                Some(k) => {
                    let (x, y, z) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                    out.extend([[x, mid, z], [mid, y, z]]);
                }
                None => out.push(t),
            }
        }
        m.triangles = out;
    }

    /// Splits triangle `i` at its middle into three.
    fn split_middle(m: &mut Mesh, i: usize) {
        let t = m.triangles[i];
        let c = (0..3).map(|k| t.iter().map(|&v| m.vertices[v as usize][k]).sum::<f32>() / 3.0).collect::<Vec<_>>();
        m.vertices.push([c[0], c[1], c[2]]);
        let mid = m.vertices.len() as u32 - 1;
        m.triangles[i] = [t[0], t[1], mid];
        m.triangles.extend([[t[1], t[2], mid], [t[2], t[0], mid]]);
    }

    #[test]
    fn tidying_takes_out_what_splits_left_in_flat_faces_and_along_straight_edges() {
        // A box whose faces a boolean might have cut up: each triangle split at its middle, then every edge at its middle (the box's
        // own edges, between two faces, and the diagonals inside them), then the middles again.
        let mut m = Mesh::cuboid([20.0, 10.0, 6.0]);
        let edges: Vec<(u32, u32)> = undirected(&m).into_keys().collect();
        for (a, b) in edges {
            split_edge(&mut m, a, b);
        }
        for i in 0..m.triangles.len() {
            split_middle(&mut m, i);
        }
        assert!(m.is_watertight() && m.triangles.len() > 100, "{}", m.triangles.len());
        let tidy = tidy_flat(&m, 1e-4);
        assert!(tidy.is_watertight());
        assert_eq!((tidy.vertices.len(), tidy.triangles.len()), (8, 12), "a box again");
        assert!((tidy.volume() - 1200.0).abs() < 1e-3, "{}", tidy.volume());
        assert!(problems(&tidy).flipped_faces == 0);
        // A cylinder's caps lose the middles they were fanned from; its round side, where three planes meet at each corner, stays.
        let c = Mesh::cylinder(5.0, 8.0, 24);
        let t = tidy_flat(&c, 1e-4);
        assert!(
            t.is_watertight() && t.triangles.len() < c.triangles.len() && t.vertices.len() == 48,
            "{} {}",
            t.vertices.len(),
            t.triangles.len()
        );
        assert!((t.volume() - c.volume()).abs() < 1e-3);
        // A sphere is curved everywhere: nothing to take out.
        let ball = Mesh::sphere(10.0, 2);
        assert_eq!(tidy_flat(&ball, 1e-4).triangles.len(), ball.triangles.len());
        // A vertex a hair off its face stays when the hair is more than the tolerance.
        let mut bump = Mesh::cuboid([10.0, 10.0, 10.0]);
        split_middle(&mut bump, 0);
        let last = bump.vertices.len() - 1;
        let normal_axis = (0..3)
            .find(|&k| bump.triangles[0].iter().all(|&v| bump.vertices[v as usize][k] == bump.vertices[bump.triangles[0][0] as usize][k]))
            .unwrap();
        bump.vertices[last][normal_axis] += 0.01;
        assert_eq!(tidy_flat(&bump, 1e-4).vertices.len(), 9, "kept");
    }
}
