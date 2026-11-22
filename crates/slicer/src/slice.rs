//! Cutting a mesh into layers. Each layer is the set of closed loops where a horizontal plane meets the surface: counter-clockwise
//! for an outer wall, clockwise for a hole, so the material is always on the left of the walk.

use std::collections::HashMap;

use lamina_core::Mesh;

pub type P2 = [f64; 2];
pub type Loop = Vec<P2>;

/// The most layers one object may have, so a tiny layer height on a tall object is refused, not run for hours.
pub const MAX_LAYERS: usize = 20_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    /// The height of the layer's top, mm: where the nozzle prints it.
    pub z: f64,
    pub height: f64,
    pub loops: Vec<Loop>,
}

/// The layers of `mesh`: the first `first` mm thick, the rest `height`, each cut through its middle. The mesh must be closed.
pub fn layers(mesh: &Mesh, first: f64, height: f64) -> Result<Vec<Layer>, String> {
    if !(first.is_finite() && first > 0.0 && height.is_finite() && height > 0.0) {
        return Err("layer heights must be above 0".into());
    }
    let top = mesh.bounds().ok_or("the mesh is empty")?.1[2];
    let count = if top <= first { 1 } else { 1 + ((top - first) / height).ceil() as usize };
    if count > MAX_LAYERS {
        return Err(format!("{count} layers is more than the {MAX_LAYERS} one object may have: use a thicker layer height"));
    }
    let mut tops: Vec<f64> = (0..count).map(|k| (first + height * k as f64).min(top.max(first))).collect();
    // A last layer thinner than a quarter of the others is a sliver no nozzle can print: the object is that much shorter.
    if count > 1 && tops[count - 1] - tops[count - 2] < 0.25 * height {
        tops.pop();
    }
    // Each layer is cut through its middle and printed as thick as it really is, so a clamped last layer is not over-extruded.
    let bottoms: Vec<f64> = std::iter::once(0.0).chain(tops.iter().copied()).collect();
    let cuts: Vec<f64> = tops.iter().zip(&bottoms).map(|(t, b)| (t + b) / 2.0).collect();
    let count = tops.len();
    let mut segments: Vec<Vec<Segment>> = vec![Vec::new(); count];
    for i in 0..mesh.triangles.len() {
        let Some(t) = mesh.triangles.get(i) else { continue };
        let Some(c) = mesh.tri(i) else { continue };
        let (lo, hi) = (c.iter().map(|p| p[2]).fold(f64::MAX, f64::min), c.iter().map(|p| p[2]).fold(f64::MIN, f64::max));
        // The layers whose cut lies in the triangle's z range; cuts are sorted, so two searches find them.
        let (from, to) = (cuts.partition_point(|&z| z < lo), cuts.partition_point(|&z| z <= hi));
        for (k, &z) in cuts.iter().enumerate().take(to).skip(from) {
            if let Some(s) = Segment::cut(mesh, t, &c, z) {
                segments[k].push(s);
            }
        }
    }
    Ok(tops.iter().zip(&bottoms).zip(&segments).map(|((&z, &b), segs)| Layer { z, height: z - b, loops: chain(segs) }).collect())
}

/// Where a triangle meets the plane: from one edge to another, which are named by their vertex indices so that two triangles
/// sharing an edge agree on it exactly, however the float arithmetic rounds.
#[derive(Clone, Copy, Debug)]
struct Segment {
    from: ((u32, u32), P2),
    to: ((u32, u32), P2),
}

impl Segment {
    fn cut(_: &Mesh, t: &[u32; 3], c: &[[f64; 3]; 3], z: f64) -> Option<Segment> {
        // A corner exactly on the plane counts as below it, for every triangle alike, so a closed mesh gives closed loops.
        let above = [c[0][2] > z, c[1][2] > z, c[2][2] > z];
        if above[0] == above[1] && above[1] == above[2] {
            return None;
        }
        let mut crossing = Vec::with_capacity(2);
        for k in 0..3 {
            let l = (k + 1) % 3;
            if above[k] != above[l] {
                let (a, b) = if t[k] < t[l] { (k, l) } else { (l, k) }; // the same corner order from both sides of the edge
                let u = (z - c[a][2]) / (c[b][2] - c[a][2]);
                crossing.push(((t[a].min(t[b]), t[a].max(t[b])), [c[a][0] + u * (c[b][0] - c[a][0]), c[a][1] + u * (c[b][1] - c[a][1])]));
            }
        }
        let [p, q] = [*crossing.first()?, *crossing.get(1)?];
        // The solid is on the left of the walk: the outward normal points to its right.
        let (u, v) = ([c[1][0] - c[0][0], c[1][1] - c[0][1], c[1][2] - c[0][2]], [c[2][0] - c[0][0], c[2][1] - c[0][1], c[2][2] - c[0][2]]);
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2]];
        let d = [q.1[0] - p.1[0], q.1[1] - p.1[1]];
        if n == [0.0, 0.0] || d == [0.0, 0.0] {
            return None; // a flat triangle, or a corner just touching the plane
        }
        Some(if d[0] * n[1] - d[1] * n[0] < 0.0 { Segment { from: p, to: q } } else { Segment { from: q, to: p } })
    }
}

