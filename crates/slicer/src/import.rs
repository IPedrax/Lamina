//! Another slicer's profile read into Lamina's settings: a PrusaSlicer `.ini` (`key = value` lines) or an OrcaSlicer or Bambu Studio
//! JSON preset (values as text, some in lists of one per extruder). The keys are mapped by a table we wrote from those programs'
//! published setting names, never from their code, and no profile file of theirs is bundled. What has no Lamina setting, or a value
//! Lamina cannot take, is reported as left out, never guessed at.

use serde_json::{Map, Value, json};

use crate::profile::Profile;

/// The largest profile read, bytes.
pub const MAX_FILE: usize = 1 << 20;

/// How their value becomes ours.
#[derive(Clone, Copy)]
enum As {
    Number,
    Whole,
    /// "15%" or 0.15: a share from 0 to 1.
    Share,
    /// A fan in % (0 to 100): ours is 0 to 255.
    FanPercent,
    Flag,
    /// A support switch: on means everywhere.
    Support,
    Pattern,
    Seam,
    Firmware,
    /// The plate as corner points, "0x0,220x0,220x220,0x220".
    Plate,
    GCode,
}

/// Their key, ours, and how it is read.
const TABLE: &[(&str, &str, As)] = &[
    // PrusaSlicer
    ("layer_height", "layer_height", As::Number),
    ("first_layer_height", "first_layer_height", As::Number),
    ("perimeters", "perimeters", As::Whole),
    ("top_solid_layers", "top_layers", As::Whole),
    ("bottom_solid_layers", "bottom_layers", As::Whole),
    ("fill_density", "infill_density", As::Share),
    ("fill_pattern", "infill_pattern", As::Pattern),
    ("skirts", "skirt_loops", As::Whole),
    ("skirt_distance", "skirt_distance", As::Number),
    ("brim_width", "brim_width", As::Number),
    ("raft_layers", "raft_layers", As::Whole),
    ("perimeter_speed", "perimeter_speed", As::Number),
    ("infill_speed", "infill_speed", As::Number),
    ("first_layer_speed", "first_layer_speed", As::Number),
    ("travel_speed", "travel_speed", As::Number),
    ("bridge_speed", "bridge_speed", As::Number),
    ("bridge_flow_ratio", "bridge_flow", As::Number),
    ("bridge_fan_speed", "bridge_fan", As::FanPercent),
    ("temperature", "nozzle_temp", As::Number),
    ("first_layer_temperature", "first_layer_nozzle_temp", As::Number),
    ("bed_temperature", "bed_temp", As::Number),
    ("first_layer_bed_temperature", "first_layer_bed_temp", As::Number),
    ("retract_length", "retract_length", As::Number),
    ("retract_speed", "retract_speed", As::Number),
    ("retract_before_travel", "retract_min_travel", As::Number),
    ("retract_lift", "z_hop", As::Number),
    ("max_fan_speed", "fan_speed", As::FanPercent),
    ("support_material", "support", As::Support),
    ("nozzle_diameter", "nozzle", As::Number),
    ("filament_diameter", "filament_diameter", As::Number),
    ("bed_shape", "bed", As::Plate),
    ("max_print_height", "max_height", As::Number),
    ("start_gcode", "start_gcode", As::GCode),
    ("end_gcode", "end_gcode", As::GCode),
    ("seam_position", "seam", As::Seam),
    ("spiral_vase", "vase", As::Flag),
    ("gcode_flavor", "firmware", As::Firmware),
    ("ironing", "ironing", As::Flag),
    ("use_firmware_retraction", "firmware_retraction", As::Flag),
    // OrcaSlicer and Bambu Studio
    ("wall_loops", "perimeters", As::Whole),
    ("top_shell_layers", "top_layers", As::Whole),
    ("bottom_shell_layers", "bottom_layers", As::Whole),
    ("sparse_infill_density", "infill_density", As::Share),
    ("sparse_infill_pattern", "infill_pattern", As::Pattern),
    ("skirt_loops", "skirt_loops", As::Whole),
    ("outer_wall_speed", "perimeter_speed", As::Number),
    ("sparse_infill_speed", "infill_speed", As::Number),
    ("initial_layer_speed", "first_layer_speed", As::Number),
    ("initial_layer_print_height", "first_layer_height", As::Number),
    ("bridge_flow", "bridge_flow", As::Number),
    ("nozzle_temperature", "nozzle_temp", As::Number),
    ("nozzle_temperature_initial_layer", "first_layer_nozzle_temp", As::Number),
    ("hot_plate_temp", "bed_temp", As::Number),
    ("hot_plate_temp_initial_layer", "first_layer_bed_temp", As::Number),
    ("retraction_length", "retract_length", As::Number),
    ("retraction_speed", "retract_speed", As::Number),
    ("retraction_minimum_travel", "retract_min_travel", As::Number),
    ("z_hop", "z_hop", As::Number),
    ("fan_max_speed", "fan_speed", As::FanPercent),
    ("enable_support", "support", As::Support),
    ("support_threshold_angle", "support_angle", As::Number),
    ("printable_area", "bed", As::Plate),
    ("printable_height", "max_height", As::Number),
    ("machine_start_gcode", "start_gcode", As::GCode),
    ("machine_end_gcode", "end_gcode", As::GCode),
    ("spiral_mode", "vase", As::Flag),
];

