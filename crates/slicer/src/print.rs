//! From meshes to G-code: layers, walls, solid and sparse infill, a skirt, and the moves between them. Absolute coordinates with
//! relative extrusion (`G90`, `M83`), which Marlin, Klipper and RepRap all take.
//!
//! Infill lines are joined end to start where a short path inside the region exists (`connect`), so a layer of infill is a few long
//! strokes. Each wall loop starts where the `seam` rule says. shortcut: there is no bridging yet (docs/ROADMAP.md).

use std::f64::consts::PI;

use lamina_core::Mesh;

use crate::geom::{self, Region};
use crate::profile::{Firmware, Profile, Seam};
use crate::slice::{self, P2};
use crate::{gap, infill, support};

/// Vertical moves are slow and fixed, mm/s.
const Z_SPEED: f64 = 20.0;
/// Infill reaches into the innermost wall by this share of a line, so the two bond.
const INFILL_OVERLAP: f64 = 0.1;
/// How much of the area the support's interface lines fill.
const SUPPORT_INTERFACE_DENSITY: f64 = 0.7;

#[derive(Debug)]
pub struct Output {
    pub gcode: String,
    pub layers: usize,
    /// Filament pushed, mm.
    pub filament_mm: f64,
    /// As the printer's planner would take it: speeding up and slowing at `max_acceleration`, corners as the junction deviation
    /// allows (`time.rs`).
    pub seconds: f64,
    pub warnings: Vec<String>,
    /// Filament pushed for each kind of move, mm, by its `;TYPE:` name.
    pub features: std::collections::BTreeMap<&'static str, f64>,
}

/// A layer: its top z, its thickness and its strokes in print order.
/// A layer: its top z, its thickness, its strokes in print order, and its region (inside which a travel needs no retraction).
type LayerPlan = (f64, f64, Vec<Stroke>, Region);
/// An object: its first layer as a region, then its layers.
type ObjectPlan = (Region, Vec<LayerPlan>);

struct Stroke {
    pts: Vec<P2>,
    closed: bool,
    speed: f64,
    kind: Feature,
    /// The flow multiplied by this (a bridge's own flow), 1 for most.
    flow: f64,
    /// In vase mode: a loop that climbs from the layer below's height to its own as it goes round, so z never stops.
    spiral: bool,
}

/// What a stroke is part of, written as `;TYPE:` before it so a preview can colour it and a reader can count it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    OuterWall,
    InnerWall,
    SolidInfill,
    SparseInfill,
    Skirt,
    Brim,
    Support,
    SupportInterface,
    Raft,
    Bridge,
    GapFill,
}

impl Feature {
    pub const ALL: [Feature; 11] = [
        Feature::OuterWall,
        Feature::InnerWall,
        Feature::SolidInfill,
        Feature::SparseInfill,
        Feature::Skirt,
        Feature::Brim,
        Feature::Support,
        Feature::SupportInterface,
        Feature::Raft,
        Feature::Bridge,
        Feature::GapFill,
    ];

    /// The name after `;TYPE:`.
    pub fn name(self) -> &'static str {
        match self {
            Feature::OuterWall => "outer-wall",
            Feature::InnerWall => "inner-wall",
            Feature::SolidInfill => "solid-infill",
            Feature::SparseInfill => "sparse-infill",
            Feature::Skirt => "skirt",
            Feature::Brim => "brim",
            Feature::Support => "support",
            Feature::SupportInterface => "support-interface",
            Feature::Raft => "raft",
            Feature::Bridge => "bridge",
            Feature::GapFill => "gap-fill",
        }
    }

    pub fn from_name(name: &str) -> Option<Feature> {
        Feature::ALL.into_iter().find(|f| f.name() == name)
    }
}

/// G-code for `objects` (each an id and its mesh where it stands on the plate, z = 0 the plate, the origin its middle).
pub fn gcode(objects: &[(u32, Mesh)], p: &Profile) -> Result<Output, String> {
    gcode_each(objects, &[], p)
}

/// The same, each object with its own profile where `own` has one (the plate's `p` otherwise). The plate's settings that every
/// object shares (the printer, the filament, the layer heights, the skirt) are `p`'s.
pub fn gcode_each(objects: &[(u32, Mesh)], own: &[Option<Profile>], p: &Profile) -> Result<Output, String> {
    for o in own.iter().flatten() {
        o.check()?;
        if (o.layer_height, o.first_layer_height) != (p.layer_height, p.first_layer_height) {
            return Err("an object's own settings may not change the layer heights: every object prints on the plate's layers".into());
        }
    }
    p.check()?;
    if objects.is_empty() {
        return Err("nothing to slice: the plate is empty".into());
    }
    let mut warnings = Vec::new();
    let (hx, hy) = (p.bed[0] / 2.0, p.bed[1] / 2.0);
    for (id, m) in objects {
        let (lo, hi) = m.bounds().ok_or_else(|| format!("object {id} is empty"))?;
        if lo[0] < -hx || hi[0] > hx || lo[1] < -hy || hi[1] > hy {
            return Err(format!(
                "object {id} does not fit the {} x {} mm plate: it spans x {:.1} to {:.1} and y {:.1} to {:.1}; move or scale it",
                p.bed[0], p.bed[1], lo[0], hi[0], lo[1], hi[1]
            ));
        }
        if lo[2] < -1e-6 {
            return Err(format!("object {id} is below the plate: drop_to_bed puts it on"));
        }
        if hi[2] + raft_height(p) > p.max_height {
            return Err(format!("object {id} is {:.1} mm tall, more than the printer's {} mm", hi[2] + raft_height(p), p.max_height));
        }
        if !m.is_watertight() {
            warnings.push(format!("object {id} is not a closed mesh: parts of it may be missing from the print"));
        }
    }
    if p.vase && objects.len() > 1 {
        warnings.push("vase mode climbs one wall without stopping: with several objects the head stops at each, so print one".into());
    }
    let w = p.line_width();
    // The plan of each object: its first layer's region, then its layers and the strokes of each.
    let mut plans: Vec<ObjectPlan> = objects
        .iter()
        .enumerate()
        .map(|(i, (_, m))| plan(m, own.get(i).and_then(Option::as_ref).unwrap_or(p), w))
        .collect::<Result<_, _>>()?;
    let count = plans.iter().map(|(_, l)| l.len()).max().unwrap_or(0);

    let mut g = Writer {
        p,
        out: String::new(),
        at: None,
        z: 0.0,
        retracted: true,
        filament: 0.0,
        planner: crate::time::Planner::new(p.max_acceleration, p.junction_deviation),
        seams: Vec::new(),
        below: Vec::new(),
        feature: None,
        by_feature: std::collections::BTreeMap::new(),
        fan: 0.0,
        fan_now: 0.0,
        region: Region::new(),
        spiral: None,
        hopped: false,
        wipe: Vec::new(),
    };
    g.line(format!("; generated by Lamina {}", env!("CARGO_PKG_VERSION")));
    if p.thumbnail > 0 {
        // A picture of the plate for the printer's screen, as the comment block Klipper's front ends and PrusaSlicer-style firmware read.
        let meshes: Vec<Mesh> = objects.iter().map(|(_, m)| m.clone()).collect();
        let png = lamina_core::render::png(&lamina_core::render::thumbnail_of(&meshes, p.thumbnail as usize));
        let text = lamina_core::base64::encode(&png);
        g.line(format!("; thumbnail begin {0}x{0} {1}", p.thumbnail, text.len()));
        for chunk in text.as_bytes().chunks(78) {
            g.line(format!("; {}", String::from_utf8_lossy(chunk)));
        }
        g.line("; thumbnail end".into());
    }
    g.line(p.expand(&p.start_gcode)?);
    g.line("G90\nM83\nG92 E0".into());
    if p.pressure_advance > 0.0 {
        g.line(match p.firmware {
            Firmware::Marlin => format!("M900 K{}", p.pressure_advance),
            Firmware::Klipper => format!("SET_PRESSURE_ADVANCE ADVANCE={}", p.pressure_advance),
            Firmware::Reprap => format!("M572 D0 S{}", p.pressure_advance),
        });
    }
    // Klipper: each object named with its outline, so one that fails can be cancelled and the others go on.
    let names: Vec<String> = objects.iter().map(|(id, _)| format!("object_{id}")).collect();
    if p.firmware == Firmware::Klipper {
        for ((_, m), name) in objects.iter().zip(&names) {
            let Some((lo, hi)) = m.bounds() else { continue };
            let corner = |x: f64, y: f64| {
                let q = g.machine([x, y]);
                format!("[{:.3},{:.3}]", q[0], q[1])
            };
            let c = g.machine([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]);
            let polygon = [corner(lo[0], lo[1]), corner(hi[0], lo[1]), corner(hi[0], hi[1]), corner(lo[0], hi[1])].join(",");
            g.line(format!("EXCLUDE_OBJECT_DEFINE NAME={name} CENTER={:.3},{:.3} POLYGON=[{polygon}]", c[0], c[1]));
        }
    }
    // Where everything stands on the first layer, by its outer walls only (a brim or a raft does not go into holes).
    let footprint = plans.iter().fold(Region::new(), |a, (first, _)| geom::union(&a, &outer(first)));
    let raft = raft(&footprint, p, w);
    let lift = raft.last().map_or(0.0, |(z, _, _, _)| z + RAFT_GAP);
    // The brim: loops outward from the footprint, touching it. A raft takes its place.
    let brim_loops = if raft.is_empty() { (p.brim_width / w).round() as usize } else { 0 };
    let brim: Vec<Stroke> = (0..brim_loops)
        .flat_map(|i| geom::inset(&footprint, -(w / 2.0 + i as f64 * w)))
        .flatten()
        .map(|pts| Stroke { pts, closed: true, speed: p.first_layer_speed, kind: Feature::Brim, flow: 1.0, spiral: false })
        .collect();
    // The skirt: outward from the brim or the raft, or from the footprint when there is neither.
    let skirt_base = match (raft.is_empty(), brim_loops) {
        (false, _) => geom::inset(&footprint, -RAFT_MARGIN),
        (true, 0) => footprint.clone(),
        (true, n) => geom::inset(&footprint, -(n as f64 * w)),
    };
    let skirt: Vec<Stroke> = (0..p.skirt_loops)
        .flat_map(|i| geom::inset(&skirt_base, -(p.skirt_distance + (f64::from(i) + 0.5) * w)))
        .filter_map(|shape| shape.into_iter().next())
        .map(|pts| Stroke { pts, closed: true, speed: p.first_layer_speed, kind: Feature::Skirt, flow: 1.0, spiral: false })
        .collect();
    let mut first = Some((skirt, brim));
    for (k, (z, height, strokes, _)) in raft.into_iter().enumerate() {
        g.line(format!(";LAYER:{k}"));
        g.set_z(z);
        if let Some((skirt, _)) = first.take() {
            g.strokes(skirt, flow(height, w, p.filament_diameter));
        }
        g.strokes(strokes, flow(height, w, p.filament_diameter));
    }
    let below = p.raft_layers as usize;
    for k in 0..count {
        g.line(format!(";LAYER:{}", k + below));
        // The fan: off for the first layer, then up a step a layer to full at `fan_full_layer`.
        let fan = fan_at(p, k + below);
        if (fan - g.fan).abs() > 0.5 {
            g.line(format!("M106 S{fan:.0}"));
            (g.fan, g.fan_now) = (fan, fan);
        }
        // A layer that would print in less than `min_layer_time` is slowed to take that long, so it cools before the next.
        let strokes_here = plans.iter().filter_map(|(_, l)| l.get(k)).flat_map(|(_, _, s, _)| s.iter());
        let time: f64 = strokes_here.map(|s| path_length(s) / s.speed).sum();
        if time > 0.0 && time < p.min_layer_time {
            let f = time / p.min_layer_time;
            for (_, layers) in plans.iter_mut() {
                if let Some((_, _, strokes, _)) = layers.get_mut(k) {
                    for s in strokes.iter_mut() {
                        s.speed = (s.speed * f).max(p.min_print_speed.min(s.speed));
                    }
                }
            }
        }
        if k + below == 2 {
            if p.nozzle_temp != p.first_layer_nozzle_temp {
                g.line(format!("M104 S{:.0}", p.nozzle_temp));
            }
            if p.bed_temp != p.first_layer_bed_temp {
                g.line(format!("M140 S{:.0}", p.bed_temp));
            }
        }
        for ((_, layers), name) in plans.iter_mut().zip(&names) {
            let Some((z, height, strokes, region)) = layers.get_mut(k) else { continue };
            // A vase layer climbs to its height as it prints, from where the last one ended.
            if !strokes.is_empty() && strokes.iter().all(|s| s.spiral) {
                g.spiral = Some((g.z, *z + lift));
            } else {
                g.spiral = None;
                g.set_z(*z + lift);
            }
            g.region = std::mem::take(region);
            if let Some((skirt, brim)) = first.take() {
                g.strokes(skirt, flow(*height, w, p.filament_diameter));
                g.strokes(brim, flow(*height, w, p.filament_diameter));
            }
            let klipper = p.firmware == Firmware::Klipper;
            if klipper {
                g.line(format!("EXCLUDE_OBJECT_START NAME={name}"));
            }
            g.strokes(std::mem::take(strokes), flow(*height, w, p.filament_diameter));
            if klipper {
                g.line(format!("EXCLUDE_OBJECT_END NAME={name}"));
            }
        }
    }
    g.retract();
    g.line(p.expand(&p.end_gcode)?);
    Ok(Output {
        gcode: g.out,
        layers: count + below,
        filament_mm: g.filament,
        seconds: g.planner.seconds(),
        warnings,
        features: g.by_feature,
    })
}