/// Joins segments end to start into closed loops. A chain that does not close (a hole in the mesh) is dropped.
fn chain(segments: &[Segment]) -> Vec<Loop> {
    let mut by_start: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, s) in segments.iter().enumerate() {
        by_start.entry(s.from.0).or_default().push(i);
    }
    let mut used = vec![false; segments.len()];
    let mut loops = Vec::new();
    for first in 0..segments.len() {
        if used[first] {
            continue;
        }
        let (mut points, mut at) = (Vec::new(), first);
        let closed = loop {
            used[at] = true;
            points.push(segments[at].from.1);
            let next = by_start.get(&segments[at].to.0).and_then(|v| v.iter().copied().find(|&j| !used[j]));
            match next {
                Some(j) => at = j,
                None => break segments[at].to.0 == segments[first].from.0,
            }
        };
        if closed && points.len() >= 3 {
            loops.push(points);
        }
    }
    loops
}

/// A loop's signed area: positive counter-clockwise.
pub fn area(l: &[P2]) -> f64 {
    (0..l.len()).map(|i| l[i][0] * l[(i + 1) % l.len()][1] - l[(i + 1) % l.len()][0] * l[i][1]).sum::<f64>() / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_is_cut_into_square_layers() {
        let layers = layers(&Mesh::cuboid([10.0, 10.0, 10.0]), 0.3, 0.2).unwrap();
        assert_eq!(layers.len(), 1 + (9.7f64 / 0.2).ceil() as usize);
        for l in &layers {
            assert_eq!(l.loops.len(), 1, "z = {}", l.z);
            assert!((area(&l.loops[0]) - 100.0).abs() < 1e-9, "counter-clockwise, 10 x 10");
        }
        assert!((layers[0].z - 0.3).abs() < 1e-12 && (layers.last().unwrap().z - 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_sphere_is_a_circle_that_grows_and_shrinks() {
        let l = layers(&Mesh::sphere(10.0, 64), 0.2, 0.2).unwrap();
        let mid = l.iter().map(|l| l.loops.first().map_or(0.0, |p| area(p))).fold(0.0, f64::max);
        assert!((mid - std::f64::consts::PI * 100.0).abs() < 3.0, "{mid}");
        assert!(l.iter().all(|l| l.loops.len() == 1));
    }

    #[test]
    fn a_tube_has_a_hole_wound_the_other_way() {
        // A cylinder with a smaller one cut out, as two closed surfaces: the inner one wound inward.
        let outer = Mesh::cylinder(10.0, 5.0, 48);
        let mut inner = Mesh::cylinder(4.0, 5.0, 48);
        for t in &mut inner.triangles {
            t.swap(1, 2);
        }
        let tube = Mesh::merged([&outer, &inner]);
        let l = layers(&tube, 0.2, 0.2).unwrap();
        let loops = &l[3].loops;
        assert_eq!(loops.len(), 2);
        let sum: f64 = loops.iter().map(|p| area(p)).sum();
        assert!((sum - std::f64::consts::PI * (100.0 - 16.0)).abs() < 5.0, "{sum}");
        assert!(loops.iter().any(|p| area(p) < 0.0));
    }

    #[test]
    fn too_many_layers_and_bad_heights_are_refused() {
        assert!(layers(&Mesh::cuboid([1.0, 1.0, 100.0]), 0.2, 0.001).is_err());
        assert!(layers(&Mesh::cuboid([1.0, 1.0, 1.0]), 0.2, 0.0).is_err());
        assert!(layers(&Mesh::cuboid([1.0, 1.0, 1.0]), f64::NAN, 0.2).is_err());
        assert!(layers(&Mesh::default(), 0.2, 0.2).is_err());
    }
}
