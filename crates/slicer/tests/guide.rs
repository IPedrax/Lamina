//! The user guide's table of settings (docs/USER-GUIDE.md, between the `settings` markers) is made from the profile: each field's
//! sentence from `ABOUT`, its range from `LIMITS` and its default. This test checks the guide still says what the code does;
//! `LAMINA_BLESS=1` writes the table again after a setting changes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use lamina_slicer::profile::{ABOUT, Profile, limits};
use serde_json::Value;

const BEGIN: &str = "<!-- settings:begin -->";
const END: &str = "<!-- settings:end -->";

fn default_of(v: &Value) -> String {
    match v {
        Value::String(s) if s.contains('\n') || s.len() > 30 => "see the window".to_string(),
        Value::String(s) if s.is_empty() => "none".to_string(),
        Value::String(s) => format!("`{s}`"),
        Value::Array(a) if a.is_empty() => "none".to_string(),
        Value::Array(a) => a.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" x "),
        other => other.to_string(),
    }
}

fn table() -> String {
    let defaults = serde_json::to_value(Profile::default()).unwrap();
    let mut out = String::from("| Setting | What it does | Range | Default |\n|---|---|---|---|\n");
    for (name, about) in ABOUT {
        let range = limits(name).map_or(String::new(), |(lo, hi)| format!("{lo} to {hi}"));
        out.push_str(&format!("| `{name}` | {about} | {range} | {} |\n", default_of(&defaults[*name])));
    }
    out
}

#[test]
fn the_guide_lists_every_setting_as_the_profile_has_it() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/USER-GUIDE.md");
    let text = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
    let (start, end) = (text.find(BEGIN).expect("the begin marker"), text.find(END).expect("the end marker"));
    let want = format!("{BEGIN}\n{}{END}", table());
    let have = &text[start..end + END.len()];
    if std::env::var_os("LAMINA_BLESS").is_some() {
        std::fs::write(&path, text.replace(have, &want)).unwrap();
        return;
    }
    assert!(have == want, "the guide's table of settings is out of date: run the tests with LAMINA_BLESS=1 and read the diff");
}
