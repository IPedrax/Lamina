//! Layer heights by hand: the plate's curve of layer height against height on the plate (`Scene::layer_heights`, points `[z, h]`),
//! read along straight lines between its points, turned into layer tops, and the edits the `layer_heights` command makes to it.

use crate::slice::MAX_LAYERS;
use lamina_core::scene::{LAYER_HEIGHTS, MAX_HEIGHT_POINTS};

/// The curve's height at `z`: along the line between the points either side, or the nearest point's below the first and above the last.
pub fn at(points: &[[f64; 2]], z: f64) -> f64 {
    let Some(first) = points.first() else { return LAYER_HEIGHTS.1 };
    let k = points.partition_point(|p| p[0] <= z);
    match (k.checked_sub(1).and_then(|i| points.get(i)), points.get(k)) {
        (None, _) => first[1],
        (Some(p), None) => p[1],
        (Some(a), Some(b)) => {
            let span = b[0] - a[0];
            if span <= 0.0 { b[1] } else { a[1] + (b[1] - a[1]) * (z - a[0]) / span }
        }
    }
}

/// The layer tops the curve gives up to `top`: the first `first` mm, then each as thick as the curve at its middle, kept from `lo` to
/// `hi` mm (what the nozzle prints).
pub fn tops(points: &[[f64; 2]], first: f64, top: f64, lo: f64, hi: f64) -> Result<Vec<f64>, String> {
    if !(first.is_finite() && first > 0.0 && lo.is_finite() && lo > 0.0 && hi.is_finite() && hi >= lo && top.is_finite()) {
        return Err("layer heights must be above 0, the smallest no more than the largest".into());
    }
    let mut out = vec![first];
    let mut z = first;
    while z < top - 1e-9 {
        if out.len() >= MAX_LAYERS {
            return Err(format!("the layer heights by hand make more than the {MAX_LAYERS} layers an object may have: make them thicker"));
        }
        // The height at the layer's middle: guessed from its bottom, then read again there.
        let kept = |h: f64| if h.is_finite() { h.clamp(lo, hi) } else { hi };
        let guess = kept(at(points, z));
        let h = kept(at(points, z + guess / 2.0));
        z += h;
        out.push(z);
    }
    Ok(out)
}

/// The curve with the layers from `from` to `to` mm made `height` thick, and the rest as it was (`base` everywhere when there is no
/// curve yet).
pub fn set(points: &[[f64; 2]], base: f64, from: f64, to: f64, height: f64) -> Vec<[f64; 2]> {
    const STEP: f64 = 1e-3;
    let old: Vec<[f64; 2]> = if points.is_empty() { vec![[0.0, base]] } else { points.to_vec() };
    let mut out: Vec<[f64; 2]> = old.iter().copied().filter(|p| p[0] < from - STEP || p[0] > to + STEP).collect();
    // The old heights just outside the band, so the curve changes only inside it.
    if from - STEP >= 0.0 {
        out.push([from - STEP, at(&old, from - STEP)]);
    }
    out.push([to + STEP, at(&old, to + STEP)]);
    out.push([from.max(0.0), height]);
    out.push([to, height]);
    tidy(out)
}

/// The curve smoothed: each height the mean of those within `radius` mm of it, weighed down with distance, sampled every tenth of
/// a millimetre up to `top`.
pub fn smooth(points: &[[f64; 2]], radius: f64, top: f64) -> Vec<[f64; 2]> {
    const STEP: f64 = 0.1;
    let end = top.max(points.last().map_or(0.0, |p| p[0])).clamp(0.0, lamina_core::mesh::MAX_COORD);
    let count = ((end / STEP).ceil().min(MAX_HEIGHT_POINTS as f64) as usize + 1).min(MAX_HEIGHT_POINTS);
    let step = if count > 1 { end / (count - 1) as f64 } else { STEP };
    let samples: Vec<f64> = (0..count).map(|k| at(points, k as f64 * step)).collect();
    let reach = (radius / step).round() as isize;
    let out: Vec<[f64; 2]> = (0..count as isize)
        .map(|k| {
            let (mut sum, mut weight) = (0.0, 0.0);
            for d in -reach..=reach {
                let Some(&h) = usize::try_from(k + d).ok().and_then(|i| samples.get(i)) else { continue };
                let w = 1.0 - d.abs() as f64 / (reach + 1) as f64;
                (sum, weight) = (sum + h * w, weight + w);
            }
            [k as f64 * step, sum / weight]
        })
        .collect();
    tidy(out)
}

/// The curve adaptive layers give (`slice::adaptive_tops`): each layer's height held from its bottom to just under its top, so the
/// curve gives the same layers back. A plate of more layers than the curve holds points for gets each layer's height at its middle.
pub fn from_tops(tops: &[f64]) -> Vec<[f64; 2]> {
    let out: Vec<[f64; 2]> = if 2 * tops.len() <= MAX_HEIGHT_POINTS {
        tops.windows(2).flat_map(|w| [[w[0], w[1] - w[0]], [w[1] - 1e-3, w[1] - w[0]]]).collect()
    } else {
        tops.windows(2).map(|w| [(w[0] + w[1]) / 2.0, w[1] - w[0]]).collect()
    };
    tidy(out)
}

