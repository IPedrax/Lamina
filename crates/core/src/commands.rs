//! The commands: what the CLI, scripts, the MCP server and the AI chat run. Parameters are strict (a misspelled key is an error)
//! and every number is checked, because an agent or a file chose them.

use std::path::Path;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::engine::{Command, Files, Session};
use crate::mesh::{Mesh, P3, Transform};
use crate::scene::Object;
use crate::{project, repair, stl, threemf};

const fn cmd(id: &'static str, params: &'static str, run: crate::engine::Run, undoable: bool, files: Files) -> Command {
    Command { id, params, run, undoable, files }
}

pub static ALL: &[Command] = &[
    cmd(
        "import_mesh",
        "{path, name?, at?:[x,y]} a mesh file put on the plate, centred on `at` (default the middle of the plate), its lowest point on the plate: an STL (binary or ASCII) as one object, or a 3MF with each of its build items as an object, kept where they stand to each other. Meshes are repaired on the way in (doubled faces dropped, flipped faces turned, small holes filled). Returns {id (the first), ids, triangles, watertight, size, repaired?: {dropped, flipped, holes_filled}}",
        import_mesh,
        true,
        Files::Read,
    ),
    cmd(
        "add_primitive",
        "{kind:\"box\"|\"cylinder\"|\"sphere\", name?, at?:[x,y] (where its centre stands, default the middle of the plate), size?:[x,y,z] (box), radius? (cylinder, sphere), height? (cylinder), segments? (3..256, default 48; a sphere is geodesic and gets 320, 1280, 5120 or 20480 triangles from it)} a shape on the plate, mm. Returns {id}",
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
    cmd(
        "repair",
        "{id} mends an object's mesh: doubled faces and skins dropped, faces wound like their neighbours and each shell outward, cracks closed and small holes filled. Returns {dropped, flipped, holes_filled, watertight}; a mesh that is still open has a hole too big to fill without guessing",
        repair,
        true,
        Files::None,
    ),
    cmd("drop_to_bed", "{id} lowers or raises an object so its lowest point touches the plate", drop_to_bed, true, Files::None),
    cmd("delete", "{id} removes an object", delete, true, Files::None),
    cmd(
        "info",
        "{} every object: id, name, triangles, size, min, max, volume_mm3, watertight (a slicer can only fill a closed mesh), and its problems: open_edges (holes), nonmanifold_edges (edges of three or more faces), flipped_faces; `repair` mends what it can",
        info,
        false,
        Files::None,
    ),
    cmd(
        "export_mesh",
        "{path, id?} writes one object, or the whole plate, where it stands: a binary STL (one mesh), or a 3MF (`.3mf`, each object its own, with positions for a printer whose zero is the plate's front left corner)",
        export_mesh,
        false,
        Files::Write,
    ),
    cmd(
        "save",
        "{path} writes the project (every object, where it stands, and the print settings) to a .lamina file, which `open` reads back",
        save,
        false,
        Files::Write,
    ),
    cmd(
        "open",
        "{path} replaces the plate and the print settings with a .lamina project's, and starts a new undo history. Returns {objects, ignored}: `ignored` lists what this version left out",
        open,
        false,
        Files::Read,
    ),
    cmd("new", "{} empties the plate and starts a new undo history; the print settings stay", new, false, Files::None),
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
    at: Option<[f64; 2]>,
}

fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default()
}

