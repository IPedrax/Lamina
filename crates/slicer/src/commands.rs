//! The slicer's commands, which an app adds to a session with `session.register(lamina_slicer::COMMANDS)`.

use lamina_core::{Command, Files, Mesh, Session};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::presets::{self, FILAMENTS, PRINTERS, PROCESSES};
use crate::print;
use crate::profile::Profile;

/// The largest profile file read, bytes.
const MAX_PROFILE_FILE: u64 = 1 << 20;

pub static COMMANDS: &[Command] = &[
    Command {
        id: "slice",
        params: "{path, ids?, printer?, filament?, process?, profile?, profile_path?} slices the objects on the plate (all, or `ids`) to G-code at `path`. \
                 Settings are laid over the defaults in this order: the project's print settings (from `open`, or the window's), `profile_path`'s JSON file, the `printer`, `filament` and `process` presets by name \
                 (see `slicer_presets`), then `profile`, an object of the fields that differ (see `slicer_profile`). \
                 Returns {path, layers, filament_mm, print_seconds, warnings}. It refuses an object that is off the plate, below it or too tall, and says which",
        run: slice,
        undoable: false,
        files: Files::Write,
    },
    Command {
        id: "slice_check",
        params: "{ids?, printer?, filament?, process?, profile?} the same slice as `slice`, with no file written: what would print and whether it can. Returns {layers, filament_mm, print_seconds, print_time, warnings, features (filament mm by kind: outer-wall, inner-wall, solid-infill, sparse-infill, skirt, brim, support, support-interface, raft, purge-tower, purge)}, or an error saying what does not fit the plate (off it, below it, too tall) or what a setting must be. Use it to check a model before the user exports it",
        run: slice_check,
        undoable: false,
        files: Files::None,
    },
    Command {
        id: "set_object_settings",
        params: "{id, profile} gives one object slicer settings of its own, laid over the plate's for it alone: perimeters, top_layers, \
                 bottom_layers, infill_density, infill_pattern, seam, perimeter_speed, infill_speed, the support fields, ironing, fuzzy_skin \
                 and `extruder` (from 1: which extruder prints it, up to the printer's `extruders`). `profile: {}` or null clears them. The printer, the filament and the layer heights are the plate's. `info` shows each object's settings",
        run: set_object_settings,
        undoable: true,
        files: Files::None,
    },
    Command {
        id: "slicer_presets",
        params: "{} the built-in presets: {printers:[{name, about, fields}], filaments:[...], processes:[...]}. Pass a name as `printer`, \
                 `filament` or `process` to `slice` or `slice_check`; a printer sets the machine's fields, a filament its temperatures and fan, \
                 a process the layers and the strength (\"0.12 mm Fine\", \"0.20 mm Standard\", \"0.28 mm Draft\", \"Strong\")",
        run: list_presets,
        undoable: false,
        files: Files::None,
    },
    Command {
        id: "import_profile",
        params: "{path} reads another slicer's profile: a PrusaSlicer `.ini`, or an OrcaSlicer or Bambu Studio `.json` preset. Returns \
                 {fields (Lamina's settings it gives, to pass as `profile`), left_out (each key Lamina has no setting for, or whose value it cannot \
                 take, with why)}. A start or end G-code with the other program's placeholders is left out. Changes nothing",
        run: import_profile,
        undoable: false,
        files: Files::Read,
    },
    Command {
        id: "set_height_range",
        params: "{id, from, to (mm above the plate), profile} gives the object's layers whose middle is from `from` up to `to` settings of their own, laid over the object's: walls (perimeters, wall_generator), infill_density, infill_pattern, seam, perimeter_speed, infill_speed, ironing, fuzzy_skin, and nozzle_temp, fan_speed, pressure_advance, retract_length, retract_speed, flow_ratio, max_volumetric_speed (a temperature, pressure advance or retraction tower). Ranges may not overlap; the same from and to replaces a range, and `profile: {}` removes it. `inspect_scene` shows them (ranges). Returns {ranges}",
        run: set_height_range,
        undoable: true,
        files: Files::None,
    },
    Command {
        id: "calibration",
        params: "{kind: \"temperature\" | \"retraction\" | \"pressure_advance\" | \"flow\", from?, to?, step?} puts a calibration part on the plate with its settings by height, as one undo step: a temperature tower (blocks 8 mm tall with a 45 degree overhang each, nozzle_temp from `from` at the bottom to `to`, default 230 to 190 by 5), two retraction towers (retract_length by 5 mm, default 0.4 to 2.0 by 0.4), a hollow square pressure advance tower (pressure_advance by 1 mm, default 0 to 0.1 by 0.005) or a row of flow blocks (flow_ratio, left to right, default 0.90 to 1.10 by 0.05). Returns {ids, steps: [{from, to, value}] (heights in mm, or for flow the blocks left to right), look (what to judge)}",
        run: calibration,
        undoable: true,
        files: Files::None,
    },
    Command {
        id: "slicer_profile",
        params: "{} every profile field with its default value: the printer (bed, nozzle, start_gcode), the process (layer_height, perimeters, infill_density) and speeds, temperatures and retraction",
        run: profile,
        undoable: false,
        files: Files::None,
    },
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    path: String,
    ids: Option<Vec<u32>>,
    printer: Option<String>,
    filament: Option<String>,
    process: Option<String>,
    profile: Option<Value>,
    profile_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    ids: Option<Vec<u32>>,
    printer: Option<String>,
    filament: Option<String>,
    process: Option<String>,
    profile: Option<Value>,
}

