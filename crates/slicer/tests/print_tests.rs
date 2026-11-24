//! The three models of docs/PRINT-TESTS.md, built by the scripts in docs/print-tests and sliced as that page says, for a Marlin and a
//! Klipper preset: each builds the part it promises and slices with what the test is about (one wall, bridges). Nothing here says
//! how a print comes out: that is the page's table, filled in at a printer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use lamina_core::Session;
use serde_json::{Value, json};

fn built(script: &str) -> Session {
    let mut s = Session::new();
    s.register(lamina_slicer::COMMANDS);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/print-tests").join(script);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    s.script(&text).unwrap_or_else(|e| panic!("{script}: {e}"));
    s
}

const PRINTERS: [&str; 2] = ["Generic 220 mm, Marlin", "Generic 300 mm, Klipper"];

fn check(s: &mut Session, printer: &str, profile: Value) -> Value {
    let p = json!({"printer": printer, "filament": "PLA", "process": "0.20 mm Standard", "profile": profile});
    s.execute("slice_check", &p).unwrap_or_else(|e| panic!("{printer}: {e}"))
}

fn info(s: &mut Session) -> Value {
    s.execute("info", &json!({})).unwrap()["objects"][0].clone()
}

#[test]
fn the_cube_is_20_mm_each_way_and_slices_on_both_printers() {
    let mut s = built("cube.txt");
    let o = info(&mut s);
    assert_eq!(o["size"], json!([20.0, 20.0, 20.0]));
    assert_eq!(o["watertight"], true);
    for printer in PRINTERS {
        let r = check(&mut s, printer, json!({}));
        assert_eq!(r["layers"], 100, "{printer}");
    }
}

#[test]
fn the_single_wall_box_prints_one_wall_and_a_bottom() {
    let mut s = built("single-wall-box.txt");
    assert_eq!(info(&mut s)["size"], json!([20.0, 20.0, 15.0]));
    for printer in PRINTERS {
        let r = check(&mut s, printer, json!({"vase": true, "bottom_layers": 1}));
        // Filament by kind, mm: above its one bottom layer (whose walls and skin are as usual) the box is one outer wall.
        let f = &r["features"];
        let (outer, inner) = (f["outer-wall"].as_f64().unwrap(), f["inner-wall"].as_f64().unwrap_or(0.0));
        assert!(outer > 100.0 && inner < 0.05 * outer && f.get("sparse-infill").is_none(), "{printer}: {f}");
    }
}

#[test]
fn the_bridge_test_stands_up_and_bridges_its_three_windows() {
    let mut s = built("bridge.txt");
    let o = info(&mut s);
    assert_eq!(o["size"], json!([85.0, 10.0, 10.0]), "standing: 85 long, 10 deep, 10 tall");
    assert_eq!(o["watertight"], true);
    // Bar minus windows of 10, 20 and 30 mm, 7 mm tall, all 10 deep.
    assert!((o["volume_mm3"].as_f64().unwrap() - (85.0 * 10.0 - 60.0 * 7.0) * 10.0).abs() < 1.0, "{}", o["volume_mm3"]);
    for printer in PRINTERS {
        let r = check(&mut s, printer, json!({}));
        assert!(r["features"]["bridge"].as_f64().unwrap() > 10.0, "{printer}: {}", r["features"]);
    }
    // The bridges are the first layer over the windows' tops (z 8), so the part stands with its base on the plate.
    let mesh = s.scene.objects[0].world();
    let g = lamina_slicer::print::gcode(&[(1, mesh)], &lamina_slicer::profile::Profile::default()).unwrap().gcode;
    let mut z = 0.0;
    let mut bridged = Vec::new();
    for l in g.lines() {
        if let Some(v) = l.strip_prefix("G1 Z").and_then(|v| v.split(' ').next()?.parse::<f64>().ok()) {
            z = v;
        }
        if l == ";TYPE:bridge" && !bridged.contains(&z) {
            bridged.push(z);
        }
    }
    assert_eq!(bridged, [8.2]);
}
