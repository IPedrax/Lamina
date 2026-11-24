//! Built-in printer and filament presets. A preset is the profile fields its kind sets and nothing else: a printer sets the machine
//! (plate, nozzle, start and end G-code, retraction for its extruder), a filament sets temperatures and the fan. Applied in order over
//! a profile, each changes only its own fields, so a printer and a filament combine and the print settings stay as they were.
//!
//! The values are ours, typical figures and not any maker's profile: printers are named by plate and firmware, not by brand, and a
//! filament's own label is the authority on its temperatures.

use lamina_i18n::ui_msg;
use serde_json::{Map, Value};

use crate::profile::Profile;

#[derive(Debug)]
pub struct Preset {
    pub name: &'static str,
    pub about: &'static str,
    /// A JSON object of profile fields; a test checks each parses, sets only its kind's fields, and gives a valid profile.
    fields: &'static str,
}

/// The fields a printer preset may set, and a filament preset.
pub const PRINTER_FIELDS: &[&str] = &[
    "bed",
    "origin_center",
    "max_height",
    "max_acceleration",
    "junction_deviation",
    "firmware",
    "firmware_retraction",
    "thumbnail",
    "nozzle",
    "filament_diameter",
    "start_gcode",
    "end_gcode",
    "retract_length",
    "retract_speed",
    "travel_speed",
    "extruders",
    "toolchange_gcode",
    "clearance_radius",
    "gantry_height",
];
/// The fields a process preset may set: the layers and the strength, never the machine or the filament.
pub const PROCESS_FIELDS: &[&str] = &[
    "layer_height",
    "first_layer_height",
    "adaptive_layers",
    "min_layer_height",
    "perimeters",
    "top_layers",
    "bottom_layers",
    "infill_density",
    "infill_pattern",
];
pub const FILAMENT_FIELDS: &[&str] = &[
    "nozzle_temp",
    "first_layer_nozzle_temp",
    "bed_temp",
    "first_layer_bed_temp",
    "fan_speed",
    "pressure_advance",
    "extruder_temps",
    "standby_temp_delta",
];

const KLIPPER_START: &str = "PRINT_START BED=[first_layer_bed_temp] EXTRUDER=[first_layer_nozzle_temp]";

pub static PRINTERS: &[Preset] = &[
    Preset {
        name: "Generic 220 mm, Marlin",
        about: ui_msg!("220 x 220 x 250 mm, 0.4 mm nozzle, direct drive: the defaults"),
        fields: r#"{"bed": [220, 220], "max_height": 250, "nozzle": 0.4, "retract_length": 0.8, "retract_speed": 35, "travel_speed": 150}"#,
    },
    Preset {
        name: "Generic 220 mm, Marlin, Bowden",
        about: ui_msg!("220 x 220 x 250 mm, 0.4 mm nozzle, a Bowden tube, which needs a longer retraction"),
        fields: r#"{"bed": [220, 220], "max_height": 250, "nozzle": 0.4, "retract_length": 5.0, "retract_speed": 40, "travel_speed": 150}"#,
    },
    Preset {
        name: "Generic 250 x 210 mm, Marlin",
        about: ui_msg!("250 x 210 x 210 mm, 0.4 mm nozzle, direct drive"),
        fields: r#"{"bed": [250, 210], "max_height": 210, "nozzle": 0.4, "retract_length": 0.8, "retract_speed": 35, "travel_speed": 180}"#,
    },
    Preset {
        name: "Generic 300 mm, Klipper",
        about: ui_msg!("300 x 300 x 300 mm, 0.4 mm nozzle; the printer's config must define PRINT_START and PRINT_END macros"),
        fields: r#"{"bed": [300, 300], "max_height": 300, "nozzle": 0.4, "retract_length": 0.6, "retract_speed": 40, "travel_speed": 250,
                   "max_acceleration": 3000, "junction_deviation": 0.0035, "firmware": "klipper", "thumbnail": 300,
                   "start_gcode": "KLIPPER_START", "end_gcode": "PRINT_END"}"#,
    },
    Preset {
        name: "Generic 350 mm, Klipper",
        about: ui_msg!("350 x 350 x 340 mm, 0.4 mm nozzle; the printer's config must define PRINT_START and PRINT_END macros"),
        fields: r#"{"bed": [350, 350], "max_height": 340, "nozzle": 0.4, "retract_length": 0.6, "retract_speed": 40, "travel_speed": 250,
                   "max_acceleration": 3000, "junction_deviation": 0.0035, "firmware": "klipper", "thumbnail": 300,
                   "start_gcode": "KLIPPER_START", "end_gcode": "PRINT_END"}"#,
    },
    Preset {
        name: "Delta 200 mm, centred",
        about: ui_msg!(
            "a round 200 mm plate, 300 mm tall, with the machine's zero in the middle; the slicer still treats the plate as its square"
        ),
        fields: r#"{"bed": [200, 200], "max_height": 300, "origin_center": true, "nozzle": 0.4, "retract_length": 4.0, "retract_speed": 40, "travel_speed": 200}"#,
    },
];

