//! One profile: the printer, the filament and the process settings a slice needs. Every field has a default for a common 220 mm
//! Marlin-style printer, so `{}` slices, and a profile is JSON with only the fields that differ. Unknown keys are an error.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::support::Support;

/// The printer's firmware.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Firmware {
    #[default]
    Marlin,
    Klipper,
    /// RepRapFirmware (Duet).
    Reprap,
}

/// Where a wall loop starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Seam {
    /// Near the seam of the layer below, so the marks make one tidy line.
    #[default]
    Aligned,
    /// At the back (greatest y), out of sight from the front.
    Rear,
    /// In the sharpest inside corner, where a mark hides; else the sharpest corner.
    Sharpest,
    /// Wherever the head already is: the least travel, the marks scattered.
    Nearest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Profile {
    // Printer
    /// The plate, mm, x then y.
    pub bed: [f64; 2],
    /// Whether the machine's zero is the middle of the plate (delta printers) rather than its front left corner.
    pub origin_center: bool,
    pub max_height: f64,
    /// The filament's pressure advance (linear advance on Marlin); 0 leaves the printer's own.
    pub pressure_advance: f64,
    /// The printer's acceleration, mm/s2, and its junction deviation, mm (Marlin's; for Klipper's square corner velocity `scv`,
    /// `scv^2 x 0.414 / acceleration`): what the time estimate plans moves with.
    pub max_acceleration: f64,
    pub junction_deviation: f64,
    pub nozzle: f64,
    pub filament_diameter: f64,
    /// G-code before and after the print. `[name]` is replaced by a setting (see `PLACEHOLDERS`); any other `[..]` is an error.
    pub start_gcode: String,
    pub end_gcode: String,
    /// The printer's firmware: Klipper gets object markers, and each its own pressure advance command.
    pub firmware: Firmware,
    /// Retract with the firmware's G10 and G11 instead of moving the filament in the G-code.
    pub firmware_retraction: bool,
    /// A picture of the plate this many pixels square in the G-code, for the printer's screen; 0 for none.
    pub thumbnail: u32,
    // Process
    pub layer_height: f64,
    pub first_layer_height: f64,
    pub perimeters: u32,
    /// Vase mode: above the bottom layers, one wall climbing without a stop, no infill, an open top.
    pub vase: bool,
    /// Adaptive layers: each layer from `min_layer_height` to `layer_height`, thin where a surface is near flat (so its steps stay
    /// small) and thick on walls. The plate's layers, shared by every object.
    pub adaptive_layers: bool,
    pub min_layer_height: f64,
    /// Ironing: a pass over each top surface with lines `ironing_spacing` apart, `ironing_flow` of a line's plastic, at
    /// `ironing_speed`, which melts its ridges flat.
    pub ironing: bool,
    pub ironing_flow: f64,
    pub ironing_spacing: f64,
    pub ironing_speed: f64,
    /// Fuzzy skin: the outer wall's points every `fuzzy_spacing` mm moved across it by up to `fuzzy_thickness` mm, for a rough grip.
    pub fuzzy_skin: bool,
    pub fuzzy_thickness: f64,
    pub fuzzy_spacing: f64,
    pub top_layers: u32,
    pub bottom_layers: u32,
    /// 0 to 1: the share of the inside that is filled.
    pub infill_density: f64,
    /// The pattern of the sparse infill: lines, grid, triangles, gyroid or honeycomb.
    pub infill_pattern: crate::infill::Pattern,
    pub skirt_loops: u32,
    pub skirt_distance: f64,
    /// How far a brim reaches out from the first layer's outer walls, mm; 0 for none. A raft takes its place.
    pub brim_width: f64,
    /// Layers of coarse lines under the objects, which are lifted onto them; 0 for none.
    pub raft_layers: u32,
    // Speeds, mm/s
    pub perimeter_speed: f64,
    pub infill_speed: f64,
    pub first_layer_speed: f64,
    pub travel_speed: f64,
    // Temperatures, °C
    pub nozzle_temp: f64,
    pub first_layer_nozzle_temp: f64,
    pub bed_temp: f64,
    pub first_layer_bed_temp: f64,
    // Retraction and cooling
    pub retract_length: f64,
    pub retract_speed: f64,
    /// A travel shorter than this is made without retracting, mm.
    pub retract_min_travel: f64,
    /// Travel inside the part without retracting, however far: it crosses no gap, so nothing strings.
    pub combing: bool,
    /// Lift the nozzle by this much while it travels retracted, mm; 0 for none.
    pub z_hop: f64,
    /// Retract while moving this far back along the stroke just printed, mm; 0 for none.
    pub wipe_distance: f64,
    /// The part-cooling fan, 0 to 255: off for the first layer, then up in equal steps to this by `fan_full_layer`.
    pub fan_speed: f64,
    pub fan_full_layer: u32,
    /// A layer printed faster than this, seconds, is slowed to take this long, but no move under `min_print_speed` (mm/s).
    pub min_layer_time: f64,
    pub min_print_speed: f64,
    /// Where each wall loop starts, which leaves a small mark.
    pub seam: Seam,
    // Bridges: solid infill over air
    pub bridge_speed: f64,
    /// Its flow, as a share of the normal flow.
    pub bridge_flow: f64,
    /// The part-cooling fan while a bridge prints, 0 to 255.
    pub bridge_fan: f64,
    // Supports
    pub support: Support,
    /// Degrees from vertical beyond which a surface is held up.
    pub support_angle: f64,
    /// 0 to 1: how much of the support's area its lines fill.
    pub support_density: f64,
    /// Space between the support's top and the model, mm (rounded to whole layers), and between the support and the model's walls.
    pub support_z_gap: f64,
    pub support_xy_gap: f64,
    /// Denser layers at the top of the support, under the model.
    pub support_interface_layers: u32,
}

