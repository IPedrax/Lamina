//! Supports: what holds up the parts of a model that would print in the air. Clean-room, from how slicers document the setting:
//!
//! 1. A layer's overhang is what it has beyond the layer below grown by `layer height x tan(support_angle)`: a wall leaning further
//!    from vertical than the angle moves out faster than that.
//! 2. Overhangs are carried down layer by layer, and stop where the model is (support rests on the model, or, with `from_plate`, only
//!    the columns that reach the plate are kept).
//! 3. What is printed keeps `support_xy_gap` from the model's walls and stops `support_z_gap` below it, so it breaks away.
//! 4. The top `support_interface_layers` under the model are denser, to give the overhang a surface to print on.

use serde::{Deserialize, Serialize};

use crate::geom::{self, Region};
use crate::profile::Profile;

/// Where supports go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    /// None.
    #[default]
    Off,
    /// Under every overhang, standing on the plate or on the model.
    Everywhere,
    /// Only under overhangs whose column reaches the plate, so nothing rests on the model's own surfaces.
    FromPlate,
}

/// One layer's supports: the sparse part and the dense interface just under the model.
#[derive(Clone, Debug, Default)]
pub struct Layer {
    pub sparse: Region,
    pub interface: Region,
}

/// The supports of each layer of an object whose layers are `regions` (bottom first), each `heights[k]` thick.
pub fn plan(regions: &[Region], heights: &[f64], p: &Profile) -> Vec<Layer> {
    let n = regions.len();
    let mut out = vec![Layer::default(); n];
    if p.support == Support::Off || n < 2 {
        return out;
    }
    let w = p.line_width();
    // 1. Overhangs. What lies beyond the allowed lean says where a layer is too steep; what is held up there is all of the layer that is
    // over air, so a steep slope gets one area of support and not a comb of the strips each layer adds beyond the lean.
    let tan = p.support_angle.to_radians().tan();
    let overhang: Vec<Region> = (0..n)
        .map(|k| {
            if k == 0 {
                return Region::new();
            }
            let lean = heights[k] * tan;
            let steep = geom::difference(&regions[k], &geom::inset(&regions[k - 1], -lean));
            if geom::is_empty(&steep) {
                return Region::new();
            }
            geom::intersect(&geom::difference(&regions[k], &regions[k - 1]), &geom::inset(&steep, -(lean + 0.01)))
        })
        .collect();
    if overhang.iter().all(geom::is_empty) {
        return out;
    }
    // 2. Carried down: what needs holding at layer k is everything above it that is not yet resting on the model.
    let mut carry = vec![Region::new(); n];
    for k in (0..n - 1).rev() {
        let above = geom::union(&carry[k + 1], &overhang[k + 1]);
        carry[k] = geom::difference(&above, &regions[k]);
    }
    if p.support == Support::FromPlate {
        // Only the columns that stand on the plate: each layer keeps what the layer below it keeps.
        for k in 1..n {
            carry[k] = geom::intersect(&carry[k], &carry[k - 1]);
        }
    }
    // 3. Gaps: from the walls by the xy gap, and below the model by whole layers.
    let gap_layers = (p.support_z_gap / p.layer_height).round().max(0.0) as usize;
    let near = |from: usize, to: usize| -> Region {
        (from..=to.min(n - 1)).fold(Region::new(), |a, j| geom::union(&a, &geom::inset(&regions[j], -p.support_xy_gap)))
    };
    for k in 0..n - 1 {
        if geom::is_empty(&carry[k]) {
            continue;
        }
        let mut area = geom::difference(&carry[k], &geom::inset(&regions[k], -p.support_xy_gap));
        if gap_layers > 0 {
            area = geom::difference(&area, &near(k + 1, k + gap_layers));
        }
        // What is narrower than a line cannot be printed: the sliver a curved surface leaves at the angle.
        area = geom::inset(&geom::inset(&area, w / 2.0), -w / 2.0);
        // 4. The interface: the part with the model within a few layers above the gap.
        let layers = p.support_interface_layers as usize;
        let interface = if layers > 0 { geom::intersect(&area, &near(k + 1 + gap_layers, k + gap_layers + layers)) } else { Region::new() };
        let sparse = geom::difference(&area, &interface);
        out[k] = Layer { sparse, interface };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slice::Loop;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Loop {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    /// A T: a 6 mm pillar 4 mm tall (20 layers), then arms 18 mm wide for 2 mm (10 layers).
    fn tee() -> (Vec<Region>, Vec<f64>) {
        let regions: Vec<Region> = (0..30)
            .map(|k| if k < 20 { geom::region(&[rect(-3.0, -3.0, 3.0, 3.0)]) } else { geom::region(&[rect(-9.0, -3.0, 9.0, 3.0)]) })
            .collect();
        (regions, vec![0.2; 30])
    }

    #[test]
    fn a_tee_gets_support_under_its_arms_only_stopping_a_gap_below_them() {
        let (regions, heights) = tee();
        let p = Profile { support: Support::Everywhere, ..Profile::default() };
        let s = plan(&regions, &heights, &p);
        let area = |l: &Layer| geom::area(&l.sparse) + geom::area(&l.interface);
        // Under the arms, two columns 9 - 3 - gap wide, 6 deep; nothing beside the pillar's own footprint.
        let side = 9.0 - 3.0 - p.support_xy_gap;
        assert!((area(&s[0]) - 2.0 * side * 6.0).abs() < 0.5, "{}", area(&s[0]));
        // The z gap of one layer: layer 19 (just under the arms, which start at layer 20) is empty, 18 is not.
        assert!(area(&s[19]) < 1e-6 && area(&s[18]) > 1.0, "{} {}", area(&s[19]), area(&s[18]));
        // The interface is the two layers just under the gap; below, the support is sparse.
        assert!(geom::area(&s[18].interface) > 1.0 && geom::area(&s[17].interface) > 1.0 && geom::area(&s[16].interface) < 1e-6);
        assert!((20..30).all(|k| area(&s[k]) < 1e-6), "nothing above the arms' underside");
        // Off is off.
        assert!(plan(&regions, &heights, &Profile::default()).iter().all(|l| geom::is_empty(&l.sparse)));
    }

    #[test]
    fn from_the_plate_only_drops_support_that_would_stand_on_the_model() {
        // A wide base 2 mm tall, the pillar, then the arms over the base: their support would stand on the base.
        let mut regions: Vec<Region> = (0..10).map(|_| geom::region(&[rect(-10.0, -3.0, 10.0, 3.0)])).collect();
        regions.extend(tee().0.into_iter().skip(10));
        let heights = vec![0.2; regions.len()];
        let everywhere = plan(&regions, &heights, &Profile { support: Support::Everywhere, ..Profile::default() });
        assert!(geom::area(&everywhere[15].sparse) > 10.0, "it stands on the base");
        assert!(everywhere[..10].iter().all(|l| geom::is_empty(&l.sparse) && geom::is_empty(&l.interface)), "none inside the base");
        let plate = plan(&regions, &heights, &Profile { support: Support::FromPlate, ..Profile::default() });
        assert!(plate.iter().all(|l| geom::is_empty(&l.sparse) && geom::is_empty(&l.interface)));
    }

    #[test]
    fn a_gentle_slope_needs_no_support_and_a_steep_one_does() {
        // Each layer 0.1 mm wider than the last (27 degrees from vertical at 0.2 mm layers), then 0.4 mm (63 degrees). The steep slope
        // is held up all along its underside, not in strips.
        let grow = |step: f64| -> Vec<Region> { (0..20).map(|k| geom::region(&[rect(0.0, 0.0, 5.0 + step * k as f64, 5.0)])).collect() };
        let p = Profile { support: Support::Everywhere, ..Profile::default() };
        assert!(plan(&grow(0.1), &[0.2; 20], &p).iter().all(|l| geom::is_empty(&l.sparse) && geom::is_empty(&l.interface)));
        let steep = plan(&grow(0.4), &[0.2; 20], &p);
        let total = geom::area(&steep[2].sparse) + geom::area(&steep[2].interface);
        // At layer 2 the slope above reaches 12.6 mm; for the z gap the support keeps clear of layer 3 (5 + 3 x 0.4 mm) by the xy gap.
        assert!((total - (12.6 - (5.0 + 3.0 * 0.4) - 0.6) * 5.0).abs() < 0.5, "{total}");
    }
}