pub static FILAMENTS: &[Preset] = &[
    Preset {
        name: "PLA",
        about: ui_msg!("nozzle 210, first layer 215, bed 60, fan full"),
        fields: r#"{"nozzle_temp": 210, "first_layer_nozzle_temp": 215, "bed_temp": 60, "first_layer_bed_temp": 60, "fan_speed": 255}"#,
    },
    Preset {
        name: "PETG",
        about: ui_msg!("nozzle 240, first layer 245, bed 80, fan half"),
        fields: r#"{"nozzle_temp": 240, "first_layer_nozzle_temp": 245, "bed_temp": 80, "first_layer_bed_temp": 80, "fan_speed": 128}"#,
    },
    Preset {
        name: "ABS",
        about: ui_msg!("nozzle 250, first layer 255, bed 100, fan off; print enclosed"),
        fields: r#"{"nozzle_temp": 250, "first_layer_nozzle_temp": 255, "bed_temp": 100, "first_layer_bed_temp": 100, "fan_speed": 0}"#,
    },
    Preset {
        name: "ASA",
        about: ui_msg!("nozzle 255, first layer 260, bed 100, fan off; print enclosed"),
        fields: r#"{"nozzle_temp": 255, "first_layer_nozzle_temp": 260, "bed_temp": 100, "first_layer_bed_temp": 100, "fan_speed": 0}"#,
    },
    Preset {
        name: "TPU 95A",
        about: ui_msg!("nozzle 225, first layer 230, bed 50, fan half; print slowly"),
        fields: r#"{"nozzle_temp": 225, "first_layer_nozzle_temp": 230, "bed_temp": 50, "first_layer_bed_temp": 50, "fan_speed": 128}"#,
    },
];

pub static PROCESSES: &[Preset] = &[
    Preset {
        name: "0.12 mm Fine",
        about: ui_msg!("thin layers for detail and smooth curves; about half again as long to print"),
        fields: r#"{"layer_height": 0.12, "first_layer_height": 0.2, "top_layers": 7, "bottom_layers": 5}"#,
    },
    Preset {
        name: "0.20 mm Standard",
        about: ui_msg!("the everyday balance of look and time: two walls, 15 % infill"),
        fields: r#"{"layer_height": 0.2, "first_layer_height": 0.2, "perimeters": 2, "top_layers": 4, "bottom_layers": 4, "infill_density": 0.15}"#,
    },
    Preset {
        name: "0.28 mm Draft",
        about: ui_msg!("thick layers for a quick test fit; visible lines"),
        fields: r#"{"layer_height": 0.28, "first_layer_height": 0.28, "top_layers": 3, "bottom_layers": 3, "infill_density": 0.1}"#,
    },
    Preset {
        name: "Strong",
        about: ui_msg!("four walls, thick skins and 40 % gyroid infill for parts that bear load; keeps the layer height"),
        fields: r#"{"perimeters": 4, "top_layers": 6, "bottom_layers": 6, "infill_density": 0.4, "infill_pattern": "gyroid"}"#,
    },
];