/// The settings a slice asks for, in the order they are laid over the defaults.
struct Settings<'a> {
    file: Option<&'a str>,
    printer: Option<&'a str>,
    filament: Option<&'a str>,
    process: Option<&'a str>,
    over: Option<Value>,
}

/// Slices the objects named (or all) with the settings: the defaults, the project's, then the file's, the presets', and the fields given.
fn run(s: &Session, ids: Option<&[u32]>, settings: Settings) -> Result<print::Output, String> {
    let merged = match settings.file {
        Some(file) => {
            if std::fs::metadata(file).map_err(|e| format!("{file}: {e}"))?.len() > MAX_PROFILE_FILE {
                return Err(format!("{file} is larger than {MAX_PROFILE_FILE} bytes"));
            }
            serde_json::from_str::<Value>(&std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?)
                .map_err(|e| format!("{file}: {e}"))?
        }
        None => json!({}),
    };
    let Value::Object(file) = merged else { return Err("a profile file holds one JSON object".into()) };
    // Under everything, the print settings of the project (what `open` read, or what the window shows).
    let mut merged = match s.print.get("profile") {
        Some(Value::Object(project)) => Value::Object(project.clone()),
        _ => json!({}),
    };
    let Value::Object(base) = &mut merged else { return Err("internal: the base profile is an object".into()) };
    base.extend(file);
    for (name, list) in [(settings.printer, PRINTERS), (settings.filament, FILAMENTS), (settings.process, PROCESSES)] {
        if let Some(name) = name {
            base.extend(presets::find(list, name)?.fields());
        }
    }
    match settings.over {
        Some(Value::Object(over)) => base.extend(over),
        Some(Value::Null) | None => {}
        Some(_) => return Err("`profile` is an object".into()),
    }
    let profile = Profile::from_json(&merged)?;
    let chosen: Vec<&lamina_core::Object> = match ids {
        Some(ids) => ids.iter().map(|&id| s.scene.get(id)).collect::<Result<_, _>>()?,
        None => s.scene.objects.iter().collect(),
    };
    Job::of(&chosen, &profile)?.gcode(&profile)
}

/// What the slicer needs from the plate, gathered from its objects and owned, so a thread can take it: the parts where they stand
/// with their own settings, settings by height and paint, and the modifiers.
#[derive(Clone, Default)]
pub struct Job {
    pub objects: Vec<(u32, Mesh)>,
    pub own: Vec<Option<Profile>>,
    pub ranges: print::RangeProfiles,
    pub paint: Vec<Option<std::sync::Arc<Vec<u8>>>>,
    pub modifiers: Vec<(lamina_core::Modifier, Mesh)>,
}

impl Job {
    /// The job for these objects with the plate's `profile` under each object's own settings.
    pub fn of(objects: &[&lamina_core::Object], profile: &Profile) -> Result<Job, String> {
        let (parts, modifiers) = split(objects);
        let own = own(&parts, profile)?;
        Ok(Job {
            objects: parts.iter().map(|o| (o.id, o.world())).collect(),
            ranges: range_profiles(&parts, &own, profile)?,
            paint: parts.iter().map(|o| o.paint.clone()).collect(),
            own,
            modifiers,
        })
    }

    pub fn plate(&self) -> print::Plate<'_> {
        print::Plate { objects: &self.objects, own: &self.own, ranges: &self.ranges, paint: &self.paint, modifiers: &self.modifiers }
    }

    pub fn gcode(&self, profile: &Profile) -> Result<print::Output, String> {
        print::gcode_plate(&self.plate(), profile)
    }
}

/// The objects to print, and the modifiers (support enforcers and blockers) where they stand, which print nothing.
pub fn split<'a>(objects: &[&'a lamina_core::Object]) -> (Vec<&'a lamina_core::Object>, Vec<(lamina_core::Modifier, Mesh)>) {
    let parts = objects.iter().filter(|o| o.modifier.is_none()).copied().collect();
    let modifiers = objects.iter().filter_map(|o| o.modifier.map(|m| (m, o.world()))).collect();
    (parts, modifiers)
}

/// The fields an object may set for itself: what it is made of and how, never the machine, the filament or the layer heights.
pub const PER_OBJECT_FIELDS: &[&str] = &[
    "perimeters",
    "wall_generator",
    "top_layers",
    "bottom_layers",
    "infill_density",
    "infill_pattern",
    "seam",
    "perimeter_speed",
    "infill_speed",
    "support",
    "support_angle",
    "support_density",
    "support_z_gap",
    "support_xy_gap",
    "support_interface_layers",
    "support_style",
    "tree_tip_spacing",
    "tree_branch_angle",
    "tree_max_radius",
    "ironing",
    "fuzzy_skin",
    "extruder",
];

/// What a height range may set: what a layer's plan or the writer reads, never the layer heights, the supports (planned for the whole
/// object) or the skins (counted across layers).
pub const RANGE_FIELDS: &[&str] = &[
    "perimeters",
    "wall_generator",
    "infill_density",
    "infill_pattern",
    "seam",
    "perimeter_speed",
    "infill_speed",
    "ironing",
    "fuzzy_skin",
    "nozzle_temp",
    "fan_speed",
    "pressure_advance",
    "retract_length",
    "retract_speed",
    "flow_ratio",
    "max_volumetric_speed",
];

/// The most ranges one object may have.
pub const MAX_RANGES: usize = 200;

/// One height range as it is kept on an object.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    pub from: f64,
    pub to: f64,
    pub profile: serde_json::Map<String, Value>,
}

