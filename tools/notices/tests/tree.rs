//! Every crate Lamina is built from, on every platform, has a licence Lamina may use (CLAUDE.md: never GPL, AGPL or LGPL alone).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;

#[test]
fn every_dependency_has_a_licence_lamina_may_use() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let run = |offline: bool| {
        let mut c = Command::new(&cargo);
        c.args(["metadata", "--format-version", "1", "--locked"]).current_dir(env!("CARGO_MANIFEST_DIR"));
        if offline {
            c.arg("--offline");
        }
        c.output().ok().filter(|o| o.status.success())
    };
    // Offline when every crate is already here; cargo fetches the other platforms' crates otherwise.
    let out = run(true).or_else(|| run(false)).expect("cargo metadata");
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let crates = lamina_notices::crates(&meta);
    assert!(crates.len() > 100, "{} crates", crates.len());
    let refused = lamina_notices::refused(&crates);
    assert!(refused.is_empty(), "these crates have no licence Lamina may use:\n{}", refused.join("\n"));
}