pub const PLACEHOLDERS: &[&str] = &[
    "nozzle_temp",
    "first_layer_nozzle_temp",
    "bed_temp",
    "first_layer_bed_temp",
    "layer_height",
    "first_layer_height",
    "nozzle",
    "filament_diameter",
    "bed_x",
    "bed_y",
    "max_height",
];

impl Default for Profile {
    fn default() -> Profile {
        Profile {
            bed: [220.0, 220.0],
            origin_center: false,
            max_height: 250.0,
            pressure_advance: 0.0,
            max_acceleration: 1500.0,
            junction_deviation: 0.013,
            nozzle: 0.4,
            filament_diameter: 1.75,
            start_gcode: "M140 S[first_layer_bed_temp]\nM104 S[first_layer_nozzle_temp]\nG28\nM190 S[first_layer_bed_temp]\nM109 S[first_layer_nozzle_temp]".into(),
            end_gcode: "M104 S0\nM140 S0\nM107\nG91\nG1 Z10 F600\nG90\nM84".into(),
            firmware: Firmware::Marlin,
            firmware_retraction: false,
            thumbnail: 0,
            layer_height: 0.2,
            first_layer_height: 0.2,
            perimeters: 2,
            vase: false,
            adaptive_layers: false,
            min_layer_height: 0.08,
            ironing: false,
            ironing_flow: 0.1,
            ironing_spacing: 0.1,
            ironing_speed: 15.0,
            fuzzy_skin: false,
            fuzzy_thickness: 0.3,
            fuzzy_spacing: 0.8,
            top_layers: 4,
            bottom_layers: 4,
            infill_density: 0.15,
            infill_pattern: crate::infill::Pattern::Lines,
            skirt_loops: 1,
            skirt_distance: 3.0,
            brim_width: 0.0,
            raft_layers: 0,
            perimeter_speed: 45.0,
            infill_speed: 80.0,
            first_layer_speed: 20.0,
            travel_speed: 150.0,
            nozzle_temp: 210.0,
            first_layer_nozzle_temp: 215.0,
            bed_temp: 60.0,
            first_layer_bed_temp: 60.0,
            retract_length: 0.8,
            retract_speed: 35.0,
            retract_min_travel: 1.5,
            combing: true,
            z_hop: 0.0,
            wipe_distance: 0.0,
            fan_speed: 255.0,
            fan_full_layer: 3,
            min_layer_time: 5.0,
            min_print_speed: 10.0,
            seam: Seam::Aligned,
            bridge_speed: 25.0,
            bridge_flow: 1.0,
            bridge_fan: 255.0,
            support: Support::Off,
            support_angle: 45.0,
            support_density: 0.15,
            support_z_gap: 0.2,
            support_xy_gap: 0.6,
            support_interface_layers: 2,
        }
    }
}

