//! 2D polygon work for the slicer: unions, differences, offsets and line clipping, on i_overlay. A region is a list of shapes, a shape
//! a list of contours (the outer one counter-clockwise, then its holes clockwise), a contour a list of points, in millimetres.

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::clip::FloatClip;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};
use i_overlay::string::clip::ClipRule;

use crate::slice::{Loop, P2};

pub type Region = Vec<Vec<Vec<P2>>>;

/// How far a simplified loop may stray from the one it came from, mm: far below what a printer resolves.
pub const RESOLUTION: f64 = 0.005;

/// The region the loops of one layer enclose: the outer loops counter-clockwise, holes clockwise, as `slice::layers` winds them.
/// The loops are simplified first: a mesh's faces leave points in a straight line (each quad of a wall is cut into two segments),
/// and offsetting a corner that turns by nothing gives doubled points and spikes.
pub fn region(loops: &[Loop]) -> Region {
    let none: Region = Vec::new();
    let simple: Vec<Loop> = loops.iter().map(|l| simplify(l, RESOLUTION)).filter(|l| l.len() >= 3).collect();
    simple.overlay(&none, OverlayRule::Subject, FillRule::NonZero)
}

/// A closed loop with the points dropped that lie within `tol` of the line through their kept neighbours (Douglas and Peucker's
/// algorithm, on an explicit stack so a long loop cannot overflow it), and repeated points dropped.
pub fn simplify(l: &[P2], tol: f64) -> Loop {
    let mut pts: Vec<P2> = Vec::with_capacity(l.len());
    for &p in l {
        if pts.last().is_none_or(|q: &P2| (p[0] - q[0]).abs() + (p[1] - q[1]).abs() > 1e-9) {
            pts.push(p);
        }
    }
    while pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() <= 1e-9) {
        pts.pop();
    }
    let n = pts.len();
    if n < 4 {
        return pts;
    }
    // Split the loop at its first point and the point farthest from it, and simplify each half as an open run.
    let far = (1..n).max_by(|&a, &b| dist2(pts[0], pts[a]).total_cmp(&dist2(pts[0], pts[b]))).unwrap_or(n / 2);
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[far] = true;
    let mut stack = vec![(0, far), (far, n)];
    while let Some((a, b)) = stack.pop() {
        let (p, q) = (pts[a], pts[b % n]);
        let worst = (a + 1..b).map(|i| (i, off_line(pts[i], p, q))).max_by(|x, y| x.1.total_cmp(&y.1));
        if let Some((i, d)) = worst
            && d > tol
        {
            keep[i] = true;
            stack.push((a, i));
            stack.push((i, b));
        }
    }
    let mut kept: Vec<P2> = pts.into_iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| p).collect();
    // Where distances tie (a side parallel to the chord) the method can keep a point in a straight line: drop those.
    let mut i = 0;
    while kept.len() > 3 && i < kept.len() {
        let n = kept.len();
        if off_line(kept[i], kept[(i + n - 1) % n], kept[(i + 1) % n]) < 1e-7 {
            kept.remove(i);
        } else {
            i += 1;
        }
    }
    kept
}

fn dist2(a: P2, b: P2) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

/// How far `p` lies from the segment `a`..`b`.
fn off_line(p: P2, a: P2, b: P2) -> f64 {
    let (d, len2) = ([b[0] - a[0], b[1] - a[1]], dist2(a, b));
    if len2 < 1e-18 {
        return dist2(p, a).sqrt();
    }
    let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0);
    dist2(p, [a[0] + t * d[0], a[1] + t * d[1]]).sqrt()
}

/// A contour without repeated points and without spikes (a point the path reaches and leaves the way it came).
fn clean(c: Vec<P2>) -> Vec<P2> {
    let mut c = c;
    loop {
        let n = c.len();
        if n < 3 {
            return c;
        }
        let bad = (0..n).find(|&i| {
            let (prev, p, next) = (c[(i + n - 1) % n], c[i], c[(i + 1) % n]);
            let (u, v) = ([p[0] - prev[0], p[1] - prev[1]], [next[0] - p[0], next[1] - p[1]]);
            let (lu, lv) = ((u[0] * u[0] + u[1] * u[1]).sqrt(), (v[0] * v[0] + v[1] * v[1]).sqrt());
            lu < 1e-6 || lv < 1e-6 || (u[0] * v[0] + u[1] * v[1]) / (lu * lv) < -0.999
        });
        match bad {
            Some(i) => {
                c.remove(i);
            }
            None => return c,
        }
    }
}

pub fn union(a: &Region, b: &Region) -> Region {
    a.overlay(b, OverlayRule::Union, FillRule::NonZero)
}

pub fn intersect(a: &Region, b: &Region) -> Region {
    a.overlay(b, OverlayRule::Intersect, FillRule::NonZero)
}

pub fn difference(a: &Region, b: &Region) -> Region {
    a.overlay(b, OverlayRule::Difference, FillRule::NonZero)
}

/// The region moved inward by `d` mm (negative `d` moves it outward). Corners stay sharp up to a limit, so a wall does not round off.
pub fn inset(r: &Region, d: f64) -> Region {
    let style = OutlineStyle::new(-d).line_join(LineJoin::Miter(0.2));
    r.iter()
        .flat_map(|shape| shape.outline(&style))
        .map(|shape: Vec<Vec<P2>>| shape.into_iter().map(clean).filter(|c| c.len() >= 3).collect::<Vec<_>>())
        .filter(|shape: &Vec<Vec<P2>>| !shape.is_empty())
        .collect()
}