/// Whether the straight move from `a` to `b` stays inside `r` (on its boundary counts).
fn stays_inside(r: &Region, a: P2, b: P2) -> bool {
    let len = dist(a, b);
    if len < 1e-9 || r.is_empty() {
        return len < 1e-9;
    }
    let kept: f64 = geom::clip_lines(&[vec![a, b]], r).iter().flat_map(|p| p.windows(2).map(|w| dist(w[0], w[1]))).sum();
    kept >= len * 0.999 - 1e-6
}

/// The fan on layer `k` (counting the raft): off on the first, then up to full by `fan_full_layer`.
fn fan_at(p: &Profile, k: usize) -> f64 {
    if k == 0 {
        return 0.0;
    }
    let full = p.fan_full_layer.max(1) as f64;
    (p.fan_speed * (k as f64 / full).min(1.0)).round()
}

/// How long a stroke's path is, mm (a loop back to its start).
fn path_length(s: &Stroke) -> f64 {
    let open: f64 = s.pts.windows(2).map(|w| dist(w[0], w[1])).sum();
    let close = if s.closed { s.pts.first().zip(s.pts.last()).map_or(0.0, |(a, b)| dist(*a, *b)) } else { 0.0 };
    open + close
}

/// A raft reaches this far beyond the footprint, mm.
pub const RAFT_MARGIN: f64 = 2.0;
/// The object starts this far above the raft's top, mm, so it comes away.
pub const RAFT_GAP: f64 = 0.15;

/// How much a raft lifts the objects.
fn raft_height(p: &Profile) -> f64 {
    if p.raft_layers == 0 { 0.0 } else { p.first_layer_height + f64::from(p.raft_layers - 1) * p.layer_height + RAFT_GAP }
}

/// The region with its holes left out: each shape's outer contour.
fn outer(r: &Region) -> Region {
    let outers: Vec<Vec<P2>> = r.iter().filter_map(|shape| shape.first().cloned()).collect();
    geom::region(&outers)
}

/// The raft's layers under `footprint`: a coarse base, crossing middle layers, and a solid top the objects print on.
fn raft(footprint: &Region, p: &Profile, w: f64) -> Vec<LayerPlan> {
    let area = geom::inset(footprint, -RAFT_MARGIN);
    let mut z = 0.0;
    (0..p.raft_layers)
        .map(|k| {
            let height = if k == 0 { p.first_layer_height } else { p.layer_height };
            z += height;
            let (spacing, angle) = match k {
                0 => (3.0 * w, 0.0),
                k if k + 1 == p.raft_layers => (w, 45.0),
                k => (2.0 * w, if k % 2 == 1 { 90.0 } else { 0.0 }),
            };
            let speed = if k == 0 { p.first_layer_speed } else { p.infill_speed };
            let strokes = fill_lines(&area, spacing, angle)
                .into_iter()
                .map(|pts| Stroke { pts, closed: false, speed, kind: Feature::Raft, flow: 1.0, spiral: false })
                .collect();
            (z, height, strokes, Region::new())
        })
        .collect()
}

/// Filament to push per mm of line: the line's cross-section (a rectangle with rounded ends, as Slic3r's documented flow model
/// has it) over the filament's.
fn flow(height: f64, width: f64, filament: f64) -> f64 {
    (height * (width - height) + PI * (height / 2.0).powi(2)) / (PI * (filament / 2.0).powi(2))
}

