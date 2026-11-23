//! How long a boolean of two detailed shapes takes, so a change that slows it is seen. Ignored, as timings depend on the machine:
//! `cargo test -p lamina-csg --release --test bench -- --ignored --nocapture`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Instant;

use lamina_core::{Mesh, Transform};
use lamina_csg::{Op, boolean};

#[test]
#[ignore]
fn a_boolean_of_two_detailed_shapes() {
    let a = Mesh::sphere(20.0, 96);
    let b = Mesh::cylinder(8.0, 50.0, 96).transformed(&Transform::translate([5.0, 3.0, -5.0]));
    for (name, op) in [("union", Op::Union), ("difference", Op::Difference), ("intersect", Op::Intersect)] {
        let start = Instant::now();
        let out = boolean(op, &a, &b);
        let said = out.as_ref().map_or_else(|e| format!("refused: {e}"), |m| format!("{} triangles", m.triangles.len()));
        println!("{} and {} triangles, {name}: {said} in {:.2} s", a.triangles.len(), b.triangles.len(), start.elapsed().as_secs_f64());
    }
}
