//! Gap fill and thin walls: what the walls and the infill leave unprinted, wider than a quarter of a line, is printed as one line along
//! its middle, its flow following the gap's width. A wall thinner than a line, which the walls cannot print at all, is the same case.
//!
//! The middle of a gap is found as a thin strip's is, without a full straight skeleton: the gap's outline is split at its two points
//! farthest apart into two long sides, one side is walked in short steps, and each step's centre is halfway to the nearest point of
//! the other side, the distance between them its width. For the strips walls leave this is the centreline; for a gap that branches it
//! is a fair line through it. (A straight skeleton or a Voronoi diagram, and walls of varying width as Arachne makes them, would do
//! better; see docs/ROADMAP.md.)

use crate::geom::{self, Region};
use crate::slice::P2;

/// The most points of a gap's outline that are looked at, so a gap with a long ragged edge does not take quadratic time.
const MAX_OUTLINE: usize = 2_000;

/// A line through a gap, and the gap's width at each point of it, mm.
pub type Fill = (Vec<P2>, Vec<f64>);

/// The gaps of a layer whose region is `region`, with `perimeters` walls `w` wide and infill over `inner`, as lines along their middles.
pub fn gaps(region: &Region, perimeters: u32, w: f64, inner: &Region) -> Vec<Fill> {
    if perimeters == 0 || geom::is_empty(region) {
        return Vec::new();
    }
    // What the walls print: each wall's line, a line wide; and the infill's area where it is at least a line wide (a narrower strip
    // of it gets no lines).
    let mut covered = geom::inset(&geom::inset(inner, w / 2.0), -w / 2.0);
    for i in 0..perimeters {
        let line = geom::inset(region, w / 2.0 + f64::from(i) * w);
        if geom::is_empty(&line) {
            break;
        }
        // The line covers a band a line wide round its path: out to half a line beyond it, in to half a line within it.
        let band = geom::difference(&geom::inset(&line, -w / 2.0), &geom::inset(&line, w / 2.0));
        covered = geom::union(&covered, &band);
    }
    let uncovered = geom::difference(region, &covered);
    // Narrower than a quarter of a line is left: the lines beside it flow into it.
    let gaps = geom::inset(&geom::inset(&uncovered, w / 8.0), -w / 8.0);
    gaps.iter().filter_map(|shape| shape.first()).filter_map(|outline| middle(outline, w)).collect()
}

fn dist(a: P2, b: P2) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// The nearest point to `p` on the polyline `side`.
fn nearest(side: &[P2], p: P2) -> P2 {
    let mut best = side[0];
    for s in side.windows(2) {
        let (a, b) = (s[0], s[1]);
        let d = [b[0] - a[0], b[1] - a[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        let t = if len2 < 1e-18 { 0.0 } else { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0) };
        let q = [a[0] + t * d[0], a[1] + t * d[1]];
        if dist(p, q) < dist(p, best) {
            best = q;
        }
    }
    best
}

/// The line along the middle of the gap with this outline, if it is long enough to print.
fn middle(outline: &[P2], w: f64) -> Option<Fill> {
    let n = outline.len();
    if !(3..=MAX_OUTLINE).contains(&n) {
        return None;
    }
    // The two points farthest apart are the gap's ends.
    let (mut i, mut j, mut far) = (0, 0, 0.0);
    for a in 0..n {
        for b in a + 1..n {
            let d = dist(outline[a], outline[b]);
            if d > far {
                (i, j, far) = (a, b, d);
            }
        }
    }
    if far < w {
        return None; // a speck, not a line
    }
    let side_a: Vec<P2> = (i..=j).map(|k| outline[k]).collect();
    let side_b: Vec<P2> = (j..=i + n).map(|k| outline[k % n]).collect();
    // Walk side A in steps of half a line; each step's centre is halfway to side B.
    let mut pts = Vec::new();
    let mut widths = Vec::new();
    let step = w / 2.0;
    for s in side_a.windows(2) {
        let len = dist(s[0], s[1]);
        if len < 1e-9 {
            continue;
        }
        let dir = [(s[1][0] - s[0][0]) / len, (s[1][1] - s[0][1]) / len];
        let steps = (len / step).ceil().max(1.0) as usize;
        for k in 0..steps {
            let t = k as f64 / steps as f64;
            let p = [s[0][0] + t * (s[1][0] - s[0][0]), s[0][1] + t * (s[1][1] - s[0][1])];
            let q = nearest(&side_b, p);
            let width = dist(p, q);
            // Across the gap, the other side is square to the way walked; near an end it is not, and that step is left out.
            let across = width > 1e-9 && ((q[0] - p[0]) * dir[0] + (q[1] - p[1]) * dir[1]).abs() < 0.5 * width;
            if across && (w / 4.0..=2.0 * w).contains(&width) {
                pts.push([(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0]);
                widths.push(width);
            }
        }
    }
    let length: f64 = pts.windows(2).map(|s| dist(s[0], s[1])).sum();
    (pts.len() >= 2 && length >= w).then_some((pts, widths))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Region {
        geom::region(&[vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]])
    }

    #[test]
    fn a_wall_thinner_than_a_line_gets_one_line_down_its_middle() {
        let w = 0.45;
        let thin = rect(0.0, 0.0, 20.0, 0.3);
        assert!(geom::is_empty(&geom::inset(&thin, w / 2.0)), "no wall fits in it");
        let fills = gaps(&thin, 2, w, &Region::new());
        assert_eq!(fills.len(), 1);
        let (pts, widths) = &fills[0];
        assert!(pts.iter().all(|p| (p[1] - 0.15).abs() < 1e-6), "along the middle: {pts:?}");
        let length = pts.first().zip(pts.last()).map_or(0.0, |(a, b)| dist(*a, *b));
        assert!(length > 19.0, "{length}");
        assert!(widths.iter().all(|w| (w - 0.3).abs() < 1e-6), "{widths:?}");
    }

    #[test]
    fn the_strip_left_between_two_walls_is_filled_and_a_wide_part_has_no_gap() {
        let w = 0.45;
        // 1.2 mm thick with two walls each side: the walls meet; with one, a strip of 0.3 mm stays in the middle.
        let wall = rect(0.0, 0.0, 20.0, 1.2);
        let one = gaps(&wall, 1, w, &geom::inset(&wall, w - 0.1 * w));
        assert!(one.iter().any(|(pts, _)| pts.iter().all(|p| (p[1] - 0.6).abs() < 0.05)), "{one:?}");
        let square = rect(0.0, 0.0, 20.0, 20.0);
        assert!(gaps(&square, 2, w, &geom::inset(&square, 2.0 * w - 0.1 * w)).is_empty());
    }
}