/// One object's first layer as a region, then its layers as (top z, thickness, strokes in print order).
fn plan(mesh: &Mesh, p: &Profile, w: f64) -> Result<ObjectPlan, String> {
    let layers = slice::layers(mesh, p.first_layer_height, p.layer_height)?;
    let regions: Vec<Region> = layers.iter().map(|l| geom::region(&l.loops)).collect();
    let heights: Vec<f64> = layers.iter().map(|l| l.height).collect();
    let supports = support::plan(&regions, &heights, p);
    let walls = f64::from(p.perimeters);
    let inner: Vec<Region> =
        regions.iter().map(|r| if p.perimeters == 0 { r.clone() } else { geom::inset(r, walls * w - INFILL_OVERLAP * w) }).collect();
    let mut out = Vec::with_capacity(layers.len());
    for (k, layer) in layers.iter().enumerate() {
        let speed = |s: f64| if k == 0 { p.first_layer_speed } else { s };
        let mut strokes = Vec::new();
        // Vase mode, above the bottom: the outer wall alone, climbing as it goes round; no infill and no top.
        if p.vase && k >= (p.bottom_layers as usize).max(1) {
            for shape in geom::inset(&regions[k], w / 2.0) {
                if let Some(pts) = shape.into_iter().next() {
                    let kind = Feature::OuterWall;
                    strokes.push(Stroke { pts, closed: true, speed: speed(p.perimeter_speed), kind, flow: 1.0, spiral: true });
                }
            }
            out.push((layer.z, layer.height, strokes, regions[k].clone()));
            continue;
        }
        // Supports: sparse lines all run one way, so the layers stack into thin walls; the interface crosses them, denser, to carry
        // the overhang. The writer prints a layer's loops (the walls) before its lines.
        let s = &supports[k];
        for (area, spacing, angle, kind) in [
            (&s.sparse, w / p.support_density, 0.0, Feature::Support),
            (&s.interface, w / SUPPORT_INTERFACE_DENSITY, 90.0, Feature::SupportInterface),
        ] {
            if !geom::is_empty(area) {
                strokes.extend(fill_lines(area, spacing, angle).into_iter().map(|pts| Stroke {
                    pts,
                    closed: false,
                    speed: speed(p.infill_speed),
                    kind,
                    flow: 1.0,
                    spiral: false,
                }));
            }
        }
        for i in (0..p.perimeters).rev() {
            let kind = if i == 0 { Feature::OuterWall } else { Feature::InnerWall };
            for shape in geom::inset(&regions[k], w / 2.0 + f64::from(i) * w) {
                strokes.extend(shape.into_iter().map(|pts| Stroke {
                    pts,
                    closed: true,
                    speed: speed(p.perimeter_speed),
                    kind,
                    flow: 1.0,
                    spiral: false,
                }));
            }
        }
        // Gap fill: what the walls and the infill leave, as lines along the middle, in runs whose flow follows the gap's width.
        for (pts, widths) in gap::gaps(&regions[k], p.perimeters, w, &inner[k]) {
            let mut start = 0;
            while start + 1 < pts.len() {
                let mut end = start + 1;
                while end + 1 < pts.len() && (widths[end + 1] - widths[start]).abs() <= 0.1 * w {
                    end += 1;
                }
                let mean = widths[start..=end].iter().sum::<f64>() / (end - start + 1) as f64;
                strokes.push(Stroke {
                    pts: pts[start..=end].to_vec(),
                    closed: false,
                    speed: speed(p.perimeter_speed),
                    kind: Feature::GapFill,
                    flow: (mean / w).clamp(0.25, 2.0),
                    spiral: false,
                });
                start = end;
            }
        }
        let covered = covered(&inner, k, p.top_layers as usize, p.bottom_layers as usize);
        let solid = geom::difference(&inner[k], &covered);
        let sparse = geom::intersect(&inner[k], &covered);
        // Alternate the direction so each layer bridges the one below.
        let angle = if k % 2 == 0 { 45.0 } else { 135.0 };
        // Bridges: solid infill over air, printed straight across at the angle whose lines rest on the layer below at both ends.
        let bridge = if k > 0 { bridges(&solid, &regions[k - 1], w) } else { Region::new() };
        let solid = if geom::is_empty(&bridge) { solid } else { geom::difference(&solid, &bridge) };
        if !geom::is_empty(&bridge) {
            let best = bridge_angle(&bridge, &regions[k - 1], w);
            strokes.extend(fill_lines(&bridge, w, best).into_iter().map(|pts| Stroke {
                pts,
                closed: false,
                speed: p.bridge_speed,
                kind: Feature::Bridge,
                flow: p.bridge_flow,
                spiral: false,
            }));
        }
        if !geom::is_empty(&solid) {
            strokes.extend(fill_lines(&solid, w, angle).into_iter().map(|pts| Stroke {
                pts,
                closed: false,
                speed: speed(p.infill_speed),
                kind: Feature::SolidInfill,
                flow: 1.0,
                spiral: false,
            }));
        }
        if p.infill_density > 0.0 && !geom::is_empty(&sparse) {
            let spacing = w / p.infill_density;
            let pattern = infill::sparse(p.infill_pattern, &sparse, layer.z, k, w, p.infill_density, lines);
            let joined = connect(pattern, &sparse, CONNECT_REACH * spacing, p.infill_pattern != infill::Pattern::Lines);
            strokes.extend(joined.into_iter().map(|pts| Stroke {
                pts,
                closed: false,
                speed: speed(p.infill_speed),
                kind: Feature::SparseInfill,
                flow: 1.0,
                spiral: false,
            }));
        }
        out.push((layer.z, layer.height, strokes, regions[k].clone()));
    }
    Ok((regions.into_iter().next().unwrap_or_default(), out))
}

/// The part of the solid infill `solid` that lies over air (beyond the layer below), without slivers narrower than a line.
fn bridges(solid: &Region, below: &Region, w: f64) -> Region {
    let over_air = geom::difference(solid, below);
    geom::inset(&geom::inset(&over_air, w / 2.0), -w / 2.0)
}

/// The angle (in steps of 15 degrees) at which lines across `bridge` rest on `below` at both ends the most, by length; of those as
/// good, the one with the shortest lines.
fn bridge_angle(bridge: &Region, below: &Region, w: f64) -> f64 {
    let mut best = (f64::MIN, f64::MAX, 0.0);
    for step in 0..12 {
        let angle = f64::from(step) * 15.0;
        let pieces = lines(bridge, w, angle);
        if pieces.is_empty() {
            continue;
        }
        let (s, c) = angle.to_radians().sin_cos();
        let (mut anchored, mut total) = (0.0, 0.0);
        for l in &pieces {
            let (a, b) = (l[0], l[l.len() - 1]);
            let len = dist(a, b);
            total += len;
            // A line rests where a point a line's width past its end is on the layer below.
            let d = if len > 1e-9 { [(b[0] - a[0]) / len, (b[1] - a[1]) / len] } else { [c, s] };
            let past_a = [a[0] - d[0] * w, a[1] - d[1] * w];
            let past_b = [b[0] + d[0] * w, b[1] + d[1] * w];
            if geom::contains(below, past_a) && geom::contains(below, past_b) {
                anchored += len;
            }
        }
        let (share, mean) = (anchored / total.max(1e-9), total / pieces.len() as f64);
        if share > best.0 + 1e-6 || ((share - best.0).abs() <= 1e-6 && mean < best.1) {
            best = (share, mean, angle);
        }
    }
    best.2
}

/// What layer `k` has material over and under, for `up` layers above and `down` below: empty when the stack is not that tall, so the
/// top and bottom layers come out solid.
fn covered(inner: &[Region], k: usize, up: usize, down: usize) -> Region {
    if k < down || k + up >= inner.len() {
        return Region::new();
    }
    let mut others = inner[k - down..=k + up].iter().enumerate().filter(|&(i, _)| i != down).map(|(_, r)| r);
    match others.next() {
        None => inner[k].clone(),
        Some(first) => others.fold(first.clone(), |a, r| geom::intersect(&a, r)),
    }
}

/// Parallel lines `spacing` apart at `angle`, on a grid anchored at the origin so every layer and object lines up, cut to `r`,
/// ordered back and forth, and joined where they can be.
fn fill_lines(r: &Region, spacing: f64, angle: f64) -> Vec<Vec<P2>> {
    connect(lines(r, spacing, angle), r, CONNECT_REACH * spacing, false)
}

/// The same lines, not joined.
fn lines(r: &Region, spacing: f64, angle: f64) -> Vec<Vec<P2>> {
    let (s, c) = angle.to_radians().sin_cos();
    let (d, n) = ([c, s], [-s, c]);
    let pts = || r.iter().flatten().flatten();
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    let mut big = 0.0f64;
    for q in pts() {
        let v = q[0] * n[0] + q[1] * n[1];
        lo = lo.min(v);
        hi = hi.max(v);
        big = big.max(q[0].abs() + q[1].abs());
    }
    if lo > hi {
        return Vec::new();
    }
    let big = big * 2.0 + 1.0;
    let (from, to) = ((lo / spacing).ceil() as i64, (hi / spacing).floor() as i64);
    let lines: Vec<Vec<P2>> = (from..=to)
        .map(|k| {
            let o = [k as f64 * spacing * n[0], k as f64 * spacing * n[1]];
            vec![[o[0] - big * d[0], o[1] - big * d[1]], [o[0] + big * d[0], o[1] + big * d[1]]]
        })
        .collect();
    let mut pieces: Vec<(i64, f64, Vec<P2>)> = geom::clip_lines(&lines, r)
        .into_iter()
        .filter(|l| l.len() >= 2)
        .map(|l| {
            let key = ((l[0][0] * n[0] + l[0][1] * n[1]) / spacing).round() as i64;
            let t = l[0][0] * d[0] + l[0][1] * d[1];
            // Odd lines are walked the other way, so the head zigzags instead of returning to the same side each time.
            if key.rem_euclid(2) == 1 { (key, -t, l.into_iter().rev().collect()) } else { (key, t, l) }
        })
        .collect();
    pieces.sort_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).unwrap_or(std::cmp::Ordering::Equal));
    pieces.into_iter().map(|(_, _, l)| l).collect()
}