/// Points sorted and rising, each height kept within [`LAYER_HEIGHTS`], with those on the straight line between their neighbours left
/// out, and no more than [`MAX_HEIGHT_POINTS`].
fn tidy(mut points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    points.retain(|p| p[0].is_finite() && p[1].is_finite());
    points.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut rising: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    for p in points {
        let p = [p[0].max(0.0), p[1].clamp(LAYER_HEIGHTS.0, LAYER_HEIGHTS.1)];
        match rising.last_mut() {
            // Two at one height: the later one is the new value.
            Some(last) if p[0] - last[0] < 1e-9 => *last = p,
            _ => rising.push(p),
        }
    }
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(rising.len());
    for (k, &p) in rising.iter().enumerate() {
        let straight = match (out.last(), rising.get(k + 1)) {
            (Some(a), Some(b)) => (at(&[*a, *b], p[0]) - p[1]).abs() < 1e-6,
            _ => false,
        };
        if !straight {
            out.push(p);
        }
    }
    out.truncate(MAX_HEIGHT_POINTS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_is_read_along_lines_and_held_past_its_ends() {
        let c = [[2.0, 0.1], [4.0, 0.3]];
        for (z, h) in [(0.0, 0.1), (2.0, 0.1), (3.0, 0.2), (3.5, 0.25), (4.0, 0.3), (40.0, 0.3)] {
            assert!((at(&c, z) - h).abs() < 1e-12, "{z}: {}", at(&c, z));
        }
    }

    #[test]
    fn a_band_set_by_hand_changes_its_layers_and_no_others() {
        let c = set(&[], 0.2, 4.0, 6.0, 0.1);
        let t = tops(&c, 0.2, 10.0, 0.02, 0.32).unwrap();
        let thick = |a: f64, b: f64| -> Vec<f64> {
            t.windows(2).filter(|w| (w[0] + w[1]) / 2.0 > a && (w[0] + w[1]) / 2.0 < b).map(|w| w[1] - w[0]).collect()
        };
        assert!(thick(0.3, 3.9).iter().all(|h| (h - 0.2).abs() < 1e-9), "{:?}", thick(0.3, 3.9));
        assert!(thick(4.1, 5.9).iter().all(|h| (h - 0.1).abs() < 1e-9), "{:?}", thick(4.1, 5.9));
        assert!(thick(6.2, 10.0).iter().all(|h| (h - 0.2).abs() < 1e-9), "{:?}", thick(6.2, 10.0));
        assert!(*t.last().unwrap() >= 10.0 - 1e-9);
        // A second band over part of the first keeps the rest of the first.
        let d = set(&c, 0.2, 5.0, 8.0, 0.3);
        assert!((at(&d, 4.5) - 0.1).abs() < 1e-9 && (at(&d, 6.0) - 0.3).abs() < 1e-9 && (at(&d, 9.0) - 0.2).abs() < 1e-9);
        assert!(lamina_core::scene::check_heights(&d).is_ok());
        // The nozzle keeps a height asked for: a curve of 1 mm prints at most `hi`.
        let thick_curve = set(&[], 0.2, 0.0, 10.0, 1.0);
        assert!(tops(&thick_curve, 0.2, 10.0, 0.02, 0.32).unwrap().windows(2).all(|w| w[1] - w[0] <= 0.32 + 1e-9));
    }

    #[test]
    fn smoothing_rounds_a_step_and_keeps_a_level_curve_level() {
        let step = set(&[], 0.3, 5.0, 20.0, 0.1);
        let s = smooth(&step, 2.0, 20.0);
        assert!(lamina_core::scene::check_heights(&s).is_ok());
        // Far from the step it is as it was; at the step, between; and it never jumps by the whole step from one sample to the next.
        assert!((at(&s, 1.0) - 0.3).abs() < 1e-9 && (at(&s, 9.0) - 0.1).abs() < 1e-9);
        assert!(at(&s, 5.0) > 0.12 && at(&s, 5.0) < 0.28, "{}", at(&s, 5.0));
        let samples: Vec<f64> = (0..100).map(|k| at(&s, k as f64 * 0.1)).collect();
        assert!(samples.windows(2).all(|w| (w[1] - w[0]).abs() < 0.05));
        let level = smooth(&[[0.0, 0.2]], 3.0, 50.0);
        assert!(level.iter().all(|p| (p[1] - 0.2).abs() < 1e-9) && level.len() <= 2, "{level:?}");
    }

    #[test]
    fn adaptive_tops_become_a_curve_that_gives_them_back() {
        let t = [0.2, 0.4, 0.5, 0.6, 0.7, 0.9, 1.1];
        let c = from_tops(&t);
        let back = tops(&c, 0.2, 1.1, 0.02, 0.32).unwrap();
        assert_eq!(back.len(), t.len(), "{back:?}");
        assert!(back.iter().zip(&t).all(|(a, b)| (a - b).abs() < 0.03), "{back:?}");
    }

    #[test]
    fn hostile_curves_give_heights_in_range_and_never_loop_for_ever() {
        let mut rng = lamina_core::fuzz::Rng::new(11);
        for _ in 0..200 {
            let n = rng.below(20);
            let pts: Vec<[f64; 2]> = (0..n)
                .map(|_| {
                    let pick = |r: &mut lamina_core::fuzz::Rng| match r.below(6) {
                        0 => f64::NAN,
                        1 => -1.0,
                        2 => 1e300,
                        _ => r.below(5000) as f64 / 100.0,
                    };
                    [pick(&mut rng), pick(&mut rng)]
                })
                .collect();
            let tidied = tidy(pts.clone());
            assert!(tidied.windows(2).all(|w| w[1][0] > w[0][0]));
            if let Ok(t) = tops(&pts, 0.2, 30.0, 0.04, 0.32) {
                assert!(t.windows(2).all(|w| w[1] - w[0] >= 0.04 - 1e-9 && w[1] - w[0] <= 0.32 + 1e-9));
            }
            let _ = smooth(&pts, 2.0, 30.0);
            let _ = set(&pts, 0.2, 1.0, 2.0, 0.1);
        }
        // A curve of layers too thin for the height is refused, not ground through.
        assert!(tops(&[[0.0, 0.02]], 0.2, 1000.0, 0.02, 0.32).is_err());
    }
}
