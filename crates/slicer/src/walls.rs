//! Walls of varying width (`wall_generator: "variable"`), in the spirit of what PrusaSlicer and Cura document for Arachne, written
//! from that description and the geometry, never their code. Classic walls are loops a line wide, one inside the other, so a part
//! thinner than its walls need gets loops that fold onto themselves (twice the plastic a thin fin needs) or nothing, and gap fill
//! for what is left. Here the parts of a layer too thin for all its walls are found first, and each is printed with as many lines
//! as its width holds, each as wide as the width shared out between them:
//!
//! 1. The thin parts: what the region loses when it is opened by the walls' depth (moved in by `perimeters` lines and out again),
//!    without what is narrower than a quarter line. A notch or a corner that the opening rounds is no thin part: a part counts only
//!    if a line along its middle is at least two lines long.
//! 2. A thin strip's middle and its width at each point come from walking one long side of it and meeting the other
//!    ([`crate::gap::middle`]). At each point the width holds `n = round(width / line)` lines, from one to twice the walls, each
//!    `width / n` wide, set side by side across the middle.
//! 3. A thin ring (a part with one hole, as a thin tube's wall is) takes its mean width from its area and outlines, and its lines
//!    are loops inset from the outside by half a line's share, one share apart.
//! 4. The classic walls print everywhere else; where they cross a thin part they are cut off, as its own lines take over.

use crate::geom::{self, Region};
use crate::slice::P2;

/// One line of a thin part: its points, its width at each, and whether it is on the part's surface (an outer wall).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub pts: Vec<P2>,
    pub widths: Vec<f64>,
    pub closed: bool,
    pub outer: bool,
}

/// How walls are made: loops a line wide (`classic`, the default), or lines that share a thin part's width (`variable`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallGenerator {
    #[default]
    Classic,
    Variable,
}

/// The parts of `region` too thin for `perimeters` walls `w` wide on each side, and the lines that print them.
pub fn thin(region: &Region, perimeters: u32, w: f64) -> (Region, Vec<Line>) {
    let mut area = Region::new();
    let mut lines = Vec::new();
    if perimeters == 0 || geom::is_empty(region) {
        return (area, lines);
    }
    let depth = f64::from(perimeters) * w;
    let opened = geom::open(region, depth);
    let rest = geom::difference(region, &opened);
    // Narrower than a quarter line is left to the lines beside it, as gap fill leaves it.
    let rest = geom::inset(&geom::inset(&rest, w / 8.0), -w / 8.0);
    let most = 2 * perimeters as usize;
    for shape in rest {
        match shape.len() {
            1 => {
                let Some((mut pts, mut widths)) = crate::gap::middle(&shape[0], w, 2.0 * depth + w / 2.0) else { continue };
                // The walk along the sides stops short of the ends, where the other side is no longer across: on to the outline.
                reach_ends(&shape[0], &mut pts, &mut widths, w);
                let length: f64 = pts.windows(2).map(|s| dist(s[0], s[1])).sum();
                if length < 2.0 * w {
                    continue; // a corner the opening rounded, not a strip
                }
                lines.extend(strip(&pts, &widths, w, most));
                area.push(shape);
            }
            2 => {
                let ring = ring(&shape, w, most);
                if !ring.is_empty() {
                    lines.extend(ring);
                    area.push(shape);
                }
            }
            _ => {} // a thin part with several holes keeps its classic walls
        }
    }
    (area, lines)
}

fn dist(a: P2, b: P2) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// How far from `from` along the unit direction `dir` the closed outline is first met, if it is.
fn ray(outline: &[P2], from: P2, dir: P2) -> Option<f64> {
    let n = outline.len();
    (0..n)
        .filter_map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            let e = [b[0] - a[0], b[1] - a[1]];
            let det = dir[0] * (-e[1]) - dir[1] * (-e[0]);
            if det.abs() < 1e-12 {
                return None;
            }
            let r = [a[0] - from[0], a[1] - from[1]];
            let t = (r[0] * (-e[1]) - r[1] * (-e[0])) / det;
            let u = (dir[0] * r[1] - dir[1] * r[0]) / det;
            (t > 1e-9 && (0.0..=1.0).contains(&u)).then_some(t)
        })
        .min_by(|x, y| x.total_cmp(y))
}

