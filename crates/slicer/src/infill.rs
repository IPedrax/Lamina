//! Sparse infill patterns. Lines (the default) alternate their direction each layer; grid crosses two directions on every layer;
//! triangles cross three, at 60 degrees, so the lines meet in a triangular lattice; gyroid follows the curves where the gyroid surface
//! `sin x cos y + sin y cos z + sin z cos x = 0` cuts the layer, found by marching squares, which change from layer to layer and are
//! strong in every direction; honeycomb is a tiling of hexagons. Every pattern is anchored to the plate's origin, so layers and objects
//! line up, and is spaced so the share of the area its lines fill is the density asked for.
//!
//! Clean-room, from the shapes' public definitions (the gyroid is Schoen's minimal surface).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::geom::{self, Region};
use crate::slice::P2;

/// The pattern of the sparse infill.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    #[default]
    Lines,
    Grid,
    Triangles,
    Gyroid,
    Honeycomb,
}

/// For lines that cut a pattern of `density` (0 to 1) with lines `w` wide, the period of the gyroid that leaves the same share filled:
/// a plane section of an isotropic surface has, per area, pi/4 of the surface's area per volume as length, and the gyroid's is about
/// 3.09 over its period.
fn gyroid_period(w: f64, density: f64) -> f64 {
    std::f64::consts::FRAC_PI_4 * 3.09 * w / density
}

/// The polylines of `pattern` at height `z` and layer `k`, cut to `r`, for lines `w` wide at `density`. Each polyline's ends lie on
/// the region's boundary or where the pattern's own lines meet.
pub fn sparse(
    pattern: Pattern,
    r: &Region,
    z: f64,
    k: usize,
    w: f64,
    density: f64,
    lines: impl Fn(&Region, f64, f64) -> Vec<Vec<P2>>,
) -> Vec<Vec<P2>> {
    if density <= 0.0 || geom::is_empty(r) {
        return Vec::new();
    }
    let spacing = w / density;
    match pattern {
        Pattern::Lines => lines(r, spacing, if k.is_multiple_of(2) { 45.0 } else { 135.0 }),
        Pattern::Grid => [45.0, 135.0].iter().flat_map(|&a| lines(r, 2.0 * spacing, a)).collect(),
        Pattern::Triangles => [0.0, 60.0, 120.0].iter().flat_map(|&a| lines(r, 3.0 * spacing, a)).collect(),
        Pattern::Gyroid => order(geom::clip_lines(&gyroid(r, z, gyroid_period(w, density)), r)),
        // A hexagon of side s fills 2 / (sqrt 3 x s) of the area with lines one wide (each edge shared by two cells).
        Pattern::Honeycomb => order(geom::clip_lines(&honeycomb(r, 2.0 * spacing / 3f64.sqrt()), r)),
    }
}

/// The region's box, or none for an empty region.
fn bounds(r: &Region) -> Option<(P2, P2)> {
    let mut it = r.iter().flatten().flatten();
    let first = *it.next()?;
    Some(it.fold((first, first), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])))
}

/// Polylines in an order that sweeps the plate front to back, each starting at the end nearer the last one's end.
fn order(mut polys: Vec<Vec<P2>>) -> Vec<Vec<P2>> {
    polys.retain(|p| p.len() >= 2);
    let mut out: Vec<Vec<P2>> = Vec::with_capacity(polys.len());
    let mut at = [f64::MIN, f64::MIN];
    while !polys.is_empty() {
        let d = |p: &P2| (p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2);
        let near = |p: &Vec<P2>| d(&p[0]).min(d(&p[p.len() - 1]));
        let i = (0..polys.len()).min_by(|&a, &b| near(&polys[a]).total_cmp(&near(&polys[b]))).unwrap_or(0);
        let mut p = polys.swap_remove(i);
        if d(&p[p.len() - 1]) < d(&p[0]) {
            p.reverse();
        }
        at = p[p.len() - 1];
        out.push(p);
    }
    out
}

/// A cell edge of the gyroid's grid: (i, j, 0) runs from grid point (i, j) to (i + 1, j), (i, j, 1) to (i, j + 1).
type Edge = (usize, usize, u8);

