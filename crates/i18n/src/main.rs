//! `cargo i18n <extract|update|check|stats> [repository root]`: keeps `translations/ui` in step with the window's sources. Lifted
//! from BIMCraft's `crates/i18n` (same author, MIT OR Apache-2.0), with one domain.

use lamina_i18n::catalog;
use std::fs;
use std::path::{Path, PathBuf};

fn run(root: &Path, action: &str) -> Result<(), String> {
    if !matches!(action, "extract" | "update" | "check" | "stats") {
        return Err("usage: cargo i18n <extract|update|check|stats> [repository root]".into());
    }
    let folder = root.join("translations").join("ui");
    let path = folder.join("lamina-ui.pot");
    let source = catalog::extract(root)?;
    let rendered = catalog::render(&source);
    match action {
        "extract" | "update" => fs::write(&path, &rendered).map_err(|e| format!("{}: {e}", path.display()))?,
        "check" if !catalog::up_to_date(&fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?, &rendered) => {
            return Err(format!("{} is out of date; run cargo i18n update", path.display()));
        }
        _ => {}
    }
    let mut paths = fs::read_dir(&folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    paths.sort();
    for path in paths.iter().filter(|p| p.extension().is_some_and(|e| e == "po")) {
        let mut document = catalog::read(path)?;
        if action == "update" {
            document = catalog::merge(document, &source);
            fs::write(path, catalog::render(&document)).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if action == "check" {
            for key in document.entries.keys() {
                if !source.entries.contains_key(key) {
                    return Err(format!("{}: obsolete active message {:?}; run cargo i18n update", path.display(), key));
                }
            }
        }
        let translated = document.entries.values().filter(|e| !e.fuzzy && !e.translation.trim().is_empty()).count();
        println!("ui/{}: {translated}/{} translated", path.file_name().unwrap_or_default().to_string_lossy(), source.entries.len());
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let action = args.next().unwrap_or_else(|| "check".into());
    let root = args.next().map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
    run(&root, &action)
}
