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

fn slice(s: &mut Session, p: &Value) -> Result<Value, String> {
    let a = Args::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    let mut merged = match &a.profile_path {
        Some(file) => {
            if std::fs::metadata(file).map_err(|e| format!("{file}: {e}"))?.len() > MAX_PROFILE_FILE {
                return Err(format!("{file} is larger than {MAX_PROFILE_FILE} bytes"));
            }
            serde_json::from_str::<Value>(&std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?)
                .map_err(|e| format!("{file}: {e}"))?
        }
        None => json!({}),
    };
    match (&mut merged, a.profile) {
        (Value::Object(base), Some(Value::Object(over))) => base.extend(over),
        (_, Some(Value::Null) | None) => {}
        (_, Some(_)) => return Err("`profile` is an object".into()),
    }
    let profile = Profile::from_json(&merged)?;
    let objects: Vec<(u32, Mesh)> = match &a.ids {
        Some(ids) => ids.iter().map(|&id| s.scene.get(id).map(|o| (id, o.world()))).collect::<Result<_, _>>()?,
        None => s.scene.objects.iter().map(|o| (o.id, o.world())).collect(),
    };
    let out = print::gcode(&objects, &profile)?;
    std::fs::write(&a.path, &out.gcode).map_err(|e| format!("{}: {e}", a.path))?;
    Ok(json!({
        "path": a.path, "layers": out.layers, "filament_mm": out.filament_mm,
        "print_seconds": out.seconds.round(), "warnings": out.warnings,
    }))
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
