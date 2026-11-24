//! Paint on an object's faces (`lamina_core::Paint`, one tag a triangle) read on its layers: where supports must or must not
//! hold it up, and where its walls' seams go.

use crate::geom::{self, Region};
use crate::slice::{Layer, P2};
use lamina_core::{Mesh, P3, Paint};
use std::collections::HashMap;

/// For each layer, the shadow on the plate of the downward faces painted `kind` that lie between the middle of the layer below and
/// the middle of this one (where the layer's overhang is), grown by `grow`: what the support planner holds up (`Support`) or keeps
/// supports off (`Block`).
pub fn shadows(mesh: &Mesh, tags: &[u8], kind: Paint, layers: &[Layer], grow: f64) -> Vec<Region> {
    let mut loops: Vec<Vec<Vec<P2>>> = vec![Vec::new(); layers.len()];
    let middles: Vec<f64> = layers.iter().map(|l| l.z - l.height / 2.0).collect();
    for (i, &tag) in tags.iter().enumerate() {
        if Paint::from_tag(tag) != kind {
            continue;
        }
        let Some(t) = mesh.tri(i) else { continue };
        // Only a face that looks down has air under it.
        let n = normal(t);
        if n[2] >= -1e-9 {
            continue;
        }
        let (lo, hi) = (t.iter().map(|q| q[2]).fold(f64::MAX, f64::min), t.iter().map(|q| q[2]).fold(f64::MIN, f64::max));
        // The layers whose band, from the middle below to their own middle, meets the face.
        let first = middles.partition_point(|&m| m < lo);
        for k in first..layers.len() {
            let bottom = if k == 0 { 0.0 } else { middles[k - 1] };
            if bottom > hi {
                break;
            }
            let part = clip(&t, bottom, middles[k]);
            if part.len() >= 3 {
                let mut flat: Vec<P2> = part.iter().map(|q| [q[0], q[1]]).collect();
                if area(&flat) < 0.0 {
                    flat.reverse();
                }
                loops[k].push(flat);
            }
        }
    }
    loops.iter().map(|l| if l.is_empty() { Region::new() } else { geom::inset(&geom::region(l), -grow) }).collect()
}

/// For each layer, where its walls' seams go: one point for each run of seam paint the layer's cut crosses, at the middle of the run.
pub fn seams(mesh: &Mesh, tags: &[u8], layers: &[Layer]) -> Vec<Vec<P2>> {
    let painted: Vec<usize> = (0..tags.len()).filter(|&i| Paint::from_tag(tags[i]) == Paint::Seam).collect();
    if painted.is_empty() {
        return vec![Vec::new(); layers.len()];
    }
    let span = |i: usize| {
        mesh.tri(i).map_or((f64::MAX, f64::MIN), |t| {
            (t.iter().map(|q| q[2]).fold(f64::MAX, f64::min), t.iter().map(|q| q[2]).fold(f64::MIN, f64::max))
        })
    };
    let spans: Vec<(f64, f64)> = painted.iter().map(|&i| span(i)).collect();
    layers
        .iter()
        .map(|l| {
            let at = l.z - l.height / 2.0;
            // The cut through each painted face: a segment between the two edges that cross the plane, each edge named by its corners'
            // indices, so the cuts through neighbouring faces join where they share an edge.
            let mut cuts: Vec<Cut> = Vec::new();
            for (n, &i) in painted.iter().enumerate() {
                if spans[n].0 > at || spans[n].1 < at {
                    continue;
                }
                let Some(&corners) = mesh.triangles.get(i) else { continue };
                let mut ends = Vec::with_capacity(2);
                for e in 0..3 {
                    let (a, b) = (corners[e], corners[(e + 1) % 3]);
                    let (Some(pa), Some(pb)) = (vertex(mesh, a), vertex(mesh, b)) else { continue };
                    if (pa[2] >= at) != (pb[2] >= at) {
                        let s = (at - pa[2]) / (pb[2] - pa[2]);
                        ends.push(((a.min(b), a.max(b)), [pa[0] + (pb[0] - pa[0]) * s, pa[1] + (pb[1] - pa[1]) * s]));
                    }
                }
                if let [(ka, pa), (kb, pb)] = ends[..] {
                    cuts.push((ka, kb, pa, pb));
                }
            }
            runs(&cuts)
        })
        .collect()
}