/// How far, in line spacings, one infill line's end may be joined to the next line's start.
const CONNECT_REACH: f64 = 3.0;

/// Joins each line's end to the next line's start where a short path inside `r` exists: the straight one, or else the way along the
/// boundary the two ends lie on (with `boundary_only`, only that, for patterns whose lines can end inside the region, where a
/// straight join would cross a cell). Joined lines print as one stroke, so the head neither travels nor retracts between them.
fn connect(lines: Vec<Vec<P2>>, r: &Region, reach: f64, boundary_only: bool) -> Vec<Vec<P2>> {
    let mut out: Vec<Vec<P2>> = Vec::new();
    for line in lines {
        let (Some(current), Some(&start)) = (out.last_mut(), line.first()) else {
            out.push(line);
            continue;
        };
        let Some(&end) = current.last() else { continue };
        match joint(end, start, r, reach, boundary_only) {
            Some(path) => {
                current.extend(path.into_iter().skip(1));
                current.extend(line.into_iter().skip(1));
            }
            None => out.push(line),
        }
    }
    out
}

/// A path from `a` to `b` no longer than `reach` that stays inside `r`, both ends included.
fn joint(a: P2, b: P2, r: &Region, reach: f64, boundary_only: bool) -> Option<Vec<P2>> {
    if dist(a, b) > reach {
        return None;
    }
    let inside = |path: &Vec<P2>| {
        let len: f64 = path.windows(2).map(|w| dist(w[0], w[1])).sum();
        let kept: f64 = geom::clip_lines(std::slice::from_ref(path), r).iter().flat_map(|p| p.windows(2).map(|w| dist(w[0], w[1]))).sum();
        len <= reach && kept >= len * 0.999 - 1e-6
    };
    let straight = vec![a, b];
    if !boundary_only && inside(&straight) {
        return Some(straight);
    }
    // Along the boundary: both ends on one contour, walked the shorter way round.
    for contour in r.iter().flatten() {
        let (Some((ia, ta)), Some((ib, tb))) = (on_contour(contour, a), on_contour(contour, b)) else { continue };
        let n = contour.len();
        let walk = |forward: bool| -> Vec<P2> {
            let mut path = vec![a];
            let (mut i, stop) = (ia, ib);
            if forward {
                if !(i == stop && tb >= ta) {
                    loop {
                        i = (i + 1) % n;
                        path.push(contour[i]);
                        if i == stop || path.len() > n + 1 {
                            break;
                        }
                    }
                }
            } else if !(i == stop && tb <= ta) {
                loop {
                    path.push(contour[i]);
                    if i == (stop + 1) % n || path.len() > n + 1 {
                        break;
                    }
                    i = (i + n - 1) % n;
                }
            }
            path.push(b);
            path
        };
        let best = [walk(true), walk(false)]
            .into_iter()
            .filter(|p| inside(p))
            .min_by(|x, y| x.windows(2).map(|w| dist(w[0], w[1])).sum::<f64>().total_cmp(&y.windows(2).map(|w| dist(w[0], w[1])).sum()));
        if best.is_some() {
            return best;
        }
    }
    None
}

/// The segment of `contour` that `p` lies on (from point `i` to `i + 1`), and how far along it, if within a micron or so.
fn on_contour(contour: &[P2], p: P2) -> Option<(usize, f64)> {
    let n = contour.len();
    (0..n)
        .filter_map(|i| {
            let (a, b) = (contour[i], contour[(i + 1) % n]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let len2 = d[0] * d[0] + d[1] * d[1];
            if len2 < 1e-18 {
                return None;
            }
            let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0);
            let off = dist(p, [a[0] + t * d[0], a[1] + t * d[1]]);
            (off < 2e-3).then_some((i, t, off))
        })
        .min_by(|x, y| x.2.total_cmp(&y.2))
        .map(|(i, t, _)| (i, t))
}

/// Where a loop starts: the index of its seam point under `rule`, with the head at `here` and the layer below's seams at `below`.
fn seam(pts: &[P2], rule: Seam, here: P2, below: &[P2]) -> Option<usize> {
    let n = pts.len();
    let nearest_to = |to: P2| (0..n).min_by(|&a, &b| dist(to, pts[a]).total_cmp(&dist(to, pts[b])));
    // The rearmost point; of several as far back, the leftmost, so the choice is the same on every layer.
    let rear = || (0..n).max_by(|&a, &b| pts[a][1].total_cmp(&pts[b][1]).then(pts[b][0].total_cmp(&pts[a][0])));
    match rule {
        Seam::Nearest => nearest_to(here),
        Seam::Rear => rear(),
        Seam::Aligned => {
            // Nearest the seam below that is nearest this loop; the first layer starts at the rear.
            let reference = below.iter().copied().min_by(|a, b| {
                let d = |q: &P2| pts.iter().map(|p| dist(*p, *q)).fold(f64::MAX, f64::min);
                d(a).total_cmp(&d(b))
            });
            match reference {
                Some(r) => nearest_to(r),
                None => rear(),
            }
        }
        Seam::Sharpest => {
            // The material is on the left of every loop, so a right turn is an inside corner, where a seam hides best; without one, the
            // sharpest outside corner. Turns within a few degrees count as equal, and the rearmost of those wins.
            let turn = |i: usize| {
                let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
                let (u, v) = ([b[0] - a[0], b[1] - a[1]], [c[0] - b[0], c[1] - b[1]]);
                (u[0] * v[1] - u[1] * v[0]).atan2(u[0] * v[0] + u[1] * v[1])
            };
            let concave =
                (0..n).filter(|&i| turn(i) < -0.1).min_by(|&a, &b| turn(a).total_cmp(&turn(b)).then(pts[b][1].total_cmp(&pts[a][1])));
            concave.or_else(|| {
                let sharpest = (0..n).map(turn).fold(f64::MIN, f64::max);
                (0..n)
                    .filter(|&i| turn(i) >= sharpest - 0.05)
                    .max_by(|&a, &b| pts[a][1].total_cmp(&pts[b][1]).then(pts[b][0].total_cmp(&pts[a][0])))
            })
        }
    }
}

struct Writer<'a> {
    p: &'a Profile,
    out: String,
    at: Option<P2>,
    z: f64,
    retracted: bool,
    filament: f64,
    planner: crate::time::Planner,
    /// Where this layer's loops started, and the layer below's, for aligned seams.
    seams: Vec<P2>,
    below: Vec<P2>,
    /// The feature last named with `;TYPE:`, named again after each layer change.
    feature: Option<Feature>,
    /// The fan the layer asks for, and the fan as last set (a bridge turns it up).
    fan: f64,
    fan_now: f64,
    /// The region of the object and layer being printed: a travel inside it needs no retraction (combing).
    region: Region,
    /// In vase mode, the heights a climbing loop goes from and to on this layer.
    spiral: Option<(f64, f64)>,
    /// Whether the head is lifted by `z_hop`, and the path a wipe follows from where the head is (the stroke just printed).
    hopped: bool,
    wipe: Vec<P2>,
    by_feature: std::collections::BTreeMap<&'static str, f64>,
}

fn dist(a: P2, b: P2) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

