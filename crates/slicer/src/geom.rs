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

/// The region the loops of one layer enclose: the outer loops counter-clockwise, holes clockwise, as `slice::layers` winds them.
pub fn region(loops: &[Loop]) -> Region {
    let none: Region = Vec::new();
    loops.to_vec().overlay(&none, OverlayRule::Subject, FillRule::NonZero)
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
    r.iter().flat_map(|shape| shape.outline(&style)).collect()
}

/// The parts of `lines` that lie inside `r`.
pub fn clip_lines(lines: &[Vec<P2>], r: &Region) -> Vec<Vec<P2>> {
    lines.to_vec().clip_by(r, FillRule::NonZero, ClipRule { invert: false, boundary_included: true })
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
    fn lines_are_cut_to_the_region() {
        let a = region(&[square(0.0, 0.0, 10.0)]);
        let cut = clip_lines(&[vec![[-5.0, 5.0], [15.0, 5.0]]], &a);
        assert_eq!(cut.len(), 1);
        assert!((cut[0][0][0] - cut[0][1][0]).abs() > 9.9 && (cut[0][0][0] - cut[0][1][0]).abs() < 10.1);
    }
}
