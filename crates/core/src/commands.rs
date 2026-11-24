//! The commands: what the CLI, scripts, the MCP server and the AI chat run. Parameters are strict (a misspelled key is an error)
//! and every number is checked, because an agent or a file chose them.

use std::path::Path;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::engine::{Command, Files, Session};
use crate::mesh::{Mesh, P3, Transform};
use crate::scene::Object;
use crate::{mesh_in, project, repair, sketch, stl, threemf};

const fn cmd(id: &'static str, params: &'static str, run: crate::engine::Run, undoable: bool, files: Files) -> Command {
    Command { id, params, run, undoable, files }
}

pub static ALL: &[Command] = &[
    cmd(
        "import_mesh",
        "{path, name?, at?:[x,y], units?, up?} a mesh file put on the plate, centred on `at` (default the middle of the plate), its lowest point on the plate: an STL (binary or ASCII) as one object; a 3MF with each build item an object, or an OBJ or glTF (.gltf, .glb) with each of its objects an object, kept where they stand to each other. An OBJ is read in `units` (mm, cm, m, in or ft; default mm) with `up` \"z\" (default) or \"y\"; a glTF is in metres with y up, as its specification says. Meshes are repaired on the way in (doubled faces dropped, flipped faces turned, small holes filled). Returns {id (the first), ids, triangles, watertight, size, repaired?: {dropped, flipped, holes_filled}}",
        import_mesh,
        true,
        Files::Read,
    ),
    cmd(
        "add_primitive",
        "{kind, name?, at?:[x,y] (where its centre stands, default the middle of the plate), segments? (3..256, default 48), and by kind: \"box\" {size:[x,y,z]}; \"rounded_box\" {size, corner (the edges' radius, under half the smallest side)}; \"cylinder\" {radius, height}; \"cone\" {radius, height, radius_top? (default 0, a point; more makes a frustum)}; \"prism\" and \"pyramid\" {radius (to a corner), height, sides (3..64)}; \"tube\" {radius, inner, height}; \"torus\" {radius (to the middle of the ring), tube (the ring's own radius, less than radius)}; \"sphere\" {radius} (geodesic: 320, 1280, 5120 or 20480 triangles from segments); \"thread\" {metric (\"M3\"..\"M30\", ISO coarse pitch) | diameter and pitch, height (its length), internal? (default false: a threaded rod with its ends chamfered; true: the tool to cut a tapped hole with `difference`, longer than the hole), clearance? (mm a side: an internal thread's tool that much wider, default 0.2; an external one that much thinner, default 0)}, ISO metric profile, right-handed, upright} a shape on the plate, mm. Returns {id}",
        add_primitive,
        true,
        Files::None,
    ),
    cmd(
        "text",
        "{text, height (of the capitals, mm), depth (how far it rises, mm), spacing? (1 is the font's own space between letters), name?, at?:[x,y]} lettering as a solid lying on the plate, read from above, centred on `at`: IBM Plex Mono, Latin, Greek and Cyrillic, `\\n` starts a new line. Capitals 5 mm or taller and a depth of 0.6 to 1 mm print cleanly. Emboss it with `union`, engrave it with `difference` (move it so it sinks into the face). Returns {id, triangles, watertight, size}",
        text,
        true,
        Files::None,
    ),
    cmd(
        "extrude",
        "{height, outline?:[[x,y],...] | shape?:\"rounded_rect\"|\"slot\"|\"circle\", size?:[x,y] (rounded_rect: width and depth; slot: length and width), corner? (rounded_rect's corner radius), radius? (circle), holes?:[[[x,y],...],...], round_holes?:[[x,y,diameter],...], segments? (per quarter turn of a round edge, default 8), name?, at?:[x,y]} a flat outline, minus its holes, lifted `height` mm into a solid: a bracket, a plate with mounting holes, a spacer. Points are mm in the sketch's own frame (a shape is centred on its origin); the solid is put on the plate centred on `at`. Returns {id, triangles, watertight, size}",
        extrude,
        true,
        Files::None,
    ),
    cmd(
        "build",
        "{recipe:[{command, params}, ...], name?, at?:[x,y]} makes one object from a list of commands run on an empty plate of their own (ids count from 1 there: the first shape is 1, the second 2, a boolean's result the next), and keeps the list on the object, so `edit` can change a step later. The steps must end with one object. Returns {id, triangles, watertight, size}",
        build,
        true,
        Files::None,
    ),
    cmd(
        "edit",
        "{id, step (counting from 0), params} changes the parameters of one step of an object's recipe (the keys given replace the step's own) and builds it again, where it stands. For \"make the hole 6 mm\": edit the step that made the hole's cylinder. `info` shows each object's recipe",
        edit,
        true,
        Files::None,
    ),
    cmd("move", "{id, by:[x,y,z]} moves an object by mm", move_by, true, Files::None),
    cmd(
        "rotate",
        "{id, axis:\"x\"|\"y\"|\"z\"|[x,y,z] (a vector, for a tilted axis), degrees} turns an object about its own centre, then sets it back on the plate",
        rotate,
        true,
        Files::None,
    ),
    cmd(
        "reset_transform",
        "{id, what:\"location\"|\"rotation\"|\"scale\"|\"all\"} clears what moves, turns or scales an object since it was made, as Blender's Alt+G, Alt+R and Alt+S: location puts it in the middle of the plate; rotation keeps its size and place but takes its turn away; scale keeps its turn and place but its own size; all does the three. It is set back on the plate. A mirror is kept by rotation and lost by scale",
        reset_transform,
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
        "simplify",
        "{id, triangles? (4 or more) | ratio? (0 to 1, the share of triangles kept)} gives a heavy mesh (a scan) fewer triangles, keeping its shape: edges collapsed cheapest first by quadric error, never turning a face over or opening a closed mesh. Give one of the two. Returns {before, triangles, watertight}",
        simplify,
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
    cmd(
        "mirror",
        "{id, axis:\"x\"|\"y\"|\"z\"} mirrors an object across the plane through its centre square to `axis` (x turns left and right over), then sets it back on the plate",
        mirror,
        true,
        Files::None,
    ),
    cmd(
        "lay_flat",
        "{id, face} turns an object so its triangle number `face` (as the window's pick or the mesh gives it) lies flat on the plate, facing down, then sets it on the plate. Returns {turned} in degrees",
        lay_flat,
        true,
        Files::None,
    ),
    cmd("drop_to_bed", "{id} lowers or raises an object so its lowest point touches the plate", drop_to_bed, true, Files::None),
    cmd("delete", "{id} removes an object", delete, true, Files::None),
    cmd(
        "paint",
        "{id, kind:\"seam\"|\"support\"|\"block\"|\"none\", at?:[x,y,z], radius? (mm, default 3)} paints the object's faces within `radius` of the point `at` on its surface (where the window's brush is clicked): \"seam\" puts each wall's seam there, \"support\" holds those faces up even with supports off, \"block\" keeps supports off them, \"none\" rubs paint out. Without `at`, \"none\" clears all its paint. Repair, simplify and edit drop the paint. Returns {painted} (faces now that kind)",
        paint,
        true,
        Files::None,
    ),
    cmd(
        "set_modifier",
        "{id, kind: \"support_enforcer\" | \"support_blocker\" | \"settings\" | \"none\"} makes an object a modifier, which is not printed: the overhangs of any part inside a support enforcer get supports (even with supports off), and those inside a support blocker get none. Place a box over the overhang it is for. A \"settings\" modifier gives the sparse infill of every part inside it its own settings: give them with `set_object_settings {id, profile: {infill_density, infill_pattern, infill_speed}}` (denser infill round a screw hole, none in a hollow). \"none\" makes it a part again. Arrange leaves modifiers where they are. Returns {}",
        set_modifier,
        true,
        Files::None,
    ),
    cmd(
        "arrange",
        "{ids?, gap? (mm between footprints, default 5), bed?:[x,y] (default the project's plate, else 220 x 220)} lays the objects (all, or `ids`) out on the plate in rows, tallest footprints first, the whole centred, none touching. Returns {fits}: false when they do not all fit the plate",
        arrange,
        true,
        Files::None,
    ),
    cmd(
        "duplicate",
        "{id, count? (1..100, default 1), arrange? (default true)} copies an object; with `arrange` the plate is laid out again so nothing overlaps. Returns {ids, fits}",
        duplicate,
        true,
        Files::None,
    ),
    cmd(
        "info",
        "{} every object: id, name, triangles, size, min, max, volume_mm3, watertight (a slicer can only fill a closed mesh), and its problems: open_edges (holes), nonmanifold_edges (edges of three or more faces), flipped_faces; `repair` mends what it can; its recipe, its own settings, its modifier (support_enforcer, support_blocker, settings, or null for a part), its settings by height (ranges) and its painted faces (painted: how many seam, support, block). Also the plate's layer height curve (layer_heights: [z, height] points, or null)",
        info,
        false,
        Files::None,
    ),
    cmd(
        "export_mesh",
        "{path, id?, up?} writes one object, or the whole plate, where it stands: a binary STL (one mesh), a 3MF (`.3mf`, each object its own, with positions for a printer whose zero is the plate's front left corner), or an OBJ (`.obj`, each object its own group, in mm, `up` \"z\" (default) or \"y\" for programs that take y as up)",
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
    cmd(
        "measure",
        "{a:[x,y,z], b:[x,y,z]} the distance between two points, mm, and along each axis. Returns {distance, dx, dy, dz}",
        measure,
        false,
        Files::None,
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
    at: Option<[f64; 2]>,
    units: Option<String>,
    up: Option<String>,
}

fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default()
}

fn import_mesh(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Import = parse(p)?;
    let path = Path::new(&a.path);
    let kind = extension(path);
    if !matches!(kind.as_str(), "stl" | "3mf" | "obj" | "gltf" | "glb") {
        return Err("a mesh is read from an .stl, .3mf, .obj, .gltf or .glb file".into());
    }
    if kind != "obj" && (a.units.is_some() || a.up.is_some()) {
        return Err("`units` and `up` are for an OBJ: an STL and a 3MF say their own, and a glTF is in metres with y up".into());
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
    if matches!(kind.as_str(), "obj" | "gltf" | "glb") {
        // A file the model names beside it (an OBJ's material library, a glTF's buffer): only in its folder or below it.
        let beside = |rel: &str| -> Option<Vec<u8>> {
            let rel = Path::new(rel);
            let ok = rel.components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir));
            let f = path.parent()?.join(rel);
            (ok && std::fs::metadata(&f).ok()?.len() <= mesh_in::MAX_BYTES as u64).then(|| std::fs::read(f).ok()).flatten()
        };
        let read = if kind == "obj" {
            let mm = match a.units.as_deref().unwrap_or("mm") {
                "mm" => 1.0,
                "cm" => 10.0,
                "m" => 1000.0,
                "in" => 25.4,
                "ft" => 304.8,
                other => return Err(format!("`units` is one of mm, cm, m, in or ft, not {other}")),
            };
            let up_is_y = match a.up.as_deref().unwrap_or("z") {
                "z" => false,
                "y" => true,
                other => return Err(format!("`up` is z or y, not {other}")),
            };
            mesh_in::obj(&String::from_utf8_lossy(&bytes), mm, up_is_y, |lib| beside(lib).map(|b| String::from_utf8_lossy(&b).into_owned()))
        } else {
            mesh_in::gltf(&bytes, beside)
        }
        .map_err(|e| format!("{}: {e}", a.path))?;
        let parts: Vec<threemf::Part> = read
            .objects
            .into_iter()
            .filter_map(|o| {
                let soup: Vec<[f32; 3]> = o.parts.iter().flat_map(|p| p.tris.iter()).flatten().map(|v| v.map(|c| c as f32)).collect();
                let mesh = stl::weld(&soup).ok()?;
                Some(threemf::Part { name: o.name, mesh, xf: Transform::IDENTITY })
            })
            .collect();
        if parts.is_empty() {
            return Err(format!("{}: it holds no triangles to print", a.path));
        }
        let mut v = import_parts(s, parts, a.name.as_deref(), a.at)?;
        if !read.left_out.is_empty() {
            v["left_out"] = json!(read.left_out);
        }
        return Ok(v);
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
    radius_top: Option<f64>,
    sides: Option<u32>,
    inner: Option<f64>,
    tube: Option<f64>,
    corner: Option<f64>,
    metric: Option<String>,
    diameter: Option<f64>,
    pitch: Option<f64>,
    internal: Option<bool>,
    clearance: Option<f64>,
}

/// ISO 261's coarse pitches for the common sizes, mm.
const COARSE: &[(&str, f64, f64)] = &[
    ("M2", 2.0, 0.4),
    ("M2.5", 2.5, 0.45),
    ("M3", 3.0, 0.5),
    ("M4", 4.0, 0.7),
    ("M5", 5.0, 0.8),
    ("M6", 6.0, 1.0),
    ("M8", 8.0, 1.25),
    ("M10", 10.0, 1.5),
    ("M12", 12.0, 1.75),
    ("M14", 14.0, 2.0),
    ("M16", 16.0, 2.0),
    ("M20", 20.0, 2.5),
    ("M24", 24.0, 3.0),
    ("M30", 30.0, 3.5),
];

/// The most triangles a thread may have: one boolean takes 30,000 in all, with the part it is joined to or cut from.
const MAX_THREAD_TRIANGLES: u32 = 24_000;

fn add_primitive(s: &mut Session, p: &Value) -> Result<Value, String> {
    const MAX: f64 = 10_000.0;
    let a: Primitive = parse(p)?;
    let segments = a.segments.unwrap_or(48);
    if !(3..=256).contains(&segments) {
        return Err("`segments` is from 3 to 256".into());
    }
    let need = |v: Option<f64>, what: &str| v.ok_or_else(|| format!("a {} needs `{what}`", a.kind)).and_then(|x| positive(what, x, MAX));
    let size = || -> Result<P3, String> {
        let size = a.size.ok_or_else(|| format!("a {} needs `size`: [x, y, z]", a.kind))?;
        size.map(|c| positive("size", c, MAX)).into_iter().collect::<Result<Vec<_>, _>>().map(|v| [v[0], v[1], v[2]])
    };
    let sides = || -> Result<u32, String> {
        let n = a.sides.ok_or_else(|| format!("a {} needs `sides`, 3 to 64", a.kind))?;
        if (3..=64).contains(&n) { Ok(n) } else { Err("`sides` is from 3 to 64".into()) }
    };
    let less = |what: &str, x: f64, than: &str, limit: f64| -> Result<f64, String> {
        if x < limit { Ok(x) } else { Err(format!("`{what}` must be less than {than} ({limit})")) }
    };
    let mesh = match a.kind.as_str() {
        "box" => Mesh::cuboid(size()?),
        "rounded_box" => {
            let size = size()?;
            let smallest = size.iter().copied().fold(f64::MAX, f64::min);
            let corner = less("corner", need(a.corner, "corner")?, "half the smallest side", smallest / 2.0)?;
            Mesh::rounded_box(size, corner, (segments / 6).clamp(2, 16))
        }
        "cylinder" => Mesh::cylinder(need(a.radius, "radius")?, need(a.height, "height")?, segments),
        "cone" => {
            let top = match a.radius_top {
                None | Some(0.0) => 0.0,
                Some(t) => positive("radius_top", t, MAX)?,
            };
            Mesh::frustum(need(a.radius, "radius")?, top, need(a.height, "height")?, segments)
        }
        "prism" => Mesh::cylinder(need(a.radius, "radius")?, need(a.height, "height")?, sides()?),
        "pyramid" => Mesh::frustum(need(a.radius, "radius")?, 0.0, need(a.height, "height")?, sides()?),
        "tube" => {
            let outer = need(a.radius, "radius")?;
            let inner = less("inner", need(a.inner, "inner")?, "radius", outer)?;
            Mesh::tube(outer, inner, need(a.height, "height")?, segments)
        }
        "torus" => {
            let ring = need(a.radius, "radius")?;
            Mesh::torus(ring, less("tube", need(a.tube, "tube")?, "radius", ring)?, segments)
        }
        "sphere" => Mesh::sphere(need(a.radius, "radius")?, segments),
        "thread" => {
            let (diameter, pitch) = match (&a.metric, a.diameter, a.pitch) {
                (Some(m), None, None) => {
                    let m = m.trim().to_uppercase();
                    COARSE.iter().find(|(name, _, _)| *name == m).map(|&(_, d, p)| (d, p)).ok_or_else(|| {
                        format!(
                            "`metric` is one of {}, or give `diameter` and `pitch`",
                            COARSE.iter().map(|c| c.0).collect::<Vec<_>>().join(", ")
                        )
                    })?
                }
                (None, Some(d), Some(p)) => (positive("diameter", d, 500.0)?, positive("pitch", p, 20.0)?),
                _ => return Err("a thread needs `metric` (\"M8\") or both `diameter` and `pitch`".into()),
            };
            if pitch > diameter / 2.0 || pitch < 0.1 {
                return Err(format!("a pitch of {pitch} mm does not fit a {diameter} mm thread: from 0.1 mm to half the diameter"));
            }
            let internal = a.internal.unwrap_or(false);
            let clearance = a.clearance.unwrap_or(if internal { 0.2 } else { 0.0 });
            if !(clearance.is_finite() && (0.0..=2.0).contains(&clearance)) {
                return Err("`clearance` is from 0 to 2 mm a side".into());
            }
            let height = need(a.height, "height")?;
            let rows = (height / (pitch / 8.0)).ceil();
            if 2.0 * f64::from(segments) * (rows + 1.0) > f64::from(MAX_THREAD_TRIANGLES) {
                return Err(format!(
                    "a thread {height} mm long at a pitch of {pitch} mm with {segments} segments has more than {MAX_THREAD_TRIANGLES} triangles: make it shorter or use fewer segments"
                ));
            }
            let offset = if internal { clearance } else { -clearance };
            Mesh::thread(diameter, pitch, height, offset, segments, !internal)
        }
        other => {
            return Err(format!("unknown kind `{other}`: box, rounded_box, cylinder, cone, prism, pyramid, tube, torus, sphere or thread"));
        }
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
struct Extrude {
    height: f64,
    outline: Option<Vec<[f64; 2]>>,
    shape: Option<String>,
    size: Option<[f64; 2]>,
    corner: Option<f64>,
    radius: Option<f64>,
    holes: Option<Vec<Vec<[f64; 2]>>>,
    round_holes: Option<Vec<[f64; 3]>>,
    segments: Option<u32>,
    name: Option<String>,
    at: Option<[f64; 2]>,
}

fn extrude(s: &mut Session, p: &Value) -> Result<Value, String> {
    const MAX: f64 = 10_000.0;
    let a: Extrude = parse(p)?;
    let segments = a.segments.unwrap_or(8);
    if !(1..=64).contains(&segments) {
        return Err("`segments` is from 1 to 64 steps per quarter turn".into());
    }
    let size = || -> Result<[f64; 2], String> {
        let [x, y] = a.size.ok_or("this shape needs `size`: [x, y]")?;
        Ok([positive("size", x, MAX)?, positive("size", y, MAX)?])
    };
    let outline = match (a.outline, a.shape.as_deref()) {
        (Some(o), None) => o,
        (None, Some("rounded_rect")) => {
            let [w, d] = size()?;
            let corner = match a.corner {
                None => 0.0,
                Some(c) if c.is_finite() && c >= 0.0 && c < w.min(d) / 2.0 => c,
                Some(c) => return Err(format!("`corner` is from 0 to under half the smaller side, not {c}")),
            };
            sketch::rounded_rect(w, d, corner, segments)
        }
        (None, Some("slot")) => {
            let [l, w] = size()?;
            sketch::slot(l, w, segments)
        }
        (None, Some("circle")) => {
            sketch::circle(0.0, 0.0, positive("radius", a.radius.ok_or("a circle needs `radius`")?, MAX)?, segments * 4)
        }
        (None, Some(other)) => return Err(format!("unknown shape `{other}`: rounded_rect, slot or circle")),
        _ => return Err("give `outline` or `shape`, one of them".into()),
    };
    let mut holes = a.holes.unwrap_or_default();
    for [x, y, d] in a.round_holes.unwrap_or_default() {
        holes.push(sketch::circle(x, y, positive("round_holes diameter", d, MAX)? / 2.0, segments * 4));
    }
    let mesh = sketch::extrude(&outline, &holes, positive("height", a.height, MAX)?)?;
    added(s, a.name.as_deref().unwrap_or("extrusion"), mesh, a.at)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    text: String,
    height: f64,
    depth: f64,
    spacing: Option<f64>,
    name: Option<String>,
    at: Option<[f64; 2]>,
}

fn text(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Text = parse(p)?;
    let mut contours = crate::text::contours(&a.text, a.height, a.spacing.unwrap_or(1.0))?;
    // Centred on the origin, as the other shapes are, so `at` is where its middle stands.
    let (lo, hi) = contours
        .iter()
        .flatten()
        .fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
    let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    contours.iter_mut().flatten().for_each(|p| *p = [p[0] - mid[0], p[1] - mid[1]]);
    let mesh = sketch::extrude_contours(&contours, positive("depth", a.depth, 1000.0)?)?;
    let name = a.name.unwrap_or_else(|| a.text.chars().take(24).collect::<String>().replace('\n', " "));
    added(s, &name, mesh, a.at)
}

/// The most steps a recipe may have, and the most text its steps may take.
const MAX_STEPS: usize = 200;
const MAX_RECIPE: usize = 1 << 20;

/// Runs `recipe` on an empty plate of its own with `s`'s commands, and gives back the one object it ends with: its mesh where it
/// stands there.
fn cook(s: &Session, recipe: &Value) -> Result<Mesh, String> {
    let steps = recipe.as_array().ok_or("`recipe` is a list of steps")?;
    if steps.is_empty() || steps.len() > MAX_STEPS {
        return Err(format!("a recipe has from 1 to {MAX_STEPS} steps"));
    }
    if recipe.to_string().len() > MAX_RECIPE {
        return Err("the recipe is too long".into());
    }
    let mut kitchen = s.sibling();
    for (n, step) in steps.iter().enumerate() {
        let Value::Object(m) = step else { return Err(format!("step {n} is not {{command, params}}")) };
        if m.keys().any(|k| k != "command" && k != "params") {
            return Err(format!("step {n} has keys other than `command` and `params`"));
        }
        let command = m.get("command").and_then(Value::as_str).ok_or_else(|| format!("step {n} has no `command`"))?;
        let c = kitchen.find(command).ok_or_else(|| format!("step {n}: unknown command `{command}`"))?;
        if c.files != Files::None || matches!(c.id, "new" | "open" | "undo" | "redo" | "build" | "edit") {
            return Err(format!("step {n}: `{}` cannot be a step of a recipe", c.id));
        }
        let params = m.get("params").cloned().unwrap_or(json!({}));
        kitchen.execute(command, &params).map_err(|e| format!("step {n} (`{command}`): {e}"))?;
    }
    match kitchen.scene.objects.as_slice() {
        [one] => Ok(one.world()),
        [] => Err("the recipe ends with nothing on its plate".into()),
        many => Err(format!("the recipe ends with {} objects: join them into one with `union`", many.len())),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Build {
    recipe: Value,
    name: Option<String>,
    at: Option<[f64; 2]>,
}

fn build(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Build = parse(p)?;
    let mesh = cook(s, &a.recipe)?;
    let v = added(s, a.name.as_deref().unwrap_or("part"), mesh, a.at)?;
    let id = v["id"].as_u64().ok_or("internal: no id")? as u32;
    s.scene_mut().get_mut(id)?.recipe = Some(std::sync::Arc::new(a.recipe));
    Ok(v)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    id: u32,
    step: usize,
    params: serde_json::Map<String, Value>,
}

fn edit(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Edit = parse(p)?;
    let o = s.scene.get(a.id)?;
    let mut recipe = o.recipe.as_deref().cloned().ok_or_else(|| format!("object {} has no recipe: it was not made with `build`", a.id))?;
    let steps = recipe.as_array_mut().ok_or("the object's recipe is not a list of steps")?;
    let count = steps.len();
    let step = steps.get_mut(a.step).ok_or_else(|| format!("the recipe has steps 0 to {}", count.saturating_sub(1)))?;
    match step.get_mut("params") {
        Some(Value::Object(params)) => params.extend(a.params),
        _ => step["params"] = Value::Object(a.params),
    }
    let mesh = cook(s, &recipe)?;
    let o = s.scene_mut().get_mut(a.id)?;
    (o.mesh, o.recipe, o.paint) = (std::sync::Arc::new(mesh), Some(std::sync::Arc::new(recipe)), None);
    // Where it stands stays; a part that grew or shrank is set back on the plate.
    settle(o)?;
    let world = o.world();
    let (lo, hi) = world.bounds().unwrap_or_default();
    Ok(
        json!({"triangles": world.triangles.len(), "watertight": world.is_watertight(), "size": [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]}),
    )
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
    axis: Axis,
    degrees: f64,
}

/// An axis by name, or as a vector (an object's own axis, as the window's local orientation gives it).
#[derive(Deserialize)]
#[serde(untagged)]
enum Axis {
    Named(String),
    Vector([f64; 3]),
}

fn rotate(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Rotate = parse(p)?;
    if !a.degrees.is_finite() {
        return Err("`degrees` must be a finite number".into());
    }
    let turn = match a.axis {
        Axis::Named(name) => match name.as_str() {
            "x" => Transform::rotate(0, a.degrees),
            "y" => Transform::rotate(1, a.degrees),
            "z" => Transform::rotate(2, a.degrees),
            other => return Err(format!("unknown axis `{other}`: x, y or z, or a vector [x, y, z]")),
        },
        Axis::Vector(v) => {
            let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if !(1e-9..=1e6).contains(&len) {
                return Err("an axis vector must be finite and not zero".into());
            }
            Transform::rotate_axis(v, a.degrees)
        }
    };
    about_centre(s.scene_mut().get_mut(a.id)?, &turn)?;
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reset {
    id: u32,
    what: String,
}

/// Clears an object's location, its turn or its scale, as Blender's Alt+G, Alt+R and Alt+S do. Its turn and its scale are the
/// polar decomposition of its matrix (`Transform::polar`); clearing one keeps the other, about the object's middle, and it is set
/// back on the plate. Its location is the middle of the plate.
fn reset_transform(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Reset = parse(p)?;
    let o = s.scene_mut().get_mut(a.id)?;
    let (turn, stretch) = o.xf.polar().ok_or("the object's transform flattens it")?;
    let m = match a.what.as_str() {
        "location" => o.xf.m,
        "rotation" => stretch,
        "scale" => turn.m,
        "all" => Transform::IDENTITY.m,
        other => return Err(format!("`what` is location, rotation, scale or all, not `{other}`")),
    };
    let (lo, hi) = size_of(o)?;
    let middle = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    o.xf = Transform { m, t: o.xf.t };
    let (lo, hi) = size_of(o)?;
    let now = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let to = if matches!(a.what.as_str(), "location" | "all") { [0.0, 0.0] } else { [middle[0], middle[1]] };
    o.xf = o.xf.then(&Transform::translate([to[0] - now[0], to[1] - now[1], 0.0]));
    settle(o)?;
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mirror {
    id: u32,
    axis: String,
}

fn mirror(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Mirror = parse(p)?;
    let mut by = [1.0; 3];
    match a.axis.as_str() {
        "x" => by[0] = -1.0,
        "y" => by[1] = -1.0,
        "z" => by[2] = -1.0,
        other => return Err(format!("unknown axis `{other}`: x, y or z")),
    }
    about_centre(s.scene_mut().get_mut(a.id)?, &Transform::scale(by))?;
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
        let o = s.scene_mut().get_mut(a.id)?;
        // Its triangles are new: paint on the old ones does not carry over.
        (o.mesh, o.paint) = (std::sync::Arc::new(mesh), None);
    }
    Ok(json!({"dropped": r.dropped, "flipped": r.flipped, "holes_filled": r.holes_filled, "watertight": r.closed}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Simplify {
    id: u32,
    triangles: Option<usize>,
    ratio: Option<f64>,
}

fn simplify(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Simplify = parse(p)?;
    let mesh = std::sync::Arc::clone(&s.scene.get(a.id)?.mesh);
    let before = mesh.triangles.len();
    let target = match (a.triangles, a.ratio) {
        (Some(t), None) if t >= 4 => t,
        (None, Some(r)) if r.is_finite() && r > 0.0 && r < 1.0 => ((before as f64 * r).round() as usize).max(4),
        (Some(_), None) => return Err("`triangles` is 4 or more".into()),
        (None, Some(_)) => return Err("`ratio` is a number between 0 and 1".into()),
        _ => return Err("give `triangles` or `ratio`, one of them".into()),
    };
    if target >= before {
        return Err(format!("object {} has {before} triangles, no more than the {target} asked for: nothing to do", a.id));
    }
    let out = crate::simplify::simplify(&mesh, target)?;
    let (triangles, watertight) = (out.triangles.len(), out.is_watertight());
    let o = s.scene_mut().get_mut(a.id)?;
    (o.mesh, o.paint) = (std::sync::Arc::new(out), None);
    Ok(json!({"before": before, "triangles": triangles, "watertight": watertight}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arrange {
    ids: Option<Vec<u32>>,
    gap: Option<f64>,
    bed: Option<[f64; 2]>,
}

/// The plate from the project's print settings, else the default 220 mm one.
pub fn plate(s: &Session) -> [f64; 2] {
    s.print["profile"]["bed"].as_array().and_then(|b| Some([b.first()?.as_f64()?, b.get(1)?.as_f64()?])).unwrap_or([220.0, 220.0])
}

/// Lays `ids` out in rows inside `bed` (a shelf packer: the deepest footprints first, each row as deep as its deepest), `gap` apart,
/// and centres the whole on the plate. Returns whether it all fits.
pub fn lay_out(s: &mut Session, ids: &[u32], gap: f64, bed: [f64; 2]) -> Result<bool, String> {
    let mut boxes: Vec<(u32, P3, P3)> =
        ids.iter().map(|&id| s.scene.get(id).and_then(size_of).map(|(lo, hi)| (id, lo, hi))).collect::<Result<_, _>>()?;
    boxes.sort_by(|a, b| (b.2[1] - b.1[1]).total_cmp(&(a.2[1] - a.1[1])).then(a.0.cmp(&b.0)));
    // Where each footprint's front left corner goes, from the plate's front left corner.
    let (mut x, mut y, mut row) = (0.0f64, 0.0f64, 0.0f64);
    let mut width = 0.0f64;
    let mut placed = Vec::with_capacity(boxes.len());
    for &(id, lo, hi) in &boxes {
        let (w, d) = (hi[0] - lo[0], hi[1] - lo[1]);
        if x > 0.0 && x + w > bed[0] {
            (x, y, row) = (0.0, y + row + gap, 0.0);
        }
        placed.push((id, lo, [x, y]));
        width = width.max(x + w);
        row = row.max(d);
        x += w + gap;
    }
    let depth = y + row;
    let shift = [-width / 2.0, -depth / 2.0];
    for (id, lo, at) in placed {
        let by = [at[0] + shift[0] - lo[0], at[1] + shift[1] - lo[1], 0.0];
        let o = s.scene_mut().get_mut(id)?;
        o.xf = o.xf.then(&Transform::translate(by));
    }
    Ok(width <= bed[0] + 1e-9 && depth <= bed[1] + 1e-9)
}

fn arrange(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Arrange = parse(p)?;
    let gap = match a.gap {
        None => 5.0,
        Some(g) if g.is_finite() && (0.0..=100.0).contains(&g) => g,
        Some(g) => return Err(format!("`gap` is from 0 to 100 mm, not {g}")),
    };
    let bed = match a.bed {
        None => plate(s),
        Some(b) => [positive("bed", b[0], 10_000.0)?, positive("bed", b[1], 10_000.0)?],
    };
    // A modifier stays where it is on the part it changes: arranged only when named.
    let ids = a.ids.unwrap_or_else(|| s.scene.objects.iter().filter(|o| o.modifier.is_none()).map(|o| o.id).collect());
    if ids.is_empty() {
        return Err("nothing to arrange: the plate is empty".into());
    }
    Ok(json!({"fits": lay_out(s, &ids, gap, bed)?}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Duplicate {
    id: u32,
    count: Option<u32>,
    arrange: Option<bool>,
}

fn duplicate(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Duplicate = parse(p)?;
    let count = a.count.unwrap_or(1);
    if !(1..=100).contains(&count) {
        return Err("`count` is from 1 to 100".into());
    }
    let o = s.scene.get(a.id)?.clone();
    let mut ids = Vec::with_capacity(count as usize);
    for _ in 0..count {
        // The mesh is an `Arc`: a copy shares it and costs nothing. It carries what was set on the first: its recipe, settings,
        // modifier kind, settings by height and paint.
        let id = s.scene_mut().add_shared(&o.name, std::sync::Arc::clone(&o.mesh), o.xf)?;
        let c = s.scene_mut().get_mut(id)?;
        (c.recipe, c.settings, c.modifier, c.ranges, c.paint) =
            (o.recipe.clone(), o.settings.clone(), o.modifier, o.ranges.clone(), o.paint.clone());
        ids.push(id);
    }
    let fits = if a.arrange.unwrap_or(true) {
        let all: Vec<u32> = s.scene.objects.iter().map(|o| o.id).collect();
        lay_out(s, &all, 5.0, plate(s))?
    } else {
        true
    };
    Ok(json!({"ids": ids, "fits": fits}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Face {
    id: u32,
    face: usize,
}

fn lay_flat(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Face = parse(p)?;
    let o = s.scene.get(a.id)?;
    let t = o.mesh.tri(a.face).ok_or_else(|| format!("object {} has no triangle {}; it has {}", a.id, a.face, o.mesh.triangles.len()))?;
    let w = t.map(|q| o.xf.apply(q));
    let n = cross([w[1][0] - w[0][0], w[1][1] - w[0][1], w[1][2] - w[0][2]], [w[2][0] - w[0][0], w[2][1] - w[0][1], w[2][2] - w[0][2]]);
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len < 1e-12 {
        return Err(format!("triangle {} has no area, so no side to lie on", a.face));
    }
    // The face's outward normal turned to point straight down: about the axis square to both, by the angle between them.
    let n = n.map(|c| c / len);
    let angle = (-n[2]).clamp(-1.0, 1.0).acos().to_degrees();
    let axis = if angle > 179.999 { [1.0, 0.0, 0.0] } else { cross(n, [0.0, 0.0, -1.0]) };
    if angle > 1e-9 {
        about_centre(s.scene_mut().get_mut(a.id)?, &Transform::rotate_axis(axis, angle))?;
    } else {
        settle(s.scene_mut().get_mut(a.id)?)?;
    }
    Ok(json!({"turned": angle}))
}

fn cross(a: P3, b: P3) -> P3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Measure {
    a: P3,
    b: P3,
}

fn measure(_: &mut Session, p: &Value) -> Result<Value, String> {
    let m: Measure = parse(p)?;
    let (a, b) = (finite3("a", m.a)?, finite3("b", m.b)?);
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    Ok(json!({"distance": (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(), "dx": d[0], "dy": d[1], "dz": d[2]}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Id = parse(p)?;
    s.scene.get(a.id)?;
    s.scene_mut().objects.retain(|o| o.id != a.id);
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaintArgs {
    id: u32,
    kind: String,
    at: Option<P3>,
    radius: Option<f64>,
}

fn paint(s: &mut Session, p: &Value) -> Result<Value, String> {
    use crate::Paint;
    let a: PaintArgs = parse(p)?;
    let tag = match a.kind.as_str() {
        "seam" => Paint::Seam,
        "support" => Paint::Support,
        "block" => Paint::Block,
        "none" => Paint::None,
        other => return Err(format!("`kind` is seam, support, block or none, not `{other}`")),
    };
    let radius = positive("radius", a.radius.unwrap_or(3.0), 1000.0)?;
    let o = s.scene_mut().get_mut(a.id)?;
    let n = o.mesh.triangles.len();
    let mut tags: Vec<u8> = o.paint.as_deref().cloned().filter(|t| t.len() == n).unwrap_or_else(|| vec![0; n]);
    match a.at {
        None if tag == Paint::None => tags.iter_mut().for_each(|t| *t = 0),
        None => return Err("`at` is needed to paint: the point on the surface to paint round".into()),
        Some(at) => {
            let at = finite3("at", at)?;
            if at.iter().any(|c| c.abs() > crate::mesh::MAX_COORD) {
                return Err(format!("`at` is a point on the object, within {} mm of the plate's middle", crate::mesh::MAX_COORD));
            }
            // A face is painted when any part of it is within the radius of the point: small faces near it and large faces it lands
            // on alike.
            let world = o.world();
            for (i, t) in tags.iter_mut().enumerate() {
                if world.tri(i).is_some_and(|c| crate::mesh::distance_to_triangle(at, c) <= radius) {
                    *t = tag as u8;
                }
            }
        }
    }
    let painted = tags.iter().filter(|&&t| t == tag as u8 && tag != Paint::None).count();
    o.paint = if tags.iter().all(|&t| t == 0) { None } else { Some(std::sync::Arc::new(tags)) };
    Ok(json!({ "painted": painted }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetModifier {
    id: u32,
    kind: String,
}

fn set_modifier(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: SetModifier = parse(p)?;
    let modifier = match a.kind.as_str() {
        "none" => None,
        kind => Some(
            serde_json::from_value(json!(kind))
                .map_err(|_| format!("`kind` is support_enforcer, support_blocker, settings or none, not `{kind}`"))?,
        ),
    };
    s.scene_mut().get_mut(a.id)?.modifier = modifier;
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
                "recipe": o.recipe.as_deref().cloned().unwrap_or(Value::Null),
                "settings": o.settings.as_deref().cloned().unwrap_or(Value::Null),
                "modifier": o.modifier.map_or(Value::Null, |m| serde_json::to_value(m).unwrap_or(Value::Null)),
                "ranges": o.ranges.as_deref().cloned().unwrap_or(Value::Null),
                "painted": painted(o),
            })
        })
        .collect();
    let heights = s.scene.layer_heights.as_deref().map_or(Value::Null, |h| json!(h));
    Ok(json!({"objects": objects, "layer_heights": heights}))
}

/// How many faces carry each kind of paint, or null when none do.
fn painted(o: &crate::Object) -> Value {
    let Some(tags) = o.paint.as_deref() else { return Value::Null };
    let count = |t: u8| tags.iter().filter(|&&x| x == t).count();
    json!({"seam": count(1), "support": count(2), "block": count(3)})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Export {
    path: String,
    id: Option<u32>,
    up: Option<String>,
}

fn export_mesh(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a: Export = parse(p)?;
    let kind = extension(Path::new(&a.path));
    let up_is_y = match a.up.as_deref() {
        None | Some("z") => false,
        Some("y") if kind == "obj" => true,
        Some("y") => return Err("`up` is for an OBJ: STL and 3MF are always z up".into()),
        Some(other) => return Err(format!("`up` is \"z\" or \"y\", not {other:?}")),
    };
    if kind == "obj" {
        let objects: Vec<(String, Mesh)> = match a.id {
            Some(id) => vec![s.scene.get(id).map(|o| (o.name.clone(), o.world()))?],
            None => s.scene.objects.iter().map(|o| (o.name.clone(), o.world())).collect(),
        };
        let triangles: usize = objects.iter().map(|(_, m)| m.triangles.len()).sum();
        if triangles == 0 {
            return Err("nothing to export: the plate is empty".into());
        }
        std::fs::write(&a.path, mesh_in::obj_text(&objects, up_is_y)).map_err(|e| format!("{}: {e}", a.path))?;
        return Ok(json!({"path": a.path, "objects": objects.len(), "triangles": triangles}));
    }
    if kind == "3mf" {
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
    fn every_kind_of_shape_is_made_closed_and_bad_sizes_are_refused() {
        let mut s = Session::new();
        for p in [
            json!({"kind": "rounded_box", "size": [20, 10, 6], "corner": 2}),
            json!({"kind": "cone", "radius": 5, "height": 8}),
            json!({"kind": "cone", "radius": 5, "height": 8, "radius_top": 2}),
            json!({"kind": "prism", "radius": 5, "height": 8, "sides": 6}),
            json!({"kind": "pyramid", "radius": 5, "height": 8, "sides": 4}),
            json!({"kind": "tube", "radius": 8, "inner": 6, "height": 10}),
            json!({"kind": "torus", "radius": 10, "tube": 2}),
        ] {
            let v = run(&mut s, "add_primitive", p.clone()).unwrap();
            assert_eq!(v["watertight"], true, "{p}");
        }
        for bad in [
            json!({"kind": "rounded_box", "size": [20, 10, 6], "corner": 3}),
            json!({"kind": "tube", "radius": 8, "inner": 8, "height": 10}),
            json!({"kind": "torus", "radius": 3, "tube": 3}),
            json!({"kind": "prism", "radius": 5, "height": 8, "sides": 2}),
            json!({"kind": "pyramid", "radius": 5, "height": 8}),
            json!({"kind": "cone", "radius": 5, "height": 8, "radius_top": -1}),
            json!({"kind": "star", "radius": 5}),
        ] {
            assert!(run(&mut s, "add_primitive", bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(s.scene.objects.len(), 7);
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
    fn text_is_a_solid_on_the_plate_that_a_boolean_engraves() {
        let mut s = Session::new();
        let r = run(&mut s, "text", json!({"text": "LAMINA", "height": 8, "depth": 1, "at": [0, 0]})).unwrap();
        assert_eq!(r["watertight"], true);
        let size = r["size"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
        assert!((size[1] - 8.0).abs() < 0.3 && (size[2] - 1.0).abs() < 1e-6 && size[0] > 30.0, "{size:?}");
        let o = s.scene.get(1).unwrap();
        assert_eq!(o.name, "LAMINA");
        let (lo, hi) = o.bounds().unwrap();
        assert!(((lo[0] + hi[0]) / 2.0).abs() < 1e-3 && lo[2].abs() < 1e-9, "centred on the plate: {lo:?} {hi:?}");
        for bad in
            [json!({"text": "", "height": 5, "depth": 1}), json!({"text": "a", "height": 5, "depth": 0}), json!({"text": "a", "height": 5})]
        {
            assert!(run(&mut s, "text", bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn simplify_takes_a_heavy_mesh_down_as_one_undo_step() {
        let mut s = Session::new();
        run(&mut s, "add_primitive", json!({"kind": "sphere", "radius": 20, "segments": 128})).unwrap();
        let before = s.scene.get(1).unwrap().mesh.triangles.len();
        let r = run(&mut s, "simplify", json!({"id": 1, "ratio": 0.1})).unwrap();
        assert_eq!(r["before"], before);
        assert!(r["triangles"].as_u64().unwrap() as usize <= before / 10 && r["watertight"] == true, "{r}");
        run(&mut s, "simplify", json!({"id": 1, "triangles": 200})).unwrap();
        assert!(s.scene.get(1).unwrap().mesh.triangles.len() <= 200);
        run(&mut s, "undo", json!({})).unwrap();
        run(&mut s, "undo", json!({})).unwrap();
        assert_eq!(s.scene.get(1).unwrap().mesh.triangles.len(), before);
        for bad in [
            json!({"id": 1}),
            json!({"id": 1, "ratio": 1.5}),
            json!({"id": 1, "triangles": 2}),
            json!({"id": 1, "triangles": 10, "ratio": 0.5}),
        ] {
            assert!(run(&mut s, "simplify", bad.clone()).is_err(), "{bad}");
        }
        assert!(run(&mut s, "simplify", json!({"id": 1, "triangles": before * 2})).unwrap_err().contains("nothing to do"));
    }

    #[test]
    fn a_plate_goes_out_as_obj_with_each_object_its_own_and_comes_back() {
        let mut s = Session::new();
        run(&mut s, "add_primitive", json!({"kind": "box", "size": [10, 10, 10], "name": "cube"})).unwrap();
        run(&mut s, "add_primitive", json!({"kind": "cylinder", "radius": 4, "height": 6, "at": [30, 5]})).unwrap();
        let volumes: Vec<f64> = s.scene.objects.iter().map(|o| o.world().volume()).collect();
        for up in ["z", "y"] {
            let path = std::env::temp_dir().join(format!("lamina-plate-{}-{up}.obj", std::process::id()));
            let r = run(&mut s, "export_mesh", json!({"path": path, "up": up})).unwrap();
            assert_eq!(r["objects"], 2);
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains("o cube\n") && text.contains(&format!("{up} up")), "{}", &text[..80]);
            let mut t = Session::new();
            let back = run(&mut t, "import_mesh", json!({"path": path, "up": up})).unwrap();
            std::fs::remove_file(&path).unwrap();
            assert_eq!(back["ids"], json!([1, 2]));
            assert_eq!(t.scene.get(1).unwrap().name, "cube");
            for (o, v) in t.scene.objects.iter().zip(&volumes) {
                assert!((o.world().volume() - v).abs() < 1e-3 * v, "{up}: {} vs {v}", o.world().volume());
                assert!(o.world().is_watertight());
            }
            // The cube is 10 mm tall whichever way up the file says.
            let (lo, hi) = t.scene.get(1).unwrap().bounds().unwrap();
            assert!((hi[2] - lo[2] - 10.0).abs() < 1e-4, "{up}");
        }
        assert!(run(&mut s, "export_mesh", json!({"path": "/tmp/x.stl", "up": "y"})).unwrap_err().contains("OBJ"));
        assert!(run(&mut s, "export_mesh", json!({"path": "/tmp/x.obj", "up": "x"})).is_err());
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
        assert!(run(&mut t, "import_mesh", json!({"path": bad, "up": "y"})).unwrap_err().contains("for an OBJ"));
        std::fs::write(&bad, "not a zip").unwrap();
        assert!(run(&mut t, "import_mesh", json!({"path": bad})).unwrap_err().contains("not a 3MF"));
        std::fs::remove_file(&bad).unwrap();
        assert_eq!(t.scene.objects.len(), 2);
    }

    #[test]
    fn obj_and_gltf_files_come_in_object_by_object() {
        let dir = std::env::temp_dir().join(format!("lamina-obj-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A 10 mm cube and, beside it, a second one, as two OBJ objects; z up in millimetres.
        let cube = |o: &str, x: f64, base: usize| {
            let v: String = [
                [0.0, 0.0, 0.0],
                [10.0, 0.0, 0.0],
                [10.0, 10.0, 0.0],
                [0.0, 10.0, 0.0],
                [0.0, 0.0, 10.0],
                [10.0, 0.0, 10.0],
                [10.0, 10.0, 10.0],
                [0.0, 10.0, 10.0],
            ]
            .iter()
            .map(|p: &[f64; 3]| format!("v {} {} {}\n", p[0] + x, p[1], p[2]))
            .collect();
            let f: String = [
                [1, 3, 2],
                [1, 4, 3],
                [5, 6, 7],
                [5, 7, 8],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 4, 8],
                [3, 8, 7],
                [4, 1, 5],
                [4, 5, 8],
            ]
            .iter()
            .map(|t: &[usize; 3]| format!("f {} {} {}\n", t[0] + base, t[1] + base, t[2] + base))
            .collect();
            format!("o {o}\n{v}{f}")
        };
        let obj = dir.join("pair.obj");
        std::fs::write(&obj, format!("{}{}", cube("left", 0.0, 0), cube("right", 20.0, 8))).unwrap();
        let mut s = Session::new();
        let r = run(&mut s, "import_mesh", json!({"path": obj})).unwrap();
        assert_eq!(r["ids"], json!([1, 2]));
        assert_eq!((s.scene.get(1).unwrap().name.as_str(), s.scene.get(2).unwrap().name.as_str()), ("left", "right"));
        let (lo, hi) = (s.scene.get(1).unwrap().bounds().unwrap(), s.scene.get(2).unwrap().bounds().unwrap());
        assert_eq!((lo.0[0], hi.1[0], lo.0[2]), (-15.0, 15.0, 0.0), "the pair centred on the plate, 10 mm apart");
        assert!(s.scene.objects.iter().all(|o| o.world().is_watertight() && (o.world().volume() - 1000.0).abs() < 1e-3));
        // In centimetres and y up: ten times the size, standing the other way.
        let r = run(&mut s, "import_mesh", json!({"path": obj, "units": "cm", "up": "y"})).unwrap();
        assert_eq!(r["size"][0], 300.0);
        assert!(run(&mut s, "import_mesh", json!({"path": obj, "units": "furlong"})).is_err());
        // A glTF: one triangle is no solid, but it comes in and is said to be open.
        let gltf = dir.join("tri.gltf");
        let bin: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        let doc = json!({
            "asset": {"version": "2.0"}, "scenes": [{"nodes": [0]}], "nodes": [{"mesh": 0, "name": "tri"}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]}],
            "bufferViews": [{"buffer": 0, "byteLength": 36}],
            "buffers": [{"byteLength": 36, "uri": format!("data:application/octet-stream;base64,{}", crate::base64::encode(&bin))}],
        });
        std::fs::write(&gltf, doc.to_string()).unwrap();
        let r = run(&mut s, "import_mesh", json!({"path": gltf})).unwrap();
        assert_eq!((r["triangles"].as_u64(), r["watertight"].as_bool()), (Some(1), Some(false)));
        assert!((r["size"][2].as_f64().unwrap() - 1000.0).abs() < 1e-3, "a metre tall, y up turned to z up: {}", r["size"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mirror_turns_an_object_over_and_it_stays_a_solid() {
        let mut s = Session::new();
        // A bar, and a post beside its +x end.
        run(&mut s, "add_primitive", json!({"kind": "box", "size": [20, 4, 4]})).unwrap();
        run(&mut s, "add_primitive", json!({"kind": "box", "size": [4, 4, 10], "at": [8, 0]})).unwrap();
        let before = s.scene.get(1).unwrap().bounds().unwrap();
        run(&mut s, "mirror", json!({"id": 2, "axis": "x"})).unwrap();
        let o = s.scene.get(2).unwrap();
        assert!(o.world().is_watertight() && (o.world().volume() - 160.0).abs() < 1e-3);
        let (lo, hi) = o.bounds().unwrap();
        assert_eq!((lo[0], hi[0], lo[2]), (6.0, 10.0, 0.0), "mirrored about its own centre, still on the plate");
        assert_eq!(s.scene.get(1).unwrap().bounds().unwrap(), before);
        // A project keeps the mirrored transform and reads it back.
        let back = crate::project::read(&crate::project::write(&s.scene, &serde_json::Value::Null)).unwrap();
        assert!(back.scene.get(2).unwrap().xf.det() < 0.0 && back.scene.get(2).unwrap().world().is_watertight());
        assert!(run(&mut s, "mirror", json!({"id": 2, "axis": "w"})).is_err());
    }

    /// Whether two footprints overlap (touching edges do not).
    fn overlap(a: (P3, P3), b: (P3, P3)) -> bool {
        a.0[0] < b.1[0] - 1e-9 && b.0[0] < a.1[0] - 1e-9 && a.0[1] < b.1[1] - 1e-9 && b.0[1] < a.1[1] - 1e-9
    }

    #[test]
    fn arrange_lays_objects_out_on_the_plate_without_overlap() {
        let mut s = Session::new();
        for (kind, extra) in
            [("box", json!({"size": [40, 30, 10]})), ("cylinder", json!({"radius": 12, "height": 30})), ("sphere", json!({"radius": 9}))]
        {
            for _ in 0..3 {
                let mut p = extra.clone();
                p["kind"] = json!(kind);
                run(&mut s, "add_primitive", p).unwrap(); // all in the middle of the plate, on top of each other
            }
        }
        let r = run(&mut s, "arrange", json!({})).unwrap();
        assert_eq!(r["fits"], true);
        let boxes: Vec<(P3, P3)> = s.scene.objects.iter().map(|o| o.bounds().unwrap()).collect();
        for (i, a) in boxes.iter().enumerate() {
            assert!(a.0[0] >= -110.0 && a.1[0] <= 110.0 && a.0[1] >= -110.0 && a.1[1] <= 110.0, "on the plate: {a:?}");
            for b in &boxes[i + 1..] {
                assert!(!overlap(*a, *b), "{a:?} {b:?}");
            }
        }
        // A plate too small says so, and the layout is still made.
        assert_eq!(run(&mut s, "arrange", json!({"bed": [60, 60]})).unwrap()["fits"], false);
        assert!(run(&mut s, "arrange", json!({"gap": -1})).is_err());
        run(&mut s, "undo", json!({})).unwrap();
    }

    #[test]
    fn duplicate_copies_an_object_sharing_its_mesh_and_lays_the_plate_out() {
        let mut s = Session::new();
        run(&mut s, "add_primitive", json!({"kind": "cylinder", "radius": 10, "height": 5})).unwrap();
        let r = run(&mut s, "duplicate", json!({"id": 1, "count": 3})).unwrap();
        assert_eq!((r["ids"].clone(), r["fits"].clone()), (json!([2, 3, 4]), json!(true)));
        assert!(std::sync::Arc::ptr_eq(&s.scene.get(1).unwrap().mesh, &s.scene.get(4).unwrap().mesh));
        let boxes: Vec<(P3, P3)> = s.scene.objects.iter().map(|o| o.bounds().unwrap()).collect();
        assert!(boxes.iter().enumerate().all(|(i, a)| boxes[i + 1..].iter().all(|b| !overlap(*a, *b))));
        run(&mut s, "undo", json!({})).unwrap();
        assert_eq!(s.scene.objects.len(), 1, "one undo takes the copies back");
        assert!(run(&mut s, "duplicate", json!({"id": 1, "count": 0})).is_err());
    }

    #[test]
    fn paint_marks_the_faces_round_a_point_and_comes_off_again() {
        let mut s = Session::new();
        run(&mut s, "add_primitive", json!({"kind": "box", "size": [20, 20, 10]})).unwrap();
        let faces = s.scene.get(1).unwrap().mesh.triangles.len();
        // The box stands on the plate with its middle at the origin: (10, 0, 5) is the middle of its +x side.
        let r = run(&mut s, "paint", json!({"id": 1, "kind": "seam", "at": [10, 0, 5], "radius": 2})).unwrap();
        let n = r["painted"].as_u64().unwrap() as usize;
        assert!((1..=2).contains(&n), "only the side's faces under the brush: {n}");
        let o = s.scene.get(1).unwrap();
        let tags = o.paint.as_deref().unwrap();
        assert_eq!(tags.len(), faces);
        let world = o.world();
        for (i, &t) in tags.iter().enumerate() {
            let c = world.tri(i).unwrap();
            let on_the_side = c.iter().all(|q| (q[0] - 10.0).abs() < 1e-9);
            assert!(t == 0 || on_the_side, "face {i} painted off the +x side");
        }
        assert_eq!(run(&mut s, "info", json!({})).unwrap()["objects"][0]["painted"]["seam"], n);
        // A copy carries it; a project keeps it.
        run(&mut s, "duplicate", json!({"id": 1, "arrange": false})).unwrap();
        assert_eq!(s.scene.get(2).unwrap().paint, s.scene.get(1).unwrap().paint);
        let dir = std::env::temp_dir().join(format!("lamina-paint-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("painted.lamina");
        run(&mut s, "save", json!({"path": path.to_str().unwrap()})).unwrap();
        let mut t = Session::new();
        run(&mut t, "open", json!({"path": path.to_str().unwrap()})).unwrap();
        assert_eq!(t.scene.get(1).unwrap().paint, s.scene.get(1).unwrap().paint);
        std::fs::remove_dir_all(&dir).ok();
        // "none" at the point rubs it out; with no point it clears it all, and nothing painted is no paint at all.
        run(&mut s, "paint", json!({"id": 1, "kind": "support", "at": [0, 0, 0], "radius": 30})).unwrap();
        assert!(s.scene.get(1).unwrap().paint.as_deref().unwrap().iter().all(|&t| t == 2), "a big brush paints every face");
        run(&mut s, "paint", json!({"id": 1, "kind": "none", "at": [10, 0, 5], "radius": 2})).unwrap();
        assert_eq!(s.scene.get(1).unwrap().paint.as_deref().unwrap().iter().filter(|&&t| t == 0).count(), n);
        run(&mut s, "paint", json!({"id": 1, "kind": "none"})).unwrap();
        assert!(s.scene.get(1).unwrap().paint.is_none());
        run(&mut s, "undo", json!({})).unwrap();
        assert!(s.scene.get(1).unwrap().paint.is_some(), "each stroke is one undo step");
        // Simplify makes new triangles: the paint goes with the old ones.
        let ball = run(&mut s, "add_primitive", json!({"kind": "sphere", "radius": 10})).unwrap()["id"].as_u64().unwrap() as u32;
        run(&mut s, "paint", json!({"id": ball, "kind": "seam", "at": [0, 0, 0], "radius": 100})).unwrap();
        run(&mut s, "simplify", json!({"id": ball, "ratio": 0.5})).unwrap();
        assert!(s.scene.get(ball).unwrap().paint.is_none());
        for bad in [
            json!({"id": 1, "kind": "glitter", "at": [0, 0, 0]}),
            json!({"id": 1, "kind": "seam"}),
            json!({"id": 1, "kind": "seam", "at": [0, 0, 0], "radius": 0}),
            json!({"id": 1, "kind": "seam", "at": [0, 0, 1e300]}),
            json!({"id": 1, "kind": "seam", "at": [0, 0, 0], "size": 3}),
            json!({"id": 9, "kind": "seam", "at": [0, 0, 0]}),
        ] {
            assert!(run(&mut s, "paint", bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_project_with_paint_of_the_wrong_length_opens_without_it() {
        let mut s = Session::new();
        run(&mut s, "add_primitive", json!({"kind": "box", "size": [10, 10, 10]})).unwrap();
        run(&mut s, "paint", json!({"id": 1, "kind": "block", "at": [0, 0, 0], "radius": 100})).unwrap();
        let dir = std::env::temp_dir().join(format!("lamina-paint-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.lamina");
        run(&mut s, "save", json!({"path": path.to_str().unwrap()})).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["objects"][0]["paint"] = json!(crate::base64::encode(&[1, 2, 3]));
        std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
        let mut t = Session::new();
        let r = run(&mut t, "open", json!({"path": path.to_str().unwrap()})).unwrap();
        assert!(t.scene.get(1).unwrap().paint.is_none());
        assert!(r.to_string().contains("objects[0].paint"), "{r}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_face_laid_flat_ends_up_on_the_plate_facing_down() {
        let mut s = Session::new();
        // A pyramid lying on its base; laying one of its slanted sides flat tips it over onto that side.
        run(&mut s, "add_primitive", json!({"kind": "pyramid", "radius": 10, "height": 12, "sides": 4})).unwrap();
        let side = (0..s.scene.get(1).unwrap().mesh.triangles.len())
            .find(|&i| s.scene.get(1).unwrap().mesh.tri(i).is_some_and(|t| t.iter().any(|p| p[2] > 11.0)))
            .unwrap();
        let r = run(&mut s, "lay_flat", json!({"id": 1, "face": side})).unwrap();
        assert!(r["turned"].as_f64().unwrap() > 90.0 && r["turned"].as_f64().unwrap() < 180.0);
        let o = s.scene.get(1).unwrap();
        let w = o.mesh.tri(side).unwrap().map(|q| o.xf.apply(q));
        assert!(w.iter().all(|p| p[2].abs() < 1e-6), "the face is on the plate: {w:?}");
        assert!(o.bounds().unwrap().0[2].abs() < 1e-6 && o.world().volume() > 0.0);
        // The base, already down, stays; a face that does not exist is refused.
        assert!(run(&mut s, "lay_flat", json!({"id": 1, "face": 9999})).is_err());
    }

    #[test]
    fn measure_gives_the_distance_between_two_points() {
        let mut s = Session::new();
        let r = run(&mut s, "measure", json!({"a": [0, 0, 0], "b": [3, 4, 12]})).unwrap();
        assert_eq!((r["distance"].as_f64(), r["dz"].as_f64()), (Some(13.0), Some(12.0)));
        assert!(run(&mut s, "measure", json!({"a": [0, 0, 0], "b": [1, 2]})).is_err());
        assert!(!s.can_undo(), "measuring changes nothing");
    }

    #[test]
    fn an_outline_or_a_shape_with_holes_is_extruded_into_a_closed_solid() {
        let mut s = Session::new();
        let plate = json!({"shape": "rounded_rect", "size": [60, 40], "corner": 5, "height": 4,
            "round_holes": [[-22, -12, 4], [22, -12, 4], [22, 12, 4], [-22, 12, 4]], "name": "plate"});
        let r = run(&mut s, "extrude", plate).unwrap();
        assert_eq!((r["watertight"].as_bool(), r["size"].clone()), (Some(true), json!([60.0, 40.0, 4.0])));
        let bracket = json!({"outline": [[0, 0], [30, 0], [30, 5], [5, 5], [5, 25], [0, 25]], "height": 10,
            "holes": [[[15, 1.5], [20, 1.5], [20, 3.5], [15, 3.5]]], "at": [50, 0]});
        assert_eq!(run(&mut s, "extrude", bracket).unwrap()["watertight"], true);
        assert_eq!(run(&mut s, "extrude", json!({"shape": "slot", "size": [20, 6], "height": 2})).unwrap()["watertight"], true);
        assert_eq!(
            run(&mut s, "extrude", json!({"shape": "circle", "radius": 8, "height": 3, "round_holes": [[0, 0, 3]]})).unwrap()["watertight"],
            true
        );
        for bad in [
            json!({"height": 5}),
            json!({"shape": "star", "height": 5}),
            json!({"shape": "rounded_rect", "size": [10, 10], "corner": 6, "height": 5}),
            json!({"outline": [[0, 0], [1, 0]], "height": 5}),
            json!({"outline": [[0, 0], [1, 0], [0, 1]], "shape": "slot", "height": 5}),
            json!({"shape": "circle", "radius": 5, "height": -1}),
        ] {
            assert!(run(&mut s, "extrude", bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(s.scene.objects.len(), 4);
    }

    #[test]
    fn a_built_part_keeps_its_recipe_and_an_edit_rebuilds_it_in_place() {
        let mut s = Session::new();
        // A plate with a hole, then two steps that place it (the core alone has no booleans: lamina-csg's tests cut holes in recipes).
        let recipe = json!([
            {"command": "extrude", "params": {"shape": "rounded_rect", "size": [40, 20], "corner": 3, "height": 4, "round_holes": [[-14, 0, 3]]}},
            {"command": "scale", "params": {"id": 1, "by": [1, 1, 1]}},
            {"command": "move", "params": {"id": 1, "by": [5, 0, 0]}},
        ]);
        let r = run(&mut s, "build", json!({"recipe": recipe, "name": "plate", "at": [30, 30]})).unwrap();
        assert_eq!((r["id"].as_u64(), r["size"].clone()), (Some(1), json!([40.0, 20.0, 4.0])));
        let info = run(&mut s, "info", json!({})).unwrap();
        assert_eq!(info["objects"][0]["recipe"][0]["command"], "extrude");
        let before = s.scene.get(1).unwrap().bounds().unwrap();
        // Make the hole bigger and the plate taller: one step changed, the part rebuilt where it stands, still on the plate.
        let r = run(&mut s, "edit", json!({"id": 1, "step": 0, "params": {"round_holes": [[-14, 0, 6]], "height": 6}})).unwrap();
        assert_eq!(r["size"], json!([40.0, 20.0, 6.0]));
        let after = s.scene.get(1).unwrap().bounds().unwrap();
        assert!((after.0[0] - before.0[0]).abs() < 1e-6 && (after.0[1] - before.0[1]).abs() < 1e-6 && after.0[2].abs() < 1e-9);
        assert_eq!(s.scene.get(1).unwrap().recipe.as_ref().unwrap()[0]["params"]["height"], 6);
        // A project keeps the recipe, so the part can still be edited after it is opened again.
        let back = crate::project::read(&crate::project::write(&s.scene, &Value::Null)).unwrap();
        assert_eq!(back.scene.get(1).unwrap().recipe, s.scene.get(1).unwrap().recipe);
        // Undo takes the edit back; a bad edit changes nothing.
        run(&mut s, "undo", json!({})).unwrap();
        assert_eq!(s.scene.get(1).unwrap().bounds().unwrap(), before);
        for bad in [
            json!({"id": 1, "step": 9, "params": {}}),
            json!({"id": 1, "step": 0, "params": {"height": -1}}),
            json!({"id": 7, "step": 0, "params": {}}),
        ] {
            assert!(run(&mut s, "edit", bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(s.scene.get(1).unwrap().bounds().unwrap(), before);
    }

    #[test]
    fn a_recipe_must_be_safe_and_end_with_one_object() {
        let mut s = Session::new();
        let shape = json!({"command": "add_primitive", "params": {"kind": "box", "size": [5, 5, 5]}});
        for bad in [
            json!([]),
            json!([shape, shape]),
            json!([{"command": "export_mesh", "params": {"path": "/tmp/x.stl"}}]),
            json!([shape, {"command": "build", "params": {"recipe": [shape]}}]),
            json!([shape, {"command": "new"}]),
            json!([{"command": "add_primitive", "params": {"kind": "box"}}]),
            json!([{"command": "nope"}]),
            json!([{"command": "add_primitive", "params": {}, "extra": 1}]),
            json!("not a list"),
        ] {
            assert!(run(&mut s, "build", json!({"recipe": bad})).is_err(), "{bad}");
        }
        assert!(s.scene.objects.is_empty());
        let mut box_made = Mesh::cuboid([1.0, 1.0, 1.0]);
        box_made.triangles.truncate(12);
        s.scene_mut().add("plain", box_made).unwrap();
        assert!(run(&mut s, "edit", json!({"id": 1, "step": 0, "params": {}})).unwrap_err().contains("no recipe"));
    }

    #[test]
    fn a_script_stops_at_the_line_that_fails() {
        let mut s = Session::new();
        let err = s.script("# a plate\nadd_primitive {\"kind\":\"box\",\"size\":[5,5,5]}\nmove {\"id\":9,\"by\":[0,0,0]}\n").unwrap_err();
        assert!(err.starts_with("line 3:"), "{err}");
    }

    #[test]
    fn a_modifier_is_kept_in_projects_shown_by_info_and_left_where_it_is_by_arrange() {
        let mut s = Session::new();
        s.execute("add_primitive", &json!({"kind": "box", "size": [20, 20, 10]})).unwrap();
        s.execute("add_primitive", &json!({"kind": "box", "size": [5, 5, 5], "at": [3, 3]})).unwrap();
        s.execute("set_modifier", &json!({"id": 2, "kind": "support_blocker"})).unwrap();
        assert_eq!(s.execute("info", &json!({})).unwrap()["objects"][1]["modifier"], "support_blocker");
        let before = s.scene.get(2).unwrap().bounds();
        s.execute("arrange", &json!({})).unwrap();
        assert_eq!(s.scene.get(2).unwrap().bounds(), before, "arrange leaves the modifier on its part");
        let path = std::env::temp_dir().join(format!("lamina-modifier-{}.lamina", std::process::id()));
        s.execute("save", &json!({"path": path})).unwrap();
        let mut t = Session::new();
        t.execute("open", &json!({"path": path})).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(t.scene.get(2).unwrap().modifier, Some(crate::Modifier::SupportBlocker));
        assert_eq!(t.scene.get(1).unwrap().modifier, None);
        s.execute("set_modifier", &json!({"id": 2, "kind": "none"})).unwrap();
        assert_eq!(s.scene.get(2).unwrap().modifier, None);
        assert!(s.execute("set_modifier", &json!({"id": 2, "kind": "infill"})).unwrap_err().contains("support_enforcer"));
        assert!(s.execute("set_modifier", &json!({"id": 9, "kind": "none"})).is_err());
        s.undo().unwrap();
        assert_eq!(s.scene.get(2).unwrap().modifier, Some(crate::Modifier::SupportBlocker), "one undo step");
    }

    #[test]
    fn reset_transform_clears_location_turn_or_scale_and_keeps_the_rest() {
        let mut s = Session::new();
        s.execute("add_primitive", &json!({"kind": "box", "size": [20, 20, 10]})).unwrap();
        let size = |s: &Session| {
            let (lo, hi) = s.scene.get(1).unwrap().bounds().unwrap();
            [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]
        };
        let middle = |s: &Session| {
            let (lo, hi) = s.scene.get(1).unwrap().bounds().unwrap();
            [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]]
        };
        let near = |a: [f64; 3], b: [f64; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-4);
        s.execute("scale", &json!({"id": 1, "by": [2, 1, 1]})).unwrap();
        s.execute("rotate", &json!({"id": 1, "axis": [0, 0, 1], "degrees": 30})).unwrap();
        s.execute("move", &json!({"id": 1, "by": [30, 10, 0]})).unwrap();
        let (turned, at) = (size(&s), middle(&s));
        assert!(near(turned, [40.0 * 30f64.to_radians().cos() + 20.0 * 0.5, 40.0 * 0.5 + 20.0 * 30f64.to_radians().cos(), 10.0]));
        // Its turn taken away: 40 x 20 again, where it stood.
        s.execute("reset_transform", &json!({"id": 1, "what": "rotation"})).unwrap();
        assert!(near(size(&s), [40.0, 20.0, 10.0]) && near(middle(&s), at), "{:?} {:?}", size(&s), middle(&s));
        s.undo().unwrap();
        // Its scale taken away: the 20 mm square, still turned 30 degrees.
        s.execute("reset_transform", &json!({"id": 1, "what": "scale"})).unwrap();
        let w = 20.0 * (30f64.to_radians().cos() + 0.5);
        assert!(near(size(&s), [w, w, 10.0]) && near(middle(&s), at), "{:?}", size(&s));
        s.undo().unwrap();
        s.execute("reset_transform", &json!({"id": 1, "what": "location"})).unwrap();
        assert!(near(middle(&s), [0.0, 0.0, 0.0]) && near(size(&s), turned));
        s.execute("reset_transform", &json!({"id": 1, "what": "all"})).unwrap();
        assert!(near(size(&s), [20.0, 20.0, 10.0]) && near(middle(&s), [0.0, 0.0, 0.0]));
        // A mirror stays when the turn goes.
        s.execute("mirror", &json!({"id": 1, "axis": "x"})).unwrap();
        s.execute("rotate", &json!({"id": 1, "axis": "z", "degrees": 45})).unwrap();
        s.execute("reset_transform", &json!({"id": 1, "what": "rotation"})).unwrap();
        assert!(s.scene.get(1).unwrap().xf.det() < 0.0 && near(size(&s), [20.0, 20.0, 10.0]));
        // A named axis and its vector turn alike; bad requests change nothing.
        let mut t = Session::new();
        t.execute("add_primitive", &json!({"kind": "box", "size": [30, 10, 10]})).unwrap();
        t.execute("rotate", &json!({"id": 1, "axis": [0, 0, 2], "degrees": 90})).unwrap();
        assert!(near(size(&t), [10.0, 30.0, 10.0]));
        for bad in [json!({"id": 1, "what": "colour"}), json!({"id": 7, "what": "all"})] {
            assert!(t.execute("reset_transform", &bad).is_err());
        }
        assert!(t.execute("rotate", &json!({"id": 1, "axis": [0, 0, 0], "degrees": 9})).is_err());
        assert!(t.execute("rotate", &json!({"id": 1, "axis": "w", "degrees": 9})).is_err());
    }
}