impl Writer<'_> {
    fn line(&mut self, s: String) {
        self.out.push_str(&s);
        self.out.push('\n');
    }

    /// Where the machine puts a point of the scene.
    fn machine(&self, v: P2) -> P2 {
        if self.p.origin_center { v } else { [v[0] + self.p.bed[0] / 2.0, v[1] + self.p.bed[1] / 2.0] }
    }

    /// Pulls the filament back: while wiping along the stroke just printed when `wipe_distance` asks, then lifts by `z_hop`.
    fn retract(&mut self) {
        let (len, speed) = (self.p.retract_length, self.p.retract_speed);
        if !self.retracted && len > 0.0 {
            let wipe = std::mem::take(&mut self.wipe);
            match (self.at, self.p.wipe_distance > 0.0 && !wipe.is_empty() && !self.p.firmware_retraction) {
                (Some(mut at), true) => {
                    // The retraction is spread over the wipe, in proportion to the distance each piece covers.
                    let mut left = self.p.wipe_distance;
                    for q in wipe {
                        if left <= 1e-9 {
                            break;
                        }
                        let d = dist(at, q);
                        if d < 1e-9 {
                            continue;
                        }
                        let t = (left / d).min(1.0);
                        let to = [at[0] + (q[0] - at[0]) * t, at[1] + (q[1] - at[1]) * t];
                        let (m, e) = (self.machine(to), len * (d * t) / self.p.wipe_distance);
                        self.line(format!("G1 X{:.3} Y{:.3} E-{e:.5} F{:.0}", m[0], m[1], self.p.travel_speed * 60.0));
                        self.planner.xy(at, to, self.p.travel_speed);
                        (at, left) = (to, left - d * t);
                    }
                    // What the stroke was too short to wipe off goes back in place.
                    let rest = len * left / self.p.wipe_distance;
                    if rest > 1e-6 {
                        self.line(format!("G1 E-{rest:.5} F{:.0}", speed * 60.0));
                    }
                    self.at = Some(at);
                }
                _ if self.p.firmware_retraction => self.line("G10".into()),
                _ => self.line(format!("G1 E-{len:.3} F{:.0}", speed * 60.0)),
            }
            self.planner.other(len, speed);
            self.retracted = true;
            if self.p.z_hop > 0.0 {
                self.line(format!("G1 Z{:.3} F{:.0}", self.z + self.p.z_hop, Z_SPEED * 60.0));
                self.planner.other(self.p.z_hop, Z_SPEED);
                self.hopped = true;
            }
        }
    }

    fn unretract(&mut self) {
        if self.hopped {
            self.line(format!("G1 Z{:.3} F{:.0}", self.z, Z_SPEED * 60.0));
            self.planner.other(self.p.z_hop, Z_SPEED);
            self.hopped = false;
        }
        if self.retracted {
            let (len, speed) = (self.p.retract_length, self.p.retract_speed);
            self.line(if self.p.firmware_retraction { "G11".into() } else { format!("G1 E{len:.3} F{:.0}", speed * 60.0) });
            self.planner.other(len, speed);
            self.retracted = false;
        }
    }

    fn set_z(&mut self, z: f64) {
        if (z - self.z).abs() > 1e-9 {
            self.below = std::mem::take(&mut self.seams);
            self.feature = None;
            self.retract();
            self.line(format!("G1 Z{z:.3} F{:.0}", Z_SPEED * 60.0));
            self.planner.other((z - self.z).abs(), Z_SPEED);
            self.z = z;
            self.hopped = false;
        }
    }

    fn travel(&mut self, to: P2) {
        if self.at.is_some_and(|from| dist(from, to) < 1e-9) {
            return; // already there: the next run of a line goes on from where the last ended
        }
        let far = self.at.is_none_or(|from| dist(from, to) >= self.p.retract_min_travel);
        // Combing: a travel that stays inside the part crosses no gap, so nothing strings and no retraction is needed.
        let inside = self.p.combing && self.at.is_some_and(|from| stays_inside(&self.region, from, to));
        if far && !inside {
            self.retract();
        }
        self.wipe.clear();
        let m = self.machine(to);
        self.line(format!("G0 X{:.3} Y{:.3} F{:.0}", m[0], m[1], self.p.travel_speed * 60.0));
        if let Some(from) = self.at {
            self.planner.xy(from, to, self.p.travel_speed);
        }
        self.at = Some(to);
    }

    fn extrude(&mut self, to: P2, per_mm: f64, speed: f64) {
        let len = self.at.map_or(0.0, |from| dist(from, to));
        if len < 1e-6 {
            return; // a move of no length pushes no plastic and only fills the printer's buffer
        }
        self.unretract();
        let (m, e) = (self.machine(to), len * per_mm);
        self.line(format!("G1 X{:.3} Y{:.3} E{e:.5} F{:.0}", m[0], m[1], speed * 60.0));
        self.filament += e;
        if let Some(f) = self.feature {
            *self.by_feature.entry(f.name()).or_default() += e;
        }
        if let Some(from) = self.at {
            self.planner.xy(from, to, speed);
        }
        self.at = Some(to);
    }

    /// An extrusion to `to` that also rises to `z` (vase mode).
    fn climb(&mut self, to: P2, z: f64, per_mm: f64, speed: f64) {
        let len = self.at.map_or(0.0, |from| dist(from, to));
        if len < 1e-6 {
            return;
        }
        self.unretract();
        let (m, e) = (self.machine(to), len * per_mm);
        self.line(format!("G1 X{:.3} Y{:.3} Z{z:.3} E{e:.5} F{:.0}", m[0], m[1], speed * 60.0));
        self.filament += e;
        if let Some(f) = self.feature {
            *self.by_feature.entry(f.name()).or_default() += e;
        }
        if let Some(from) = self.at {
            self.planner.xy(from, to, speed);
        }
        (self.at, self.z) = (Some(to), z);
    }

    /// Prints `strokes`: the nearest loop goes first and starts where the seam rule says; open ones keep their order.
    fn strokes(&mut self, mut strokes: Vec<Stroke>, per_mm: f64) {
        let (mut loops, open): (Vec<Stroke>, Vec<Stroke>) = strokes.drain(..).partition(|s| s.closed);
        while !loops.is_empty() {
            let here = self.at.unwrap_or([0.0, 0.0]);
            let nearest = |s: &Stroke| s.pts.iter().map(|q| dist(here, *q)).fold(f64::MAX, f64::min);
            let Some(i) =
                (0..loops.len()).min_by(|&a, &b| nearest(&loops[a]).partial_cmp(&nearest(&loops[b])).unwrap_or(std::cmp::Ordering::Equal))
            else {
                break;
            };
            let mut s = loops.swap_remove(i);
            if let Some(start) = seam(&s.pts, self.p.seam, here, &self.below) {
                s.pts.rotate_left(start);
                self.seams.push(s.pts[0]);
            }
            self.stroke(&s, per_mm);
        }
        for s in &open {
            self.stroke(s, per_mm);
        }
    }

    fn stroke(&mut self, s: &Stroke, per_mm: f64) {
        let Some(first) = s.pts.first() else { return };
        if self.feature != Some(s.kind) {
            self.line(format!(";TYPE:{}", s.kind.name()));
            self.feature = Some(s.kind);
        }
        let fan = if s.kind == Feature::Bridge { self.p.bridge_fan } else { self.fan };
        if (fan - self.fan_now).abs() > 0.5 {
            self.line(format!("M106 S{fan:.0}"));
            self.fan_now = fan;
        }
        let per_mm = per_mm * s.flow;
        self.travel(*first);
        if let (true, Some((from, to))) = (s.spiral, self.spiral) {
            // Each point a little higher, in proportion to the way round, so the loop ends where the next begins.
            let total = path_length(s).max(1e-9);
            let mut done = 0.0;
            let mut prev = *first;
            for q in s.pts.iter().skip(1).chain(std::iter::once(first)) {
                done += dist(prev, *q);
                prev = *q;
                self.climb(*q, from + (to - from) * (done / total), per_mm, s.speed);
            }
        } else {
            for q in s.pts.iter().skip(1) {
                self.extrude(*q, per_mm, s.speed);
            }
            if s.closed {
                self.extrude(*first, per_mm, s.speed);
            }
        }
        // A wipe goes on round a loop from where it closed, or back along a line.
        self.wipe = if s.closed { s.pts.iter().skip(1).copied().collect() } else { s.pts.iter().rev().skip(1).copied().collect() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The filament the file pushes in the print (the sum of the positive E of moves that travel), as volume in mm³.
    fn volume(g: &str, p: &Profile) -> f64 {
        let e: f64 =
            g.lines().filter(|l| l.starts_with("G1 X")).filter_map(|l| l.split(" E").nth(1)?.split(' ').next()?.parse::<f64>().ok()).sum();
        e * PI * (p.filament_diameter / 2.0).powi(2)
    }

    fn cube(side: f64) -> Vec<(u32, Mesh)> {
        vec![(1, Mesh::cuboid([side, side, side]))]
    }

    #[test]
    fn a_cube_prints_the_right_amount_of_plastic_inside_the_plate() {
        let p = Profile::default();
        let out = gcode(&cube(20.0), &p).unwrap();
        assert_eq!(out.layers, 100);
        assert!(out.gcode.contains("M104 S215") && out.gcode.trim_end().ends_with("M84"));
        let v = volume(&out.gcode, &p);
        // Two walls, four solid skins top and bottom, 15 % inside: a bit under half of the cube's 8000 mm³, and never most of it or nearly none.
        assert!((1500.0..4500.0).contains(&v), "{v}");
        // Every move stays on the plate, and the head never goes down.
        let mut z = 0.0;
        // The end block lifts the head with a relative move (G91), which is not a drop to Z10.
        for l in out.gcode.split("M104 S0").next().unwrap().lines() {
            assert!(l.starts_with(';') || (!l.contains("NaN") && !l.contains("inf")), "{l}");
            if let Some(x) = l.strip_prefix("G1 Z") {
                let new: f64 = x.split(' ').next().unwrap().parse().unwrap();
                assert!(new >= z - 1e-9, "{l}");
                z = new;
            }
            if let Some(m) = l.strip_prefix("G1 X").or_else(|| l.strip_prefix("G0 X")) {
                let x: f64 = m.split(' ').next().unwrap().parse().unwrap();
                assert!((0.0..=220.0).contains(&x), "{l}");
            }
        }
        assert!((z - 20.0).abs() < 1e-6, "{z}");
        assert!(out.seconds > 600.0 && out.seconds < 7200.0, "{}", out.seconds);
    }

    #[test]
    fn full_infill_fills_the_cube_and_no_infill_leaves_it_hollow() {
        let full = Profile { infill_density: 1.0, ..Profile::default() };
        let v = volume(&gcode(&cube(20.0), &full).unwrap().gcode, &full);
        assert!((6400.0..9200.0).contains(&v), "{v}");
        let hollow = Profile { infill_density: 0.0, ..Profile::default() };
        let h = volume(&gcode(&cube(20.0), &hollow).unwrap().gcode, &hollow);
        assert!(h < v / 2.0 && h > 500.0, "{h} {v}");
    }

    #[test]
    fn a_machine_zero_in_the_middle_shifts_nothing_and_a_corner_zero_shifts_half_the_plate() {
        let corner = gcode(&cube(10.0), &Profile::default()).unwrap().gcode;
        let centre = gcode(&cube(10.0), &Profile { origin_center: true, ..Profile::default() }).unwrap().gcode;
        assert!(corner.contains("X110.") || corner.contains("X105.") || corner.contains("X115."), "a cube at the middle of a 220 plate");
        assert!(centre.contains("X-") && !centre.contains("X110."));
    }

    #[test]
    fn what_cannot_be_printed_is_refused_with_a_reason() {
        let p = Profile::default();
        let mut far = Mesh::cuboid([10.0, 10.0, 10.0]);
        far = far.transformed(&lamina_core::Transform::translate([200.0, 0.0, 0.0]));
        assert!(gcode(&[(7, far)], &p).unwrap_err().contains("object 7 does not fit"));
        let tall = Mesh::cuboid([10.0, 10.0, 300.0]);
        assert!(gcode(&[(1, tall)], &p).unwrap_err().contains("tall"));
        let under = Mesh::cuboid([10.0, 10.0, 10.0]).transformed(&lamina_core::Transform::translate([0.0, 0.0, -5.0]));
        assert!(gcode(&[(1, under)], &p).unwrap_err().contains("drop_to_bed"));
        assert!(gcode(&[], &p).is_err());
        let mut open = Mesh::cuboid([10.0, 10.0, 10.0]);
        open.triangles.pop();
        assert!(gcode(&[(1, open)], &p).unwrap().warnings[0].contains("not a closed mesh"));
    }

    #[test]
    fn supports_add_plastic_under_a_tee_and_only_when_asked() {
        let pillar = Mesh::cuboid([6.0, 6.0, 4.0]);
        let arms = Mesh::cuboid([18.0, 6.0, 2.0]).transformed(&lamina_core::Transform::translate([0.0, 0.0, 4.0]));
        // Two closed boxes overlapping a hair would print twice; touching faces are fine for the slicer, which unions each layer.
        let tee = vec![(1, Mesh::merged([&pillar, &arms]))];
        let off = Profile::default();
        let on = Profile { support: support::Support::Everywhere, ..Profile::default() };
        let (a, b) = (gcode(&tee, &off).unwrap(), gcode(&tee, &on).unwrap());
        let (va, vb) = (volume(&a.gcode, &off), volume(&b.gcode, &on));
        // Two columns about 5.4 x 6 x 3.8 mm at 15 %, and two denser layers at the top: a few tens of mm3 more.
        assert!(vb - va > 15.0 && vb - va < 120.0, "{va} {vb}");
        // The support is under the arms: an extrusion on layer 5 (z 1.2) reaches beyond the pillar, never without supports.
        let reach = |g: &str| {
            g.split(";LAYER:5\n").nth(1).and_then(|l| l.split(";LAYER:6").next()).map_or(0.0, |l| {
                l.lines()
                    .filter(|l| l.starts_with("G1 X"))
                    .filter_map(|l| l[4..].split(' ').next()?.parse::<f64>().ok())
                    .map(|x| (x - 110.0).abs())
                    .fold(0.0, f64::max)
            })
        };
        assert!(reach(&a.gcode) < 3.1 && reach(&b.gcode) > 7.0, "{} {}", reach(&a.gcode), reach(&b.gcode));
    }

    /// The farthest a first-layer extrusion reaches from the middle of the plate, in x.
    fn first_layer_reach(g: &str) -> f64 {
        let first = g.split(";LAYER:1\n").next().unwrap_or("");
        first
            .lines()
            .filter(|l| l.starts_with("G1 X"))
            .filter_map(|l| l[4..].split(' ').next()?.parse::<f64>().ok())
            .map(|x| (x - 110.0).abs())
            .fold(0.0, f64::max)
    }

    #[test]
    fn a_brim_rings_the_first_layer_and_the_skirt_moves_out_past_it() {
        let cube = cube(10.0);
        let plain = Profile { skirt_loops: 0, ..Profile::default() };
        let brim = Profile { brim_width: 5.0, ..plain.clone() };
        let (a, b) = (gcode(&cube, &plain).unwrap().gcode, gcode(&cube, &brim).unwrap().gcode);
        assert!((first_layer_reach(&a) - 5.0).abs() < 0.3, "{}", first_layer_reach(&a));
        assert!((first_layer_reach(&b) - 10.0).abs() < 0.5, "a brim 5 mm wide: {}", first_layer_reach(&b));
        // Only the first layer has it.
        let second = |g: &str| g.split(";LAYER:1\n").nth(1).unwrap_or("").split(";LAYER:2").next().unwrap_or("").to_string();
        assert_eq!(first_layer_reach(&format!(";LAYER:0\n{}", second(&a))), first_layer_reach(&format!(";LAYER:0\n{}", second(&b))));
        let skirted = gcode(&cube, &Profile { brim_width: 5.0, ..Profile::default() }).unwrap().gcode;
        assert!(first_layer_reach(&skirted) > 10.0 + 2.9, "the skirt is outside the brim: {}", first_layer_reach(&skirted));
    }

    #[test]
    fn a_raft_lifts_the_object_onto_layers_of_its_own() {
        let p = Profile { raft_layers: 3, skirt_loops: 0, ..Profile::default() };
        let out = gcode(&cube(10.0), &p).unwrap();
        assert_eq!(out.layers, 50 + 3);
        // The raft reaches past the cube by its margin, and the cube's first layer is above it by the gap.
        assert!((first_layer_reach(&out.gcode) - (5.0 + RAFT_MARGIN)).abs() < 0.5, "{}", first_layer_reach(&out.gcode));
        let z_of = |layer: usize| {
            out.gcode
                .split(&format!(";LAYER:{layer}\n"))
                .nth(1)
                .and_then(|l| l.lines().find_map(|l| l.strip_prefix("G1 Z")?.split(' ').next()?.parse::<f64>().ok()))
        };
        assert_eq!(z_of(2), Some(0.6));
        assert!((z_of(3).unwrap() - (0.6 + RAFT_GAP + 0.2)).abs() < 1e-6, "{:?}", z_of(3));
        assert!(gcode(&[(1, Mesh::cuboid([10.0, 10.0, 249.5]))], &p).unwrap_err().contains("tall"), "the raft counts toward the height");
    }

    /// Retractions on one layer of the G-code.
    fn retractions(g: &str, layer: usize) -> usize {
        g.split(&format!(";LAYER:{layer}\n"))
            .nth(1)
            .unwrap_or("")
            .split(";LAYER:")
            .next()
            .unwrap_or("")
            .lines()
            .filter(|l| l.starts_with("G1 E-"))
            .count()
    }

    #[test]
    fn infill_lines_are_joined_so_the_head_does_not_retract_between_them() {
        let p = Profile::default();
        let out = gcode(&cube(20.0), &p).unwrap().gcode;
        // A sparse layer: once onto the layer, once from the walls to the infill, and no more.
        assert!(retractions(&out, 50) <= 3, "{}", retractions(&out, 50));
        // A solid layer too.
        assert!(retractions(&out, 1) <= 3, "{}", retractions(&out, 1));
        // The joints stay inside: every extrusion of the layer is within the cube.
        let layer = out.split(";LAYER:50\n").nth(1).unwrap().split(";LAYER:").next().unwrap();
        for l in layer.lines().filter(|l| l.starts_with("G1 X")) {
            let mut w = l.split(' ');
            let x: f64 = w.nth(1).unwrap()[1..].parse().unwrap();
            let y: f64 = w.next().unwrap()[1..].parse().unwrap();
            assert!((x - 110.0).abs() <= 10.0 && (y - 110.0).abs() <= 10.0, "{l}");
        }
    }

    #[test]
    fn a_joint_follows_the_boundary_round_a_corner_it_cannot_cut() {
        // An L: the straight way from one arm's end to the other leaves the shape; the way along the edge does not.
        let l = geom::region(&[vec![[0.0, 0.0], [10.0, 0.0], [10.0, 2.0], [2.0, 2.0], [2.0, 10.0], [0.0, 10.0]]]);
        let path = joint([10.0, 1.0], [1.0, 10.0], &l, 30.0, false).unwrap();
        assert!(path.len() > 2 && path.contains(&[2.0, 2.0]), "{path:?}");
        assert!(joint([10.0, 1.0], [1.0, 10.0], &l, 5.0, false).is_none(), "too far");
        let square = geom::region(&[vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]]);
        assert_eq!(joint([10.0, 2.0], [10.0, 3.0], &square, 3.0, false), Some(vec![[10.0, 2.0], [10.0, 3.0]]));
        assert!(joint([5.0, 5.0], [6.0, 5.0], &square, 3.0, true).is_none(), "inside the region, only the boundary joins");
    }

    /// Where each wall loop of the G-code starts: the point after the travel that begins it, for every layer.
    fn loop_starts(g: &str) -> Vec<P2> {
        let mut out = Vec::new();
        let mut travelled = false;
        for l in g.lines() {
            if let Some(m) = l.strip_prefix("G0 X") {
                let mut w = m.split(' ');
                let (x, y) = (w.next().unwrap().parse().unwrap(), w.next().unwrap()[1..].parse().unwrap());
                if travelled {
                    out.pop();
                }
                out.push([x, y]);
                travelled = true;
            } else if l.starts_with("G1 X") {
                travelled = false;
            }
        }
        out
    }

    #[test]
    fn seams_start_where_the_rule_says() {
        let tower = vec![(1, Mesh::cylinder(8.0, 6.0, 48))];
        let starts = |seam: Seam| -> Vec<P2> {
            let p = Profile { seam, skirt_loops: 0, infill_density: 0.0, top_layers: 0, bottom_layers: 0, ..Profile::default() };
            loop_starts(&gcode(&tower, &p).unwrap().gcode)
        };
        // Rear: every loop starts at its point of greatest y, straight behind the centre.
        let rear = starts(Seam::Rear);
        // (A loop that starts right above where the last layer's ended needs no travel, so not every start shows as one.)
        assert!(rear.len() >= 30 && rear.iter().all(|q| (q[0] - 110.0).abs() < 1e-3 && q[1] > 110.0), "{rear:?}");
        // Aligned: the seams line up from layer to layer, so they all lie close to one line up the side.
        let aligned = starts(Seam::Aligned);
        assert!(aligned.windows(2).all(|w| dist(w[0], w[1]) < 1.0), "{aligned:?}");
        // Nearest wanders, which is what the other rules are for.
        let nearest = starts(Seam::Nearest);
        assert!(nearest.iter().any(|q| (q[0] - 110.0).abs() > 1.0), "{nearest:?}");
    }

    #[test]
    fn the_sharpest_seam_hides_in_an_inside_corner() {
        let l: Vec<P2> = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 2.0], [2.0, 2.0], [2.0, 10.0], [0.0, 10.0]];
        assert_eq!(seam(&l, Seam::Sharpest, [50.0, 50.0], &[]), Some(3), "the inside corner of the L");
        let square: Vec<P2> = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert_eq!(seam(&square, Seam::Sharpest, [50.0, 50.0], &[]), Some(3), "no inside corner: the rear left of the equal corners");
        assert_eq!(seam(&square, Seam::Nearest, [11.0, -1.0], &[]), Some(1));
        assert_eq!(seam(&square, Seam::Aligned, [0.0, 0.0], &[[9.0, 11.0]]), Some(2));
    }

    #[test]
    fn every_kind_of_move_is_named_before_it() {
        let p = Profile { brim_width: 2.0, support: support::Support::Everywhere, ..Profile::default() };
        let pillar = Mesh::cuboid([6.0, 6.0, 4.0]);
        let arms = Mesh::cuboid([18.0, 6.0, 2.0]).transformed(&lamina_core::Transform::translate([0.0, 0.0, 4.0]));
        let g = gcode(&[(1, Mesh::merged([&pillar, &arms]))], &p).unwrap().gcode;
        for f in [
            Feature::OuterWall,
            Feature::InnerWall,
            Feature::SolidInfill,
            Feature::SparseInfill,
            Feature::Skirt,
            Feature::Brim,
            Feature::Support,
            Feature::SupportInterface,
        ] {
            assert!(g.contains(&format!(";TYPE:{}\n", f.name())), "{f:?}");
        }
        // Every extrusion follows a type line on its layer.
        for layer in g.split(";LAYER:").skip(1) {
            let first_extrusion = layer.find("G1 X").unwrap_or(usize::MAX);
            assert!(layer.find(";TYPE:").unwrap_or(usize::MAX) < first_extrusion, "a layer prints before naming what");
        }
        let raft = gcode(&cube(10.0), &Profile { raft_layers: 2, ..Profile::default() }).unwrap();
        assert!(raft.gcode.contains(";TYPE:raft\n"));
        // The filament of every kind adds up to the whole.
        let sum: f64 = raft.features.values().sum();
        assert!((sum - raft.filament_mm).abs() < 1e-6 && raft.features["raft"] > 0.0, "{:?}", raft.features);
        assert_eq!(Feature::from_name("outer-wall"), Some(Feature::OuterWall));
    }

    #[test]
    fn every_infill_pattern_slices_a_cube_to_about_the_same_plastic() {
        let base = volume(&gcode(&cube(20.0), &Profile::default()).unwrap().gcode, &Profile::default());
        for pattern in [infill::Pattern::Grid, infill::Pattern::Triangles, infill::Pattern::Gyroid, infill::Pattern::Honeycomb] {
            let p = Profile { infill_pattern: pattern, ..Profile::default() };
            let out = gcode(&cube(20.0), &p).unwrap();
            let v = volume(&out.gcode, &p);
            assert!((v - base).abs() < 0.12 * base, "{pattern:?}: {v} against {base} for lines");
            assert!(out.features["sparse-infill"] > 0.0);
        }
    }

    #[test]
    fn a_span_between_two_pillars_is_bridged_straight_across_with_the_fan_up() {
        // Two pillars 5 mm square, 20 mm apart, and a slab 1 mm thick across them at 5 mm.
        let left = Mesh::cuboid([5.0, 10.0, 5.0]).transformed(&lamina_core::Transform::translate([-12.5, 0.0, 0.0]));
        let right = Mesh::cuboid([5.0, 10.0, 5.0]).transformed(&lamina_core::Transform::translate([12.5, 0.0, 0.0]));
        let slab = Mesh::cuboid([30.0, 10.0, 1.0]).transformed(&lamina_core::Transform::translate([0.0, 0.0, 5.0]));
        let p = Profile { bridge_speed: 22.0, bridge_flow: 0.9, fan_speed: 128.0, ..Profile::default() };
        let out = gcode(&[(1, Mesh::merged([&left, &right, &slab]))], &p).unwrap();
        let layer = out.gcode.split(";LAYER:25\n").nth(1).unwrap().split(";LAYER:").next().unwrap();
        let bridge = layer.split(";TYPE:bridge\n").nth(1).expect("the first layer over the gap is a bridge");
        assert!(bridge.starts_with("M106 S255"), "the fan comes up for it");
        let moves: Vec<(f64, f64)> = bridge
            .lines()
            .take_while(|l| !l.starts_with(";TYPE"))
            .filter(|l| l.starts_with("G1 X") && l.contains(" F1320"))
            .map(|l| {
                let mut w = l.split(' ');
                (w.nth(1).unwrap()[1..].parse().unwrap(), w.next().unwrap()[1..].parse().unwrap())
            })
            .collect();
        assert!(moves.len() >= 2, "printed at the bridge speed: {bridge}");
        // The long strokes run along x, from pillar to pillar.
        let long =
            moves.windows(2).map(|w| ((w[1].0 - w[0].0).abs(), (w[1].1 - w[0].1).abs())).filter(|d| d.0 + d.1 > 5.0).collect::<Vec<_>>();
        assert!(!long.is_empty() && long.iter().all(|d| d.1 < 0.5), "{long:?}");
        assert!(out.features["bridge"] > 0.0 && layer.contains("M106 S128"), "the fan goes back after the bridge");
    }

    #[test]
    fn a_small_layer_is_slowed_to_cool_and_the_fan_comes_up_in_steps() {
        let tiny = vec![(1, Mesh::cuboid([5.0, 5.0, 5.0]))];
        let speeds = |p: &Profile| -> Vec<f64> {
            let g = gcode(&tiny, p).unwrap().gcode;
            let layer = g.split(";LAYER:12\n").nth(1).unwrap().split(";LAYER:").next().unwrap().to_string();
            layer.lines().filter(|l| l.starts_with("G1 X")).filter_map(|l| l.rsplit(" F").next()?.parse::<f64>().ok()).collect()
        };
        let quick = speeds(&Profile { min_layer_time: 0.0, ..Profile::default() });
        let slow = speeds(&Profile { min_layer_time: 15.0, ..Profile::default() });
        assert!(quick.contains(&2700.0), "walls at 45 mm/s without a minimum: {quick:?}");
        assert!(slow.iter().all(|&f| (600.0..2700.0).contains(&f)), "slowed, never under 10 mm/s: {slow:?}");
        let g = gcode(&tiny, &Profile { fan_full_layer: 4, fan_speed: 200.0, ..Profile::default() }).unwrap().gcode;
        let fans: Vec<&str> = g.lines().filter(|l| l.starts_with("M106")).collect();
        assert_eq!(fans, ["M106 S50", "M106 S100", "M106 S150", "M106 S200"], "a quarter more each layer from the second");
    }

    #[test]
    fn combing_saves_retractions_and_z_hop_and_wipe_go_with_each_one() {
        let two = vec![
            (1, Mesh::cuboid([15.0, 15.0, 4.0]).transformed(&lamina_core::Transform::translate([-15.0, 0.0, 0.0]))),
            (2, Mesh::cylinder(7.0, 4.0, 48).transformed(&lamina_core::Transform::translate([15.0, 0.0, 0.0]))),
        ];
        let count = |p: &Profile| {
            gcode(&two, p).unwrap().gcode.lines().filter(|l| l.starts_with("G1 E-") || (l.starts_with("G1 X") && l.contains(" E-"))).count()
        };
        let (combed, plain) = (count(&Profile::default()), count(&Profile { combing: false, ..Profile::default() }));
        assert!(combed < plain, "combing retracts less: {combed} against {plain}");
        // Each retraction lifts by the hop and comes back down before the next extrusion.
        let hop = Profile { z_hop: 0.4, ..Profile::default() };
        let g = gcode(&two, &hop).unwrap().gcode;
        let layer = g.split(";LAYER:10\n").nth(1).unwrap().split(";LAYER:").next().unwrap();
        assert!(layer.contains("G1 Z2.600 F1200") && layer.contains("G1 Z2.200 F1200"), "lifted from 2.2 to 2.6 and back");
        // A wipe retracts while moving along the stroke just printed.
        let wiped = gcode(&two, &Profile { wipe_distance: 2.0, ..Profile::default() }).unwrap().gcode;
        assert!(wiped.lines().any(|l| l.starts_with("G1 X") && l.contains(" E-")), "a wiping retraction moves");
        let e = |g: &str| -> f64 { g.lines().filter_map(|l| l.split(" E").nth(1)?.split(' ').next()?.parse::<f64>().ok()).sum() };
        assert!((e(&wiped) - e(&gcode(&two, &Profile::default()).unwrap().gcode)).abs() < 0.5, "a wipe takes back what a retraction does");
    }

    #[test]
    fn the_time_counts_acceleration_so_a_slower_machine_takes_longer() {
        let p = Profile::default();
        let quick = gcode(&cube(20.0), &Profile { max_acceleration: 5000.0, ..p.clone() }).unwrap().seconds;
        let slow = gcode(&cube(20.0), &Profile { max_acceleration: 500.0, ..p.clone() }).unwrap().seconds;
        assert!(slow > quick * 1.1, "{slow} {quick}");
        // Never less than distance over speed would say: the plain sum of each move's time at its own speed.
        let g = gcode(&cube(20.0), &p).unwrap();
        let mut at: Option<(f64, f64)> = None;
        let mut bound = 0.0;
        for l in g.gcode.lines().filter(|l| l.starts_with("G1 X") || l.starts_with("G0 X")) {
            let w: Vec<&str> = l.split(' ').collect();
            let (x, y): (f64, f64) = (w[1][1..].parse().unwrap(), w[2][1..].parse().unwrap());
            let f: f64 = w.last().unwrap()[1..].parse::<f64>().unwrap() / 60.0;
            if let Some((px, py)) = at {
                bound += (x - px).hypot(y - py) / f;
            }
            at = Some((x, y));
        }
        assert!(g.seconds > bound, "{} against at least {bound}", g.seconds);
    }

    #[test]
    fn vase_mode_climbs_one_wall_without_stopping_and_leaves_the_top_open() {
        let p = Profile { vase: true, ..Profile::default() };
        let out = gcode(&[(1, Mesh::cylinder(15.0, 10.0, 64))], &p).unwrap();
        let g = &out.gcode;
        let above: &str = g.split(";LAYER:5\n").nth(1).unwrap().split("M104 S0").next().unwrap();
        assert!(!above.contains("TYPE:sparse-infill") && !above.contains("TYPE:solid-infill") && !above.contains("TYPE:inner-wall"));
        // No layer change stops the head: every move up there carries its own z, and z only rises.
        assert!(!above.lines().any(|l| l.starts_with("G1 Z")), "no separate z moves");
        let zs: Vec<f64> = above.lines().filter_map(|l| l.split(" Z").nth(1)?.split(' ').next()?.parse().ok()).collect();
        assert!(zs.len() > 500 && zs.windows(2).all(|w| w[1] >= w[0] - 1e-9), "z climbs steadily");
        assert!((zs.last().unwrap() - 10.0).abs() < 1e-6);
        assert!(above.lines().filter(|l| l.starts_with("G1 E-")).count() <= 1, "no retraction on the way up");
    }

    #[test]
    fn klipper_gets_object_markers_and_each_firmware_its_own_pressure_advance() {
        let two = vec![
            (3, Mesh::cuboid([10.0, 10.0, 2.0]).transformed(&lamina_core::Transform::translate([-15.0, 0.0, 0.0]))),
            (7, Mesh::cuboid([10.0, 10.0, 2.0]).transformed(&lamina_core::Transform::translate([15.0, 0.0, 0.0]))),
        ];
        let k = Profile { firmware: Firmware::Klipper, pressure_advance: 0.04, ..Profile::default() };
        let g = gcode(&two, &k).unwrap().gcode;
        assert!(g.contains("EXCLUDE_OBJECT_DEFINE NAME=object_3 CENTER=95.000,110.000 POLYGON=[[90.000,105.000],[100.000,105.000],[100.000,115.000],[90.000,115.000]]"), "{}", &g[..g.len().min(2000)]);
        assert_eq!(g.matches("EXCLUDE_OBJECT_START NAME=object_7").count(), 10, "once a layer");
        assert_eq!(g.matches("EXCLUDE_OBJECT_START").count(), g.matches("EXCLUDE_OBJECT_END").count());
        assert!(g.find(";TYPE:skirt").unwrap() < g.find("EXCLUDE_OBJECT_START").unwrap(), "the skirt belongs to no object");
        assert!(g.contains("SET_PRESSURE_ADVANCE ADVANCE=0.04"));
        let marlin = gcode(&two, &Profile { pressure_advance: 0.04, ..Profile::default() }).unwrap().gcode;
        assert!(marlin.contains("M900 K0.04") && !marlin.contains("EXCLUDE_OBJECT"));
        let rrf =
            gcode(&two, &Profile { firmware: Firmware::Reprap, pressure_advance: 0.04, firmware_retraction: true, ..Profile::default() })
                .unwrap()
                .gcode;
        assert!(rrf.contains("M572 D0 S0.04") && rrf.contains("\nG10\n") && rrf.contains("\nG11\n") && !rrf.contains("G1 E-"));
    }

    #[test]
    fn a_thumbnail_is_a_png_in_a_comment_block() {
        let g = gcode(&cube(10.0), &Profile { thumbnail: 48, ..Profile::default() }).unwrap().gcode;
        let begin = g.lines().position(|l| l.starts_with("; thumbnail begin 48x48 ")).expect("a thumbnail block");
        let end = g.lines().position(|l| l == "; thumbnail end").unwrap();
        let text: String = g.lines().skip(begin + 1).take(end - begin - 1).map(|l| l.trim_start_matches("; ")).collect();
        let len: usize = g.lines().nth(begin).unwrap().rsplit(' ').next().unwrap().parse().unwrap();
        assert_eq!(text.len(), len);
        assert!(g.lines().skip(begin + 1).take(end - begin - 1).all(|l| l.len() <= 80));
        assert_eq!(&lamina_core::base64::decode(&text).unwrap()[..8], b"\x89PNG\r\n\x1a\n");
        assert!(begin < g.find("M140").map_or(usize::MAX, |i| g[..i].lines().count()), "before the start G-code");
    }

    #[test]
    fn a_wall_thinner_than_a_line_prints_as_gap_fill_instead_of_vanishing() {
        // A fin 0.3 mm thick and 20 mm long, standing on a small base so it slices.
        let fin = Mesh::cuboid([20.0, 0.3, 5.0]).transformed(&lamina_core::Transform::translate([0.0, 0.0, 1.0]));
        let base = Mesh::cuboid([4.0, 4.0, 1.0]);
        let out = gcode(&[(1, Mesh::merged([&base, &fin]))], &Profile::default()).unwrap();
        let layer = out.gcode.split(";LAYER:15\n").nth(1).unwrap().split(";LAYER:").next().unwrap();
        assert!(layer.contains(";TYPE:gap-fill"), "{layer}");
        let xs: Vec<f64> = layer.lines().filter(|l| l.starts_with("G1 X")).filter_map(|l| l[4..].split(' ').next()?.parse().ok()).collect();
        let span = xs.iter().copied().fold(f64::MIN, f64::max) - xs.iter().copied().fold(f64::MAX, f64::min);
        assert!(span > 18.0, "the fin is printed along its length: {span}");
        // Thinner than a line, so less plastic a millimetre than a full line pushes.
        assert!(out.features["gap-fill"] > 0.0);
    }

    #[test]
    fn a_tube_and_a_sphere_slice_without_trouble() {
        let p = Profile::default();
        assert!(gcode(&[(1, Mesh::sphere(10.0, 48))], &p).unwrap().layers >= 99);
        let outer = Mesh::cylinder(10.0, 6.0, 48);
        let mut inner = Mesh::cylinder(5.0, 6.0, 48);
        inner.triangles.iter_mut().for_each(|t| t.swap(1, 2));
        let tube = gcode(&[(1, Mesh::merged([&outer, &inner]))], &p).unwrap();
        // A hole means a wall round it: the volume is well under a solid cylinder's.
        let solid = gcode(&[(1, outer.clone())], &p).unwrap();
        assert!(volume(&tube.gcode, &p) < volume(&solid.gcode, &p));
    }
}