/// A cut through a face: the edges it leaves by, each named by its corners' indices, and where on each.
type Cut = ((u32, u32), (u32, u32), P2, P2);

/// The middle of each run of joined cuts: the mean of their middles weighed by length, put back on the nearest cut.
fn runs(cuts: &[Cut]) -> Vec<P2> {
    let mut parent: Vec<usize> = (0..cuts.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut by_edge: HashMap<(u32, u32), usize> = HashMap::new();
    for (n, c) in cuts.iter().enumerate() {
        for edge in [c.0, c.1] {
            match by_edge.get(&edge) {
                Some(&m) => {
                    let (a, b) = (root(&mut parent, n), root(&mut parent, m));
                    parent[a] = b;
                }
                None => {
                    by_edge.insert(edge, n);
                }
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for n in 0..cuts.len() {
        let r = root(&mut parent, n);
        groups.entry(r).or_default().push(n);
    }
    let mut out: Vec<P2> = groups
        .values()
        .filter_map(|members| {
            let (mut sum, mut length) = ([0.0, 0.0], 0.0);
            for &n in members {
                let (a, b) = (cuts[n].2, cuts[n].3);
                let l = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt().max(1e-9);
                sum = [sum[0] + (a[0] + b[0]) / 2.0 * l, sum[1] + (a[1] + b[1]) / 2.0 * l];
                length += l;
            }
            let mean = [sum[0] / length, sum[1] / length];
            members.iter().map(|&n| nearest_on(mean, cuts[n].2, cuts[n].3)).min_by(|x, y| far(*x, mean).total_cmp(&far(*y, mean)))
        })
        .collect();
    // The same points in the same order on every run, whatever order the map kept them in.
    out.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    out
}

/// Starts the closed loop `pts` at the point of it nearest one of `targets`, when one is within `reach`: a corner of the loop if
/// it is there, else a new corner put in on its side. Returns whether it did.
pub fn start_near(pts: &mut Vec<P2>, targets: &[P2], reach: f64) -> bool {
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let best = targets
        .iter()
        .flat_map(|&t| (0..n).map(move |i| (i, t)))
        .map(|(i, t)| {
            let q = nearest_on(t, pts[i], pts[(i + 1) % n]);
            (i, q, far(q, t))
        })
        .min_by(|a, b| a.2.total_cmp(&b.2));
    let Some((i, q, d)) = best else { return false };
    if d > reach {
        return false;
    }
    let start = if far(q, pts[i]) < 1e-6 {
        i
    } else if far(q, pts[(i + 1) % n]) < 1e-6 {
        (i + 1) % n
    } else {
        pts.insert(i + 1, q);
        i + 1
    };
    pts.rotate_left(start);
    true
}

fn vertex(mesh: &Mesh, i: u32) -> Option<P3> {
    mesh.vertices.get(i as usize).map(|v| v.map(f64::from))
}

fn normal(t: [P3; 3]) -> P3 {
    let (u, v) = ([t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]], [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]]);
    [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
}

/// The part of the triangle from height `lo` to `hi`, as a polygon.
fn clip(t: &[P3; 3], lo: f64, hi: f64) -> Vec<P3> {
    let keep = |poly: Vec<P3>, inside: &dyn Fn(P3) -> bool, at: f64| -> Vec<P3> {
        let mut out = Vec::with_capacity(poly.len() + 2);
        for k in 0..poly.len() {
            let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
            if inside(a) {
                out.push(a);
            }
            if inside(a) != inside(b) && (b[2] - a[2]).abs() > 1e-12 {
                let s = (at - a[2]) / (b[2] - a[2]);
                out.push([a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s, at]);
            }
        }
        out
    };
    let above = keep(t.to_vec(), &|q: P3| q[2] >= lo, lo);
    keep(above, &|q: P3| q[2] <= hi, hi)
}

fn area(pts: &[P2]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| pts[i][0] * pts[(i + 1) % n][1] - pts[(i + 1) % n][0] * pts[i][1]).sum::<f64>() / 2.0
}

fn far(a: P2, b: P2) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// The point of the segment from `a` to `b` nearest `p`.
fn nearest_on(p: P2, a: P2, b: P2) -> P2 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0] * d[0] + d[1] * d[1];
    if len < 1e-18 {
        return a;
    }
    let s = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len).clamp(0.0, 1.0);
    [a[0] + d[0] * s, a[1] + d[1] * s]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slice;

    fn painted(mesh: &Mesh, kind: u8, pick: impl Fn([P3; 3]) -> bool) -> Vec<u8> {
        (0..mesh.triangles.len()).map(|i| if pick(mesh.tri(i).unwrap()) { kind } else { 0 }).collect()
    }

    #[test]
    fn a_painted_underside_casts_its_shadow_on_the_layer_over_it_only() {
        // A box 10 mm square standing 5 mm up on nothing: its underside at z = 5 is the overhang.
        let m = Mesh::cuboid([10.0, 10.0, 4.0]).transformed(&lamina_core::Transform::translate([0.0, 0.0, 5.0]));
        let layers = slice::layers(&m, 0.2, 0.2).unwrap();
        let tags = painted(&m, 2, |t| t.iter().all(|q| (q[2] - 5.0).abs() < 1e-9));
        let s = shadows(&m, &tags, Paint::Support, &layers, 0.0);
        let k = layers.iter().position(|l| l.z > 5.0 && l.z - l.height / 2.0 > 5.0).unwrap();
        assert!((geom::area(&s[k]) - 100.0).abs() < 1e-6, "the whole underside under its first layer: {}", geom::area(&s[k]));
        assert!(s.iter().enumerate().all(|(j, r)| j == k || geom::area(r) < 1e-9), "and on no other layer");
        // The kind must match, and an upward face casts nothing.
        assert!(shadows(&m, &tags, Paint::Block, &layers, 0.0).iter().all(geom::is_empty));
        let top = painted(&m, 2, |t| t.iter().all(|q| (q[2] - 9.0).abs() < 1e-9));
        assert!(shadows(&m, &top, Paint::Support, &layers, 0.0).iter().all(geom::is_empty));
    }

    #[test]
    fn a_seam_painted_on_a_side_is_found_at_the_middle_of_the_stroke_on_each_layer() {
        // A box 20 mm square, 4 mm tall, with a box-shaped mesh dense enough to paint a band on one side.
        let m = Mesh::cuboid([20.0, 20.0, 4.0]);
        let layers = slice::layers(&m, 0.2, 0.2).unwrap();
        // The whole +x side painted: each layer's cut through it runs from y = -10 to 10, so its middle is (10, 0).
        let tags = painted(&m, 1, |t| t.iter().all(|q| (q[0] - 10.0).abs() < 1e-9));
        let s = seams(&m, &tags, &layers);
        assert_eq!(s.len(), layers.len());
        for (k, at) in s.iter().enumerate() {
            assert_eq!(at.len(), 1, "layer {k}: {at:?}");
            assert!(far(at[0], [10.0, 0.0]) < 1e-6, "layer {k}: {at:?}");
        }
        // A loop round the box inset by half a line starts at the corner put in nearest it.
        let mut square = vec![[-9.8, -9.8], [9.8, -9.8], [9.8, 9.8], [-9.8, 9.8]];
        assert!(start_near(&mut square, &s[3], 1.0));
        assert_eq!(square.len(), 5);
        assert!(far(square[0], [9.8, 0.0]) < 1e-9, "{square:?}");
        // A target too far away changes nothing.
        let mut other = vec![[30.0, 30.0], [40.0, 30.0], [40.0, 40.0]];
        assert!(!start_near(&mut other, &s[3], 1.0));
        assert_eq!(other, vec![[30.0, 30.0], [40.0, 30.0], [40.0, 40.0]]);
    }

    #[test]
    fn painting_reads_hostile_tags_without_failing() {
        let m = Mesh::cuboid([10.0, 10.0, 4.0]).transformed(&lamina_core::Transform::translate([0.0, 0.0, 1.0]));
        let layers = slice::layers(&m, 0.2, 0.2).unwrap();
        let mut rng = lamina_core::fuzz::Rng::new(7);
        for _ in 0..50 {
            // Tags of any value and any length, more or fewer than the triangles.
            let len = rng.below(m.triangles.len() * 2);
            let tags = rng.bytes(len);
            let _ = shadows(&m, &tags, Paint::Support, &layers, 0.2);
            let _ = seams(&m, &tags, &layers);
        }
    }
}
