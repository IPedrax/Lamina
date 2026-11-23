//! Built-in printer and filament presets. A preset is the profile fields its kind sets and nothing else: a printer sets the machine
//! (plate, nozzle, start and end G-code, retraction for its extruder), a filament sets temperatures and the fan. Applied in order over
//! a profile, each changes only its own fields, so a printer and a filament combine and the print settings stay as they were.
//!
//! The values are ours, typical figures and not any maker's profile: printers are named by plate and firmware, not by brand, and a
//! filament's own label is the authority on its temperatures.

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
    "nozzle",
    "filament_diameter",
    "start_gcode",
    "end_gcode",
    "retract_length",
    "retract_speed",
    "travel_speed",
];
pub const FILAMENT_FIELDS: &[&str] = &["nozzle_temp", "first_layer_nozzle_temp", "bed_temp", "first_layer_bed_temp", "fan_speed"];

const KLIPPER_START: &str = "PRINT_START BED=[first_layer_bed_temp] EXTRUDER=[first_layer_nozzle_temp]";

pub static PRINTERS: &[Preset] = &[
    Preset {
        name: "Generic 220 mm, Marlin",
        about: "220 x 220 x 250 mm, 0.4 mm nozzle, direct drive: the defaults",
        fields: r#"{"bed": [220, 220], "max_height": 250, "nozzle": 0.4, "retract_length": 0.8, "retract_speed": 35, "travel_speed": 150}"#,
    },
    Preset {
        name: "Generic 220 mm, Marlin, Bowden",
        about: "220 x 220 x 250 mm, 0.4 mm nozzle, a Bowden tube, which needs a longer retraction",
        fields: r#"{"bed": [220, 220], "max_height": 250, "nozzle": 0.4, "retract_length": 5.0, "retract_speed": 40, "travel_speed": 150}"#,
    },
    Preset {
        name: "Generic 250 x 210 mm, Marlin",
        about: "250 x 210 x 210 mm, 0.4 mm nozzle, direct drive",
        fields: r#"{"bed": [250, 210], "max_height": 210, "nozzle": 0.4, "retract_length": 0.8, "retract_speed": 35, "travel_speed": 180}"#,
    },
    Preset {
        name: "Generic 300 mm, Klipper",
        about: "300 x 300 x 300 mm, 0.4 mm nozzle; the printer's config must define PRINT_START and PRINT_END macros",
        fields: r#"{"bed": [300, 300], "max_height": 300, "nozzle": 0.4, "retract_length": 0.6, "retract_speed": 40, "travel_speed": 250,
                   "max_acceleration": 3000, "junction_deviation": 0.0035, "start_gcode": "KLIPPER_START", "end_gcode": "PRINT_END"}"#,
    },
    Preset {
        name: "Generic 350 mm, Klipper",
        about: "350 x 350 x 340 mm, 0.4 mm nozzle; the printer's config must define PRINT_START and PRINT_END macros",
        fields: r#"{"bed": [350, 350], "max_height": 340, "nozzle": 0.4, "retract_length": 0.6, "retract_speed": 40, "travel_speed": 250,
                   "max_acceleration": 3000, "junction_deviation": 0.0035, "start_gcode": "KLIPPER_START", "end_gcode": "PRINT_END"}"#,
    },
    Preset {
        name: "Delta 200 mm, centred",
        about: "a round 200 mm plate, 300 mm tall, with the machine's zero in the middle; the slicer still treats the plate as its square",
        fields: r#"{"bed": [200, 200], "max_height": 300, "origin_center": true, "nozzle": 0.4, "retract_length": 4.0, "retract_speed": 40, "travel_speed": 200}"#,
    },
];

pub static FILAMENTS: &[Preset] = &[
    Preset {
        name: "PLA",
        about: "nozzle 210, first layer 215, bed 60, fan full",
        fields: r#"{"nozzle_temp": 210, "first_layer_nozzle_temp": 215, "bed_temp": 60, "first_layer_bed_temp": 60, "fan_speed": 255}"#,
    },
    Preset {
        name: "PETG",
        about: "nozzle 240, first layer 245, bed 80, fan half",
        fields: r#"{"nozzle_temp": 240, "first_layer_nozzle_temp": 245, "bed_temp": 80, "first_layer_bed_temp": 80, "fan_speed": 128}"#,
    },
    Preset {
        name: "ABS",
        about: "nozzle 250, first layer 255, bed 100, fan off; print enclosed",
        fields: r#"{"nozzle_temp": 250, "first_layer_nozzle_temp": 255, "bed_temp": 100, "first_layer_bed_temp": 100, "fan_speed": 0}"#,
    },
    Preset {
        name: "ASA",
        about: "nozzle 255, first layer 260, bed 100, fan off; print enclosed",
        fields: r#"{"nozzle_temp": 255, "first_layer_nozzle_temp": 260, "bed_temp": 100, "first_layer_bed_temp": 100, "fan_speed": 0}"#,
    },
    Preset {
        name: "TPU 95A",
        about: "nozzle 225, first layer 230, bed 50, fan half; print slowly",
        fields: r#"{"nozzle_temp": 225, "first_layer_nozzle_temp": 230, "bed_temp": 50, "first_layer_bed_temp": 50, "fan_speed": 128}"#,
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

/// The fields of `profile` named in `keys`: what a preset of that kind saved from the settings shown would hold.
pub fn capture(profile: &Profile, keys: &[&str]) -> Map<String, Value> {
    let Ok(Value::Object(all)) = serde_json::to_value(profile) else { return Map::new() };
    all.into_iter().filter(|(k, _)| keys.contains(&k.as_str())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_sets_only_its_own_fields_and_gives_a_valid_profile() {
        for (list, keys) in [(PRINTERS, PRINTER_FIELDS), (FILAMENTS, FILAMENT_FIELDS)] {
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
