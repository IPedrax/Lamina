//! Lamina's core: meshes, the scene, STL files, and the command engine every front end drives. No UI and no network in here.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod base64;
pub mod bridge;
pub mod commands;
pub mod engine;
pub mod mesh;
pub mod project;
pub mod repair;
pub mod scene;
pub mod stl;

pub use engine::{Command, Files, Session};
pub use mesh::{Mesh, P3, Transform};
pub use scene::{Object, Scene};

/// The rule that only the AI chat reaches the network: no other crate may depend on an HTTP client or on the key's crypto.
#[cfg(test)]
mod network_boundary {
    #[test]
    fn only_the_assistant_depends_on_ureq_or_ring() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut manifests = 0;
        for dir in ["crates", "apps"] {
            for entry in std::fs::read_dir(root.join("../..").join(dir)).unwrap().flatten() {
                let Ok(text) = std::fs::read_to_string(entry.path().join("Cargo.toml")) else { continue };
                manifests += 1;
                let allowed = entry.file_name() == "assistant";
                for dep in ["ureq", "ring", "reqwest", "hyper", "rustls"] {
                    assert!(
                        allowed || !text.contains(dep),
                        "{} depends on {dep}: only crates/assistant may reach the network",
                        entry.path().display()
                    );
                }
            }
        }
        assert!(manifests >= 4, "found {manifests} manifests");
    }
}