/// Each number's range, as `check` enforces it and the window shows it. The layer heights are also no more than 0.8 of the nozzle,
/// the plate's sides from 10 to 2000 mm, and a thumbnail 0 or from 16 to 512 pixels.
pub const LIMITS: &[(&str, f64, f64)] = &[
    ("max_height", 1.0, 2000.0),
    ("pressure_advance", 0.0, 2.0),
    ("max_acceleration", 10.0, 100_000.0),
    ("junction_deviation", 0.0, 1.0),
    ("nozzle", 0.1, 2.0),
    ("filament_diameter", 1.0, 3.5),
    ("layer_height", 0.02, 1.6),
    ("first_layer_height", 0.02, 1.6),
    ("min_layer_height", 0.02, 1.6),
    ("ironing_flow", 0.01, 0.5),
    ("ironing_spacing", 0.05, 1.0),
    ("ironing_speed", 1.0, 300.0),
    ("fuzzy_thickness", 0.05, 2.0),
    ("fuzzy_spacing", 0.1, 5.0),
    ("perimeters", 0.0, 10.0),
    ("top_layers", 0.0, 50.0),
    ("bottom_layers", 0.0, 50.0),
    ("infill_density", 0.0, 1.0),
    ("skirt_loops", 0.0, 10.0),
    ("skirt_distance", 0.0, 50.0),
    ("brim_width", 0.0, 50.0),
    ("raft_layers", 0.0, 10.0),
    ("perimeter_speed", 1.0, 1000.0),
    ("infill_speed", 1.0, 1000.0),
    ("first_layer_speed", 1.0, 1000.0),
    ("travel_speed", 1.0, 1000.0),
    ("nozzle_temp", 0.0, 400.0),
    ("first_layer_nozzle_temp", 0.0, 400.0),
    ("bed_temp", 0.0, 400.0),
    ("first_layer_bed_temp", 0.0, 400.0),
    ("retract_length", 0.0, 20.0),
    ("retract_speed", 1.0, 200.0),
    ("retract_min_travel", 0.0, 100.0),
    ("z_hop", 0.0, 5.0),
    ("wipe_distance", 0.0, 10.0),
    ("fan_speed", 0.0, 255.0),
    ("fan_full_layer", 1.0, 100.0),
    ("min_layer_time", 0.0, 120.0),
    ("min_print_speed", 1.0, 100.0),
    ("bridge_speed", 1.0, 1000.0),
    ("bridge_flow", 0.5, 1.5),
    ("bridge_fan", 0.0, 255.0),
    ("support_angle", 10.0, 89.0),
    ("support_density", 0.05, 1.0),
    ("support_z_gap", 0.0, 2.0),
    ("support_xy_gap", 0.0, 5.0),
    ("support_interface_layers", 0.0, 10.0),
];

/// A number field's range, from [`LIMITS`].
pub fn limits(field: &str) -> Option<(f64, f64)> {
    LIMITS.iter().find(|(name, _, _)| *name == field).map(|&(_, lo, hi)| (lo, hi))
}

