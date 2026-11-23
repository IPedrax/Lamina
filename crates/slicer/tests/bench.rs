//! How long the slicer takes on a heavy plate, so a change that slows it is seen. Ignored, as timings depend on the machine:
//! `cargo test -p lamina-slicer --release --test bench -- --ignored --nocapture`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Instant;

use lamina_core::{Mesh, Transform};
use lamina_slicer::print;
use lamina_slicer::profile::Profile;

/// About 100,000 triangles: five detailed spheres in a row.
fn heavy() -> Vec<(u32, Mesh)> {
    (0..5).map(|k| (k + 1, Mesh::sphere(15.0, 256).transformed(&Transform::translate([f64::from(k) * 35.0 - 70.0, 0.0, 0.0])))).collect()
}

#[test]
#[ignore]
fn slicing_a_hundred_thousand_triangles() {
    let plate = heavy();
    let triangles: usize = plate.iter().map(|(_, m)| m.triangles.len()).sum();
    for (name, p) in [
        ("defaults", Profile::default()),
        (
            "gyroid 20 %, supports",
            Profile {
                infill_pattern: lamina_slicer::infill::Pattern::Gyroid,
                infill_density: 0.2,
                support: lamina_slicer::profile::Support::Everywhere,
                ..Profile::default()
            },
        ),
        ("adaptive layers", Profile { adaptive_layers: true, ..Profile::default() }),
    ] {
        let start = Instant::now();
        let out = print::gcode(&plate, &p).unwrap();
        println!("{triangles} triangles, {name}: {} layers in {:.2} s", out.layers, start.elapsed().as_secs_f64());
    }
}
