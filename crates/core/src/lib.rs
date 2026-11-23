//! Lamina's core: meshes, the scene, STL files, and the command engine every front end drives. No UI and no network in here.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod base64;
pub mod bridge;
pub mod cancel;
pub mod commands;
pub mod engine;
pub mod fuzz;
pub mod mesh;
pub mod mesh_in;
pub mod project;
pub mod render;
pub mod repair;
pub mod scene;
pub mod simplify;
pub mod sketch;
pub mod stl;
pub mod threemf;
pub mod zip;

pub use engine::{Command, Files, Session};
pub use mesh::{Mesh, P3, Transform};
pub use scene::{Object, Scene};

/// The rule that only the AI chat and the printer connection reach the network: no other crate may depend on an HTTP client or on
/// the key's crypto.
#[cfg(test)]
mod network_boundary {
    #[test]
    fn only_the_assistant_and_the_printer_connection_depend_on_an_http_client() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut manifests = 0;
        for dir in ["crates", "apps", "tools"] {
            for entry in std::fs::read_dir(root.join("../..").join(dir)).unwrap().flatten() {
                let Ok(text) = std::fs::read_to_string(entry.path().join("Cargo.toml")) else { continue };
                manifests += 1;
                // The assistant talks to the AI providers and seals keys with ring. The printer crate (roadmap 6.1) sends G-code to the
                // one printer address the user typed, with the same ureq and rustls, and needs no ring of its own. Nothing else may.
                let allowed: &[&str] = match entry.file_name().to_str() {
                    Some("assistant") => &["ureq", "ring", "rustls"],
                    Some("printer") => &["ureq", "rustls"],
                    _ => &[],
                };
                for dep in ["ureq", "ring", "reqwest", "hyper", "rustls"] {
                    assert!(
                        allowed.contains(&dep) || !text.contains(dep),
                        "{} depends on {dep}: only crates/assistant and crates/printer may reach the network",
                        entry.path().display()
                    );
                }
            }
        }
        assert!(manifests >= 7, "found {manifests} manifests");
    }
}
