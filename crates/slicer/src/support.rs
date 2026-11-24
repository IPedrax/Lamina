//! Supports: what holds up the parts of a model that would print in the air. Clean-room, from how slicers document the setting:
//!
//! 1. A layer's overhang is what it has beyond the layer below grown by `layer height x tan(support_angle)`: a wall leaning further
//!    from vertical than the angle moves out faster than that.
//! 2. Overhangs are carried down layer by layer, and stop where the model is (support rests on the model, or, with `from_plate`, only
//!    the columns that reach the plate are kept).
//! 3. What is printed keeps `support_xy_gap` from the model's walls and stops `support_z_gap` below it, so it breaks away.
//! 4. The top `support_interface_layers` under the model are denser, to give the overhang a surface to print on.
//!
//! Trees (`support_style: "tree"`) keep the interface where their tips hold it and replace the columns under it, as tree supports are
//! described:
//!
//! 5. Tips stand under the bottom of each column's top, `tree_tip_spacing` apart, on the layer below it.
//! 6. Walked down a layer at a time, each branch leans toward the nearest branch it can still meet before the plate, at most
//!    `tree_branch_angle` from vertical, and stays out of the region from which the plate cannot be reached without passing
//!    through the model (the model grown by the xy gap, and that region's shadow, shrunk by the lean for each layer it rises).
//!    Branches that meet go on as one, thicker for the tips they carry and for how far they have come down.
//! 7. A branch that cannot reach the plate rests on the model, or with `from_plate` is dropped with every tip it carries.
//! 8. Each layer's branches are the union of their circles, kept out of the model by the gaps as the columns are.
//! 9. The interface stays only within reach of a tip whose branch was kept, or that rests on the model: a branch dropped with
//!    `from_plate` takes the interface it would have held with it, so none is left over air.
//!
//! Supports are planned for the plate, not for one object alone: `solid` is everything on each layer (the object and the others
//! beside it), which support stands on, keeps the xy gap from and goes round, while only the object's own layers make overhangs.

use serde::{Deserialize, Serialize};

use crate::geom::{self, Region};
use crate::profile::Profile;
use crate::slice::{Loop, P2};

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

/// How the support under an overhang is built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportStyle {
    /// Straight columns under the whole overhang, filled with lines.
    #[default]
    Columns,
    /// Branches from small tips under the overhang, merging into trunks on the way down.
    Tree,
}

/// One layer's supports: the sparse part and the dense interface just under the model.
#[derive(Clone, Debug, Default)]
pub struct Layer {
    pub sparse: Region,
    pub interface: Region,
}

/// The supports of each layer of an object whose layers are `regions` (bottom first), each `heights[k]` thick. `solid[k]` is all that
/// stands on the plate's layer k, this object with the others (the plate's layers are every object's), and holds at least as many
/// layers as `regions`; for an object printed alone it is `regions`.
pub fn plan(regions: &[Region], solid: &[Region], heights: &[f64], p: &Profile) -> Vec<Layer> {
    plan_with(regions, solid, heights, p, &Volumes::default())
}

/// What support enforcers and blockers cover on each of the plate's layers (each modifier object's slice).
#[derive(Clone, Debug, Default)]
pub struct Volumes {
    pub enforce: Vec<Region>,
    pub block: Vec<Region>,
}