fn import_mesh(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Import = parse(p)?;
    let path = Path::new(&a.path);
    let kind = extension(path);
    if !matches!(kind.as_str(), "stl" | "3mf") {
        return Err("a mesh is read from an .stl or a .3mf file".into());
    }
    let len = std::fs::metadata(path).map_err(|e| format!("{}: {e}", a.path))?.len();
    if len > stl::MAX_FILE {
        return Err(format!("{} is {len} bytes, more than the {} a file may hold", a.path, stl::MAX_FILE));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", a.path))?;
    let stem = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "mesh".into());
    if kind == "3mf" {
        return import_parts(s, threemf::read(&bytes).map_err(|e| format!("{}: {e}", a.path))?, a.name.as_deref(), a.at);
    }
    let mesh = stl::read(&bytes)?;
    let name = a.name.unwrap_or(stem);
    let (mesh, report) = repair::repair(&mesh)?;
    let mut v = added(s, &name, mesh, a.at)?;
    v["ids"] = json!([v["id"]]);
    if report.changed() {
        v["repaired"] = json!({"dropped": report.dropped, "flipped": report.flipped, "holes_filled": report.holes_filled});
    }
    Ok(v)
}

/// The parts of a 3MF file on the plate: each repaired and placed with its transform, the group kept as it was laid out, centred on
/// `at` and resting on the plate.
fn import_parts(s: &mut Session, parts: Vec<threemf::Part>, name: Option<&str>, at: Option<[f64; 2]>) -> Result<Value, String> {
    let at = finite3("at", at.map_or([0.0; 3], |[x, y]| [x, y, 0.0]))?;
    let mut placed = Vec::with_capacity(parts.len());
    let mut total = repair::Report::default();
    for (k, part) in parts.into_iter().enumerate() {
        let (mesh, r) = repair::repair(&part.mesh)?;
        (total.dropped, total.flipped, total.holes_filled) =
            (total.dropped + r.dropped, total.flipped + r.flipped, total.holes_filled + r.holes_filled);
        let det = part.xf.det();
        if det.is_nan() || det.abs() <= 1e-12 {
            return Err(format!("build item {} flattens its object to nothing", k + 1));
        }
        // A mirroring transform is baked into the mesh, faces turned to face out, so the object's own transform never mirrors.
        let (mesh, xf) = if det < 0.0 {
            let mut m = mesh.transformed(&part.xf);
            m.triangles.iter_mut().for_each(|t| t.swap(1, 2));
            (m, Transform::IDENTITY)
        } else {
            (mesh, part.xf)
        };
        let label = match name {
            Some(n) if k == 0 => n.to_string(),
            Some(n) => format!("{n} {}", k + 1),
            None => part.name,
        };
        placed.push((label, mesh, xf));
    }
    let bounds = placed.iter().filter_map(|(_, m, xf)| m.transformed(xf).bounds()).reduce(|(alo, ahi), (blo, bhi)| {
        ([alo[0].min(blo[0]), alo[1].min(blo[1]), alo[2].min(blo[2])], [ahi[0].max(bhi[0]), ahi[1].max(bhi[1]), ahi[2].max(bhi[2])])
    });
    let (lo, hi) = bounds.ok_or("the 3MF file holds no solid to print")?;
    let shift = Transform::translate([at[0] - (lo[0] + hi[0]) / 2.0, at[1] - (lo[1] + hi[1]) / 2.0, -lo[2]]);
    let (mut ids, mut triangles, mut watertight) = (Vec::new(), 0, true);
    for (label, mesh, xf) in placed {
        triangles += mesh.triangles.len();
        watertight &= mesh.is_watertight();
        ids.push(s.scene_mut().add_transformed(&label, mesh, xf.then(&shift))?);
    }
    let mut v = json!({
        "id": ids[0], "ids": ids, "triangles": triangles, "watertight": watertight,
        "size": [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
    });
    if total.changed() {
        v["repaired"] = json!({"dropped": total.dropped, "flipped": total.flipped, "holes_filled": total.holes_filled});
    }
    Ok(v)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Primitive {
    kind: String,
    name: Option<String>,
    at: Option<[f64; 2]>,
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
    added(s, a.name.as_deref().unwrap_or(&a.kind), mesh, a.at)
}

fn added(s: &mut Session, name: &str, mesh: Mesh, at: Option<[f64; 2]>) -> Result<Value, String> {
    let (triangles, watertight) = (mesh.triangles.len(), mesh.is_watertight());
    let at = finite3("at", at.map_or([0.0; 3], |[x, y]| [x, y, 0.0]))?;
    let id = s.scene_mut().add(name, mesh)?;
    let o = s.scene_mut().get_mut(id)?;
    o.xf = o.xf.then(&Transform::translate(at));
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

fn repair(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Id = parse(p)?;
    let (mesh, r) = repair::repair(&s.scene.get(a.id)?.mesh)?;
    if mesh.triangles.is_empty() {
        return Err(format!("object {} would have nothing left: it is not a solid that can be mended", a.id));
    }
    // Only a change makes an undo step.
    if r.changed() {
        s.scene_mut().get_mut(a.id)?.mesh = std::sync::Arc::new(mesh);
    }
    Ok(json!({"dropped": r.dropped, "flipped": r.flipped, "holes_filled": r.holes_filled, "watertight": r.closed}))
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
            let problems = repair::problems(&o.mesh);
            json!({
                "id": o.id, "name": o.name, "triangles": world.triangles.len(),
                "size": [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]], "min": lo, "max": hi,
                "volume_mm3": world.volume(), "watertight": world.is_watertight(),
                "open_edges": problems.open_edges, "nonmanifold_edges": problems.nonmanifold_edges, "flipped_faces": problems.flipped_faces,
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
    if extension(Path::new(&a.path)) == "3mf" {
        let objects: Vec<&Object> = match a.id {
            Some(id) => vec![s.scene.get(id)?],
            None => s.scene.objects.iter().collect(),
        };
        if objects.is_empty() {
            return Err("nothing to export: the plate is empty".into());
        }
        // The plate from the project's print settings, else the default 220 mm one: the scene's origin is its middle.
        let bed =
            s.print["profile"]["bed"].as_array().and_then(|b| Some([b.first()?.as_f64()?, b.get(1)?.as_f64()?])).unwrap_or([220.0, 220.0]);
        let corner = Transform::translate([bed[0] / 2.0, bed[1] / 2.0, 0.0]);
        let parts: Vec<threemf::Part> =
            objects.iter().map(|o| threemf::Part { name: o.name.clone(), mesh: (*o.mesh).clone(), xf: o.xf.then(&corner) }).collect();
        let triangles: usize = parts.iter().map(|p| p.mesh.triangles.len()).sum();
        std::fs::write(&a.path, threemf::write(&parts)).map_err(|e| format!("{}: {e}", a.path))?;
        return Ok(json!({"path": a.path, "objects": parts.len(), "triangles": triangles}));
    }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathOnly {
    path: String,
}

fn save(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: PathOnly = parse(p)?;
    let text = project::write(&s.scene, &s.print);
    // To a file beside it first, then over the old one, so a save cut short never leaves half a project.
    let path = Path::new(&a.path);
    let tmp = path.with_extension("lamina.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", a.path))?;
    Ok(json!({"path": a.path, "objects": s.scene.objects.len()}))
}

fn open(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: PathOnly = parse(p)?;
    let len = std::fs::metadata(&a.path).map_err(|e| format!("{}: {e}", a.path))?.len();
    if len > project::MAX_FILE {
        return Err(format!("{} is {len} bytes, more than the {} a project may hold", a.path, project::MAX_FILE));
    }
    let text = std::fs::read_to_string(&a.path).map_err(|e| format!("{}: {e}", a.path))?;
    let loaded = project::read(&text).map_err(|e| format!("{}: {e}", a.path))?;
    let objects = loaded.scene.objects.len();
    s.replace(loaded.scene, loaded.print);
    Ok(json!({"objects": objects, "ignored": loaded.ignored}))
}

fn new(s: &mut Session, p: &Value) -> Result<Value, String> {
    let _: Empty = parse(p)?;
    let print = std::mem::take(&mut s.print);
    s.replace(crate::scene::Scene::default(), print);
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

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
    fn a_shape_can_be_put_where_it_should_stand() {
        let mut s = Session::new();
        run(&mut s, "add_primitive", json!({"kind": "cylinder", "radius": 5, "height": 8, "at": [30, -10]})).unwrap();
        assert_eq!(
            s.scene.get(1).unwrap().bounds().map(|(lo, hi)| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]]),
            Some([30.0, -10.0, 0.0])
        );
        assert!(run(&mut s, "add_primitive", json!({"kind": "box", "size": [1, 1, 1], "at": [1e300, 0]})).is_err());
        assert_eq!(s.scene.objects.len(), 1);
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
    fn a_project_is_saved_and_opened_with_its_settings_and_a_fresh_history() {
        let dir = std::env::temp_dir().join(format!("lamina-project-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plate.lamina").to_string_lossy().into_owned();
        let mut s = Session::new();
        run(&mut s, "add_primitive", json!({"kind": "box", "size": [20, 10, 5]})).unwrap();
        run(&mut s, "add_primitive", json!({"kind": "cylinder", "radius": 3, "height": 9, "at": [20, 20]})).unwrap();
        s.print = json!({"profile": {"infill_density": 0.3}, "filament": "PETG"});
        run(&mut s, "save", json!({"path": path})).unwrap();
        let before = s.scene.clone();
        run(&mut s, "new", json!({})).unwrap();
        assert!(s.scene.objects.is_empty() && !s.can_undo(), "new empties the plate and the history");
        assert_eq!(s.print["filament"], "PETG", "new keeps the print settings");
        s.print = serde_json::Value::Null;
        let r = run(&mut s, "open", json!({"path": path})).unwrap();
        assert_eq!(r["objects"], 2);
        assert_eq!(s.print["profile"]["infill_density"], 0.3);
        assert!(!s.can_undo());
        for (a, b) in before.objects.iter().zip(&s.scene.objects) {
            assert_eq!((a.id, &a.name, a.xf, &*a.mesh), (b.id, &b.name, b.xf, &*b.mesh));
        }
        // A file that is not a project changes nothing.
        std::fs::write(dir.join("bad.lamina"), "{\"format\": 99}").unwrap();
        let err = run(&mut s, "open", json!({"path": dir.join("bad.lamina")})).unwrap_err();
        assert!(err.contains("newer Lamina"), "{err}");
        assert_eq!(s.scene.objects.len(), 2);
        assert!(run(&mut s, "open", json!({"path": "/nonexistent.lamina"})).is_err());
        assert!(run(&mut s, "new", json!({"keep": true})).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_imperfect_stl_is_repaired_on_import_and_info_reports_problems() {
        let path = std::env::temp_dir().join(format!("lamina-broken-{}.stl", std::process::id()));
        let mut broken = Mesh::cuboid([10.0, 10.0, 10.0]);
        broken.triangles[0].swap(1, 2); // one face flipped
        broken.triangles.truncate(10); // and one side missing
        std::fs::write(&path, stl::write(&broken)).unwrap();
        let mut s = Session::new();
        let v = run(&mut s, "import_mesh", json!({"path": path})).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(v["watertight"], true);
        assert_eq!((v["repaired"]["flipped"].as_u64(), v["repaired"]["holes_filled"].as_u64()), (Some(1), Some(1)));
        let info = run(&mut s, "info", json!({})).unwrap();
        assert_eq!((info["objects"][0]["open_edges"].as_u64(), info["objects"][0]["flipped_faces"].as_u64()), (Some(0), Some(0)));
        assert!((info["objects"][0]["volume_mm3"].as_f64().unwrap() - 1000.0).abs() < 1e-3);
    }

    #[test]
    fn repair_mends_an_object_in_one_undo_step_and_says_what_it_did() {
        let mut s = Session::new();
        let mut open = Mesh::cuboid([10.0, 10.0, 10.0]);
        open.triangles.pop();
        open.triangles[2].swap(1, 2);
        s.scene_mut().add("open", open).unwrap();
        let info = run(&mut s, "info", json!({})).unwrap();
        assert_eq!(info["objects"][0]["watertight"], false);
        assert!(info["objects"][0]["open_edges"].as_u64().unwrap() > 0 && info["objects"][0]["flipped_faces"] == 1);
        let r = run(&mut s, "repair", json!({"id": 1})).unwrap();
        assert_eq!((r["watertight"].as_bool(), r["flipped"].as_u64(), r["holes_filled"].as_u64()), (Some(true), Some(1), Some(1)));
        assert!(s.scene.get(1).unwrap().mesh.is_watertight());
        // A sound mesh is left alone and makes no undo step; undo takes the repair back.
        let r = run(&mut s, "repair", json!({"id": 1})).unwrap();
        assert_eq!((r["dropped"].as_u64(), r["flipped"].as_u64()), (Some(0), Some(0)));
        run(&mut s, "undo", json!({})).unwrap();
        assert!(!s.scene.get(1).unwrap().mesh.is_watertight());
        assert!(run(&mut s, "repair", json!({"id": 7})).is_err());
    }

    #[test]
    fn a_plate_goes_out_as_3mf_and_comes_back_as_it_was_laid_out() {
        let path = std::env::temp_dir().join(format!("lamina-plate-{}.3mf", std::process::id()));
        let mut s = Session::new();
        run(&mut s, "add_primitive", json!({"kind": "box", "size": [10, 10, 10], "name": "cube"})).unwrap();
        run(&mut s, "add_primitive", json!({"kind": "cylinder", "radius": 4, "height": 6, "at": [30, 5]})).unwrap();
        run(&mut s, "rotate", json!({"id": 2, "axis": "z", "degrees": 30})).unwrap();
        let r = run(&mut s, "export_mesh", json!({"path": path})).unwrap();
        assert_eq!(r["objects"], 2);
        let before: Vec<(P3, P3)> = s.scene.objects.iter().map(|o| o.bounds().unwrap()).collect();
        let (lo, hi) = before.iter().fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), (a, b)| {
            ([lo[0].min(a[0]), lo[1].min(a[1]), lo[2].min(a[2])], [hi[0].max(b[0]), hi[1].max(b[1]), hi[2].max(b[2])])
        });
        let mut t = Session::new();
        let r = run(&mut t, "import_mesh", json!({"path": path, "at": [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]})).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(r["ids"], json!([1, 2]));
        assert_eq!(t.scene.get(1).unwrap().name, "cube");
        // Centred where the group was centred, so every object lands where it stood.
        for (o, (lo, hi)) in t.scene.objects.iter().zip(before) {
            let (a, b) = o.bounds().unwrap();
            for k in 0..3 {
                assert!((a[k] - lo[k]).abs() < 1e-3 && (b[k] - hi[k]).abs() < 1e-3, "{:?} {:?} {lo:?} {hi:?}", a, b);
            }
        }
        assert!(t.scene.objects.iter().all(|o| o.world().is_watertight()));
        let bad = std::env::temp_dir().join(format!("lamina-bad-{}.3mf", std::process::id()));
        std::fs::write(&bad, "not a zip").unwrap();
        assert!(run(&mut t, "import_mesh", json!({"path": bad})).unwrap_err().contains("not a 3MF"));
        std::fs::remove_file(&bad).unwrap();
        assert_eq!(t.scene.objects.len(), 2);
    }

    #[test]
    fn a_script_stops_at_the_line_that_fails() {
        let mut s = Session::new();
        let err = s.script("# a plate\nadd_primitive {\"kind\":\"box\",\"size\":[5,5,5]}\nmove {\"id\":9,\"by\":[0,0,0]}\n").unwrap_err();
        assert!(err.starts_with("line 3:"), "{err}");
    }
}
