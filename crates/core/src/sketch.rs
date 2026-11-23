//! Sketch and extrude: a flat outline, with holes, lifted into a solid. Most mechanical parts are one: a bracket, a plate with
//! mounting holes, a spacer. The outline is cleaned by `i_overlay` (crossings resolved, holes cut, each closed area a shape), each
//! shape's cap is triangulated by `i_triangle` (MIT OR Apache-2.0, the same author's), and the caps are joined by walls along the
//! caps' own boundary edges, so the solid is closed whatever the outline was.

use std::collections::HashMap;

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use i_triangle::float::triangulatable::Triangulatable;

use crate::mesh::{MAX_COORD, MAX_TRIANGLES, Mesh};

pub type P2 = [f64; 2];

/// The most points an outline (all its contours together) may have.
pub const MAX_POINTS: usize = 100_000;

/// A rectangle `w` by `d` centred on the origin, its corners rounded to `r` (0 for sharp), `segments` steps per quarter turn.
pub fn rounded_rect(w: f64, d: f64, r: f64, segments: u32) -> Vec<P2> {
    let (hx, hy) = (w / 2.0, d / 2.0);
    let r = r.clamp(0.0, hx.min(hy));
    if r <= 0.0 {
        return vec![[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]];
    }
    let n = segments.max(1);
    let mut out = Vec::new();
    for (k, (cx, cy)) in [(hx - r, -hy + r), (hx - r, hy - r), (-hx + r, hy - r), (-hx + r, -hy + r)].into_iter().enumerate() {
        let start = -std::f64::consts::FRAC_PI_2 + std::f64::consts::FRAC_PI_2 * k as f64;
        for i in 0..=n {
            let a = start + std::f64::consts::FRAC_PI_2 * f64::from(i) / f64::from(n);
            out.push([cx + r * a.cos(), cy + r * a.sin()]);
        }
    }
    out
}

/// A circle of radius `r` about (`x`, `y`), counter-clockwise, `segments` sides.
pub fn circle(x: f64, y: f64, r: f64, segments: u32) -> Vec<P2> {
    let n = segments.max(3);
    (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * f64::from(i) / f64::from(n);
            [x + r * a.cos(), y + r * a.sin()]
        })
        .collect()
}

/// A slot: a rectangle `length` by `width` with round ends, along x, centred on the origin.
pub fn slot(length: f64, width: f64, segments: u32) -> Vec<P2> {
    rounded_rect(length.max(width), width, width / 2.0, segments)
}

