//! Lamina's slicer. Clean-room: behaviour follows what slicers document, no code is taken from PrusaSlicer, OrcaSlicer, Bambu Studio or Cura.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod commands;
pub mod expr;
pub mod gap;
pub mod geom;
pub mod infill;
pub mod presets;
pub mod print;
pub mod profile;
pub mod slice;
pub mod support;
pub mod time;

pub use commands::COMMANDS;