/// The same, with the plate's support enforcers and blockers: the overhang inside a blocker needs no support, and every part of a
/// layer over air inside an enforcer does, whatever its angle and even with supports off.
pub fn plan_with(regions: &[Region], solid: &[Region], heights: &[f64], p: &Profile, v: &Volumes) -> Vec<Layer> {
    let n = regions.len().min(solid.len());
    let mut out = vec![Layer::default(); n];
    let enforced = v.enforce.iter().take(n).any(|r| !geom::is_empty(r));
    if (p.support == Support::Off && !enforced) || n < 2 {
        return out;
    }
    let w = p.line_width();
    // 1. Overhangs. What lies beyond the allowed lean says where a layer is too steep; what is held up there is all of the layer that is
    // over air, so a steep slope gets one area of support and not a comb of the strips each layer adds beyond the lean.
    let tan = p.support_angle.to_radians().tan();
    let none = Region::new();
    let overhang: Vec<Region> = (0..n)
        .map(|k| {
            if k == 0 {
                return Region::new();
            }
            let lean = heights[k] * tan;
            let steep = geom::difference(&regions[k], &geom::inset(&regions[k - 1], -lean));
            let by_angle = if p.support == Support::Off || geom::is_empty(&steep) {
                Region::new()
            } else {
                geom::intersect(&geom::difference(&regions[k], &regions[k - 1]), &geom::inset(&steep, -(lean + 0.01)))
            };
            // An enforcer holds up all that is over air inside it; a blocker none of what is inside it.
            let enforce = v.enforce.get(k).unwrap_or(&none);
            let held = if geom::is_empty(enforce) {
                by_angle
            } else {
                geom::union(&by_angle, &geom::intersect(&geom::difference(&regions[k], &regions[k - 1]), enforce))
            };
            match v.block.get(k) {
                Some(b) if !geom::is_empty(b) => geom::difference(&held, b),
                _ => held,
            }
        })
        .collect();
    if overhang.iter().all(geom::is_empty) {
        return out;
    }
    // 2. Carried down: what needs holding at layer k is everything above it that is not yet resting on the model, or on another
    // object.
    let mut carry = vec![Region::new(); n];
    for k in (0..n - 1).rev() {
        let above = geom::union(&carry[k + 1], &overhang[k + 1]);
        carry[k] = geom::difference(&above, &solid[k]);
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
        (from..=to.min(n - 1)).fold(Region::new(), |a, j| geom::union(&a, &geom::inset(&solid[j], -p.support_xy_gap)))
    };
    let tree = p.support_style == SupportStyle::Tree;
    let mut roofs = vec![Region::new(); if tree { n } else { 0 }];
    for k in 0..n - 1 {
        if geom::is_empty(&carry[k]) {
            continue;
        }
        let mut area = geom::difference(&carry[k], &geom::inset(&solid[k], -p.support_xy_gap));
        if gap_layers > 0 {
            area = geom::difference(&area, &near(k + 1, k + gap_layers));
        }
        // What is narrower than a line cannot be printed: the sliver a curved surface leaves at the angle.
        area = geom::inset(&geom::inset(&area, w / 2.0), -w / 2.0);
        // 4. The interface: the part with the model within a few layers above the gap. A tree's tips go under the top of each
        // column (its roof), which is the interface when there is one, else the column's last layer.
        let layers = p.support_interface_layers as usize;
        let roof_layers = if tree { layers.max(1) } else { layers };
        let roof =
            if roof_layers > 0 { geom::intersect(&area, &near(k + 1 + gap_layers, k + gap_layers + roof_layers)) } else { Region::new() };
        let interface = if layers > 0 { roof.clone() } else { Region::new() };
        let sparse = geom::difference(&area, &interface);
        out[k] = Layer { sparse, interface };
        if tree {
            roofs[k] = roof;
        }
    }
    if tree {
        return trees(&solid[..n], heights, p, &roofs, out, &|k| near(k, k + gap_layers));
    }
    out
}

/// How much a branch's radius grows for each mm it goes down, so a long branch stands firmer: about a degree. More costs plastic
/// fast, as every branch pays it (0.05 made a ball on a stalk take more than its columns).
const GROWTH: f64 = 0.02;

/// About how many tips a tree support may have in all.
const MAX_TIPS: f64 = 2000.0;

/// A branch as the trees are walked down: where it stands, the tips it carries, how much it has thickened on the way, the branch
/// it was merged into (itself while it stands alone), and whether it was dropped for not reaching the plate.
struct Branch {
    at: P2,
    tips: f64,
    grown: f64,
    into: usize,
    dropped: bool,
}

fn dist(a: P2, b: P2) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// A circle as a polygon, counter-clockwise.
fn disc(c: P2, r: f64) -> Loop {
    (0..24).map(|i| std::f64::consts::TAU * f64::from(i) / 24.0).map(|a| [c[0] + r * a.cos(), c[1] + r * a.sin()]).collect()
}

