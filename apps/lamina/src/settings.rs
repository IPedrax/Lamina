//! What the window keeps between runs, in one file in the user's settings folder: the print settings, the printer and filament last
//! chosen, the presets the user saved, and the assistant's key when the user asks for it to be remembered (sealed by
//! `lamina_assistant::seal`, never as typed). The file is read hostilely (a size cap; a part that does not read is dropped and said,
//! not the whole file) and written whole: to a file beside it, readable by its owner only, then moved over it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lamina_assistant::{Provider, seal};
use lamina_slicer::presets;
use lamina_slicer::profile::Profile;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The largest settings file read, bytes.
pub const MAX_FILE: u64 = 1 << 20;

/// User presets: a name and the profile fields it sets.
pub type Presets = BTreeMap<String, Map<String, Value>>;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filament: Option<String>,
    #[serde(default)]
    pub printers: Presets,
    #[serde(default)]
    pub filaments: Presets,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub processes: Presets,
    /// Sealed keys by provider (`Provider::key`).
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    /// Recent projects, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<String>,
    /// Where the window sends a sliced plate, and its API key when remembered (sealed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<crate::printer::Connection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_key: Option<String>,
    /// The theme, when it is not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<crate::comfort::Look>,
    /// The assistant's local model server and model, when they are not the defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_model: Option<String>,
    /// The Agents section: whether the window lets agents in, on which port, and whether they may change the plate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<crate::control::Agents>,
    /// The 3D view drawn on the processor even where there is a graphics card.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cpu_view: bool,
    /// File, Check for updates at start unticked: the window does not ask GitHub for a newer release when it starts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_update_check: bool,
}

/// Where the settings file is (`lamina_core::bridge::settings_file`, which the CLI reads too, to find the window's bridge file).
pub fn default_path() -> Option<PathBuf> {
    lamina_core::bridge::settings_file()
}

