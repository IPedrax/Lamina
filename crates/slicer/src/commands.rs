//! The slicer's commands, which an app adds to a session with `session.register(lamina_slicer::COMMANDS)`.

use lamina_core::{Command, Files, Mesh, Session};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::print;
use crate::profile::Profile;

/// The largest profile file read, bytes.
const MAX_PROFILE_FILE: u64 = 1 << 20;

pub static COMMANDS: &[Command] = &[
    Command {
        id: "slice",
        params: "{path, ids?, profile?, profile_path?} slices the objects on the plate (all, or `ids`) to G-code at `path`. `profile` is an object \
                 of the fields that differ from the defaults (see `slicer_profile`), applied over `profile_path`'s JSON file when both are given. \
                 Returns {path, layers, filament_mm, print_seconds, warnings}. It refuses an object that is off the plate, below it or too tall, and says which",
        run: slice,
        undoable: false,
        files: Files::Write,
    },
    Command {
        id: "slice_check",
        params: "{ids?, profile?} the same slice as `slice`, with no file written: what would print and whether it can. Returns {layers, filament_mm, print_seconds, print_time, warnings}, or an error saying what does not fit the plate (off it, below it, too tall) or what a setting must be. Use it to check a model before the user exports it",
        run: slice_check,
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
    profile: Option<Value>,
    profile_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    ids: Option<Vec<u32>>,
    profile: Option<Value>,
}

/// Slices the objects named (or all) with the profile: the file's, if any, with `over`'s fields on top.
fn run(s: &Session, ids: Option<&[u32]>, over: Option<Value>, profile_path: Option<&str>) -> Result<print::Output, String> {
    let mut merged = match profile_path {
        Some(file) => {
            if std::fs::metadata(file).map_err(|e| format!("{file}: {e}"))?.len() > MAX_PROFILE_FILE {
                return Err(format!("{file} is larger than {MAX_PROFILE_FILE} bytes"));
            }
            serde_json::from_str::<Value>(&std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?)
                .map_err(|e| format!("{file}: {e}"))?
        }
        None => json!({}),
    };
    match (&mut merged, over) {
        (Value::Object(base), Some(Value::Object(over))) => base.extend(over),
        (_, Some(Value::Null) | None) => {}
        (_, Some(_)) => return Err("`profile` is an object".into()),
    }
    let profile = Profile::from_json(&merged)?;
    let objects: Vec<(u32, Mesh)> = match ids {
        Some(ids) => ids.iter().map(|&id| s.scene.get(id).map(|o| (id, o.world()))).collect::<Result<_, _>>()?,
        None => s.scene.objects.iter().map(|o| (o.id, o.world())).collect(),
    };
    print::gcode(&objects, &profile)
}

/// 3725 seconds as "1h 02m".
fn clock(seconds: f64) -> String {
    let m = (seconds / 60.0).round() as u64;
    if m >= 60 { format!("{}h {:02}m", m / 60, m % 60) } else { format!("{m}m") }
}

fn summary(out: &print::Output) -> Value {
    json!({
        "layers": out.layers, "filament_mm": out.filament_mm, "print_seconds": out.seconds.round(),
        "print_time": clock(out.seconds), "warnings": out.warnings,
    })
}

fn slice(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = Args::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    let out = run(s, a.ids.as_deref(), a.profile, a.profile_path.as_deref())?;
    std::fs::write(&a.path, &out.gcode).map_err(|e| format!("{}: {e}", a.path))?;
    let mut v = summary(&out);
    v["path"] = json!(a.path);
    Ok(v)
}

fn slice_check(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = Check::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    run(s, a.ids.as_deref(), a.profile, None).map(|out| summary(&out))
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
