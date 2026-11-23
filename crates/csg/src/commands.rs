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
