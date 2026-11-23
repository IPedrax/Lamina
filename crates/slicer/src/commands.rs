//! The slicer's commands, which an app adds to a session with `session.register(lamina_slicer::COMMANDS)`.

use lamina_core::{Command, Files, Mesh, Session};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::presets::{self, FILAMENTS, PRINTERS};
use crate::print;
use crate::profile::Profile;

/// The largest profile file read, bytes.
const MAX_PROFILE_FILE: u64 = 1 << 20;

pub static COMMANDS: &[Command] = &[
    Command {
        id: "slice",
        params: "{path, ids?, printer?, filament?, profile?, profile_path?} slices the objects on the plate (all, or `ids`) to G-code at `path`. \
                 Settings are laid over the defaults in this order: the project's print settings (from `open`, or the window's), `profile_path`'s JSON file, the `printer` and `filament` presets by name (see \
                 `slicer_presets`), then `profile`, an object of the fields that differ (see `slicer_profile`). \
                 Returns {path, layers, filament_mm, print_seconds, warnings}. It refuses an object that is off the plate, below it or too tall, and says which",
        run: slice,
        undoable: false,
        files: Files::Write,
    },
    Command {
        id: "slice_check",
        params: "{ids?, printer?, filament?, profile?} the same slice as `slice`, with no file written: what would print and whether it can. Returns {layers, filament_mm, print_seconds, print_time, warnings, features (filament mm by kind: outer-wall, inner-wall, solid-infill, sparse-infill, skirt, brim, support, support-interface, raft)}, or an error saying what does not fit the plate (off it, below it, too tall) or what a setting must be. Use it to check a model before the user exports it",
        run: slice_check,
        undoable: false,
        files: Files::None,
    },
    Command {
        id: "set_object_settings",
        params: "{id, profile} gives one object slicer settings of its own, laid over the plate's for it alone: perimeters, top_layers, \
                 bottom_layers, infill_density, infill_pattern, seam, perimeter_speed, infill_speed and the support fields. `profile: {}` or \
                 null clears them. The printer, the filament and the layer heights are the plate's. `info` shows each object's settings",
        run: set_object_settings,
        undoable: true,
        files: Files::None,
    },
    Command {
        id: "slicer_presets",
        params: "{} the built-in printer and filament presets: {printers:[{name, about, fields}], filaments:[...]}. Pass a name as `printer` or \
                 `filament` to `slice` or `slice_check`; a printer sets the machine's fields, a filament its temperatures and fan",
        run: list_presets,
        undoable: false,
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
    profile: Option<Value>,
    profile_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    ids: Option<Vec<u32>>,
    printer: Option<String>,
    filament: Option<String>,
    profile: Option<Value>,
}

/// The settings a slice asks for, in the order they are laid over the defaults.
struct Settings<'a> {
    file: Option<&'a str>,
    printer: Option<&'a str>,
    filament: Option<&'a str>,
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
    for (name, list) in [(settings.printer, PRINTERS), (settings.filament, FILAMENTS)] {
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
    let objects: Vec<(u32, Mesh)> = chosen.iter().map(|o| (o.id, o.world())).collect();
    print::gcode_each(&objects, &own(&chosen, &profile)?, &profile)
}

/// The fields an object may set for itself: what it is made of and how, never the machine, the filament or the layer heights.
pub const PER_OBJECT_FIELDS: &[&str] = &[
    "perimeters",
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
];

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
    presets::apply(&Profile::default(), &fields)?;
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
    let settings =
        Settings { file: a.profile_path.as_deref(), printer: a.printer.as_deref(), filament: a.filament.as_deref(), over: a.profile };
    let out = run(s, a.ids.as_deref(), settings)?;
    std::fs::write(&a.path, &out.gcode).map_err(|e| format!("{}: {e}", a.path))?;
    let mut v = summary(&out);
    v["path"] = json!(a.path);
    Ok(v)
}

fn slice_check(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = Check::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    let settings = Settings { file: None, printer: a.printer.as_deref(), filament: a.filament.as_deref(), over: a.profile };
    run(s, a.ids.as_deref(), settings).map(|out| summary(&out))
}

fn list_presets(_: &mut Session, _: &Value) -> Result<Value, String> {
    let list = |l: &[presets::Preset]| -> Vec<Value> {
        l.iter().map(|p| json!({"name": p.name, "about": p.about, "fields": p.fields()})).collect()
    };
    Ok(json!({"printers": list(PRINTERS), "filaments": list(FILAMENTS)}))
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
}