/// The solid `outline` minus `holes`, `height` tall, its bottom on z = 0.
pub fn extrude(outline: &[P2], holes: &[Vec<P2>], height: f64) -> Result<Mesh, String> {
    if outline.len() < 3 {
        return Err("an outline needs at least three points".into());
    }
    let points = outline.len() + holes.iter().map(Vec::len).sum::<usize>();
    if points > MAX_POINTS {
        return Err(format!("{points} points is more than the {MAX_POINTS} an outline may have"));
    }
    if outline.iter().chain(holes.iter().flatten()).flatten().any(|c| !c.is_finite() || c.abs() > MAX_COORD) {
        return Err(format!("every point must be finite and within {MAX_COORD} mm"));
    }
    if !(height.is_finite() && height > 0.0 && height <= MAX_COORD) {
        return Err("`height` must be a number above 0".into());
    }
    // Clean: the outline filled as it winds (crossings resolved), the holes taken out.
    let subject: Vec<Vec<P2>> = vec![outline.to_vec()];
    let cut: Vec<Vec<P2>> = holes.iter().filter(|h| h.len() >= 3).cloned().collect();
    let shapes = subject.overlay(&cut, OverlayRule::Difference, FillRule::NonZero);
    if shapes.is_empty() {
        return Err("the outline encloses nothing (or the holes take all of it)".into());
    }
    // Each shape is welded on its own: two that touch at a point keep their own walls there, so every edge has two faces.
    let mut solids: Vec<Mesh> = Vec::new();
    let mut total = 0usize;
    for shape in &shapes {
        let mut corners: Vec<[f32; 3]> = Vec::new();
        let t = shape.as_slice().triangulate().to_triangulation::<u32>();
        total += t.indices.len() * 4;
        if total / 3 > MAX_TRIANGLES {
            return Err(format!("the solid would have more than the {MAX_TRIANGLES} triangles a mesh may hold"));
        }
        let at = |i: u32, z: f64| -> Option<[f32; 3]> { t.points.get(i as usize).map(|p| [p[0] as f32, p[1] as f32, z as f32]) };
        // Caps: the triangulation winds counter-clockwise, which faces up; the bottom faces down.
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in t.indices.chunks_exact(3) {
            let (a, b, c) = (tri[0], tri[1], tri[2]);
            if let (Some(pa), Some(pb), Some(pc)) = (at(a, height), at(b, height), at(c, height)) {
                corners.extend([pa, pb, pc]);
            }
            if let (Some(pa), Some(pb), Some(pc)) = (at(a, 0.0), at(b, 0.0), at(c, 0.0)) {
                corners.extend([pa, pc, pb]);
            }
            for (p, q) in [(a, b), (b, c), (c, a)] {
                *edges.entry((p, q)).or_default() += 1;
            }
        }
        // Walls: on each edge the cap uses once, its material on the left, the wall faces right, outward.
        let mut boundary: Vec<(u32, u32)> = edges.keys().copied().filter(|&(p, q)| !edges.contains_key(&(q, p))).collect();
        boundary.sort_unstable();
        for (p, q) in boundary {
            if let (Some(pb), Some(qb), Some(qt), Some(pt)) = (at(p, 0.0), at(q, 0.0), at(q, height), at(p, height)) {
                corners.extend([pb, qb, qt, pb, qt, pt]);
            }
        }
        solids.push(crate::stl::weld(&corners)?);
    }
    let mesh = Mesh::merged(&solids);
    if !mesh.is_watertight() {
        return Err("the outline did not make a closed solid: give it simple, non-touching contours".into());
    }
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plate_with_holes_is_a_closed_solid_of_the_right_volume() {
        let plate = rounded_rect(60.0, 40.0, 5.0, 8);
        let holes: Vec<Vec<P2>> =
            [(-22.0, -12.0), (22.0, -12.0), (22.0, 12.0), (-22.0, 12.0)].iter().map(|&(x, y)| circle(x, y, 2.0, 32)).collect();
        let m = extrude(&plate, &holes, 4.0).unwrap();
        assert!(m.is_watertight());
        // The plate less its rounded corners, less four 32-sided holes of radius 2.
        let area = 60.0 * 40.0 - (4.0 - std::f64::consts::PI) * 25.0 - 4.0 * 0.5 * 32.0 * 4.0 * (std::f64::consts::PI / 16.0).sin();
        assert!((m.volume() - area * 4.0).abs() < 0.002 * area * 4.0, "{} vs {}", m.volume(), area * 4.0);
        let (lo, hi) = m.bounds().unwrap();
        assert!((hi[0] - lo[0] - 60.0).abs() < 1e-4 && lo[2] == 0.0 && (hi[2] - 4.0).abs() < 1e-6);
    }

    #[test]
    fn an_outline_wound_either_way_or_crossing_itself_still_makes_a_solid() {
        let square: Vec<P2> = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let clockwise: Vec<P2> = square.iter().rev().copied().collect();
        for o in [&square, &clockwise] {
            let m = extrude(o, &[], 2.0).unwrap();
            assert!(m.is_watertight() && (m.volume() - 200.0).abs() < 1e-3, "{}", m.volume());
        }
        // An L, with a hole lying outside it (ignored) and a slot cut through it.
        let l: Vec<P2> = vec![[0.0, 0.0], [30.0, 0.0], [30.0, 10.0], [10.0, 10.0], [10.0, 30.0], [0.0, 30.0]];
        let away = circle(100.0, 100.0, 3.0, 12);
        let mut cut = slot(12.0, 4.0, 6);
        cut.iter_mut().for_each(|p| p[0] += 18.0);
        cut.iter_mut().for_each(|p| p[1] += 5.0);
        let m = extrude(&l, &[away, cut], 3.0).unwrap();
        assert!(m.is_watertight() && m.volume() < 500.0 * 3.0 && m.volume() > 450.0 * 3.0, "{}", m.volume());
        // A figure eight crosses itself: two shapes, one solid each, closed.
        let eight: Vec<P2> = vec![[0.0, 0.0], [10.0, 10.0], [20.0, 0.0], [20.0, 10.0], [10.0, 0.0], [0.0, 10.0]];
        assert!(extrude(&eight, &[], 1.0).is_ok_and(|m| m.is_watertight()));
    }

    #[test]
    fn hostile_outlines_are_errors() {
        let ok: Vec<P2> = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        assert!(extrude(&ok[..2], &[], 1.0).is_err());
        assert!(extrude(&[[0.0, 0.0], [f64::NAN, 0.0], [0.0, 1.0]], &[], 1.0).is_err());
        assert!(extrude(&ok, &[], 0.0).is_err());
        assert!(extrude(&[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]], &[], 1.0).is_err(), "no area");
        assert!(extrude(&ok, &[vec![[-1.0, -1.0], [5.0, -1.0], [-1.0, 5.0]]], 1.0).is_err(), "the hole takes it all");
    }
}
