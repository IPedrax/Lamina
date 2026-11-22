//! Lamina's core: meshes, the scene, STL files, and the command engine every front end drives. No UI and no network in here.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod bridge;
pub mod commands;
pub mod engine;
pub mod mesh;
pub mod scene;
pub mod stl;

pub use engine::{Command, Files, Session};
pub use mesh::{Mesh, P3, Transform};
pub use scene::{Object, Scene};