/// Each end of a strip's middle carried on along its last step to a quarter line short of the outline, at the width it ends with,
/// so the strip is printed to its ends.
fn reach_ends(outline: &[P2], pts: &mut Vec<P2>, widths: &mut Vec<f64>, w: f64) {
    for end in [true, false] {
        let n = pts.len();
        if n < 2 {
            return;
        }
        let (tip, before, width) = if end { (pts[n - 1], pts[n - 2], widths[n - 1]) } else { (pts[0], pts[1], widths[0]) };
        let l = dist(tip, before);
        if l < 1e-9 {
            continue;
        }
        let dir = [(tip[0] - before[0]) / l, (tip[1] - before[1]) / l];
        let Some(t) = ray(outline, tip, dir) else { continue };
        let go = t - w / 4.0;
        if go > w / 4.0 && go < 4.0 * width.max(w) {
            let q = [tip[0] + dir[0] * go, tip[1] + dir[1] * go];
            if end {
                pts.push(q);
                widths.push(width);
            } else {
                pts.insert(0, q);
                widths.insert(0, width);
            }
        }
    }
}

/// How many lines a width holds, from one to `most`.
fn count(width: f64, w: f64, most: usize) -> usize {
    ((width / w).round() as usize).clamp(1, most.max(1))
}

/// A strip's lines from its middle `pts` and widths: in runs where the count of lines stays the same, each line set off the middle
/// by its place across the width.
fn strip(pts: &[P2], widths: &[f64], w: f64, most: usize) -> Vec<Line> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < pts.len() {
        let n = count(widths[start], w, most);
        let mut end = start + 1;
        while end < pts.len() && count(widths[end], w, most) == n {
            end += 1;
        }
        // Each run shares its first point with the last one's end, so the lines go on without a gap.
        let from = start.saturating_sub(1);
        let run = &pts[from..end];
        if run.len() >= 2 {
            for j in 0..n {
                let mut line = Line { pts: Vec::with_capacity(run.len()), widths: Vec::new(), closed: false, outer: j == 0 || j + 1 == n };
                for (k, &q) in run.iter().enumerate() {
                    let (a, b) = (run[k.saturating_sub(1)], run[(k + 1).min(run.len() - 1)]);
                    let l = dist(a, b).max(1e-12);
                    let normal = [-(b[1] - a[1]) / l, (b[0] - a[0]) / l];
                    let width = widths[from + k];
                    let share = width / n as f64;
                    let off = (j as f64 - (n as f64 - 1.0) / 2.0) * share;
                    line.pts.push([q[0] + normal[0] * off, q[1] + normal[1] * off]);
                    line.widths.push(share);
                }
                out.push(line);
            }
        }
        start = end;
    }
    out
}