/// The gyroid's curves at height `z` over the region's box, `period` mm, as polylines.
fn gyroid(r: &Region, z: f64, period: f64) -> Vec<Vec<P2>> {
    let Some((lo, hi)) = bounds(r) else { return Vec::new() };
    let k = std::f64::consts::TAU / period;
    // Sixteen cells to a period, but never more than 600 a side, so a large plate is coarser rather than slow.
    let cell = (period / 16.0).max((hi[0] - lo[0]).max(hi[1] - lo[1]) / 600.0);
    let x0 = (lo[0] / cell).floor() * cell - cell;
    let y0 = (lo[1] / cell).floor() * cell - cell;
    let (nx, ny) = (((hi[0] - x0) / cell).ceil() as usize + 2, ((hi[1] - y0) / cell).ceil() as usize + 2);
    let (sz, cz) = (k * z).sin_cos();
    let f = |i: usize, j: usize| {
        let (x, y) = (k * (x0 + i as f64 * cell), k * (y0 + j as f64 * cell));
        x.sin() * y.cos() + y.sin() * cz + sz * x.cos()
    };
    let v: Vec<f64> = (0..ny).flat_map(|j| (0..nx).map(move |i| (i, j))).map(|(i, j)| f(i, j)).collect();
    let at = |i: usize, j: usize| v[j * nx + i];
    let point = |i: usize, j: usize| [x0 + i as f64 * cell, y0 + j as f64 * cell];
    // Where the curve crosses a cell edge: edge (i, j, 0) runs from point (i, j) to (i + 1, j), edge (i, j, 1) to (i, j + 1).
    let cross = |e: Edge| -> P2 {
        let (i, j, d) = e;
        let (i2, j2) = if d == 0 { (i + 1, j) } else { (i, j + 1) };
        let (a, b) = (at(i, j), at(i2, j2));
        let t = if (a - b).abs() < 1e-12 { 0.5 } else { a / (a - b) };
        let (p, q) = (point(i, j), point(i2, j2));
        [p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]
    };
    let mut segments: Vec<(Edge, Edge)> = Vec::new();
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let c = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
            let edges = [(i, j, 0u8), (i + 1, j, 1u8), (i, j + 1, 0u8), (i, j, 1u8)]; // bottom, right, top, left
            let crossed: Vec<usize> = (0..4).filter(|&e| (c[e] > 0.0) != (c[(e + 1) % 4] > 0.0)).collect();
            match crossed.as_slice() {
                [a, b] => segments.push((edges[*a], edges[*b])),
                [a, b, cc, d] => {
                    // A saddle: the middle's sign says which corners join.
                    let middle = c.iter().sum::<f64>() / 4.0;
                    if (middle > 0.0) == (c[0] > 0.0) {
                        segments.push((edges[*a], edges[*d]));
                        segments.push((edges[*b], edges[*cc]));
                    } else {
                        segments.push((edges[*a], edges[*b]));
                        segments.push((edges[*cc], edges[*d]));
                    }
                }
                _ => {}
            }
        }
    }
    chain(&segments).into_iter().map(|p| p.into_iter().map(cross).collect()).collect()
}

/// Joins segments that share an end into polylines, each segment used once.
fn chain<K: Copy + Eq + std::hash::Hash + Ord>(segments: &[(K, K)]) -> Vec<Vec<K>> {
    let mut at: HashMap<K, Vec<usize>> = HashMap::new();
    for (n, &(a, b)) in segments.iter().enumerate() {
        at.entry(a).or_default().push(n);
        at.entry(b).or_default().push(n);
    }
    let mut used = vec![false; segments.len()];
    let mut out = Vec::new();
    // Start at loose ends first (ends of open curves), then whatever is left (closed curves).
    let mut starts: Vec<usize> = (0..segments.len()).collect();
    starts.sort_by_key(|&n| {
        let (a, b) = segments[n];
        std::cmp::Reverse(at.get(&a).is_some_and(|v| v.len() == 1) || at.get(&b).is_some_and(|v| v.len() == 1))
    });
    for first in starts {
        if used[first] {
            continue;
        }
        used[first] = true;
        let (a, b) = segments[first];
        let (a, b) = if at.get(&b).is_some_and(|v| v.len() == 1) { (b, a) } else { (a, b) };
        let mut line = vec![a, b];
        let mut end = b;
        while let Some(&n) = at.get(&end).and_then(|v| v.iter().find(|&&n| !used[n])) {
            used[n] = true;
            let (p, q) = segments[n];
            end = if p == end { q } else { p };
            line.push(end);
        }
        out.push(line);
    }
    out
}

