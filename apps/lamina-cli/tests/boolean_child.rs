//! Booleans in a child process, with the real program as the child: what `lamina-cli` (and the desktop) do for every boolean.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::time::Duration;

use lamina_core::{Mesh, Transform};
use lamina_csg::{Op, in_child};

const I: Transform = Transform::IDENTITY;

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_lamina-cli"))
}

#[test]
fn a_boolean_runs_in_the_child_and_comes_back_closed() {
    let cube = Mesh::cuboid([20.0, 20.0, 10.0]);
    let tool = Mesh::cylinder(5.0, 14.0, 32).transformed(&lamina_core::Transform::translate([0.0, 0.0, -2.0]));
    let m = in_child(exe(), Op::Difference, (&cube, I), (&tool, I), Duration::from_secs(120)).unwrap();
    assert!(m.is_watertight() && m.volume() < 4000.0 && m.volume() > 3000.0, "{}", m.volume());
    // A tilted tool crosses the pipe with its transform: the hole goes through at 30 degrees, or the boolean says it cannot.
    let turned = Transform::rotate(0, 30.0).then(&Transform::translate([0.0, 5.0, -6.0]));
    match in_child(exe(), Op::Difference, (&cube, I), (&Mesh::cylinder(3.0, 30.0, 24), turned), Duration::from_secs(120)) {
        Ok(m) => assert!(m.is_watertight() && m.volume() < 4000.0 - 200.0, "{}", m.volume()),
        Err(e) => assert!(e.contains("hundredth"), "{e}"),
    }
    // What the engine refuses is the child's error, word for word.
    let mut open = Mesh::cuboid([5.0, 5.0, 5.0]);
    open.triangles.pop();
    assert!(in_child(exe(), Op::Union, (&cube, I), (&open, I), Duration::from_secs(60)).unwrap_err().contains("not a closed mesh"));
}

#[test]
fn a_child_that_is_stopped_is_an_error_and_the_next_boolean_still_works() {
    let (a, b) = (Mesh::sphere(10.0, 128), Mesh::sphere(10.0, 128).transformed(&lamina_core::Transform::translate([5.0, 1.0, 2.0])));
    let err = in_child(exe(), Op::Union, (&a, I), (&b, I), Duration::from_millis(1)).unwrap_err();
    assert!(err.contains("stopped"), "{err}");
    let (c, d) = (
        Mesh::cuboid([10.0, 10.0, 10.0]),
        Mesh::cuboid([10.0, 10.0, 10.0]).transformed(&lamina_core::Transform::translate([5.0, 0.0, 0.0])),
    );
    assert!(in_child(exe(), Op::Union, (&c, I), (&d, I), Duration::from_secs(60)).unwrap().is_watertight());
}

#[test]
fn the_cli_runs_its_booleans_in_a_child() {
    let out = std::process::Command::new(exe())
        .args([
            "run",
            "--cmd",
            r#"add_primitive {"kind":"box","size":[10,10,10]}"#,
            "--cmd",
            r#"add_primitive {"kind":"box","size":[4,4,14],"at":[0,0]}"#,
            "--cmd",
            r#"move {"id":2,"by":[0,0,-2]}"#,
            "--cmd",
            r#"difference {"id":1,"minus":[2]}"#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let last = String::from_utf8_lossy(&out.stdout).lines().last().unwrap().to_string();
    assert!(last.contains("\"watertight\":true"), "{last}");
}
