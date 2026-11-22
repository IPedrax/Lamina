//! The commands: what the CLI, scripts, the MCP server and the AI chat run. Parameters are strict (a misspelled key is an error)
//! and every number is checked, because an agent or a file chose them.

use std::path::Path;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::engine::{Command, Files, Session};
use crate::mesh::{Mesh, P3, Transform};
use crate::scene::Object;
use crate::stl;

const fn cmd(id: &'static str, params: &'static str, run: crate::engine::Run, undoable: bool, files: Files) -> Command {
    Command { id, params, run, undoable, files }
}

pub static ALL: &[Command] = &[
    cmd(
        "import_mesh",
        "{path, name?} an STL file (binary or ASCII) put on the plate, centred, its lowest point on the plate. Returns {id, triangles, watertight, size}",
        import_mesh,
        true,
        Files::Read,
    ),
    cmd(
        "add_primitive",
        "{kind:\"box\"|\"cylinder\"|\"sphere\", name?, size?:[x,y,z] (box), radius? (cylinder, sphere), height? (cylinder), segments? (3..256, default 48)} a shape on the plate, mm. Returns {id}",
        add_primitive,
        true,
        Files::None,
    ),
    cmd("move", "{id, by:[x,y,z]} moves an object by mm", move_by, true, Files::None),
    cmd(
        "rotate",
        "{id, axis:\"x\"|\"y\"|\"z\", degrees} turns an object about its own centre, then sets it back on the plate",
        rotate,
        true,
        Files::None,
    ),
    cmd(
        "scale",
        "{id, factor? | by?:[x,y,z]} scales an object about its centre by a positive factor (0.001..1000), then sets it back on the plate",
        scale,
        true,
        Files::None,
    ),
    cmd("drop_to_bed", "{id} lowers or raises an object so its lowest point touches the plate", drop_to_bed, true, Files::None),
    cmd("delete", "{id} removes an object", delete, true, Files::None),
    cmd(
        "info",
        "{} every object: id, name, triangles, size, min, max, volume_mm3, watertight (a slicer can only fill a closed mesh)",
        info,
        false,
        Files::None,
    ),
    cmd(
        "export_mesh",
        "{path, id?} writes a binary STL of one object, or of the whole plate, where it stands",
        export_mesh,
        false,
        Files::Write,
    ),
    cmd("undo", "{} takes back the last change", undo, false, Files::None),
    cmd("redo", "{} puts back what undo took back", redo, false, Files::None),
];

fn parse<T: DeserializeOwned>(params: &Value) -> Result<T, String> {
    let none = json!({});
    T::deserialize(if params.is_null() { &none } else { params }).map_err(|e| format!("bad parameters: {e}"))
}

/// A finite number in `(0, max]`.
fn positive(what: &str, x: f64, max: f64) -> Result<f64, String> {
    if x.is_finite() && x > 0.0 && x <= max { Ok(x) } else { Err(format!("`{what}` must be a number above 0 and at most {max}, not {x}")) }
}

fn finite3(what: &str, v: P3) -> Result<P3, String> {
    if v.iter().all(|c| c.is_finite()) { Ok(v) } else { Err(format!("`{what}` must be three finite numbers")) }
}

fn size_of(o: &Object) -> Result<(P3, P3), String> {
    o.bounds().ok_or_else(|| format!("object {} is empty", o.id))
}

/// Turns the object about the middle of its bounds by `op`, then rests it on the plate.
fn about_centre(o: &mut Object, op: &Transform) -> Result<(), String> {
    let (lo, hi) = size_of(o)?;
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    o.xf = o.xf.then(&Transform::translate(c.map(|x| -x))).then(op).then(&Transform::translate(c));
    settle(o)
}

fn settle(o: &mut Object) -> Result<(), String> {
    let (lo, _) = size_of(o)?;
    o.xf = o.xf.then(&Transform::translate([0.0, 0.0, -lo[2]]));
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    path: String,
    name: Option<String>,
}

fn import_mesh(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Import = parse(p)?;
    let path = Path::new(&a.path);
    if !path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("stl")) {
        return Err("only .stl files are read so far".into());
    }
    let len = std::fs::metadata(path).map_err(|e| format!("{}: {e}", a.path))?.len();
    if len > stl::MAX_FILE {
        return Err(format!("{} is {len} bytes, more than the {} a file may hold", a.path, stl::MAX_FILE));
    }
    let mesh = stl::read(&std::fs::read(path).map_err(|e| format!("{}: {e}", a.path))?)?;
    let name = a.name.unwrap_or_else(|| path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "mesh".into()));
    added(s, &name, mesh)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Primitive {
    kind: String,
    name: Option<String>,
    size: Option<P3>,
    radius: Option<f64>,
    height: Option<f64>,
    segments: Option<u32>,
}