/// What each setting does, in a sentence: the window's tooltips and the user guide read it. Every field of the profile has one (a
/// test checks).
pub const ABOUT: &[(&str, &str)] = &[
    ("bed", "The plate's size, mm, x then y."),
    ("origin_center", "Whether the printer's zero is the middle of the plate (most delta printers) instead of its front left corner."),
    ("max_height", "The tallest the printer can print, mm."),
    ("pressure_advance", "The filament's pressure advance (linear advance on Marlin), sent at the start; 0 leaves the printer's own."),
    ("max_acceleration", "The printer's acceleration, mm/s2: the print time estimate plans each move with it."),
    ("junction_deviation", "How fast the printer takes corners (Marlin's junction deviation, mm); used for the time estimate."),
    ("nozzle", "The nozzle's diameter, mm: lines are this wide, and layers at most 80 % of it."),
    ("filament_diameter", "The filament's diameter, mm, usually 1.75."),
    ("start_gcode", "G-code sent before the print; [name] is replaced by that setting, and {expression} by its value."),
    ("end_gcode", "G-code sent after the print; [name] is replaced by that setting, and {expression} by its value."),
    ("firmware", "The printer's firmware: Klipper also gets object markers, so one failed object can be cancelled alone."),
    ("firmware_retraction", "Retract with the firmware's G10 and G11, so the printer's own retraction settings apply."),
    ("thumbnail", "A picture of the plate this many pixels square, for the printer's screen; 0 for none."),
    ("layer_height", "The height of each layer, mm: thinner is finer and slower."),
    ("first_layer_height", "The height of the first layer, mm: a thicker one sticks better to an uneven plate."),
    ("adaptive_layers", "Thin layers where a surface is nearly flat, so its steps hardly show, and thick ones on walls, to save time."),
    ("min_layer_height", "The thinnest an adaptive layer may be, mm; the thickest is the layer height."),
    ("ironing", "A second pass over each top surface with a trickle of plastic, which melts its ridges flat; slower, smoother tops."),
    ("ironing_flow", "How much plastic ironing lays, as a share of a full line: 0.1 is usual."),
    ("ironing_spacing", "How far apart the ironing lines are, mm: much closer than the line width."),
    ("ironing_speed", "How fast ironing moves, mm/s."),
    ("fuzzy_skin", "Roughen the outer wall with small random bumps, for a textured grip that hides layer lines."),
    ("fuzzy_thickness", "How far a fuzzy skin's bumps stand in or out, mm."),
    ("fuzzy_spacing", "How far apart a fuzzy skin's bumps are along the wall, mm."),
    ("perimeters", "Walls round each layer: more is stronger and slower."),
    ("vase", "Vase mode: above the bottom, one wall that climbs without stopping, with no infill and an open top."),
    ("top_layers", "Solid layers that close the top of the part."),
    ("bottom_layers", "Solid layers at the bottom of the part."),
    ("infill_density", "How much of the inside is filled, from 0 to 1 (the window shows it in %)."),
    ("infill_pattern", "The pattern of the sparse infill: lines, grid, triangles, gyroid or honeycomb."),
    ("skirt_loops", "Loops round the part on the first layer, not touching it, to start the flow; 0 for none."),
    ("skirt_distance", "How far the skirt stands from the part, mm."),
    ("brim_width", "Loops round the first layer, touching it, that hold its edges down, mm wide; 0 for none."),
    ("raft_layers", "Layers of lines under the part, which is lifted onto them; 0 for none."),
    ("perimeter_speed", "How fast walls print, mm/s."),
    ("infill_speed", "How fast infill and support print, mm/s."),
    ("first_layer_speed", "How fast the first layer prints, mm/s: slower sticks better."),
    ("travel_speed", "How fast the head moves when it is not printing, mm/s."),
    ("nozzle_temp", "The nozzle's temperature, C."),
    ("first_layer_nozzle_temp", "The nozzle's temperature for the first layer, C."),
    ("bed_temp", "The plate's temperature, C; 0 for an unheated plate."),
    ("first_layer_bed_temp", "The plate's temperature for the first layer, C."),
    ("retract_length", "How far the filament is pulled back before a travel, mm, so it does not ooze and string."),
    ("retract_speed", "How fast the filament is pulled back and pushed again, mm/s."),
    ("retract_min_travel", "A travel shorter than this is made without retracting, mm."),
    ("combing", "Travel inside the part without retracting, however far: it crosses no gap, so nothing strings."),
    ("z_hop", "Lift the nozzle by this much while it travels, mm, so it does not knock the part; 0 for none."),
    ("wipe_distance", "Retract while moving back along the line just printed, mm, which leaves a smaller blob; 0 for none."),
    ("fan_speed", "The part-cooling fan, 0 to 255: off for the first layer, then up in steps to this."),
    ("fan_full_layer", "The layer from which the fan runs at its full setting."),
    ("min_layer_time", "A layer that would print faster than this, s, is slowed so it cools before the next."),
    ("min_print_speed", "The slowest a layer is slowed to cool, mm/s."),
    ("seam", "Where each wall starts and ends, which leaves a small mark."),
    ("bridge_speed", "How fast a bridge (solid infill over air) prints, mm/s."),
    ("bridge_flow", "A bridge's flow as a share of the normal flow: under 1 makes thinner strands that sag less."),
    ("bridge_fan", "The part-cooling fan while a bridge prints, 0 to 255."),
    ("support", "Supports under overhangs: off, everywhere, or only those that stand on the plate."),
    ("support_angle", "A surface leaning further than this from vertical, degrees, is held up."),
    ("support_density", "How much of the support's area its lines fill, from 0 to 1."),
    ("support_z_gap", "The space between the top of a support and the part, mm, so it comes away."),
    ("support_xy_gap", "The space between a support and the part's walls, mm."),
    ("support_interface_layers", "Denser layers at the top of a support, under the part, for a cleaner underside."),
];