/// An object's height ranges, checked: within limits, not overlapping, each field one a range may set. None for an object with none.
pub fn ranges_of(o: &lamina_core::Object) -> Result<Vec<Range>, String> {
    let Some(v) = o.ranges.as_deref() else { return Ok(Vec::new()) };
    let ranges: Vec<Range> = serde_json::from_value(v.clone()).map_err(|e| format!("object {}'s height ranges: {e}", o.id))?;
    check_ranges(&ranges).map_err(|e| format!("object {}'s height ranges: {e}", o.id))?;
    Ok(ranges)
}

fn check_ranges(ranges: &[Range]) -> Result<(), String> {
    if ranges.len() > MAX_RANGES {
        return Err(format!("more than {MAX_RANGES} ranges"));
    }
    for r in ranges {
        if !(r.from.is_finite() && r.to.is_finite() && r.from >= 0.0 && r.to > r.from && r.to <= 10_000.0) {
            return Err(format!("a range from {} to {} mm: `to` must be above `from`, both from 0 to 10000", r.from, r.to));
        }
        if let Some(k) = r.profile.keys().find(|k| !RANGE_FIELDS.contains(&k.as_str())) {
            return Err(format!("`{k}` cannot change by height; a range may set {}", RANGE_FIELDS.join(", ")));
        }
        presets::apply(&Profile { extruders: crate::profile::MAX_EXTRUDERS, ..Profile::default() }, &r.profile)?;
    }
    for (i, a) in ranges.iter().enumerate() {
        if let Some(b) = ranges[i + 1..].iter().find(|b| a.from < b.to && b.from < a.to) {
            return Err(format!("the range from {} to {} mm overlaps the one from {} to {} mm", a.from, a.to, b.from, b.to));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetRange {
    id: u32,
    from: f64,
    to: f64,
    profile: serde_json::Map<String, Value>,
}

fn set_height_range(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = SetRange::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    let o = s.scene.get(a.id)?;
    let mut ranges = ranges_of(o)?;
    ranges.retain(|r| !(r.from == a.from && r.to == a.to));
    if !a.profile.is_empty() {
        ranges.push(Range { from: a.from, to: a.to, profile: a.profile });
    }
    ranges.sort_by(|x, y| x.from.total_cmp(&y.from));
    check_ranges(&ranges)?;
    let value = serde_json::to_value(&ranges).map_err(|e| e.to_string())?;
    let o = s.scene_mut().get_mut(a.id)?;
    o.ranges = if ranges.is_empty() { None } else { Some(std::sync::Arc::new(value.clone())) };
    Ok(json!({ "ranges": value }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Calibrate {
    kind: String,
    from: Option<f64>,
    to: Option<f64>,
    step: Option<f64>,
}

/// Puts a mesh on the plate stood as it is, its footprint in the middle of the plate shifted by `x`, its bottom on the plate.
fn place(s: &mut Session, name: &str, mesh: Mesh, turn: lamina_core::Transform, x: f64) -> Result<u32, String> {
    use lamina_core::Transform;
    let (lo, hi) = mesh.transformed(&turn).bounds().ok_or("internal: an empty calibration part")?;
    let to = Transform::translate([x - (lo[0] + hi[0]) / 2.0, -(lo[1] + hi[1]) / 2.0, -lo[2]]);
    s.scene_mut().add_transformed(name, mesh, turn.then(&to))
}

/// Calibration parts: each kind's part, and the values its heights (or blocks) print with.
fn calibration(s: &mut Session, p: &Value) -> Result<Value, String> {
    use lamina_core::Transform;
    let a = Calibrate::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    let (field, from, to, step, most, look) = match a.kind.as_str() {
        "temperature" => (
            "nozzle_temp",
            230.0,
            190.0,
            5.0,
            12,
            "the coolest block whose layers still bond (try to snap it) with a clean overhang and little stringing",
        ),
        "retraction" => ("retract_length", 0.4, 2.0, 0.4, 12, "the lowest height band with no hairs between the towers"),
        "pressure_advance" => {
            ("pressure_advance", 0.0, 0.1, 0.005, 40, "the height where the corners are sharpest, neither bulging nor rounded: its value")
        }
        "flow" => ("flow_ratio", 0.9, 1.1, 0.05, 9, "the block whose top is smooth and closed with no ridges: its flow ratio"),
        other => return Err(format!("`kind` is temperature, retraction, pressure_advance or flow, not `{other}`")),
    };
    let (from, to, step) = (a.from.unwrap_or(from), a.to.unwrap_or(to), a.step.unwrap_or(step).abs());
    if !(from.is_finite() && to.is_finite() && step.is_finite() && step > 0.0) {
        return Err("`from`, `to` and `step` must be finite numbers, the step above 0".into());
    }
    let count = ((to - from).abs() / step).round() as usize + 1;
    if count > most || count < 2 {
        return Err(format!("{count} steps: a {} test takes from 2 to {most}; change `step`", a.kind));
    }
    let values: Vec<f64> = (0..count).map(|i| from + (to - from).signum() * step * i as f64).collect();
    let mut steps = Vec::new();
    let mut ids = Vec::new();
    let ranges = |s: &mut Session, id: u32, cuts: &[(f64, f64, f64)]| -> Result<(), String> {
        let list: Vec<Range> = cuts
            .iter()
            .map(|&(lo, hi, v)| Range { from: lo, to: hi, profile: serde_json::Map::from_iter([(field.to_string(), json!(v))]) })
            .collect();
        check_ranges(&list)?;
        s.scene_mut().get_mut(id)?.ranges = Some(std::sync::Arc::new(serde_json::to_value(&list).map_err(|e| e.to_string())?));
        Ok(())
    };
    match a.kind.as_str() {
        "temperature" => {
            // The side seen from the front: a block 20 mm wide for each value, each with a 45 degree overhang on its right.
            let h = 8.0;
            let mut outline = vec![[0.0, 0.0], [20.0, 0.0]];
            for i in 0..count {
                let b = i as f64 * h;
                outline.extend([[20.0, b + h / 2.0], [20.0 + h / 2.0, b + h], [20.0, b + h]]);
            }
            outline.push([0.0, count as f64 * h]);
            outline.dedup();
            let mesh = lamina_core::sketch::extrude(&outline, &[], 10.0)?;
            let id = place(s, "temperature tower", mesh, Transform::rotate(0, 90.0), 0.0)?;
            let cuts: Vec<(f64, f64, f64)> = values.iter().enumerate().map(|(i, &v)| (i as f64 * h, (i + 1) as f64 * h, v)).collect();
            ranges(s, id, &cuts)?;
            ids.push(id);
            steps = cuts.iter().map(|c| json!({"from": c.0, "to": c.1, "value": c.2})).collect();
        }
        "retraction" | "pressure_advance" => {
            let h = if a.kind == "retraction" { 5.0 } else { 1.0 };
            let cuts: Vec<(f64, f64, f64)> = values.iter().enumerate().map(|(i, &v)| (i as f64 * h, (i + 1) as f64 * h, v)).collect();
            let tall = count as f64 * h;
            let parts: Vec<(Mesh, f64)> = if a.kind == "retraction" {
                vec![(Mesh::cylinder(3.0, tall, 32), -20.0), (Mesh::cylinder(3.0, tall, 32), 20.0)]
            } else {
                // A hollow square, walls 1.2 mm: sharp corners at every height.
                let hole: Vec<[f64; 2]> = vec![[1.2, 1.2], [1.2, 38.8], [38.8, 38.8], [38.8, 1.2]];
                vec![(lamina_core::sketch::extrude(&[[0.0, 0.0], [40.0, 0.0], [40.0, 40.0], [0.0, 40.0]], &[hole], tall)?, 0.0)]
            };
            for (k, (mesh, x)) in parts.into_iter().enumerate() {
                let name = if a.kind == "retraction" { format!("retraction tower {}", k + 1) } else { "pressure advance tower".into() };
                let id = place(s, &name, mesh, Transform::IDENTITY, x)?;
                ranges(s, id, &cuts)?;
                ids.push(id);
            }
            steps = cuts.iter().map(|c| json!({"from": c.0, "to": c.1, "value": c.2})).collect();
        }
        _ => {
            // Flow: a block 20 x 20 x 3 mm for each value, left to right, each one range from its bottom to its top.
            for (i, &v) in values.iter().enumerate() {
                let x = (i as f64 - (count as f64 - 1.0) / 2.0) * 25.0;
                let id = place(s, &format!("flow {v:.2}"), Mesh::cuboid([20.0, 20.0, 3.0]), Transform::IDENTITY, x)?;
                ranges(s, id, &[(0.0, 3.0, v)])?;
                ids.push(id);
                steps.push(json!({"from": 0.0, "to": 3.0, "value": v}));
            }
        }
    }
    Ok(json!({ "ids": ids, "steps": steps, "look": look }))
}

/// Each object's height ranges as profiles: each range's fields laid over the object's own profile (or the plate's).
pub fn range_profiles(objects: &[&lamina_core::Object], own: &[Option<Profile>], plate: &Profile) -> Result<print::RangeProfiles, String> {
    objects
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let base = own.get(i).and_then(Option::as_ref).unwrap_or(plate);
            ranges_of(o)?.into_iter().map(|r| Ok((r.from, r.to, presets::apply(base, &r.profile)?))).collect::<Result<Vec<_>, String>>()
        })
        .collect()
}

/// Each object's own profile, its settings laid over the plate's, or none where it has none.
pub fn own(objects: &[&lamina_core::Object], plate: &Profile) -> Result<Vec<Option<Profile>>, String> {
    objects
        .iter()
        .map(|o| match o.settings.as_deref() {
            Some(Value::Object(fields)) => {
                presets::apply(plate, fields).map(Some).map_err(|e| format!("object {}'s own settings: {e}", o.id))
            }
            _ => Ok(None),
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectSettings {
    id: u32,
    profile: Option<serde_json::Map<String, Value>>,
}

fn set_object_settings(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = ObjectSettings::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    s.scene.get(a.id)?;
    let fields = a.profile.unwrap_or_default();
    if let Some(k) = fields.keys().find(|k| !PER_OBJECT_FIELDS.contains(&k.as_str())) {
        return Err(format!("`{k}` is the plate's, not an object's; an object may set {}", PER_OBJECT_FIELDS.join(", ")));
    }
    // The printer's extruder count is the slice's, checked then; here any extruder a printer may have.
    presets::apply(&Profile { extruders: crate::profile::MAX_EXTRUDERS, ..Profile::default() }, &fields)?;
    let o = s.scene_mut().get_mut(a.id)?;
    o.settings = if fields.is_empty() { None } else { Some(std::sync::Arc::new(Value::Object(fields))) };
    Ok(json!({}))
}

/// 3725 seconds as "1h 02m".
fn clock(seconds: f64) -> String {
    let m = (seconds / 60.0).round() as u64;
    if m >= 60 { format!("{}h {:02}m", m / 60, m % 60) } else { format!("{m}m") }
}

fn summary(out: &print::Output) -> Value {
    let features: serde_json::Map<String, Value> =
        out.features.iter().map(|(k, v)| (k.to_string(), json!((v * 10.0).round() / 10.0))).collect();
    json!({
        "layers": out.layers, "filament_mm": out.filament_mm, "print_seconds": out.seconds.round(),
        "print_time": clock(out.seconds), "warnings": out.warnings, "features": features,
    })
}

fn slice(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = Args::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    let settings = Settings {
        file: a.profile_path.as_deref(),
        printer: a.printer.as_deref(),
        filament: a.filament.as_deref(),
        process: a.process.as_deref(),
        over: a.profile,
    };
    let out = run(s, a.ids.as_deref(), settings)?;
    std::fs::write(&a.path, &out.gcode).map_err(|e| format!("{}: {e}", a.path))?;
    let mut v = summary(&out);
    v["path"] = json!(a.path);
    Ok(v)
}

fn slice_check(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = Check::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    let settings = Settings {
        file: None,
        printer: a.printer.as_deref(),
        filament: a.filament.as_deref(),
        process: a.process.as_deref(),
        over: a.profile,
    };
    run(s, a.ids.as_deref(), settings).map(|out| summary(&out))
}

fn list_presets(_: &mut Session, _: &Value) -> Result<Value, String> {
    let list = |l: &[presets::Preset]| -> Vec<Value> {
        l.iter().map(|p| json!({"name": p.name, "about": p.about, "fields": p.fields()})).collect()
    };
    Ok(json!({"printers": list(PRINTERS), "filaments": list(FILAMENTS), "processes": list(PROCESSES)}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathOnly {
    path: String,
}

fn import_profile(_: &mut Session, p: &Value) -> Result<Value, String> {
    let a = PathOnly::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    let size = std::fs::metadata(&a.path).map_err(|e| format!("{}: {e}", a.path))?.len();
    if size > crate::import::MAX_FILE as u64 {
        return Err(format!("{}: larger than a profile ({} MB at most)", a.path, crate::import::MAX_FILE >> 20));
    }
    let text = std::fs::read_to_string(&a.path).map_err(|e| format!("{}: {e}", a.path))?;
    let ini = a.path.to_ascii_lowercase().ends_with(".ini");
    let r = crate::import::read(&text, ini)?;
    Ok(json!({"fields": r.fields, "left_out": r.left_out}))
}

fn profile(_: &mut Session, _: &Value) -> Result<Value, String> {
    serde_json::to_value(Profile::default()).map_err(|e| e.to_string())
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
    fn a_session_models_then_slices_to_a_file() {
        let path = std::env::temp_dir().join(format!("lamina-slice-{}.gcode", std::process::id()));
        let path = path.to_string_lossy().into_owned();
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "cylinder", "radius": 8, "height": 12})).unwrap();
        let r = s
            .execute("slice", &json!({"path": path, "profile": {"infill_density": 0.3, "layer_height": 0.24, "first_layer_height": 0.24}}))
            .unwrap();
        let g = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(r["layers"], 50);
        assert!(g.contains(";LAYER:49") && !g.contains(";LAYER:50"));
        assert!(r["filament_mm"].as_f64().unwrap() > 100.0);
        assert!(s.execute("slicer_profile", &json!({})).unwrap()["nozzle"] == 0.4);
    }

    #[test]
    fn slice_check_says_what_would_print_without_writing_a_file() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [20, 20, 20]})).unwrap();
        let r = s.execute("slice_check", &json!({})).unwrap();
        assert_eq!(r["layers"], 100);
        assert!(r["print_time"].as_str().unwrap().contains('m') && r["filament_mm"].as_f64().unwrap() > 100.0);
        // It is not a file command, so the chat may run it, and it says why a model cannot print.
        assert_eq!(s.find("slice_check").unwrap().files, Files::None);
        s.execute("scale", &json!({"id": 1, "factor": 20})).unwrap();
        assert!(s.execute("slice_check", &json!({})).unwrap_err().contains("does not fit"));
        assert!(s.execute("slice_check", &json!({"path": "/tmp/x"})).is_err());
    }

    #[test]
    fn presets_go_in_by_name_and_the_fields_given_win() {
        let path = std::env::temp_dir().join(format!("lamina-presets-{}.gcode", std::process::id())).to_string_lossy().into_owned();
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [10, 10, 4]})).unwrap();
        s.execute("slice", &json!({"path": path, "printer": "Generic 300 mm, Klipper", "filament": "petg", "profile": {"bed_temp": 85}}))
            .unwrap();
        let g = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(g.contains("PRINT_START BED=80 EXTRUDER=245"), "the printer's start G-code with the filament's first layer");
        assert!(g.contains("M140 S85"), "the bed temperature given wins over the filament's 80");
        assert!(g.contains("X150."), "a 300 mm plate puts the middle at 150");
        assert!(s.execute("slice_check", &json!({"filament": "wood"})).unwrap_err().contains("there are: PLA"));
        let listed = s.execute("slicer_presets", &json!({})).unwrap();
        assert!(listed["filaments"].as_array().unwrap().iter().any(|f| f["name"] == "ABS" && f["fields"]["fan_speed"] == 0));
    }

    #[test]
    fn a_slice_starts_from_the_print_settings_of_the_project() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [10, 10, 6]})).unwrap();
        assert_eq!(s.execute("slice_check", &json!({})).unwrap()["layers"], 30);
        s.print = json!({"profile": {"layer_height": 0.3, "first_layer_height": 0.3}});
        assert_eq!(s.execute("slice_check", &json!({})).unwrap()["layers"], 20);
        // What a slice is given still wins.
        let r = s.execute("slice_check", &json!({"profile": {"layer_height": 0.2, "first_layer_height": 0.2}})).unwrap();
        assert_eq!(r["layers"], 30);
        s.print = json!({"profile": {"nozzle": -1}});
        assert!(s.execute("slice_check", &json!({})).is_err(), "bad project settings are an error, not a silent default");
    }

    #[test]
    fn an_object_may_print_with_settings_of_its_own() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [15, 15, 6], "at": [-20, 0]})).unwrap();
        s.execute("add_primitive", &json!({"kind": "box", "size": [15, 15, 6], "at": [20, 0]})).unwrap();
        let plain = s.execute("slice_check", &json!({})).unwrap()["filament_mm"].as_f64().unwrap();
        s.execute("set_object_settings", &json!({"id": 2, "profile": {"infill_density": 1.0, "perimeters": 4}})).unwrap();
        let info = s.execute("info", &json!({})).unwrap();
        assert_eq!(info["objects"][1]["settings"]["perimeters"], 4);
        let back = lamina_core::project::read(&lamina_core::project::write(&s.scene, &Value::Null)).unwrap();
        assert_eq!(back.scene.get(2).unwrap().settings, s.scene.get(2).unwrap().settings, "a project keeps them");
        let denser = s.execute("slice_check", &json!({})).unwrap()["filament_mm"].as_f64().unwrap();
        assert!(denser > plain * 1.2, "one solid object: {denser} against {plain}");
        // Only that object: sliced alone, the first is as it was.
        let alone = s.execute("slice_check", &json!({"ids": [1]})).unwrap()["filament_mm"].as_f64().unwrap();
        assert!((alone * 2.0 - plain).abs() < 1.0, "{alone} {plain}");
        for bad in [
            json!({"id": 1, "profile": {"nozzle": 0.6}}),
            json!({"id": 1, "profile": {"layer_height": 0.1}}),
            json!({"id": 1, "profile": {"infill_density": 7}}),
            json!({"id": 9, "profile": {}}),
        ] {
            assert!(s.execute("set_object_settings", &bad).is_err(), "{bad}");
        }
        s.execute("set_object_settings", &json!({"id": 2, "profile": null})).unwrap();
        assert!((s.execute("slice_check", &json!({})).unwrap()["filament_mm"].as_f64().unwrap() - plain).abs() < 1e-6);
    }

    #[test]
    fn an_object_may_print_with_another_extruder_the_printer_has() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [10, 10, 4], "at": [-20, 0]})).unwrap();
        s.execute("add_primitive", &json!({"kind": "box", "size": [10, 10, 4], "at": [20, 0]})).unwrap();
        s.execute("set_object_settings", &json!({"id": 2, "profile": {"extruder": 2}})).unwrap();
        let e = s.execute("slice_check", &json!({})).unwrap_err();
        assert!(e.contains("object 2's own settings") && e.contains("`extruders`"), "{e}");
        let r = s.execute("slice_check", &json!({"profile": {"extruders": 2}})).unwrap();
        assert!(r["features"]["purge"].as_f64().unwrap() > 0.0 && r["features"]["purge-tower"].as_f64().unwrap() > 0.0);
        for bad in [json!({"id": 2, "profile": {"extruder": 9}}), json!({"id": 2, "profile": {"extruder": 0}})] {
            assert!(s.execute("set_object_settings", &bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_process_preset_sets_the_layers_and_the_strength() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [20, 20, 6]})).unwrap();
        let standard = s.execute("slice_check", &json!({"process": "0.20 mm standard"})).unwrap();
        let fine = s.execute("slice_check", &json!({"process": "0.12 mm Fine"})).unwrap();
        let draft = s.execute("slice_check", &json!({"process": "0.28 mm Draft"})).unwrap();
        let layers = |v: &Value| v["layers"].as_u64().unwrap();
        assert!(layers(&fine) > layers(&standard) && layers(&standard) > layers(&draft));
        let strong = s.execute("slice_check", &json!({"process": "Strong"})).unwrap();
        assert!(strong["filament_mm"].as_f64().unwrap() > standard["filament_mm"].as_f64().unwrap() * 1.3);
        assert_eq!(layers(&strong), layers(&standard), "Strong keeps the layer height");
        assert!(s.execute("slice_check", &json!({"process": "ludicrous"})).unwrap_err().contains("0.12 mm Fine"));
        assert_eq!(s.execute("slicer_presets", &json!({})).unwrap()["processes"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn a_duration_reads_as_hours_and_minutes() {
        assert_eq!((clock(30.0), clock(1500.0), clock(3725.0)), ("1m".into(), "25m".into(), "1h 02m".into()));
    }

    #[test]
    fn bad_input_is_refused_and_the_scene_is_untouched() {
        let mut s = session();
        assert!(s.execute("slice", &json!({"path": "/tmp/x.gcode"})).unwrap_err().contains("plate is empty"));
        s.execute("add_primitive", &json!({"kind": "box", "size": [10, 10, 10]})).unwrap();
        for bad in [
            json!({"path": "/tmp/x.gcode", "profile": {"nozzle": -1}}),
            json!({"path": "/tmp/x.gcode", "profile": 5}),
            json!({"path": "/tmp/x.gcode", "ids": [99]}),
            json!({"path": "/tmp/x.gcode", "profile_path": "/nonexistent.json"}),
            json!({"pathh": "/tmp/x.gcode"}),
        ] {
            assert!(s.execute("slice", &bad).is_err(), "{bad}");
        }
        assert_eq!(s.scene.objects.len(), 1);
    }

    /// The G-code of the session's plate, through `slice` to a file.
    fn sliced(s: &mut Session, profile: Value) -> String {
        let path = std::env::temp_dir().join(format!("lamina-ranges-{}-{}.gcode", std::process::id(), s.scene.objects.len()));
        s.execute("slice", &json!({"path": path, "profile": profile})).unwrap();
        let g = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        g
    }

    /// The lines of layer `k` of the G-code.
    fn layer(g: &str, k: usize) -> String {
        g.split(&format!(";LAYER:{k}\n")).nth(1).unwrap_or("").split(";LAYER:").next().unwrap_or("").to_string()
    }

    #[test]
    fn a_seam_painted_with_the_paint_command_moves_where_the_walls_start() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "cylinder", "radius": 10, "height": 4, "segments": 64})).unwrap();
        // The wall's start on layer 5: where the head is when the first outer-wall line begins (the plate's middle is 0, 0).
        let start = |g: &str| -> [f64; 2] {
            let mut at = [f64::NAN; 2];
            let mut wall = false;
            for l in layer(g, 5).lines() {
                if let Some(t) = l.strip_prefix(";TYPE:") {
                    wall = t == "outer-wall";
                    continue;
                }
                let word = |c: char| l.split(' ').find_map(|w| w.strip_prefix(c)?.parse::<f64>().ok());
                if let (Some(x), Some(y)) = (word('X'), word('Y')) {
                    if wall && word('E').is_some_and(|e| e > 0.0) {
                        return at;
                    }
                    at = [x - 110.0, y - 110.0];
                }
            }
            at
        };
        let profile = json!({"skirt_loops": 0, "seam": "rear", "perimeters": 1, "infill_density": 0});
        let rear = start(&sliced(&mut s, profile.clone()));
        assert!(rear[1] > 9.0, "the rear: {rear:?}");
        // Painted down the front (-y), the wall starts there.
        for z in [0.5, 1.5, 2.5, 3.5] {
            s.execute("paint", &json!({"id": 1, "kind": "seam", "at": [0, -10, z], "radius": 1})).unwrap();
        }
        let front = start(&sliced(&mut s, profile.clone()));
        assert!(front[1] < -9.0 && front[0].abs() < 1.0, "the paint: {front:?}");
        // Rubbed out, it is back at the rear.
        s.execute("paint", &json!({"id": 1, "kind": "none"})).unwrap();
        assert_eq!(start(&sliced(&mut s, profile)), rear);
    }

    #[test]
    fn a_height_range_changes_the_layers_whose_middle_is_in_it_and_no_others() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [10, 10, 10]})).unwrap();
        // Layers 0.2 mm: layer k's middle is 0.2 k + 0.1, so 4 to 6 mm is layers 20 to 29.
        let r = s.execute("set_height_range", &json!({"id": 1, "from": 4, "to": 6, "profile": {"nozzle_temp": 230, "retract_length": 2}}));
        assert_eq!(r.unwrap()["ranges"][0]["profile"]["nozzle_temp"], 230);
        s.execute("set_height_range", &json!({"id": 1, "from": 6, "to": 8, "profile": {"infill_density": 0.5, "perimeters": 4}})).unwrap();
        let info = s.execute("info", &json!({})).unwrap();
        assert_eq!(info["objects"][0]["ranges"].as_array().unwrap().len(), 2);
        let g = sliced(&mut s, json!({"skirt_loops": 0}));
        assert!(layer(&g, 20).contains("M104 S230") && !layer(&g, 19).contains("M104"), "hotter from layer 20");
        assert!(layer(&g, 30).contains("M104 S210"), "back at layer 30");
        assert!(layer(&g, 25).contains("G1 E-2.000") && !layer(&g, 15).contains("G1 E-2.000"), "its own retraction");
        let loops = |k: usize| {
            let l = layer(&g, k);
            l.split(";TYPE:")
                .filter(|t| t.starts_with("inner-wall") || t.starts_with("outer-wall"))
                .map(|t| t.matches("G0 ").count() + 1)
                .sum::<usize>()
        };
        assert!(loops(35) > loops(15), "four walls from 6 to 8 mm: {} against {}", loops(35), loops(15));
        let infill = |k: usize| {
            layer(&g, k).split(";TYPE:sparse-infill").nth(1).map_or(0, |t| t.split(";TYPE:").next().unwrap_or("").lines().count())
        };
        assert!(infill(34) > infill(14), "denser infill in the range");
        // Ranges may not overlap or set what is not by height; the same range again replaces it, `{}` removes it.
        assert!(
            s.execute("set_height_range", &json!({"id": 1, "from": 5, "to": 7, "profile": {"fan_speed": 0}}))
                .unwrap_err()
                .contains("overlaps")
        );
        assert!(
            s.execute("set_height_range", &json!({"id": 1, "from": 1, "to": 2, "profile": {"layer_height": 0.1}}))
                .unwrap_err()
                .contains("cannot change by height")
        );
        assert!(s.execute("set_height_range", &json!({"id": 1, "from": 3, "to": 2, "profile": {"fan_speed": 0}})).is_err());
        assert!(s.execute("set_height_range", &json!({"id": 1, "from": 1, "to": 2, "profile": {"nozzle_temp": 9999}})).is_err());
        s.execute("set_height_range", &json!({"id": 1, "from": 4, "to": 6, "profile": {"nozzle_temp": 220}})).unwrap();
        s.execute("set_height_range", &json!({"id": 1, "from": 6, "to": 8, "profile": {}})).unwrap();
        let ranges = s.execute("info", &json!({})).unwrap()["objects"][0]["ranges"].clone();
        assert_eq!(ranges.as_array().unwrap().len(), 1);
        assert_eq!(ranges[0]["profile"], json!({"nozzle_temp": 220}));
        // Kept in projects, undone in one step.
        let path = std::env::temp_dir().join(format!("lamina-ranges-{}.lamina", std::process::id()));
        s.execute("save", &json!({"path": path})).unwrap();
        let mut t = session();
        t.execute("open", &json!({"path": path})).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(t.execute("info", &json!({})).unwrap()["objects"][0]["ranges"], ranges);
        s.undo().unwrap();
        assert_eq!(s.execute("info", &json!({})).unwrap()["objects"][0]["ranges"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_pressure_advance_and_fan_range_are_set_and_set_back() {
        let mut s = session();
        s.execute("add_primitive", &json!({"kind": "box", "size": [10, 10, 4]})).unwrap();
        s.execute("set_height_range", &json!({"id": 1, "from": 2, "to": 3, "profile": {"pressure_advance": 0.05, "fan_speed": 100}}))
            .unwrap();
        let g = sliced(&mut s, json!({"firmware": "klipper", "pressure_advance": 0.02, "skirt_loops": 0}));
        assert!(layer(&g, 10).contains("SET_PRESSURE_ADVANCE ADVANCE=0.05") && layer(&g, 10).contains("M106 S100"));
        assert!(layer(&g, 15).contains("SET_PRESSURE_ADVANCE ADVANCE=0.02") && layer(&g, 15).contains("M106 S255"), "set back above");
    }

    #[test]
    fn calibration_parts_carry_their_settings_by_height() {
        // A temperature tower: nine blocks of 8 mm from 230 C down to 190 C, each set once in the G-code, in that order.
        let mut s = session();
        let r = s.execute("calibration", &json!({"kind": "temperature"})).unwrap();
        assert_eq!(r["steps"].as_array().unwrap().len(), 9);
        let o = &s.execute("info", &json!({})).unwrap()["objects"][0];
        assert_eq!(o["size"], json!([24.0, 10.0, 72.0]), "standing, 72 mm tall with its overhangs");
        assert_eq!(o["watertight"], true);
        let g = sliced(&mut s, json!({"skirt_loops": 0}));
        let temps: Vec<&str> = g.lines().filter(|l| l.starts_with("M104 S")).collect();
        let want: Vec<String> = (0..9).map(|i| format!("M104 S{}", 230 - 5 * i)).collect();
        let from = temps.iter().position(|t| *t == "M104 S230").unwrap();
        assert_eq!(temps[from..from + 9], want, "each block hotter than the one above: {temps:?}");
        // Retraction: two towers, each 5 mm band retracting its own length.
        let mut s = session();
        let r = s.execute("calibration", &json!({"kind": "retraction"})).unwrap();
        assert_eq!(r["ids"].as_array().unwrap().len(), 2);
        let g = sliced(&mut s, json!({"skirt_loops": 0}));
        for v in ["0.400", "1.200", "2.000"] {
            assert!(g.contains(&format!("G1 E-{v} ")), "retracts {v}");
        }
        // Pressure advance on Klipper, a step a millimetre.
        let mut s = session();
        s.execute("calibration", &json!({"kind": "pressure_advance", "to": 0.04, "step": 0.01})).unwrap();
        let g = sliced(&mut s, json!({"skirt_loops": 0, "firmware": "klipper"}));
        for v in ["0.01", "0.02", "0.03", "0.04"] {
            assert!(g.contains(&format!("SET_PRESSURE_ADVANCE ADVANCE={v}")), "{v}");
        }
        // Flow: five blocks, each pushing more plastic than the one on its left.
        let mut s = session();
        let r = s.execute("calibration", &json!({"kind": "flow"})).unwrap();
        let ids: Vec<u64> = r["ids"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
        assert_eq!(ids.len(), 5);
        let g = sliced(&mut s, json!({"skirt_loops": 0, "firmware": "klipper"}));
        let pushed = |id: u64| -> f64 {
            g.split(&format!("EXCLUDE_OBJECT_START NAME=object_{id}\n"))
                .skip(1)
                .map(|part| part.split("EXCLUDE_OBJECT_END").next().unwrap_or(""))
                .flat_map(|part| part.lines())
                .filter(|l| l.starts_with("G1 X"))
                .filter_map(|l| l.split(" E").nth(1)?.split(' ').next()?.parse::<f64>().ok())
                .sum()
        };
        let e: Vec<f64> = ids.iter().map(|&id| pushed(id)).collect();
        assert!(e.windows(2).all(|w| w[1] > w[0]), "{e:?}");
        assert!((e[4] / e[0] - 1.10 / 0.90).abs() < 0.01, "{e:?}");
        // What cannot be calibrated, or with too many steps, or out of the setting's range, is refused and adds nothing.
        let mut s = session();
        for bad in [json!({"kind": "speed"}), json!({"kind": "temperature", "step": 0.5}), json!({"kind": "temperature", "from": 600})] {
            assert!(s.execute("calibration", &bad).is_err(), "{bad}");
        }
        assert!(s.scene.objects.is_empty());
        s.execute("calibration", &json!({"kind": "flow"})).unwrap();
        s.undo().unwrap();
        assert!(s.scene.objects.is_empty(), "one undo takes every block back");
    }
}