/// A thin ring's lines: loops inset from its outside by half a line's share, one share apart, the share its mean width (from its
/// area and the length of its two outlines) over the count of lines that width holds.
fn ring(shape: &[Vec<P2>], w: f64, most: usize) -> Vec<Line> {
    let around: f64 = shape.iter().map(|c| (0..c.len()).map(|i| dist(c[i], c[(i + 1) % c.len()])).sum::<f64>()).sum();
    let region = vec![shape.to_vec()];
    let width = 2.0 * geom::area(&region) / around.max(1e-9);
    if width < w / 4.0 {
        return Vec::new();
    }
    let n = count(width, w, most);
    let share = width / n as f64;
    let mut out = Vec::new();
    for j in 0..n {
        for loop_shape in geom::inset(&region, (j as f64 + 0.5) * share) {
            for c in loop_shape {
                let widths = vec![share; c.len()];
                out.push(Line { pts: c, widths, closed: true, outer: j == 0 || j + 1 == n });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<P2> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    /// The plastic a set of lines lays per mm of height, as width times length summed.
    fn laid(lines: &[Line]) -> f64 {
        lines
            .iter()
            .map(|l| {
                let mut a = 0.0;
                for k in 1..l.pts.len() {
                    a += dist(l.pts[k - 1], l.pts[k]) * (l.widths[k - 1] + l.widths[k]) / 2.0;
                }
                if l.closed && l.pts.len() > 2 {
                    a += dist(l.pts[l.pts.len() - 1], l.pts[0]) * l.widths[0];
                }
                a
            })
            .sum()
    }

    #[test]
    fn a_fin_thinner_than_a_line_is_one_line_as_wide_as_the_fin() {
        let w = 0.45;
        let fin = geom::region(&[rect(0.0, 0.0, 20.0, 0.6)]);
        let (area, lines) = thin(&fin, 2, w);
        assert!((geom::area(&area) - 12.0).abs() < 0.1, "the whole fin: {}", geom::area(&area));
        assert_eq!(lines.len(), 1, "one line");
        assert!(lines[0].pts.iter().all(|p| (p[1] - 0.3).abs() < 1e-6), "down the middle");
        assert!(lines[0].widths.iter().all(|&x| (x - 0.6).abs() < 1e-6), "0.6 mm wide");
        assert!((laid(&lines) - 12.0).abs() < 0.7, "its plastic: {}", laid(&lines));
    }

    #[test]
    fn a_wall_a_little_over_two_lines_is_two_lines_sharing_its_width() {
        let w = 0.45;
        let wall = geom::region(&[rect(0.0, 0.0, 20.0, 1.0)]);
        let (_, lines) = thin(&wall, 2, w);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.outer && l.widths.iter().all(|&x| (x - 0.5).abs() < 1e-6)), "{lines:?}");
        let ys: Vec<f64> = lines.iter().map(|l| l.pts[l.pts.len() / 2][1]).collect();
        assert!(ys.iter().any(|y| (y - 0.25).abs() < 1e-6) && ys.iter().any(|y| (y - 0.75).abs() < 1e-6), "{ys:?}");
    }

    #[test]
    fn a_wedge_gets_one_line_widening_then_more_and_its_area_in_plastic() {
        let w = 0.45;
        // 30 mm long, from 0.3 mm wide to 2 mm.
        let wedge = geom::region(&[vec![[0.0, -0.15], [30.0, -1.0], [30.0, 1.0], [0.0, 0.15]]]);
        let (area, lines) = thin(&wedge, 2, w);
        // The width over a line's: 0.92 at x 2, 1.93 at 10, 2.93 at 18, 3.56 at 23, so one line, two, three, then four (twice the
        // walls). From about x 25.6, where it is 1.75 mm, two walls fit each side: no longer thin, the classic walls print it.
        let at = |x: f64| lines.iter().filter(|l| l.pts.iter().any(|p| (p[0] - x).abs() < 0.3)).count();
        assert_eq!([at(2.0), at(10.0), at(18.0), at(23.0), at(28.0)], [1, 2, 3, 4, 0]);
        let reach = area.iter().flatten().flatten().map(|p| p[0]).fold(0.0, f64::max);
        assert!((25.0..27.0).contains(&reach), "{reach}");
        // The thin part's area in plastic, within a tenth.
        let want = geom::area(&area);
        assert!((laid(&lines) - want).abs() < 0.1 * want, "{} against {want}", laid(&lines));
        for l in &lines {
            assert!(l.widths.iter().all(|&x| (w / 4.0 - 1e-9..=1.5 * w).contains(&x)), "{:?}", l.widths);
        }
    }

    #[test]
    fn a_thin_tube_wall_is_loops_and_a_thick_part_has_no_thin_parts() {
        let w = 0.45;
        let circle =
            |r: f64| (0..64).map(|i| f64::from(i) * std::f64::consts::TAU / 64.0).map(|a| [r * a.cos(), r * a.sin()]).collect::<Vec<_>>();
        let mut hole = circle(9.0);
        hole.reverse();
        let tube = geom::region(&[circle(10.0), hole]);
        let (area, lines) = thin(&tube, 2, w);
        assert!((geom::area(&area) - geom::area(&tube)).abs() < 0.5, "the whole wall is thin");
        assert_eq!(lines.len(), 2, "two loops share its 1 mm");
        assert!(
            lines.iter().all(|l| l.closed && l.widths.iter().all(|&x| (x - 0.5).abs() < 0.02)),
            "{:?}",
            lines.iter().map(|l| l.widths[0]).collect::<Vec<_>>()
        );
        // A 20 mm square has nothing thin; its corners are no strips.
        let square = geom::region(&[rect(0.0, 0.0, 20.0, 20.0)]);
        let (area, lines) = thin(&square, 2, w);
        assert!(geom::is_empty(&area) && lines.is_empty());
        // No walls, nothing.
        assert!(thin(&tube, 0, w).1.is_empty());
    }
}