/// What a setting does, from [`ABOUT`]; empty for a name that is not a setting.
pub fn about(field: &str) -> &'static str {
    ABOUT.iter().find(|(name, _)| *name == field).map_or("", |&(_, text)| text)
}

fn within(what: &str, x: f64, lo: f64, hi: f64) -> Result<(), String> {
    if x.is_finite() && x >= lo && x <= hi { Ok(()) } else { Err(format!("`{what}` must be from {lo} to {hi}, not {x}")) }
}

impl Profile {
    /// A profile from JSON: the defaults with these fields changed, checked.
    pub fn from_json(v: &Value) -> Result<Profile, String> {
        let p =
            if v.is_null() { Profile::default() } else { serde_json::from_value(v.clone()).map_err(|e| format!("bad profile: {e}"))? };
        p.check().map(|()| p)
    }

    /// The width of one extruded line: a little wider than the nozzle, as slicers use.
    pub fn line_width(&self) -> f64 {
        self.nozzle * 1.125
    }

    pub fn check(&self) -> Result<(), String> {
        within("bed", self.bed[0].min(self.bed[1]), 10.0, 2000.0)?;
        within("bed", self.bed[0].max(self.bed[1]), 10.0, 2000.0)?;
        if self.thumbnail != 0 {
            within("thumbnail", f64::from(self.thumbnail), 16.0, 512.0)?;
        }
        // Every number in the table; the layer heights also no more than 0.8 of the nozzle.
        let all = serde_json::to_value(self).map_err(|e| e.to_string())?;
        for &(name, lo, hi) in LIMITS {
            let v = all.get(name).and_then(Value::as_f64).ok_or_else(|| format!("internal: `{name}` is not a number"))?;
            let hi = if name.ends_with("layer_height") { hi.min(self.nozzle * 0.8) } else { hi };
            within(name, v, lo, hi)?;
        }
        if self.adaptive_layers && self.min_layer_height > self.layer_height {
            return Err(format!(
                "`min_layer_height` ({}) must be no more than `layer_height` ({}): adaptive layers go from one to the other",
                self.min_layer_height, self.layer_height
            ));
        }
        self.expand(&self.start_gcode)?;
        self.expand(&self.end_gcode)?;
        Ok(())
    }

    /// A setting by the name `[name]` and `{expr}` know it.
    fn placeholder(&self, name: &str) -> Option<f64> {
        Some(match name {
            "nozzle_temp" => self.nozzle_temp,
            "first_layer_nozzle_temp" => self.first_layer_nozzle_temp,
            "bed_temp" => self.bed_temp,
            "first_layer_bed_temp" => self.first_layer_bed_temp,
            "layer_height" => self.layer_height,
            "first_layer_height" => self.first_layer_height,
            "nozzle" => self.nozzle,
            "filament_diameter" => self.filament_diameter,
            "bed_x" => self.bed[0],
            "bed_y" => self.bed[1],
            "max_height" => self.max_height,
            _ => return None,
        })
    }

