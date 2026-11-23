//! One profile: the printer, the filament and the process settings a slice needs. Every field has a default for a common 220 mm
//! Marlin-style printer, so `{}` slices, and a profile is JSON with only the fields that differ. Unknown keys are an error.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::support::Support;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Profile {
    // Printer
    /// The plate, mm, x then y.
    pub bed: [f64; 2],
    /// Whether the machine's zero is the middle of the plate (delta printers) rather than its front left corner.
    pub origin_center: bool,
    pub max_height: f64,
    pub nozzle: f64,
    pub filament_diameter: f64,
    /// G-code before and after the print. `[name]` is replaced by a setting (see `PLACEHOLDERS`); any other `[..]` is an error.
    pub start_gcode: String,
    pub end_gcode: String,
    // Process
    pub layer_height: f64,
    pub first_layer_height: f64,
    pub perimeters: u32,
    pub top_layers: u32,
    pub bottom_layers: u32,
    /// 0 to 1: the share of the inside that is filled.
    pub infill_density: f64,
    pub skirt_loops: u32,
    pub skirt_distance: f64,
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
    /// The part-cooling fan, 0 to 255, from the third layer on.
    pub fan_speed: f64,
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
            nozzle: 0.4,
            filament_diameter: 1.75,
            start_gcode: "M140 S[first_layer_bed_temp]\nM104 S[first_layer_nozzle_temp]\nG28\nM190 S[first_layer_bed_temp]\nM109 S[first_layer_nozzle_temp]".into(),
            end_gcode: "M104 S0\nM140 S0\nM107\nG91\nG1 Z10 F600\nG90\nM84".into(),
            layer_height: 0.2,
            first_layer_height: 0.2,
            perimeters: 2,
            top_layers: 4,
            bottom_layers: 4,
            infill_density: 0.15,
            skirt_loops: 1,
            skirt_distance: 3.0,
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
            fan_speed: 255.0,
            support: Support::Off,
            support_angle: 45.0,
            support_density: 0.15,
            support_z_gap: 0.2,
            support_xy_gap: 0.6,
            support_interface_layers: 2,
        }
    }
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
        within("max_height", self.max_height, 1.0, 2000.0)?;
        within("nozzle", self.nozzle, 0.1, 2.0)?;
        within("filament_diameter", self.filament_diameter, 1.0, 3.5)?;
        within("layer_height", self.layer_height, 0.02, self.nozzle * 0.8)?;
        within("first_layer_height", self.first_layer_height, 0.02, self.nozzle * 0.8)?;
        within("perimeters", f64::from(self.perimeters), 0.0, 10.0)?;
        within("top_layers", f64::from(self.top_layers), 0.0, 50.0)?;
        within("bottom_layers", f64::from(self.bottom_layers), 0.0, 50.0)?;
        within("infill_density", self.infill_density, 0.0, 1.0)?;
        within("skirt_loops", f64::from(self.skirt_loops), 0.0, 10.0)?;
        within("skirt_distance", self.skirt_distance, 0.0, 50.0)?;
        for (what, s) in [
            ("perimeter_speed", self.perimeter_speed),
            ("infill_speed", self.infill_speed),
            ("first_layer_speed", self.first_layer_speed),
            ("travel_speed", self.travel_speed),
        ] {
            within(what, s, 1.0, 1000.0)?;
        }
        for (what, t) in [
            ("nozzle_temp", self.nozzle_temp),
            ("first_layer_nozzle_temp", self.first_layer_nozzle_temp),
            ("bed_temp", self.bed_temp),
            ("first_layer_bed_temp", self.first_layer_bed_temp),
        ] {
            within(what, t, 0.0, 400.0)?;
        }
        within("retract_length", self.retract_length, 0.0, 20.0)?;
        within("retract_speed", self.retract_speed, 1.0, 200.0)?;
        within("retract_min_travel", self.retract_min_travel, 0.0, 100.0)?;
        within("fan_speed", self.fan_speed, 0.0, 255.0)?;
        within("support_angle", self.support_angle, 10.0, 89.0)?;
        within("support_density", self.support_density, 0.05, 1.0)?;
        within("support_z_gap", self.support_z_gap, 0.0, 2.0)?;
        within("support_xy_gap", self.support_xy_gap, 0.0, 5.0)?;
        within("support_interface_layers", f64::from(self.support_interface_layers), 0.0, 10.0)?;
        self.expand(&self.start_gcode)?;
        self.expand(&self.end_gcode)?;
        Ok(())
    }

    /// `text` with each `[name]` replaced by that setting.
    pub fn expand(&self, text: &str) -> Result<String, String> {
        let mut out = String::new();
        let mut rest = text;
        while let Some(open) = rest.find('[') {
            out.push_str(&rest[..open]);
            let close = rest[open..].find(']').ok_or("a `[` in the G-code is never closed")? + open;
            let name = &rest[open + 1..close];
            let v = match name {
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
                other => return Err(format!("unknown placeholder `[{other}]` in the G-code; known: {}", PLACEHOLDERS.join(", "))),
            };
            out.push_str(&format!("{v}"));
            rest = &rest[close + 1..];
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
            json!({"support": "everywhere", "support_angle": 0}),
        ] {
            assert!(Profile::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn placeholders_are_filled() {
        let p = Profile::default();
        assert!(p.expand(&p.start_gcode).unwrap().contains("M104 S215"));
    }
}