/// A tiling of hexagons of side `s` (pointy side up, anchored to the origin) over the region's box, each edge once, as polylines.
fn honeycomb(r: &Region, s: f64) -> Vec<Vec<P2>> {
    let Some((lo, hi)) = bounds(r) else { return Vec::new() };
    let (w, h) = (3f64.sqrt() * s, 1.5 * s);
    let rows = ((lo[1] / h).floor() as i64 - 1)..=((hi[1] / h).ceil() as i64 + 1);
    let cols = ((lo[0] / w).floor() as i64 - 1)..=((hi[0] / w).ceil() as i64 + 1);
    if rows.clone().count() * cols.clone().count() > 400_000 {
        return Vec::new(); // a hexagon smaller than the region could hold: no pattern rather than hours
    }
    // Corners keyed by a grid of half-steps, so cells that share a corner name it alike.
    let key = |p: P2| ((p[0] / (w / 2.0)).round() as i64, (p[1] / (s / 2.0)).round() as i64);
    let mut segments: Vec<((i64, i64), (i64, i64))> = Vec::new();
    let mut place: HashMap<(i64, i64), P2> = HashMap::new();
    for row in rows {
        for col in cols.clone() {
            let cx = col as f64 * w + if row.rem_euclid(2) == 1 { w / 2.0 } else { 0.0 };
            let cy = row as f64 * h;
            let corners: Vec<P2> = (0..6)
                .map(|i| {
                    let a = std::f64::consts::FRAC_PI_6 + std::f64::consts::FRAC_PI_3 * f64::from(i);
                    [cx + s * a.cos(), cy + s * a.sin()]
                })
                .collect();
            for i in 0..6 {
                let (p, q) = (corners[i], corners[(i + 1) % 6]);
                let (kp, kq) = (key(p), key(q));
                place.entry(kp).or_insert(p);
                place.entry(kq).or_insert(q);
                segments.push(if kp < kq { (kp, kq) } else { (kq, kp) });
            }
        }
    }
    segments.sort_unstable();
    segments.dedup();
    chain(&segments).into_iter().map(|line| line.into_iter().filter_map(|k| place.get(&k).copied()).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(s: f64) -> Region {
        geom::region(&[vec![[0.0, 0.0], [s, 0.0], [s, s], [0.0, s]]])
    }

    fn length(polys: &[Vec<P2>]) -> f64 {
        polys.iter().flat_map(|p| p.windows(2).map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt())).sum()
    }

    /// Plain parallel lines, as `print::fill_lines` makes them, without its joins: enough to measure.
    fn lines(r: &Region, spacing: f64, angle: f64) -> Vec<Vec<P2>> {
        let (s, c) = angle.to_radians().sin_cos();
        let (d, n) = ([c, s], [-s, c]);
        let raw: Vec<Vec<P2>> = (-200..200)
            .map(|k| {
                let o = [k as f64 * spacing * n[0], k as f64 * spacing * n[1]];
                vec![[o[0] - 500.0 * d[0], o[1] - 500.0 * d[1]], [o[0] + 500.0 * d[0], o[1] + 500.0 * d[1]]]
            })
            .collect();
        geom::clip_lines(&raw, r)
    }

    #[test]
    fn every_pattern_fills_about_the_share_asked_for_and_stays_inside() {
        let r = square(40.0);
        let (w, density) = (0.45, 0.2);
        let want = 40.0 * 40.0 * density / w;
        for pattern in [Pattern::Lines, Pattern::Grid, Pattern::Triangles, Pattern::Gyroid, Pattern::Honeycomb] {
            let polys = sparse(pattern, &r, 5.0, 3, w, density, lines);
            let got = length(&polys);
            assert!((got - want).abs() < 0.15 * want, "{pattern:?}: {got:.0} mm of line, {want:.0} wanted");
            assert!(polys.iter().flatten().all(|p| p[0] >= -1e-6 && p[0] <= 40.0 + 1e-6 && p[1] >= -1e-6 && p[1] <= 40.0 + 1e-6));
        }
    }

    #[test]
    fn the_gyroid_changes_from_layer_to_layer_and_comes_back_after_a_period() {
        let r = square(20.0);
        let p = gyroid_period(0.45, 0.2);
        let at = |z: f64| sparse(Pattern::Gyroid, &r, z, 0, 0.45, 0.2, lines);
        let (a, b, c) = (at(1.0), at(1.0 + p / 4.0), at(1.0 + p));
        assert!((length(&a) - length(&c)).abs() < 0.02 * length(&a), "periodic in z");
        let first = |v: &Vec<Vec<P2>>| v.iter().flatten().map(|p| (p[0] * 100.0).round() as i64).min();
        assert_ne!(first(&a), first(&b), "a quarter period up, the curves have moved");
    }

    #[test]
    fn segments_chain_into_polylines_using_each_once() {
        let segs = [(1, 2), (3, 4), (2, 3), (7, 8), (8, 9), (9, 7)];
        let mut lines = chain(&segs);
        lines.sort();
        assert_eq!(lines.iter().map(|l| l.len()).sum::<usize>(), 4 + 4, "one open line of four points, one closed loop");
        assert!(lines.iter().any(|l| l == &[1, 2, 3, 4] || l == &[4, 3, 2, 1]));
    }
}