    /// `text` with each `[name]` replaced by that setting and each `{expr}` by its value (`crate::expr`); `{{` is a `{`.
    pub fn expand(&self, text: &str) -> Result<String, String> {
        let mut out = String::new();
        let mut rest = text;
        while let Some(open) = rest.find(['[', '{']) {
            out.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            if rest[open..].starts_with("{{") {
                out.push('{');
                rest = &rest[open + 2..];
                continue;
            }
            let (closer, what) = if rest[open..].starts_with('[') { (']', "[") } else { ('}', "{") };
            let close = after.find(closer).ok_or_else(|| format!("a `{what}` in the G-code is never closed"))?;
            let inside = &after[..close];
            if what == "[" {
                let v = self
                    .placeholder(inside)
                    .ok_or_else(|| format!("unknown placeholder `[{inside}]` in the G-code; known: {}", PLACEHOLDERS.join(", ")))?;
                out.push_str(&format!("{v}"));
            } else {
                let v = crate::expr::eval(inside, &|n| self.placeholder(n)).map_err(|e| format!("in `{{{inside}}}`: {e}"))?;
                out.push_str(&crate::expr::show(v));
            }
            rest = &after[close + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_defaults_are_a_valid_profile_and_a_partial_one_changes_only_what_it_names() {
        assert_eq!(Profile::from_json(&Value::Null).unwrap(), Profile::default());
        let p = Profile::from_json(&json!({"nozzle": 0.6, "layer_height": 0.3})).unwrap();
        assert_eq!((p.nozzle, p.layer_height, p.perimeters), (0.6, 0.3, 2));
    }

    #[test]
    fn hostile_profiles_are_errors() {
        for bad in [
            json!({"nozzle": 0}),
            json!({"layer_height": 0.5}),
            json!({"infill_density": 2}),
            json!({"nozzle_temp": 9999}),
            json!({"travel_speed": null}),
            json!({"layer_hieght": 0.2}),
            json!({"start_gcode": "M104 S[nope]"}),
            json!({"start_gcode": "M104 S[nozzle_temp"}),
            json!({"bed": [0, 0]}),
            json!({"support": "sometimes"}),
            json!({"seam": "random"}),
            json!({"infill_pattern": "cubic"}),
            json!({"support": "everywhere", "support_angle": 0}),
        ] {
            assert!(Profile::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn hostile_profiles_never_panic_and_a_slice_of_any_valid_one_finishes() {
        use lamina_core::fuzz::{Rng, json as random};
        let keys: Vec<String> = serde_json::to_value(Profile::default()).unwrap().as_object().unwrap().keys().cloned().collect();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let cube = vec![(1, lamina_core::Mesh::cuboid([4.0, 4.0, 1.0]))];
        let mut r = Rng::new(0x9f0);
        let mut sliced = 0;
        for _ in 0..3000 {
            let mut v = random(&mut r, &keys, 2);
            if !v.is_object() {
                v = json!({});
            }
            if let Ok(p) = Profile::from_json(&v) {
                // A thin layer on a valid profile is slow, not wrong: keep the slices few and small.
                if sliced < 40 && p.layer_height >= 0.1 && p.first_layer_height >= 0.1 {
                    sliced += 1;
                    let _ = crate::print::gcode(&cube, &p);
                }
            }
        }
        // Text in the G-code fields: any `[` or `]` is a placeholder or an error, never a panic.
        for _ in 0..3000 {
            let text = String::from_utf8_lossy(&r.input(&[b"M104 S[nozzle_temp]\nG28 [bed_x]"])).into_owned();
            let _ = Profile::default().expand(&text);
        }
    }

    #[test]
    fn every_number_of_the_profile_has_its_limits_in_the_table() {
        let all = serde_json::to_value(Profile::default()).unwrap();
        for (k, v) in all.as_object().unwrap() {
            if v.is_number() && k != "thumbnail" {
                assert!(limits(k).is_some(), "`{k}` has no limits");
            }
        }
        assert!(LIMITS.iter().all(|(k, _, _)| all.get(*k).is_some_and(Value::is_number)), "a limit for a field that is not a number");
    }

    #[test]
    fn every_setting_says_what_it_does() {
        let all = serde_json::to_value(Profile::default()).unwrap();
        for k in all.as_object().unwrap().keys() {
            assert!(!about(k).is_empty(), "`{k}` has no description in ABOUT");
        }
        assert!(ABOUT.iter().all(|(k, _)| all.get(*k).is_some()), "a description for a field the profile does not have");
        assert_eq!(about("nonsense"), "");
    }

    #[test]
    fn expressions_in_braces_are_worked_out_beside_the_placeholders() {
        let p = Profile { bed_temp: 100.0, first_layer_bed_temp: 105.0, ..Profile::default() };
        assert_eq!(p.expand("M190 S{first_layer_bed_temp - 5}").unwrap(), "M190 S100");
        assert_eq!(p.expand("{if(bed_temp > 90, 1, 0)} [bed_temp]").unwrap(), "1 100");
        assert_eq!(p.expand("SET {{x}} {round(nozzle * 100)}").unwrap(), "SET {x}} 40");
        for bad in ["{nope}", "{1 +", "{1/0}", "[nope]", "{if(1,2)}"] {
            assert!(p.expand(bad).is_err(), "{bad}");
        }
        assert!(Profile { start_gcode: "M104 S{nozzle_temp + }".into(), ..Profile::default() }.check().is_err());
    }

    #[test]
    fn placeholders_are_filled() {
        let p = Profile::default();
        assert!(p.expand(&p.start_gcode).unwrap().contains("M104 S215"));
    }
}