/// Keys that say what a preset is rather than what it sets: neither taken nor reported.
const ABOUT_ITSELF: &[&str] = &[
    "name",
    "inherits",
    "from",
    "type",
    "version",
    "setting_id",
    "instantiation",
    "compatible_printers",
    "filament_settings_id",
    "print_settings_id",
    "printer_settings_id",
];

/// What reading a profile gave: the fields Lamina takes, and the keys left out with why.
#[derive(Debug, Default, PartialEq)]
pub struct Imported {
    pub fields: Map<String, Value>,
    pub left_out: Vec<String>,
}

/// The first value of a JSON preset's entry (a list holds one per extruder) as text.
fn first_text(v: &Value) -> Option<String> {
    match v {
        Value::Array(a) => a.first().and_then(first_text),
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "1" } else { "0" }.into()),
        _ => None,
    }
}

fn number(t: &str) -> Option<f64> {
    t.trim().parse::<f64>().ok().filter(|x| x.is_finite())
}

/// Their value, read as `how`, or why it cannot be.
fn convert(value: &Value, how: As, ini: bool) -> Result<Value, String> {
    let text = first_text(value).ok_or("not a value")?;
    let t = text.trim();
    let bad = || format!("`{t}` cannot be read");
    Ok(match how {
        As::Number => json!(number(t).ok_or_else(bad)?),
        As::Whole => json!(number(t).filter(|x| *x >= 0.0).ok_or_else(bad)?.round() as u64),
        As::Share => {
            let (n, percent) = match t.strip_suffix('%') {
                Some(p) => (number(p), true),
                None => (number(t), false),
            };
            let n = n.ok_or_else(bad)?;
            json!(if percent || n > 1.0 { n / 100.0 } else { n })
        }
        As::FanPercent => json!((number(t.trim_end_matches('%')).ok_or_else(bad)? * 2.55).round()),
        As::Flag => json!(matches!(t, "1" | "true" | "True")),
        As::Support => json!(if matches!(t, "1" | "true" | "True") { "everywhere" } else { "off" }),
        As::Pattern => json!(match t {
            "rectilinear" | "line" | "monotonic" | "zig-zag" | "zigzag" => "lines",
            "grid" | "alignedrectilinear" => "grid",
            "triangles" | "stars" => "triangles",
            "gyroid" => "gyroid",
            "honeycomb" | "3dhoneycomb" => "honeycomb",
            _ => return Err(format!("the pattern `{t}` is not one Lamina has (lines, grid, triangles, gyroid, honeycomb)")),
        }),
        As::Seam => json!(match t {
            "aligned" => "aligned",
            "rear" | "back" => "rear",
            "nearest" | "random" => "nearest",
            _ => return Err(format!("the seam `{t}` is not one Lamina has")),
        }),
        As::Firmware => json!(match t {
            "marlin" | "marlin2" | "marlinlegacy" => "marlin",
            "klipper" => "klipper",
            "reprapfirmware" | "reprap" => "reprap",
            _ => return Err(format!("the firmware `{t}` is not one Lamina writes for")),
        }),
        As::Plate => {
            // Corner points, as one text ("0x0,220x0,...") or a list of them.
            let points: Vec<String> = match value {
                Value::Array(a) => a.iter().filter_map(first_text).collect(),
                _ => t.split(',').map(str::to_string).collect(),
            };
            let corners: Vec<[f64; 2]> =
                points.iter().filter_map(|p| p.split_once('x').and_then(|(x, y)| Some([number(x)?, number(y)?]))).collect();
            if corners.len() < 3 || corners.len() != points.len() {
                return Err(bad());
            }
            let span =
                |k: usize| corners.iter().map(|c| c[k]).fold(f64::MIN, f64::max) - corners.iter().map(|c| c[k]).fold(f64::MAX, f64::min);
            json!([span(0), span(1)])
        }
        As::GCode => {
            let code = if ini { t.replace("\\n", "\n") } else { text.clone() };
            // Its placeholders are another program's: taken only if Lamina's own reading of it works.
            Profile::default()
                .expand(&code)
                .map_err(|e| format!("its placeholders are not Lamina's ({e}); write it again in the window"))?;
            json!(code)
        }
    })
}