fn add_primitive(s: &mut Session, p: &Value) -> Result<Value, String> {
    const MAX: f64 = 10_000.0;
    let a: Primitive = parse(p)?;
    let segments = a.segments.unwrap_or(48);
    if !(3..=256).contains(&segments) {
        return Err("`segments` is from 3 to 256".into());
    }
    let need = |v: Option<f64>, what: &str| v.ok_or_else(|| format!("a {} needs `{what}`", a.kind)).and_then(|x| positive(what, x, MAX));
    let mesh = match a.kind.as_str() {
        "box" => {
            let size = a.size.ok_or("a box needs `size`: [x, y, z]")?;
            Mesh::cuboid(size.map(|c| positive("size", c, MAX)).into_iter().collect::<Result<Vec<_>, _>>().map(|v| [v[0], v[1], v[2]])?)
        }
        "cylinder" => Mesh::cylinder(need(a.radius, "radius")?, need(a.height, "height")?, segments),
        "sphere" => Mesh::sphere(need(a.radius, "radius")?, segments),
        other => return Err(format!("unknown kind `{other}`: box, cylinder or sphere")),
    };
    added(s, a.name.as_deref().unwrap_or(&a.kind), mesh)
}

fn added(s: &mut Session, name: &str, mesh: Mesh) -> Result<Value, String> {
    let (triangles, watertight) = (mesh.triangles.len(), mesh.is_watertight());
    let id = s.scene_mut().add(name, mesh)?;
    let (lo, hi) = size_of(s.scene.get(id)?)?;
    Ok(json!({"id": id, "triangles": triangles, "watertight": watertight, "size": [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Move {
    id: u32,
    by: P3,
}

fn move_by(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Move = parse(p)?;
    let by = finite3("by", a.by)?;
    let o = s.scene_mut().get_mut(a.id)?;
    o.xf = o.xf.then(&Transform::translate(by));
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rotate {
    id: u32,
    axis: String,
    degrees: f64,
}

fn rotate(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Rotate = parse(p)?;
    if !a.degrees.is_finite() {
        return Err("`degrees` must be a finite number".into());
    }
    let axis = match a.axis.as_str() {
        "x" => 0,
        "y" => 1,
        "z" => 2,
        other => return Err(format!("unknown axis `{other}`: x, y or z")),
    };
    about_centre(s.scene_mut().get_mut(a.id)?, &Transform::rotate(axis, a.degrees))?;
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scale {
    id: u32,
    factor: Option<f64>,
    by: Option<P3>,
}

fn scale(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Scale = parse(p)?;
    let by = match (a.factor, a.by) {
        (Some(f), None) => [f; 3],
        (None, Some(b)) => b,
        _ => return Err("give `factor` or `by`, one of them".into()),
    };
    let mut checked = [0.0; 3];
    for (slot, c) in checked.iter_mut().zip(by) {
        *slot =
            positive("factor", c, 1000.0).and_then(|c| if c >= 0.001 { Ok(c) } else { Err("`factor` is at least 0.001".to_string()) })?;
    }
    about_centre(s.scene_mut().get_mut(a.id)?, &Transform::scale(checked))?;
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    id: u32,
}

fn drop_to_bed(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Id = parse(p)?;
    settle(s.scene_mut().get_mut(a.id)?)?;
    Ok(json!({}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Id = parse(p)?;
    s.scene.get(a.id)?;
    s.scene_mut().objects.retain(|o| o.id != a.id);
    Ok(json!({}))
}

fn info(s: &mut Session, p: &Value) -> Result<Value, String> {
    let _: Value = parse(p)?;
    let objects: Vec<Value> = s
        .scene
        .objects
        .iter()
        .map(|o| {
            let world = o.world();
            let (lo, hi) = world.bounds().unwrap_or_default();
            json!({
                "id": o.id, "name": o.name, "triangles": world.triangles.len(),
                "size": [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]], "min": lo, "max": hi,
                "volume_mm3": world.volume(), "watertight": world.is_watertight(),
            })
        })
        .collect();
    Ok(json!({"objects": objects}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Export {
    path: String,
    id: Option<u32>,
}

fn export_mesh(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Export = parse(p)?;
    let worlds: Vec<Mesh> = match a.id {
        Some(id) => vec![s.scene.get(id)?.world()],
        None => s.scene.objects.iter().map(Object::world).collect(),
    };
    let merged = Mesh::merged(&worlds);
    if merged.triangles.is_empty() {
        return Err("nothing to export: the plate is empty".into());
    }
    std::fs::write(&a.path, stl::write(&merged)).map_err(|e| format!("{}: {e}", a.path))?;
    Ok(json!({"path": a.path, "triangles": merged.triangles.len()}))
}

fn undo(s: &mut Session, _: &Value) -> Result<Value, String> {
    s.undo().map(|what| json!({"undid": what}))
}

fn redo(s: &mut Session, _: &Value) -> Result<Value, String> {
    s.redo().map(|what| json!({"redid": what}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Session, id: &str, p: Value) -> Result<Value, String> {
        s.execute(id, &p)
    }

    #[test]
    fn a_box_sits_centred_on_the_plate() {
        let mut s = Session::new();
        let v = run(&mut s, "add_primitive", json!({"kind": "box", "size": [20, 10, 5]})).unwrap();
        assert_eq!(v["watertight"], true);
        let o = s.scene.get(1).unwrap();
        assert_eq!(o.bounds(), Some(([-10.0, -5.0, 0.0], [10.0, 5.0, 5.0])));
    }

    #[test]
    fn rotating_a_box_onto_its_side_rests_it_on_the_plate() {
        let mut s = Session::new();
        run(&mut s, "add_primitive", json!({"kind": "box", "size": [20, 10, 5]})).unwrap();
        run(&mut s, "rotate", json!({"id": 1, "axis": "y", "degrees": 90})).unwrap();
        let (lo, hi) = s.scene.get(1).unwrap().bounds().unwrap();
        assert!(lo[2].abs() < 1e-4 && (hi[2] - 20.0).abs() < 1e-4, "{lo:?} {hi:?}");
    }

    #[test]
    fn undo_redo_and_a_failed_command_leave_the_scene_whole() {
        let mut s = Session::new();
        run(&mut s, "add_primitive", json!({"kind": "sphere", "radius": 5})).unwrap();
        assert!(run(&mut s, "move", json!({"id": 1, "by": [1e9, 0, 0]})).is_err(), "beyond reach is refused");
        assert_eq!(s.scene.objects.len(), 1);
        run(&mut s, "delete", json!({"id": 1})).unwrap();
        assert!(s.scene.objects.is_empty());
        run(&mut s, "undo", json!({})).unwrap();
        assert_eq!(s.scene.objects.len(), 1);
        run(&mut s, "redo", json!({})).unwrap();
        assert!(s.scene.objects.is_empty());
    }

    #[test]
    fn stl_goes_out_and_comes_back_in() {
        let path = std::env::temp_dir().join(format!("lamina-test-{}.stl", std::process::id()));
        let path = path.to_string_lossy().into_owned();
        let mut s = Session::new();
        run(&mut s, "add_primitive", json!({"kind": "cylinder", "radius": 4, "height": 9})).unwrap();
        run(&mut s, "export_mesh", json!({"path": path})).unwrap();
        run(&mut s, "import_mesh", json!({"path": path, "name": "again"})).unwrap();
        std::fs::remove_file(&path).unwrap();
        let info = run(&mut s, "info", json!({})).unwrap();
        assert_eq!(info["objects"][1]["watertight"], true);
        assert_eq!(info["objects"][1]["name"], "again");
    }

    #[test]
    fn hostile_parameters_are_errors_never_panics() {
        let mut s = Session::new();
        run(&mut s, "add_primitive", json!({"kind": "box", "size": [1, 1, 1]})).unwrap();
        let bad = [
            ("add_primitive", json!({"kind": "box", "size": [0, 1, 1]})),
            ("add_primitive", json!({"kind": "box", "sizee": [1, 1, 1]})),
            ("add_primitive", json!({"kind": "sphere", "radius": 1e300})),
            ("add_primitive", json!({"kind": "cylinder", "radius": 1, "height": 1, "segments": 4_000_000_000u64})),
            ("scale", json!({"id": 1, "factor": -1})),
            ("scale", json!({"id": 1, "factor": 0})),
            ("scale", json!({"id": 1})),
            ("rotate", json!({"id": 1, "axis": "w", "degrees": 1})),
            ("move", json!({"id": 99, "by": [0, 0, 0]})),
            ("import_mesh", json!({"path": "/nonexistent.stl"})),
            ("import_mesh", json!({"path": "/etc/passwd"})),
            ("nope", json!({})),
        ];
        for (id, p) in bad {
            assert!(s.execute(id, &p).is_err(), "{id} {p}");
        }
        assert_eq!(s.scene.objects.len(), 1);
    }

    #[test]
    fn a_script_stops_at_the_line_that_fails() {
        let mut s = Session::new();
        let err = s.script("# a plate\nadd_primitive {\"kind\":\"box\",\"size\":[5,5,5]}\nmove {\"id\":9,\"by\":[0,0,0]}\n").unwrap_err();
        assert!(err.starts_with("line 3:"), "{err}");
    }
}