impl Preset {
    /// The fields it sets.
    pub fn fields(&self) -> Map<String, Value> {
        let text = self.fields.replace("KLIPPER_START", KLIPPER_START);
        match serde_json::from_str(&text) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(), // a test proves every built-in parses
        }
    }
}

/// A built-in preset by name, in any case, or an error naming the ones there are.
pub fn find<'a>(list: &'a [Preset], name: &str) -> Result<&'a Preset, String> {
    list.iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("no preset `{name}`; there are: {}", list.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")))
}

/// `profile` with `fields` laid over it, checked as a whole.
pub fn apply(profile: &Profile, fields: &Map<String, Value>) -> Result<Profile, String> {
    let mut merged = serde_json::to_value(profile).map_err(|e| e.to_string())?;
    if let Value::Object(m) = &mut merged {
        m.extend(fields.clone());
    }
    Profile::from_json(&merged)
}

/// The key with which a user's preset names the preset it is laid over.
pub const INHERITS: &str = "inherits";
/// The longest chain of presets laid over one another.
pub const MAX_CHAIN: usize = 8;

/// The fields of the preset `name`, with the chain it inherits resolved: a user's preset (found by `mine`) may name another, the user's
/// or a built-in of `built_in`, in `inherits`, and its own fields are laid over that one's. A chain longer than [`MAX_CHAIN`], one that
/// comes back to itself, or one that names a preset nobody has is an error that says which.
pub fn resolve<'a>(
    name: &str,
    mine: impl Fn(&str) -> Option<&'a Map<String, Value>>,
    built_in: &[Preset],
) -> Result<Map<String, Value>, String> {
    let mut chain: Vec<Map<String, Value>> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut at = name.to_string();
    loop {
        if seen.iter().any(|s| s.eq_ignore_ascii_case(&at)) {
            return Err(format!("the presets {} inherit from one another in a circle", seen.join(" > ")));
        }
        if seen.len() >= MAX_CHAIN {
            return Err(format!("the preset `{name}` inherits through more than {MAX_CHAIN} others"));
        }
        seen.push(at.clone());
        match mine(&at) {
            Some(fields) => {
                let mut own = fields.clone();
                let parent = own.remove(INHERITS);
                chain.push(own);
                match parent {
                    None => break,
                    Some(Value::String(p)) if !p.trim().is_empty() => at = p,
                    Some(_) => return Err(format!("`{INHERITS}` in the preset `{at}` is the name of another preset")),
                }
            }
            None => {
                chain.push(
                    find(built_in, &at)
                        .map_err(|e| if seen.len() > 1 { format!("`{}` inherits `{at}`: {e}", seen[seen.len() - 2]) } else { e })?
                        .fields(),
                );
                break;
            }
        }
    }
    // The base first, then each preset over the one it inherits.
    Ok(chain.into_iter().rev().fold(Map::new(), |mut all, fields| {
        all.extend(fields);
        all
    }))
}