/// The parts of `lines` that lie inside `r`.
pub fn clip_lines(lines: &[Vec<P2>], r: &Region) -> Vec<Vec<P2>> {
    lines.to_vec().clip_by(r, FillRule::NonZero, ClipRule { invert: false, boundary_included: true })
}

/// Whether `p` is inside `r` (its outer contours wind counter-clockwise, its holes clockwise, so the winding number says).
pub fn contains(r: &Region, p: P2) -> bool {
    let mut winding = 0i32;
    for c in r.iter().flatten() {
        let n = c.len();
        for i in 0..n {
            let (a, b) = (c[i], c[(i + 1) % n]);
            let side = (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1]);
            if a[1] <= p[1] && b[1] > p[1] && side > 0.0 {
                winding += 1;
            } else if a[1] > p[1] && b[1] <= p[1] && side < 0.0 {
                winding -= 1;
            }
        }
    }
    winding != 0
}

/// A region's area, mm²: outer contours minus holes.
pub fn area(r: &Region) -> f64 {
    r.iter().flatten().map(|c| crate::slice::area(c)).sum()
}

pub fn is_empty(r: &Region) -> bool {
    r.is_empty() || area(r) < 1e-6
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, s: f64) -> Loop {
        vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s]]
    }

    #[test]
    fn booleans_work_on_regions() {
        let (a, b) = (region(&[square(0.0, 0.0, 10.0)]), region(&[square(5.0, 5.0, 10.0)]));
        assert!((area(&union(&a, &b)) - 175.0).abs() < 1e-6);
        assert!((area(&intersect(&a, &b)) - 25.0).abs() < 1e-6);
        assert!((area(&difference(&a, &b)) - 75.0).abs() < 1e-6);
    }

    #[test]
    fn inset_shrinks_a_square_and_a_hole_grows() {
        let a = region(&[square(0.0, 0.0, 10.0)]);
        assert!((area(&inset(&a, 1.0)) - 64.0).abs() < 1e-6, "{}", area(&inset(&a, 1.0)));
        assert!(is_empty(&inset(&a, 6.0)));
        let mut hole = square(3.0, 3.0, 4.0);
        hole.reverse();
        let ring = region(&[square(0.0, 0.0, 10.0), hole]);
        assert!((area(&ring) - 84.0).abs() < 1e-6);
        assert!((area(&inset(&ring, 1.0)) - (64.0 - 36.0)).abs() < 1e-6, "{}", area(&inset(&ring, 1.0)));
    }

    #[test]
    fn a_point_is_inside_a_region_but_not_in_its_hole() {
        let mut hole = square(3.0, 3.0, 4.0);
        hole.reverse();
        let ring = region(&[square(0.0, 0.0, 10.0), hole]);
        assert!(contains(&ring, [1.0, 1.0]) && !contains(&ring, [5.0, 5.0]) && !contains(&ring, [11.0, 5.0]));
    }

    #[test]
    fn lines_are_cut_to_the_region() {
        let a = region(&[square(0.0, 0.0, 10.0)]);
        let cut = clip_lines(&[vec![[-5.0, 5.0], [15.0, 5.0]]], &a);
        assert_eq!(cut.len(), 1);
        assert!((cut[0][0][0] - cut[0][1][0]).abs() > 9.9 && (cut[0][0][0] - cut[0][1][0]).abs() < 10.1);
    }

    #[test]
    fn straight_runs_are_simplified_and_an_inset_circle_stays_a_clean_circle() {
        // A 48-sided circle whose sides each carry a point in their line, as a mesh's wall quads leave when sliced.
        let corners: Vec<P2> = (0..48)
            .map(|i| {
                let a = std::f64::consts::TAU * f64::from(i) / 48.0;
                [8.0 * a.cos(), 8.0 * a.sin()]
            })
            .collect();
        let mut circle = Vec::new();
        for i in 0..48 {
            let (p, q) = (corners[i], corners[(i + 1) % 48]);
            circle.extend([p, p, [p[0] * 0.37 + q[0] * 0.63, p[1] * 0.37 + q[1] * 0.63]]);
        }
        assert_eq!(simplify(&circle, RESOLUTION).len(), 48);
        let r = region(&[circle]);
        for d in [0.225, 0.675, 1.125] {
            let rings = inset(&r, d);
            let pts = &rings[0][0];
            let radius = 8.0 * (std::f64::consts::PI / 48.0).cos() - d;
            assert_eq!(pts.len(), 48, "inset {d}");
            for (k, p) in pts.iter().enumerate() {
                let q = pts[(k + 1) % pts.len()];
                assert!((p[0] - q[0]).abs() + (p[1] - q[1]).abs() > 0.1, "no repeated point at {d}");
                assert!(p[0].hypot(p[1]) < 8.0 - d + 1e-6 && p[0].hypot(p[1]) > radius - 0.01, "no spike: {p:?} at {d}");
            }
        }
        // A square keeps its corners; a long thin loop is not lost.
        assert_eq!(simplify(&square(0.0, 0.0, 10.0), RESOLUTION).len(), 4);
        assert_eq!(simplify(&[[0.0, 0.0], [5.0, 0.0], [10.0, 0.0], [10.0, 0.01], [0.0, 0.01]], RESOLUTION).len(), 4);
    }
}