/// The profile in `text`, a PrusaSlicer `.ini` when `ini`, else an OrcaSlicer or Bambu Studio JSON preset.
pub fn read(text: &str, ini: bool) -> Result<Imported, String> {
    if text.len() > MAX_FILE {
        return Err(format!("the profile is larger than {} MB", MAX_FILE >> 20));
    }
    let entries: Vec<(String, Value)> = if ini {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with(';') && !l.starts_with('['))
            .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), Value::String(v.trim().to_string()))))
            .collect()
    } else {
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(m)) => m.into_iter().collect(),
            Ok(_) => return Err("a JSON preset is an object".into()),
            Err(e) => return Err(format!("the preset is not JSON: {e}")),
        }
    };
    let mut out = Imported::default();
    for (key, value) in entries {
        if ABOUT_ITSELF.contains(&key.as_str()) {
            continue;
        }
        match TABLE.iter().find(|(theirs, _, _)| *theirs == key) {
            Some((_, ours, how)) => match convert(&value, *how, ini) {
                Ok(v) => {
                    out.fields.insert((*ours).to_string(), v);
                }
                Err(e) => out.left_out.push(format!("{key}: {e}")),
            },
            None => out.left_out.push(format!("{key}: Lamina has no such setting")),
        }
    }
    // What was taken must make a valid profile together; a field that does not is dropped and said.
    let keys: Vec<String> = out.fields.keys().cloned().collect();
    for k in keys {
        let mut one = Map::new();
        if let Some(v) = out.fields.get(&k) {
            one.insert(k.clone(), v.clone());
        }
        if let Err(e) = crate::presets::apply(&Profile::default(), &one) {
            out.fields.remove(&k);
            out.left_out.push(format!("{k}: {e}"));
        }
    }
    out.left_out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prusaslicer_ini_comes_in_through_the_table() {
        let ini = "# generated by a slicer\nlayer_height = 0.15\nfill_density = 20%\nfill_pattern = gyroid\nperimeters = 3\n\
                   temperature = 215\nbed_shape = 0x0,250x0,250x210,0x210\nmax_fan_speed = 100\nsupport_material = 1\n\
                   start_gcode = G28\\nM104 S[first_layer_temperature]\nend_gcode = M84\nseam_position = rear\n\
                   gcode_flavor = klipper\nthumbnails = 16x16\nfill_angle = 45\n";
        let r = read(ini, true).unwrap();
        let f = &r.fields;
        assert_eq!(f["layer_height"], json!(0.15));
        assert_eq!(f["infill_density"], json!(0.2));
        assert_eq!(f["infill_pattern"], "gyroid");
        assert_eq!(f["perimeters"], 3);
        assert_eq!(f["bed"], json!([250.0, 210.0]));
        assert_eq!(f["fan_speed"], json!(255.0));
        assert_eq!(
            (f["support"].as_str(), f["seam"].as_str(), f["firmware"].as_str()),
            (Some("everywhere"), Some("rear"), Some("klipper"))
        );
        assert_eq!(f["end_gcode"], "M84");
        assert!(!f.contains_key("start_gcode"), "its placeholder is PrusaSlicer's");
        assert!(r.left_out.iter().any(|l| l.starts_with("start_gcode:") && l.contains("placeholders")), "{:?}", r.left_out);
        assert!(r.left_out.iter().any(|l| l.starts_with("fill_angle: Lamina has no such setting")));
        assert!(crate::presets::apply(&Profile::default(), f).is_ok());
    }

    #[test]
    fn an_orcaslicer_json_preset_comes_in_with_its_lists_and_text_numbers() {
        let json = r#"{"name": "My PETG", "inherits": "Generic PETG", "nozzle_temperature": ["245"], "hot_plate_temp": ["80"],
            "nozzle_temperature_initial_layer": ["250"], "fan_max_speed": ["40"], "wall_loops": "4", "sparse_infill_density": "35%",
            "sparse_infill_pattern": "lightning", "printable_area": ["0x0", "256x0", "256x256", "0x256"], "layer_height": "0.9",
            "filament_type": ["PETG"]}"#;
        let r = read(json, false).unwrap();
        let f = &r.fields;
        assert_eq!(
            (f["nozzle_temp"].as_f64(), f["bed_temp"].as_f64(), f["first_layer_nozzle_temp"].as_f64()),
            (Some(245.0), Some(80.0), Some(250.0))
        );
        assert_eq!((f["perimeters"].as_u64(), f["infill_density"].as_f64(), f["fan_speed"].as_f64()), (Some(4), Some(0.35), Some(102.0)));
        assert_eq!(f["bed"], json!([256.0, 256.0]));
        assert!(!f.contains_key("infill_pattern") && r.left_out.iter().any(|l| l.contains("lightning")));
        assert!(!f.contains_key("layer_height"), "0.9 mm is past what a 0.4 nozzle prints: {:?}", r.left_out);
        assert!(r.left_out.iter().any(|l| l.starts_with("filament_type")));
        assert!(!r.left_out.iter().any(|l| l.starts_with("name") || l.starts_with("inherits")));
    }

    #[test]
    fn hostile_profiles_are_errors_or_left_out_never_a_panic() {
        assert!(read("[1, 2]", false).is_err() && read("{", false).is_err());
        assert!(read(&"x".repeat(MAX_FILE + 1), true).is_err());
        let r = read("layer_height = 0.2.3\nbed_shape = 0x0,nope\nperimeters = -2\n", true).unwrap();
        assert!(r.fields.is_empty() && r.left_out.len() == 3, "{r:?}");
        let mut g = lamina_core::fuzz::Rng::new(0x1b1);
        for _ in 0..1500 {
            let _ = read(&String::from_utf8_lossy(&g.input(&[b"fill_density = 15%\nbed_shape = 0x0,200x0,200x200\n"])), true);
            let _ = read(&String::from_utf8_lossy(&g.input(&[br#"{"wall_loops": "3", "printable_area": ["0x0","1x0","1x1"]}"#])), false);
        }
    }
}