/// The fields of `profile` named in `keys`: what a preset of that kind saved from the settings shown would hold.
pub fn capture(profile: &Profile, keys: &[&str]) -> Map<String, Value> {
    let Ok(Value::Object(all)) = serde_json::to_value(profile) else { return Map::new() };
    all.into_iter().filter(|(k, _)| keys.contains(&k.as_str())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preset_inherits_a_chain_and_a_circle_or_a_missing_parent_is_said() {
        let mut mine: std::collections::BTreeMap<String, Map<String, Value>> = std::collections::BTreeMap::new();
        let obj = |v: Value| v.as_object().cloned().unwrap();
        mine.insert("Hot PLA".into(), obj(serde_json::json!({"inherits": "PLA", "nozzle_temp": 220})));
        mine.insert("Hot PLA, slow fan".into(), obj(serde_json::json!({"inherits": "hot pla", "fan_speed": 100})));
        let look = |n: &str| mine.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v);
        let f = resolve("Hot PLA, slow fan", look, FILAMENTS).unwrap();
        assert_eq!((f["nozzle_temp"].as_f64(), f["fan_speed"].as_f64(), f["bed_temp"].as_f64()), (Some(220.0), Some(100.0), Some(60.0)));
        assert!(!f.contains_key("inherits"));
        assert_eq!(resolve("PLA", look, FILAMENTS).unwrap(), find(FILAMENTS, "PLA").unwrap().fields());
        // A circle, a missing parent, and a parent that is no name.
        mine.insert("A".into(), obj(serde_json::json!({"inherits": "B"})));
        mine.insert("B".into(), obj(serde_json::json!({"inherits": "A"})));
        mine.insert("C".into(), obj(serde_json::json!({"inherits": "Nylon"})));
        mine.insert("D".into(), obj(serde_json::json!({"inherits": 7})));
        let look = |n: &str| mine.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v);
        assert!(resolve("A", look, FILAMENTS).unwrap_err().contains("circle"));
        assert!(resolve("C", look, FILAMENTS).unwrap_err().contains("`C` inherits `Nylon`"));
        assert!(resolve("D", look, FILAMENTS).is_err());
        // Too long a chain.
        let mut long: std::collections::BTreeMap<String, Map<String, Value>> = std::collections::BTreeMap::new();
        for i in 0..20 {
            long.insert(format!("p{i}"), obj(serde_json::json!({"inherits": format!("p{}", i + 1)})));
        }
        assert!(resolve("p0", |n: &str| long.get(n), FILAMENTS).unwrap_err().contains("more than"));
    }

    #[test]
    fn every_built_in_sets_only_its_own_fields_and_gives_a_valid_profile() {
        for (list, keys) in [(PRINTERS, PRINTER_FIELDS), (FILAMENTS, FILAMENT_FIELDS), (PROCESSES, PROCESS_FIELDS)] {
            for p in list {
                let f = p.fields();
                assert!(!f.is_empty(), "{} does not parse", p.name);
                assert!(f.keys().all(|k| keys.contains(&k.as_str())), "{} sets a field that is not its kind's: {f:?}", p.name);
                let profile = apply(&Profile::default(), &f).unwrap_or_else(|e| panic!("{}: {e}", p.name));
                profile.expand(&profile.start_gcode).unwrap();
            }
        }
    }

    #[test]
    fn a_printer_and_a_filament_combine_and_leave_the_print_settings_alone() {
        let mine = Profile { infill_density: 0.4, ..Profile::default() };
        let p = apply(&mine, &find(PRINTERS, "generic 300 mm, klipper").unwrap().fields()).unwrap();
        let p = apply(&p, &find(FILAMENTS, "PETG").unwrap().fields()).unwrap();
        assert_eq!((p.bed, p.nozzle_temp, p.infill_density), ([300.0, 300.0], 240.0, 0.4));
        assert_eq!(p.expand(&p.start_gcode).unwrap(), "PRINT_START BED=80 EXTRUDER=245");
        assert!(find(FILAMENTS, "unobtainium").unwrap_err().contains("PLA, PETG"));
    }

    #[test]
    fn capture_takes_only_the_fields_asked_for() {
        let f = capture(&Profile { bed_temp: 70.0, ..Profile::default() }, FILAMENT_FIELDS);
        assert_eq!(f.len(), FILAMENT_FIELDS.len());
        assert_eq!(f["bed_temp"], 70.0);
        assert!(apply(&Profile::default(), &f).is_ok());
    }
}
