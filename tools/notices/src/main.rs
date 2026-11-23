//! `lamina-notices [OUT]`: checks every crate Lamina is built from (on every platform) has a licence Lamina may use, and writes the
//! third-party notice to OUT (THIRD-PARTY-NOTICES.txt by default), which the release packages carry. Fails, writing nothing, when a
//! crate's licence will not do.

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let out = std::env::args().nth(1).unwrap_or_else(|| "THIRD-PARTY-NOTICES.txt".into());
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let meta = match Command::new(cargo).args(["metadata", "--format-version", "1", "--locked"]).output() {
        Ok(o) if o.status.success() => o.stdout,
        Ok(o) => {
            eprintln!("cargo metadata failed: {}", String::from_utf8_lossy(&o.stderr));
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("could not run cargo: {e}");
            return ExitCode::FAILURE;
        }
    };
    let meta: serde_json::Value = match serde_json::from_slice(&meta) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("cargo metadata's answer does not read: {e}");
            return ExitCode::FAILURE;
        }
    };
    let crates = lamina_notices::crates(&meta);
    let refused = lamina_notices::refused(&crates);
    if !refused.is_empty() {
        eprintln!("these crates have no licence Lamina may use:\n{}", refused.join("\n"));
        return ExitCode::FAILURE;
    }
    match std::fs::write(&out, lamina_notices::notice(&crates)) {
        Ok(()) => {
            println!("{} crates, notice written to {out}", crates.len());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{out}: {e}");
            ExitCode::FAILURE
        }
    }
}