impl Settings {
    /// The settings in the file at `path` (none yet where there is no file), and a note for each part left out because it did not read.
    pub fn read(path: &Path) -> (Settings, Vec<String>) {
        let mut notes = Vec::new();
        let text = match std::fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), notes),
            Err(e) => return (Settings::default(), vec![format!("{}: {e}", path.display())]),
            Ok(m) if m.len() > MAX_FILE => {
                return (Settings::default(), vec![format!("{} is too big to be settings: left alone", path.display())]);
            }
            Ok(_) => std::fs::read_to_string(path),
        };
        let all = match text.map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string())) {
            Ok(Value::Object(m)) => m,
            Ok(_) => return (Settings::default(), vec![format!("{} is not a settings file: left alone", path.display())]),
            Err(e) => return (Settings::default(), vec![format!("{} does not read ({e}): started from the defaults", path.display())]),
        };
        let mut s = Settings::default();
        if let Some(v) = all.get("profile") {
            match Profile::from_json(v) {
                Ok(p) => s.profile = Some(p),
                Err(e) => notes.push(format!("The saved print settings were left out: {e}")),
            }
        }
        let text = |k: &str| all.get(k).and_then(Value::as_str).filter(|t| t.len() <= 200).map(str::to_string);
        (s.printer, s.filament, s.process) = (text("printer"), text("filament"), text("process"));
        for (key, into) in [("printers", &mut s.printers), ("filaments", &mut s.filaments), ("processes", &mut s.processes)] {
            for (name, fields) in all.get(key).and_then(Value::as_object).into_iter().flatten() {
                // Its own fields must be settings; what it inherits is resolved when it is applied.
                let ok = fields.as_object().filter(|f| {
                    let mut own = (*f).clone();
                    let parent = own.remove(presets::INHERITS);
                    parent.as_ref().is_none_or(|p| p.as_str().is_some_and(|p| p.len() <= 200))
                        && presets::apply(&Profile::default(), &own).is_ok()
                        && name.len() <= 200
                });
                match ok {
                    Some(f) => {
                        into.insert(name.clone(), f.clone());
                    }
                    None => notes.push(format!("The preset \"{name}\" was left out: its settings are not valid")),
                }
            }
        }
        s.recent = all
            .get("recent")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|p| !p.is_empty() && p.len() <= 4096)
            .take(crate::files::MAX_RECENT)
            .map(str::to_string)
            .collect();
        if let Some(v) = all.get("connection") {
            match serde_json::from_value::<crate::printer::Connection>(v.clone()) {
                Ok(c) if c.address.len() <= 500 && c.user.len() <= 200 => s.connection = Some(c),
                _ => notes.push("The printer connection was left out: it does not read".into()),
            }
        }
        if let Some(v) = all.get("connection_key") {
            match v.as_str().filter(|t| seal::well_formed(t)) {
                Some(t) => s.connection_key = Some(t.to_string()),
                None => notes.push("The saved printer key was left out: it is not a sealed key".into()),
            }
        }
        if let Some(v) = all.get("local_url") {
            match v.as_str().filter(|u| lamina_assistant::local::endpoint(u).is_ok()) {
                Some(u) => s.local_url = Some(u.to_string()),
                None => notes.push("The local model's address was left out: it is not on this computer".into()),
            }
        }
        if let Some(v) = all.get("local_model") {
            match v.as_str().filter(|m| lamina_assistant::model_ok_for(Provider::Local, m)) {
                Some(m) => s.local_model = Some(m.to_string()),
                None => notes.push("The local model's name was left out: it is not a model id".into()),
            }
        }
        if let Some(v) = all.get("agents") {
            match serde_json::from_value::<crate::control::Agents>(v.clone()) {
                Ok(a) => s.agents = Some(a),
                Err(_) => notes.push("The Agents settings were left out: they do not read".into()),
            }
        }
        if let Some(v) = all.get("cpu_view") {
            match v.as_bool() {
                Some(b) => s.cpu_view = b,
                None => notes.push("The 3D view's processor setting was left out: it is not true or false".into()),
            }
        }
        if let Some(v) = all.get("no_update_check") {
            match v.as_bool() {
                Some(b) => s.no_update_check = b,
                None => notes.push("The update check's setting was left out: it is not true or false".into()),
            }
        }
        if let Some(v) = all.get("look") {
            match serde_json::from_value::<crate::comfort::Look>(v.clone()) {
                Ok(l) => s.look = Some(l),
                Err(_) => notes.push("The theme was left out: it is not dark, light or system".into()),
            }
        }
        for (provider, sealed) in all.get("keys").and_then(Value::as_object).into_iter().flatten() {
            match sealed.as_str().filter(|t| Provider::from_key(provider).is_some() && seal::well_formed(t)) {
                Some(t) => {
                    s.keys.insert(provider.clone(), t.to_string());
                }
                None => notes.push(format!("The saved {provider} key was left out: it is not a sealed key")),
            }
        }
        (s, notes)
    }

    /// Writes the settings to `path`, whole: to a file beside it that only the user may read, then over the old one.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let fail = |e: std::io::Error| format!("could not save the settings to {}: {e}", path.display());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        // The file may hold a sealed key: no one else on the machine reads it.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp).map_err(fail)?;
        std::io::Write::write_all(&mut file, text.as_bytes()).map_err(fail)?;
        drop(file);
        std::fs::rename(&tmp, path).map_err(fail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_go_out_and_come_back_and_only_the_owner_reads_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        let mut s = Settings {
            profile: Some(Profile { infill_density: 0.35, ..Profile::default() }),
            printer: Some("Generic 300 mm, Klipper".into()),
            recent: vec!["/home/me/plate.lamina".into()],
            connection: Some(crate::printer::Connection {
                kind: lamina_printer::Kind::PrusaLink,
                address: "mk4.local".into(),
                user: "maker".into(),
            }),
            connection_key: Some(seal::seal("ABCDEF0123456789").unwrap()),
            look: Some(crate::comfort::Look::Light),
            cpu_view: true,
            no_update_check: true,
            ..Settings::default()
        };
        s.filaments
            .insert("My PETG".into(), presets::capture(&Profile { nozzle_temp: 236.0, ..Profile::default() }, presets::FILAMENT_FIELDS));
        s.keys.insert("anthropic".into(), seal::seal("sk-ant-test-0123456789").unwrap());
        s.write(&path).unwrap();
        let (back, notes) = Settings::read(&path);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(back, s);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("sk-ant-test"), "the key is never written as typed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn a_bad_part_is_left_out_and_said_not_the_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"profile": {"nozzle": -3}, "printer": "Mine", "filaments": {"Good": {"nozzle_temp": 200}, "Bad": {"nozzle_temp": 9999}},
               "keys": {"anthropic": "plain-text-key", "nobody": "lm1:1:2"}, "unknown": true}"#,
        )
        .unwrap();
        let (s, notes) = Settings::read(&path);
        assert_eq!(s.profile, None);
        assert_eq!(s.printer.as_deref(), Some("Mine"));
        assert_eq!(s.filaments.keys().collect::<Vec<_>>(), ["Good"]);
        assert!(s.keys.is_empty());
        assert_eq!(notes.len(), 4, "{notes:?}");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Settings::read(&path).0, Settings::default());
        assert_eq!(Settings::read(&dir.path().join("none.json")), (Settings::default(), vec![]));
    }

    #[test]
    fn hostile_settings_files_never_panic() {
        use lamina_core::fuzz::Rng;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let good = Settings {
            profile: Some(Profile::default()),
            printer: Some("Generic 300 mm, Klipper".into()),
            recent: vec!["/a/b.lamina".into()],
            keys: [("anthropic".to_string(), seal::seal("sk-ant-test-0123456789").unwrap())].into_iter().collect(),
            ..Settings::default()
        };
        let text = serde_json::to_string(&good).unwrap();
        let mut r = Rng::new(0x5e7);
        for _ in 0..1000 {
            std::fs::write(&path, r.input(&[text.as_bytes()])).unwrap();
            let _ = Settings::read(&path);
        }
    }
}