/// Steps 5 to 8: trees under the columns' `roofs`, keeping the columns' interface. `regions`: what stands on each layer (the model
/// and the objects beside it). `keep_out(k)`: where support may not be on layer `k` (the model grown by the xy gap, there and in the
/// z gap above).
fn trees(
    regions: &[Region],
    heights: &[f64],
    p: &Profile,
    roofs: &[Region],
    columns: Vec<Layer>,
    keep_out: &dyn Fn(usize) -> Region,
) -> Vec<Layer> {
    let n = regions.len();
    let w = p.line_width();
    let tan = p.tree_branch_angle.to_radians().tan();
    // 5. Tips: one in each cell of a grid `s` apart that the bottom of a roof reaches (the cell's middle, or else the roof's point
    // nearest it), on the layer under the roof (or its last layer, without an interface). None within 3/4 of `s` of a tip a little
    // higher, so a sloping roof, whose bottom is a strip on each layer, is not given a row on each.
    let shift = usize::from(p.support_interface_layers > 0);
    let bottoms: Vec<Region> = (0..n)
        .map(|k| match k {
            k if k < shift => Region::new(),
            0 => roofs[0].clone(),
            k => geom::difference(&roofs[k], &roofs[k - 1]),
        })
        .collect();
    // The walk below compares every branch with every other: a huge overhang under close tips gets them wider apart instead.
    let s = p.tree_tip_spacing.max((bottoms.iter().map(geom::area).sum::<f64>() / MAX_TIPS).sqrt());
    let z: Vec<f64> = heights
        .iter()
        .scan(0.0, |z, h| {
            *z += h;
            Some(*z)
        })
        .collect();
    let mut tips: Vec<Vec<P2>> = vec![Vec::new(); n];
    let mut placed: Vec<(f64, P2)> = Vec::new();
    for k in (shift..n).rev() {
        // Without the slivers two nearly equal roofs leave.
        for shape in geom::inset(&geom::inset(&bottoms[k], 0.01), -0.01) {
            let one = vec![shape];
            let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
            for q in one.iter().flatten().flatten() {
                (lo, hi) = ([lo[0].min(q[0]), lo[1].min(q[1])], [hi[0].max(q[0]), hi[1].max(q[1])]);
            }
            let cells = |a: f64, b: f64| (a / s).floor() as i64..=(b / s).floor() as i64;
            for cx in cells(lo[0], hi[0]) {
                for cy in cells(lo[1], hi[1]) {
                    let c = [(cx as f64 + 0.5) * s, (cy as f64 + 0.5) * s];
                    let t = if geom::contains(&one, c) {
                        c
                    } else {
                        match geom::nearest(&one, c) {
                            Some(q) if dist(q, c) <= s * 0.71 => q,
                            _ => continue,
                        }
                    };
                    if placed.iter().any(|&(h, q)| (h - z[k]).abs() <= s && dist(q, t) < 0.75 * s) {
                        continue;
                    }
                    placed.push((z[k], t));
                    tips[k - shift].push(t);
                }
            }
        }
    }
    if tips.iter().all(Vec::is_empty) {
        return columns.into_iter().map(|l| Layer { sparse: Region::new(), interface: l.interface }).collect();
    }
    // 6. Where a branch's middle may not be: near the model, or where the plate cannot be reached from without passing through it
    // (what is blocked on the layer below, shrunk by how far a branch may lean in a layer).
    let walls: Vec<Region> = regions.iter().map(|r| geom::inset(r, -(p.support_xy_gap + w / 2.0))).collect();
    let mut blocked: Vec<Region> = Vec::with_capacity(n);
    for k in 0..n {
        let lean = heights[k] * tan;
        let here = match blocked.last() {
            Some(below) if lean > 1e-9 => geom::union(&geom::inset(below, lean), &walls[k]),
            Some(below) => geom::union(below, &walls[k]),
            None => walls[k].clone(),
        };
        blocked.push(here);
    }
    let free = |k: usize, q: P2| !geom::contains(&blocked[k], q);
    // The nearest point outside the blocked region, within `reach` of `from`: just past its boundary.
    let escape = |k: usize, from: P2, reach: f64| -> Option<P2> {
        let q = geom::nearest(&blocked[k], from)?;
        let l = dist(q, from);
        let out = if l > 1e-9 { [q[0] + (q[0] - from[0]) / l * 0.01, q[1] + (q[1] - from[1]) / l * 0.01] } else { q };
        (l <= reach && free(k, out)).then_some(out)
    };
    let mut branches: Vec<Branch> = Vec::new();
    // Each tip: its layer, where it is, and its branch (none where the roof rests on the model there).
    let mut holds: Vec<(usize, P2, Option<usize>)> = Vec::new();
    let mut active: Vec<usize> = Vec::new();
    let mut circles: Vec<Vec<(P2, f64, usize)>> = vec![Vec::new(); n];
    for j in (0..n).rev() {
        let lean = heights[(j + 1).min(n - 1)] * tan;
        let at: Vec<P2> = active.iter().map(|&i| branches[i].at).collect();
        let mut kept = Vec::with_capacity(at.len() + tips[j].len());
        for (a, &i) in active.iter().enumerate() {
            let here = at[a];
            // Toward the nearest branch it can still meet above the plate, and not across the model (where they would meet is
            // free), half the way at most, as that one comes too.
            let mut near: Vec<(P2, f64)> = (0..at.len())
                .filter(|&b| b != a)
                .map(|b| (at[b], dist(here, at[b])))
                .filter(|&(_, l)| l > 1e-9 && l <= 2.0 * lean * j as f64)
                .collect();
            if near.len() > 3 {
                near.select_nth_unstable_by(2, |x, y| x.1.total_cmp(&y.1));
                near.truncate(3);
            }
            near.sort_by(|x, y| x.1.total_cmp(&y.1));
            let toward = near.into_iter().find(|(q, _)| free(j, [(here[0] + q[0]) / 2.0, (here[1] + q[1]) / 2.0]));
            let want = toward.map_or(here, |(q, l)| {
                let t = lean.min(l / 2.0) / l;
                [here[0] + (q[0] - here[0]) * t, here[1] + (q[1] - here[1]) * t]
            });
            match [want, here].into_iter().find(|&q| free(j, q)).or_else(|| escape(j, here, 1.5 * lean + w / 2.0)) {
                Some(q) => {
                    branches[i].at = q;
                    kept.push(i);
                }
                // 7. It cannot reach the plate. From the plate only, it goes, with every tip it carries.
                None if p.support == Support::FromPlate => branches[i].dropped = true,
                // Else it ends on the model where the model is under it, and keeps clear of the model's walls till then.
                None if geom::contains(&regions[j], here) => {}
                None => {
                    branches[i].at = if geom::contains(&walls[j], want) { here } else { want };
                    kept.push(i);
                }
            }
        }
        for &t in &tips[j] {
            if geom::contains(&regions[j], t) {
                holds.push((j, t, None)); // the roof rests on the model here
                continue;
            }
            let at = match (free(j, t), escape(j, t, w)) {
                (true, _) => t,
                (false, Some(q)) => q,
                (false, None) if p.support == Support::FromPlate => continue,
                (false, None) => t,
            };
            holds.push((j, t, Some(branches.len())));
            kept.push(branches.len());
            branches.push(Branch { at, tips: 1.0, grown: 0.0, into: branches.len(), dropped: false });
        }
        // Branches that meet go on as one, from where the one with more tips is.
        active.clear();
        for i in kept {
            match active.iter().position(|&m| dist(branches[m].at, branches[i].at) <= w) {
                Some(x) => {
                    let m = active[x];
                    let (win, lose) = if branches[i].tips > branches[m].tips { (i, m) } else { (m, i) };
                    branches[win].tips += branches[lose].tips;
                    branches[win].grown = branches[win].grown.max(branches[lose].grown);
                    branches[lose].into = win;
                    active[x] = win;
                }
                None => active.push(i),
            }
        }
        for &i in &active {
            let b = &mut branches[i];
            circles[j].push((b.at, (w * b.tips.sqrt() + b.grown).min(p.tree_max_radius.max(w)), i));
            b.grown += heights[j] * GROWTH;
        }
    }
    // 8. Each layer's circles, of the branches not dropped, out of the model's way, without slivers, and under the interface. From
    // the plate only, nothing of a branch may hang over what is below it either (its edge would rest on the model's top).
    let mut below = Region::new();
    let mut shadows: Vec<Region> = Vec::with_capacity(if p.support == Support::FromPlate { n } else { 0 });
    if p.support == Support::FromPlate {
        for r in regions.iter().take(n) {
            shadows.push(below.clone());
            below = geom::union(&below, r);
        }
    }
    let root = |mut i: usize| {
        for _ in 0..branches.len() {
            if branches[i].into == i {
                break;
            }
            i = branches[i].into;
        }
        i
    };
    // The interface where a tip holds it: within the reach of a tip whose branch was kept, or that rests on the model, a little below
    // it (the interface's layers over the tip) or a tip's spacing either way (a tip on a slope stands for the strips beside it).
    let reach = 0.75 * s;
    let tall = heights.iter().copied().fold(0.0, f64::max) * (p.support_interface_layers as f64 + 1.0);
    let held = |m: usize| -> Region {
        let near: Vec<Loop> = holds
            .iter()
            .filter(|(j, _, b)| (z[m] - z[*j]) >= -s && (z[m] - z[*j]) <= tall + s && b.is_none_or(|b| !branches[root(b)].dropped))
            .map(|&(_, t, _)| disc(t, reach))
            .collect();
        geom::region(&near)
    };
    columns
        .into_iter()
        .enumerate()
        .map(|(j, layer)| {
            let interface = if geom::is_empty(&layer.interface) {
                layer.interface
            } else {
                let kept = geom::intersect(&layer.interface, &held(j));
                geom::inset(&geom::inset(&kept, w / 2.0), -w / 2.0)
            };
            let discs: Vec<Loop> = circles[j].iter().filter(|c| !branches[root(c.2)].dropped).map(|&(c, r, _)| disc(c, r)).collect();
            if discs.is_empty() {
                return Layer { sparse: Region::new(), interface };
            }
            let mut tree = geom::difference(&geom::region(&discs), &keep_out(j));
            if let Some(shadow) = shadows.get(j) {
                tree = geom::difference(&tree, shadow);
            }
            let tree = geom::inset(&geom::inset(&tree, w / 2.0), -w / 2.0);
            Layer { sparse: geom::difference(&tree, &interface), interface }
        })
        .collect()
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
        let s = plan(&regions, &regions, &heights, &p);
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
        assert!(plan(&regions, &regions, &heights, &Profile::default()).iter().all(|l| geom::is_empty(&l.sparse)));
    }

    #[test]
    fn from_the_plate_only_drops_support_that_would_stand_on_the_model() {
        // A wide base 2 mm tall, the pillar, then the arms over the base: their support would stand on the base.
        let mut regions: Vec<Region> = (0..10).map(|_| geom::region(&[rect(-10.0, -3.0, 10.0, 3.0)])).collect();
        regions.extend(tee().0.into_iter().skip(10));
        let heights = vec![0.2; regions.len()];
        let everywhere = plan(&regions, &regions, &heights, &Profile { support: Support::Everywhere, ..Profile::default() });
        assert!(geom::area(&everywhere[15].sparse) > 10.0, "it stands on the base");
        assert!(everywhere[..10].iter().all(|l| geom::is_empty(&l.sparse) && geom::is_empty(&l.interface)), "none inside the base");
        let plate = plan(&regions, &regions, &heights, &Profile { support: Support::FromPlate, ..Profile::default() });
        assert!(plate.iter().all(|l| geom::is_empty(&l.sparse) && geom::is_empty(&l.interface)));
        // Trees alike: on the base, never in it; from the plate only, none.
        let tree = |support| Profile { support, support_style: SupportStyle::Tree, ..Profile::default() };
        let everywhere = plan(&regions, &regions, &heights, &tree(Support::Everywhere));
        assert!(geom::area(&everywhere[15].sparse) > 0.5, "a tree stands on the base");
        assert!(everywhere[..10].iter().all(|l| geom::is_empty(&l.sparse) && geom::is_empty(&l.interface)), "none inside the base");
        assert!(
            plan(&regions, &regions, &heights, &tree(Support::FromPlate))
                .iter()
                .all(|l| geom::is_empty(&l.sparse) && geom::is_empty(&l.interface))
        );
    }

    fn tree() -> Profile {
        Profile { support: Support::Everywhere, support_style: SupportStyle::Tree, ..Profile::default() }
    }

    #[test]
    fn a_trees_interface_is_only_where_a_kept_tip_holds_it() {
        // A block 10 mm square and 4 mm tall (20 layers), then air. Two roofs of interface at layers 26 and 27: one over the block's
        // middle, whose tips cannot reach the plate, and one beside the block, over the plate.
        let n = 30;
        let block = geom::region(&[rect(-5.0, -5.0, 5.0, 5.0)]);
        let regions: Vec<Region> = (0..n).map(|k| if k < 20 { block.clone() } else { Region::new() }).collect();
        let heights = vec![0.2; n];
        let over_block = geom::region(&[rect(-2.0, -2.0, 2.0, 2.0)]);
        let over_plate = geom::region(&[rect(10.0, -2.0, 14.0, 2.0)]);
        let roof = geom::union(&over_block, &over_plate);
        let mut roofs = vec![Region::new(); n];
        let mut columns = vec![Layer::default(); n];
        for k in [26, 27] {
            roofs[k] = roof.clone();
            columns[k].interface = roof.clone();
        }
        let keep_out = |k: usize| geom::inset(&regions[k], -0.6);
        let p = Profile { support: Support::FromPlate, ..tree() };
        let s = trees(&regions, &heights, &p, &roofs, columns.clone(), &keep_out);
        for k in [26, 27] {
            assert!(geom::area(&geom::intersect(&s[k].interface, &over_block)) < 1e-6, "layer {k}: no interface left over air");
            assert!(geom::area(&geom::intersect(&s[k].interface, &over_plate)) > 10.0, "layer {k}: the held one stays");
        }
        assert!(s.iter().all(|l| geom::area(&geom::intersect(&l.sparse, &over_block)) < 1e-6), "no branch over the block");
        // Everywhere, the branches over the block end on it, and hold their interface.
        let everywhere = trees(&regions, &heights, &tree(), &roofs, columns, &keep_out);
        assert!(geom::area(&geom::intersect(&everywhere[26].interface, &over_block)) > 10.0);
    }

    #[test]
    fn support_stands_on_another_object_and_never_inside_it() {
        // The tee, and beside its pillar another object: a box 2 mm tall (10 layers) under the right arm, from x 4 to 8.
        let (tee, heights) = tee();
        let other = geom::region(&[rect(4.0, -3.0, 8.0, 3.0)]);
        let solid: Vec<Region> = tee.iter().enumerate().map(|(k, r)| if k < 10 { geom::union(r, &other) } else { r.clone() }).collect();
        for style in [SupportStyle::Columns, SupportStyle::Tree] {
            let each = |support| Profile { support, support_style: style, ..Profile::default() };
            let alone = plan(&tee, &tee, &heights, &each(Support::Everywhere));
            let s = plan(&tee, &solid, &heights, &each(Support::Everywhere));
            let near = geom::inset(&other, -(Profile::default().support_xy_gap - 0.01));
            for k in 0..10 {
                for l in [&s[k], &alone[k]] {
                    assert!(geom::area(&geom::intersect(&l.sparse, &tee[k])) < 1e-6, "{style:?}: never inside the tee");
                }
                assert!(geom::area(&geom::intersect(&s[k].sparse, &near)) < 1e-6, "{style:?}: layer {k} keeps the xy gap from the box");
            }
            // Planned alone, the right arm's support went through the box.
            assert!(geom::area(&geom::intersect(&alone[5].sparse, &other)) > 1.0, "{style:?}: the old plan stood in the box");
            // Above the box it holds the arm, standing on the box's top.
            assert!(geom::area(&geom::intersect(&s[12].sparse, &geom::inset(&other, 0.5))) > 0.5, "{style:?}: it stands on the box");
            // From the plate only, the column over the box is left out, and the left arm's still stands.
            let plate = plan(&tee, &solid, &heights, &each(Support::FromPlate));
            // (A strip beside the box, between it and the arm's end, does reach the plate.)
            let left = geom::region(&[rect(-9.0, -3.0, -3.0, 3.0)]);
            assert!(plate.iter().all(|l| geom::area(&geom::intersect(&l.sparse, &other)) < 1e-6), "{style:?}: nothing on the box");
            assert!(geom::area(&geom::intersect(&plate[2].sparse, &left)) > 0.5, "{style:?}: the left arm keeps its support");
        }
    }

    /// The shapes of `r` and the middle of each (the mean of its outer contour's points).
    fn middles(r: &Region) -> Vec<P2> {
        r.iter()
            .filter_map(|shape| shape.first())
            .map(|c| {
                let n = c.len() as f64;
                [c.iter().map(|q| q[0]).sum::<f64>() / n, c.iter().map(|q| q[1]).sum::<f64>() / n]
            })
            .collect()
    }

    #[test]
    fn a_tree_holds_a_tall_tees_arms_from_the_plate_and_touches_them_only_through_the_interface() {
        // A 6 mm pillar 20 mm tall (100 layers), then arms 30 mm wide for 2 mm.
        let regions: Vec<Region> = (0..110)
            .map(|k| if k < 100 { geom::region(&[rect(-3.0, -3.0, 3.0, 3.0)]) } else { geom::region(&[rect(-15.0, -3.0, 15.0, 3.0)]) })
            .collect();
        let heights = vec![0.2; regions.len()];
        let p = tree();
        let s = plan(&regions, &regions, &heights, &p);
        // On the plate, a trunk on each side, beyond the pillar and its gap.
        let feet = middles(&s[0].sparse);
        assert!(feet.iter().any(|m| m[0] > 4.0) && feet.iter().any(|m| m[0] < -4.0), "{feet:?}");
        assert!(feet.len() <= 4, "the tips gathered into a few trunks: {feet:?}");
        // The interface under the arms is the columns' own; the tree is under it, never beside the arms' underside.
        let columns = plan(&regions, &regions, &heights, &Profile { support_style: SupportStyle::Columns, ..p.clone() });
        for k in [97, 98] {
            assert!(geom::area(&s[k].interface) > 50.0 && (geom::area(&s[k].interface) - geom::area(&columns[k].interface)).abs() < 1e-6);
        }
        assert!(geom::is_empty(&s[99].sparse) && geom::is_empty(&s[99].interface), "the z gap");
        assert!(!geom::is_empty(&s[96].sparse), "the tips, under the interface");
        let reach = (p.support_z_gap / p.layer_height).round() as usize + p.support_interface_layers as usize;
        for (k, l) in s.iter().enumerate() {
            assert!(geom::area(&geom::intersect(&l.sparse, &regions[k])) < 1e-6, "inside the model at layer {k}");
            assert!(geom::area(&geom::intersect(&l.interface, &regions[k])) < 1e-6, "inside the model at layer {k}");
            for above in &regions[k + 1..(k + 1 + reach).min(regions.len())] {
                assert!(geom::area(&geom::intersect(&l.sparse, above)) < 1e-6, "the tree touches the model at layer {k}");
            }
        }
    }

    #[test]
    fn two_nearby_overhangs_share_a_trunk() {
        // A wall 2 mm thick, 15 mm tall, with two ledges 3 mm wide sticking out on one side, 6 mm apart, for 1 mm.
        let wall = rect(-1.0, -8.0, 1.0, 8.0);
        let regions: Vec<Region> = (0..80)
            .map(|k| {
                if k < 75 {
                    geom::region(std::slice::from_ref(&wall))
                } else {
                    geom::region(&[wall.clone(), rect(1.0, -6.0, 5.0, -3.0), rect(1.0, 3.0, 5.0, 6.0)])
                }
            })
            .collect();
        let s = plan(&regions, &regions, &heights_of(&regions), &tree());
        let top = middles(&s[71].sparse);
        assert!(top.iter().any(|m| m[1] < -2.0) && top.iter().any(|m| m[1] > 2.0), "a branch under each ledge: {top:?}");
        let foot = middles(&s[0].sparse);
        assert_eq!(foot.len(), 1, "one trunk on the plate: {foot:?}");
        assert!(foot[0][0] > 1.0 && foot[0][1].abs() < 1.0, "between the ledges, beside the wall: {foot:?}");
    }

    fn heights_of(regions: &[Region]) -> Vec<f64> {
        vec![0.2; regions.len()]
    }

    #[test]
    fn a_part_with_nothing_to_hold_up_gets_no_tree() {
        let regions: Vec<Region> = (0..40).map(|_| geom::region(&[rect(-5.0, -5.0, 5.0, 5.0)])).collect();
        assert!(
            plan(&regions, &regions, &heights_of(&regions), &tree())
                .iter()
                .all(|l| geom::is_empty(&l.sparse) && geom::is_empty(&l.interface))
        );
        let cube = vec![(1, lamina_core::Mesh::cuboid([10.0, 10.0, 8.0]))];
        let out = crate::print::gcode(&cube, &tree()).unwrap();
        assert!(!out.features.contains_key("support") && !out.gcode.contains(";TYPE:support"), "{:?}", out.features);
    }

    #[test]
    fn a_huge_overhang_under_close_tips_gets_them_wider_apart() {
        // A slab 100 mm square on a pillar, with tips asked 1 mm apart: 10,000 tips, more than the walk takes.
        let regions: Vec<Region> = (0..20)
            .map(|k| if k < 15 { geom::region(&[rect(-3.0, -3.0, 3.0, 3.0)]) } else { geom::region(&[rect(-50.0, -50.0, 50.0, 50.0)]) })
            .collect();
        let s = plan(&regions, &regions, &heights_of(&regions), &Profile { tree_tip_spacing: 1.0, ..tree() });
        let tips = s[11].sparse.len();
        assert!((1000..=(MAX_TIPS * 1.3) as usize).contains(&tips), "{tips} tips");
    }

    #[test]
    fn a_tree_under_a_tees_arms_uses_less_plastic_than_columns() {
        use lamina_core::{Mesh, Transform};
        let pillar = Mesh::cuboid([6.0, 6.0, 20.0]);
        let arms = Mesh::cuboid([30.0, 6.0, 2.0]).transformed(&Transform::translate([0.0, 0.0, 20.0]));
        let tee = vec![(1, Mesh::merged([&pillar, &arms]))];
        let support = |style| {
            let out = crate::print::gcode(&tee, &Profile { support_style: style, ..tree() }).unwrap();
            (out.features.get("support").copied().unwrap_or(0.0), out.features.get("support-interface").copied().unwrap_or(0.0))
        };
        let (columns, trees) = (support(SupportStyle::Columns), support(SupportStyle::Tree));
        eprintln!("support filament, mm: columns {columns:?}, tree {trees:?}");
        assert!(trees.0 > 0.0 && trees.0 < 0.6 * columns.0, "columns {columns:?}, tree {trees:?}");
        assert!((trees.1 - columns.1).abs() < 1e-6, "the same interface: {columns:?} {trees:?}");
    }

    #[test]
    fn a_gentle_slope_needs_no_support_and_a_steep_one_does() {
        // Each layer 0.1 mm wider than the last (27 degrees from vertical at 0.2 mm layers), then 0.4 mm (63 degrees). The steep slope
        // is held up all along its underside, not in strips.
        let grow = |step: f64| -> Vec<Region> { (0..20).map(|k| geom::region(&[rect(0.0, 0.0, 5.0 + step * k as f64, 5.0)])).collect() };
        let p = Profile { support: Support::Everywhere, ..Profile::default() };
        assert!(plan(&grow(0.1), &grow(0.1), &[0.2; 20], &p).iter().all(|l| geom::is_empty(&l.sparse) && geom::is_empty(&l.interface)));
        let steep = plan(&grow(0.4), &grow(0.4), &[0.2; 20], &p);
        let total = geom::area(&steep[2].sparse) + geom::area(&steep[2].interface);
        // At layer 2 the slope above reaches 12.6 mm; for the z gap the support keeps clear of layer 3 (5 + 3 x 0.4 mm) by the xy gap.
        assert!((total - (12.6 - (5.0 + 3.0 * 0.4) - 0.6) * 5.0).abs() < 0.5, "{total}");
    }
}
