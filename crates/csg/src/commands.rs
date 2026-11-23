//! The boolean commands, which an app adds to a session with `session.register(lamina_csg::COMMANDS)`. Each replaces its operands
//! with one new object where the result stands, as one undo step.

use lamina_core::{Command, Files, Mesh, Session, Transform};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Op, isolated};

pub static COMMANDS: &[Command] = &[
    Command {
        id: "union",
        params: "{ids:[a, b, ...], name?} joins two or more objects into one solid. The objects are replaced by the result, which stands where they \
                 stood. Returns {id, triangles, watertight, size}. Objects that do not touch stay two bodies in one object",
        run: union,
        undoable: true,
        files: Files::None,
    },
    Command {
        id: "difference",
        params: "{id, minus:[a, b, ...], name?} cuts the objects in `minus` out of object `id`: a hole, a notch, a pocket. Make the cutting shape \
                 stick out past the faces it cuts through (taller than the part for a through hole). The objects are replaced by the result. \
                 Returns {id, triangles, watertight, size}",
        run: difference,
        undoable: true,
        files: Files::None,
    },
    Command {
        id: "cut",
        params: "{id, axis?:\"x\"|\"y\"|\"z\" (default z), at (mm along `axis`, between the object's least and greatest), keep?:\"both\"|\
                 \"lower\"|\"upper\" (default both; lower is the side toward minus the axis), connectors?:{kind?:\"dowel\"|\"pin\"|\"none\", \
                 count? (1..8, default 2), diameter? (mm, default from the face, 3 to 8), depth? (mm into each piece, default 1.5 x the \
                 diameter, at most 15), clearance? (mm a side, default 0.2)}, lay_out? (default false), name?} cuts an object in two with the \
                 plane square to `axis` at `at` (`{id, z}` still cuts level at height z). Each piece is closed and named with \" 1\" or \" 2\". \
                 Without `kind`, dowels are made where the cut face has room (else none, said in `note`): holes in both pieces at the same \
                 places and a loose dowel pin for each pair, added as objects. \"pin\" joins pegs to piece 1 and cuts sockets in piece 2. \
                 Two or more are spread over the face so the pieces cannot turn; where only one fits it is a square key. Without \
                 `lay_out` everything stays where it stood, put together, the dowels in their holes; with it each piece is turned to print \
                 (onto its cut face, pegs up) and the plate is arranged. Returns {ids (piece 1 first), dowels, connectors (sizes and the \
                 centres `at`, as cut), note?, fits? (laid out)}",
        run: crate::split::cut,
        undoable: true,
        files: Files::None,
    },
    Command {
        id: "split_to_fit",
        params: "{id, bed?:[x, y] (default the project's plate), height? (default the printer's), connectors? (as `cut` takes them), \
                 gap? (mm between pieces, default 5)} cuts an object too big for the printer into pieces that each fit: across its longest \
                 side that does not fit first, each such side in as few equal pieces as fit, with connectors at every cut. Each piece is \
                 turned to print where that helps (onto a cut face, pegs up), the dowels stand beside them, and the whole plate is arranged. \
                 Returns {ids (the pieces), dowels, cuts:[{axis, at, connectors}], fits (all on the plate at once), each_fits, note?}",
        run: crate::split::split_to_fit,
        undoable: true,
        files: Files::None,
    },
    Command {
        id: "intersect",
        params: "{ids:[a, b, ...], name?} keeps only the volume all the objects share, and errors when there is none. Returns {id, triangles, watertight, size}",
        run: intersect,
        undoable: true,
        files: Files::None,
    },
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Many {
    ids: Vec<u32>,
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cut {
    id: u32,
    minus: Vec<u32>,
    name: Option<String>,
}

fn union(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = Many::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    fold(s, Op::Union, &a.ids, a.name)
}

fn intersect(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = Many::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    fold(s, Op::Intersect, &a.ids, a.name)
}

fn difference(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = Cut::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    if a.minus.is_empty() {
        return Err("`minus` needs at least one object to cut out".into());
    }
    let mut all = vec![a.id];
    all.extend(&a.minus);
    fold(s, Op::Difference, &all, a.name)
}

/// The first of `ids` combined with each of the others in turn, the operands replaced by the result.
fn fold(s: &mut Session, op: Op, ids: &[u32], name: Option<String>) -> Result<Value, String> {
    if ids.len() < 2 {
        return Err("a boolean needs at least two objects".into());
    }
    let mut seen = ids.to_vec();
    seen.sort_unstable();
    seen.dedup();
    if seen.len() != ids.len() {
        return Err("an object is named twice".into());
    }
    // Each object as it is kept: its own mesh and its transform, so its flat faces are found where they are exactly flat.
    let placed: Vec<(std::sync::Arc<Mesh>, Transform)> =
        ids.iter().map(|&id| s.scene.get(id).map(|o| (o.mesh.clone(), o.xf))).collect::<Result<_, _>>()?;
    let name = name.unwrap_or_else(|| s.scene.get(ids[0]).map(|o| o.name.clone()).unwrap_or_default());
    let mut result = isolated(op, (&placed[0].0, placed[0].1), (&placed[1].0, placed[1].1))?;
    for (mesh, xf) in &placed[2..] {
        result = isolated(op, (&result, Transform::IDENTITY), (mesh, *xf))?;
    }
    if result.triangles.is_empty() {
        return Err(match op {
            Op::Intersect => "the objects share no volume, so there is nothing to keep".into(),
            Op::Difference => "nothing is left: the cut removes the whole object".into(),
            Op::Union => "the result is empty".into(),
        });
    }
    let scene = s.scene_mut();
    scene.objects.retain(|o| !ids.contains(&o.id));
    let watertight = result.is_watertight();
    let triangles = result.triangles.len();
    let id = scene.add_placed(&name, result)?;
    let (lo, hi) = scene.get(id)?.bounds().ok_or("the result is empty")?;
    let mut out =
        json!({"id": id, "triangles": triangles, "watertight": watertight, "size": [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]});
    if !watertight {
        out["warning"] = json!("the result is not a closed mesh, so a slice may miss parts of it: try other sizes or fewer segments");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recipe_with_a_hole_is_rebuilt_when_the_hole_is_edited() {
        let mut s = Session::new();
        s.register(COMMANDS);
        let recipe = json!([
            {"command": "add_primitive", "params": {"kind": "box", "size": [20, 20, 10]}},
            {"command": "add_primitive", "params": {"kind": "cylinder", "radius": 2, "height": 14, "segments": 32}},
            {"command": "move", "params": {"id": 2, "by": [0, 0, -2]}},
            {"command": "difference", "params": {"id": 1, "minus": [2]}},
        ]);
        s.execute("build", &json!({"recipe": recipe})).unwrap();
        let small = s.scene.get(1).unwrap().world().volume();
        // "Make the hole 6 mm": the cylinder's step, radius 3.
        s.execute("edit", &json!({"id": 1, "step": 1, "params": {"radius": 3}})).unwrap();
        let big = s.scene.get(1).unwrap().world();
        assert!(big.is_watertight() && small - big.volume() > 140.0, "{small} {}", big.volume());
    }

    #[test]
    fn a_cut_laid_out_makes_two_closed_parts_whose_volumes_add_up_side_by_side_on_the_plate() {
        let mut s = Session::new();
        s.register(COMMANDS);
        s.execute("add_primitive", &json!({"kind": "cylinder", "radius": 10, "height": 20, "segments": 32})).unwrap();
        let whole = s.scene.get(1).unwrap().world().volume();
        let r = s.execute("cut", &json!({"id": 1, "z": 8, "connectors": {"kind": "none"}, "lay_out": true})).unwrap();
        let ids: Vec<u32> = r["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
        assert_eq!(ids.len(), 2);
        let (lower, upper) = (s.scene.get(ids[0]).unwrap(), s.scene.get(ids[1]).unwrap());
        assert!(lower.world().is_watertight() && upper.world().is_watertight());
        let (vl, vu) = (lower.world().volume(), upper.world().volume());
        assert!((vl + vu - whole).abs() < 1e-3 * whole && (vl / whole - 0.4).abs() < 0.01, "{vl} {vu} {whole}");
        let (llo, lhi) = lower.bounds().unwrap();
        let (ulo, uhi) = upper.bounds().unwrap();
        assert!((lhi[2] - 8.0).abs() < 1e-3 && (uhi[2] - 12.0).abs() < 1e-3, "{lhi:?} {ulo:?} {uhi:?}");
        assert!(llo[2].abs() < 1e-6 && ulo[2].abs() < 1e-6, "both on the plate: {llo:?} {ulo:?}");
        assert!(ulo[0] > lhi[0], "the upper part stands beside the lower one: {llo:?} {ulo:?}");
        s.execute("undo", &json!({})).unwrap();
        assert_eq!(s.scene.objects.len(), 1, "one undo puts the whole back");
        assert_eq!(s.execute("cut", &json!({"id": 1, "z": 5, "keep": "lower"})).unwrap()["ids"].as_array().unwrap().len(), 1);
        for bad in [json!({"id": 1, "z": 30}), json!({"id": 9, "z": 3}), json!({"id": 2, "z": 3, "keep": "middle"})] {
            assert!(s.execute("cut", &bad).is_err(), "{bad}");
        }
    }

    fn session() -> Session {
        let mut s = Session::new();
        s.register(COMMANDS);
        s
    }

    #[test]
    fn a_hole_is_one_command_and_one_undo_step() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [20, 20, 10], "name": "plate"})).unwrap();
        s.execute("add_primitive", &json!({"kind": "cylinder", "radius": 4, "height": 14})).unwrap();
        s.execute("move", &json!({"id": 2, "by": [0, 0, -2]})).unwrap();
        let r = s.execute("difference", &json!({"id": 1, "minus": [2]})).unwrap();
        assert_eq!(r["watertight"], true);
        assert_eq!(s.scene.objects.len(), 1);
        assert_eq!(s.scene.objects[0].name, "plate");
        let info = s.execute("info", &json!({})).unwrap();
        // A 48-sided hole of radius 4 has an area of 24 * 16 * sin(2 pi / 48), and goes through 10 mm of the plate.
        let hole = 24.0 * 16.0 * (std::f64::consts::TAU / 48.0).sin() * 10.0;
        assert!((info["objects"][0]["volume_mm3"].as_f64().unwrap() - (4000.0 - hole)).abs() < 1.0);
        s.undo().unwrap();
        assert_eq!(s.scene.objects.len(), 2, "one undo brings both operands back");
    }

    #[test]
    fn bad_requests_change_nothing() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [10, 10, 10]})).unwrap();
        s.execute("add_primitive", &json!({"kind": "box", "size": [5, 5, 5]})).unwrap();
        s.execute("move", &json!({"id": 2, "by": [50, 0, 0]})).unwrap();
        for (c, p) in [
            ("union", json!({"ids": [1]})),
            ("union", json!({"ids": [1, 1]})),
            ("union", json!({"ids": [1, 9]})),
            ("union", json!({"ids": [1, 2], "nme": "x"})),
            ("difference", json!({"id": 1, "minus": []})),
            ("difference", json!({"id": 1, "minus": [1]})),
            ("intersect", json!({"ids": [1, 2]})),
        ] {
            assert!(s.execute(c, &p).is_err(), "{c} {p}");
        }
        assert_eq!(s.scene.objects.len(), 2);
    }

    /// Runs a command that makes one object and gives its id.
    fn make(s: &mut Session, command: &str, p: Value) -> u32 {
        let r = s.execute(command, &p).unwrap_or_else(|e| panic!("{command} {p}: {e}"));
        r["id"].as_u64().unwrap() as u32
    }

    /// Moves an object so the middle of its box is at (`x`, `y`) and its bottom at `z`, as the build scripts place parts.
    fn place(s: &mut Session, id: u32, x: f64, y: f64, z: f64) {
        let (lo, hi) = s.scene.get(id).unwrap().bounds().unwrap();
        let by = [x - (lo[0] + hi[0]) / 2.0, y - (lo[1] + hi[1]) / 2.0, z - lo[2]];
        s.execute("move", &json!({"id": id, "by": by})).unwrap();
    }

    fn volume(s: &Session, id: u32) -> f64 {
        s.scene.get(id).unwrap().world().volume()
    }

    /// A boolean that must work: its result is closed and has the volume asked for, within `tol` mm3.
    fn must(s: &mut Session, command: &str, p: Value, want: f64, tol: f64) -> u32 {
        let r = s.execute(command, &p).unwrap_or_else(|e| panic!("{command} {p}: {e}"));
        let id = r["id"].as_u64().unwrap() as u32;
        let m = s.scene.get(id).unwrap().world();
        assert!(m.is_watertight(), "{command} {p}: not closed");
        assert!((m.volume() - want).abs() <= tol, "{command} {p}: {} mm3, not {want}", m.volume());
        id
    }

    type P2 = [f64; 2];

    /// The outlines of the Trendgarden mould's build script: Mjolnir, the rune thurisaz and its hole, a cross pattee, mm.
    fn hammer() -> Vec<P2> {
        let bezier = |p0: P2, p1: P2, p2: P2, n: usize| -> Vec<P2> {
            (0..=n)
                .map(|i| {
                    let t = i as f64 / n as f64;
                    let u = 1.0 - t;
                    [u * u * p0[0] + 2.0 * u * t * p1[0] + t * t * p2[0], u * u * p0[1] + 2.0 * u * t * p1[1] + t * t * p2[1]]
                })
                .collect()
        };
        let right = bezier([13.0, -16.0], [4.5, -9.0], [3.3, 4.8], 14);
        let mut out = right.clone();
        out.extend([[2.2, 6.0], [2.2, 11.6], [-2.2, 11.6], [-2.2, 6.0]]);
        out.extend(right.iter().rev().map(|p| [-p[0], p[1]]));
        out.extend((1..16).map(|i| {
            let a = std::f64::consts::PI * f64::from(i) / 16.0;
            [-13.0 * a.cos(), -16.0 - 1.6 * a.sin()]
        }));
        out
    }

    const THURS: [P2; 7] = [[-0.7, -6.0], [0.7, -6.0], [0.7, -2.4], [4.3, 0.4], [0.7, 3.2], [0.7, 6.0], [-0.7, 6.0]];
    const THURS_HOLE: [P2; 3] = [[0.72, -0.75], [2.55, 0.4], [0.72, 1.55]];
    const CROSS: [P2; 12] = [
        [4.5, 10.0],
        [-4.5, 10.0],
        [-2.2, 2.2],
        [-11.0, 4.5],
        [-11.0, -4.5],
        [-2.2, -2.2],
        [-4.5, -15.0],
        [4.5, -15.0],
        [2.2, -2.2],
        [11.0, -4.5],
        [11.0, 4.5],
        [2.2, 2.2],
    ];

    fn mirrored(p: &[P2]) -> Vec<P2> {
        p.iter().map(|q| [-q[0], q[1]]).collect()
    }

    fn shifted(p: &[P2], dx: f64, dy: f64) -> Vec<P2> {
        p.iter().map(|q| [q[0] + dx, q[1] + dy]).collect()
    }

    fn circle(x: f64, y: f64, r: f64, n: u32) -> Vec<P2> {
        lamina_core::sketch::circle(x, y, r, n)
    }

    /// The mould's base (88 x 62 x 11.25 mm, its middle at y = 35) with, when `pits`, the rune's and the boss's pits cut 0.7 mm into
    /// its top, then a pin unioned in each loop. Returns the base's id and its volume as it should be.
    fn mould_base(s: &mut Session, pits: bool) -> (u32, f64) {
        let base = make(s, "extrude", json!({"height": 11.25, "shape": "rounded_rect", "size": [88, 62], "corner": 4}));
        place(s, base, 0.0, 35.0, 0.0);
        let (mut id, mut want) = (base, volume(s, base));
        if pits {
            let pit = make(s, "extrude", json!({"height": 1.0, "outline": mirrored(&THURS), "holes": [mirrored(&THURS_HOLE)]}));
            place(s, pit, -20.0, 35.0 - 1.2 - 8.1, 10.55);
            want -= volume(s, pit) * 0.7;
            id = must(s, "difference", json!({"id": id, "minus": [pit]}), want, 0.5);
            let pit = make(s, "add_primitive", json!({"kind": "cylinder", "radius": 2.0, "height": 1.0, "segments": 24}));
            place(s, pit, 20.0, 34.0, 10.55);
            want -= volume(s, pit) * 0.7;
            id = must(s, "difference", json!({"id": id, "minus": [pit]}), want, 0.5);
        }
        for (x, y, r) in [(-20.0, 13.8, 2.5), (20.0, 11.8, 2.2)] {
            let pin = make(s, "add_primitive", json!({"kind": "cylinder", "radius": r, "height": 3.0, "segments": 24}));
            place(s, pin, x, 35.0 + y, 10.95);
            want += volume(s, pin) * 2.7 / 3.0;
            id = must(s, "union", json!({"ids": [id, pin]}), want, 0.5);
        }
        (id, want)
    }

    /// The mould's top: an 88 x 62 x 2.8 mm slab whose six holes are the two pendants' cavities, their loops and the pour channels,
    /// overlapping the base by 0.05 mm. Returns its id and the volume it adds to the base.
    fn mould_slab(s: &mut Session) -> (u32, f64) {
        let (hx, hy, cx, cy) = (-20.0, -1.2, 20.0, -1.0);
        let rect = |x0: f64, y0: f64, x1: f64, y1: f64| vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        let holes = vec![
            shifted(&hammer(), hx, hy),
            circle(hx, hy + 15.0, 5.0, 32),
            rect(hx - 1.5, hy + 18.0, hx + 1.5, 33.0),
            shifted(&CROSS, cx, cy),
            circle(cx, cy + 12.8, 4.2, 32),
            rect(cx - 1.5, cy + 14.8, cx + 1.5, 33.0),
        ];
        let slab = make(s, "extrude", json!({"height": 2.8, "shape": "rounded_rect", "size": [88, 62], "corner": 4, "holes": holes}));
        place(s, slab, 0.0, 35.0, 11.2);
        (slab, volume(s, slab) * 2.75 / 2.8)
    }

    /// Case 1 of the chained booleans that failed: the slab unioned onto a base whose top has had two pits cut into it.
    #[test]
    fn a_slab_joins_a_base_that_has_pits_cut_in_it() {
        let mut s = session();
        let (base, want) = mould_base(&mut s, true);
        let (slab, adds) = mould_slab(&mut s);
        must(&mut s, "union", json!({"ids": [base, slab]}), want + adds, 1.0);
    }

    /// Case 2: lettering engraved into the finished mould, itself the result of three unions.
    #[test]
    fn text_engraves_a_part_made_by_unions() {
        let mut s = session();
        let (base, want) = mould_base(&mut s, false);
        let (slab, adds) = mould_slab(&mut s);
        let mould = must(&mut s, "union", json!({"ids": [base, slab]}), want + adds, 1.0);
        let want = volume(&s, mould);
        let word = make(&mut s, "text", json!({"text": "TRENDGÅRDEN", "height": 5, "depth": 1.2}));
        place(&mut s, word, 0.0, 10.0, 14.0 - 0.7);
        let gone = volume(&s, word) * 0.7 / 1.2;
        must(&mut s, "difference", json!({"id": mould, "minus": [word]}), want - gone, 0.5);
    }

    /// Case 3: a pendant made by two unions used as the tool that presses its shape into a plain box.
    #[test]
    fn a_part_made_by_unions_cuts_a_box() {
        let mut s = session();
        let body = make(&mut s, "extrude", json!({"height": 3, "outline": hammer()}));
        place(&mut s, body, 0.0, 0.0, 0.0);
        let body_area = volume(&s, body) / 3.0;
        let (lo, hi) = s.scene.get(body).unwrap().bounds().unwrap();
        // The sketch's origin, where the build script put the loop and the rune.
        let (ox, oy) = (0.0, (lo[1] + hi[1]) / 2.0 - (11.6 + -17.6) / 2.0);
        let ring = make(&mut s, "add_primitive", json!({"kind": "tube", "radius": 5, "inner": 2.5, "height": 2.96, "segments": 24}));
        place(&mut s, ring, ox, oy + 15.0, 0.02);
        let pendant = s.execute("union", &json!({"ids": [body, ring]})).unwrap()["id"].as_u64().unwrap() as u32;
        let rune = make(&mut s, "extrude", json!({"height": 2.7, "outline": THURS, "holes": [THURS_HOLE]}));
        place(&mut s, rune, ox, oy - 8.1, 1.0);
        let pendant = s.execute("union", &json!({"ids": [pendant, rune]})).unwrap()["id"].as_u64().unwrap() as u32;
        // Seen from above, the pendant is the hammer and the loop: the loop's ring with the hammer's neck in it.
        let ring_outline = [circle(ox, oy + 15.0, 5.0, 24), circle(ox, oy + 15.0, 2.5, 24).into_iter().rev().collect()];
        let mut seen = vec![shifted(&hammer(), 0.0, oy)];
        seen.extend(ring_outline);
        let footprint = lamina_core::sketch::extrude_contours(&seen, 1.0).unwrap().volume();
        let block = make(&mut s, "add_primitive", json!({"kind": "box", "size": [60, 60, 10]}));
        place(&mut s, block, 0.0, oy - 3.0, -8.0);
        // The box's top is 2 mm above the pendant's bottom: all of the hammer up to there, and the loop from 0.02 mm.
        let want = volume(&s, block) - body_area * 2.0 - (footprint - body_area) * 1.98;
        must(&mut s, "difference", json!({"id": block, "minus": [pendant]}), want, 0.5);
    }

    /// Case 4: a block cut again and again, each cut taking the last one's result.
    #[test]
    fn a_block_takes_many_cuts_in_a_row() {
        let mut s = session();
        let mut block = make(&mut s, "extrude", json!({"height": 12, "shape": "rounded_rect", "size": [60, 40], "corner": 4}));
        place(&mut s, block, 0.0, 0.0, 0.0);
        let mut want = volume(&s, block);
        let tools: Vec<(&str, Value, [f64; 3], f64)> = vec![
            ("add_primitive", json!({"kind": "cylinder", "radius": 3, "height": 16}), [-22.0, -12.0, -2.0], 12.0 / 16.0),
            ("add_primitive", json!({"kind": "cylinder", "radius": 3, "height": 16}), [22.0, -12.0, -2.0], 12.0 / 16.0),
            ("extrude", json!({"height": 5, "shape": "rounded_rect", "size": [24, 10], "corner": 2}), [0.0, 10.0, 9.0], 3.0 / 5.0),
            ("text", json!({"text": "LAMINA", "height": 6, "depth": 1}), [0.0, -10.0, 11.4], 0.6),
            ("add_primitive", json!({"kind": "box", "size": [6, 20, 4]}), [-24.0, 6.0, 10.0], 0.5),
            ("add_primitive", json!({"kind": "cylinder", "radius": 2, "height": 16, "segments": 32}), [24.0, 12.0, -2.0], 0.75),
            ("extrude", json!({"height": 4, "shape": "slot", "size": [14, 4]}), [5.0, -17.0, 10.0], 0.5),
        ];
        for (command, p, [x, y, z], share) in tools {
            let tool = make(&mut s, command, p);
            place(&mut s, tool, x, y, z);
            want -= volume(&s, tool) * share;
            block = must(&mut s, "difference", json!({"id": block, "minus": [tool]}), want, 0.5);
        }
    }

    #[test]
    fn a_union_slices() {
        let mut s = session();
        s.register(lamina_slicer::COMMANDS);
        s.execute("add_primitive", &json!({"kind": "box", "size": [20, 20, 10]})).unwrap();
        s.execute("add_primitive", &json!({"kind": "cylinder", "radius": 6, "height": 20})).unwrap();
        s.execute("union", &json!({"ids": [1, 2]})).unwrap();
        let r = s.execute("slice_check", &json!({})).unwrap();
        assert_eq!(r["layers"], 100);
        assert_eq!(r["warnings"], json!([]));
    }
}
