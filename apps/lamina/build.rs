//! Compiles the window's translations (`translations/ui/*.po`) into the sorted tables `lang.rs` looks up. A catalog that does not
//! read, or a finished translation that does not keep its `{}` placeholders, stops the build and names the file and the message.

use std::path::Path;

fn main() -> Result<(), String> {
    let root = std::env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?;
    let out = std::env::var("OUT_DIR").map_err(|e| e.to_string())?;
    lamina_i18n::catalog::compile(&Path::new(&root).join("../../translations/ui"), &Path::new(&out).join("catalogs.rs"))
}
