//! A start that doesn't finish, and a log. A release build on Windows has no console, so a window that wouldn't open would say
//! nothing. What went wrong is written to `lamina.log` beside the settings file, and on Windows shown in a message box; each start
//! and the graphics it got go there too. A start that dies without a word (a graphics driver taking the process down with it) leaves
//! `graphics.json` marked unfinished, and the next start tries another graphics backend; the one that keeps a window up for
//! [`SURVIVE`] is kept for the starts after it. `WGPU_BACKEND` (`vulkan`, `dx12`, `opengl`) is the user's own choice, and nothing
//! here overrides it.
//!
//! Lifted from BIMCraft's `apps/bimcraft/src/startup.rs` (same author, MIT OR Apache-2.0), where a laptop's graphics taking the
//! process down under Vulkan was its issue 14; without its translations and the marks its older versions kept.

use std::backtrace::Backtrace;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use eframe::wgpu::Backends;
use serde_json::{Value, json};

use crate::lang::{t, tf};

/// What a start asks of the graphics, in the order starts that didn't finish try it: wgpu's own choice (every backend the system
/// has, the fastest adapter first), then each backend alone. DirectX 12 comes before Vulkan: wgpu lists Vulkan's adapters first
/// and keeps that order among adapters of the same kind, so on most Windows machines its own choice already was Vulkan, and
/// a start that died on it learns nothing by trying Vulkan again.
const LADDER: &[(&str, Option<Backends>)] =
    &[("automatic", None), ("DirectX 12", Some(Backends::DX12)), ("Vulkan", Some(Backends::VULKAN)), ("OpenGL", Some(Backends::GL))];

/// The choices this system has. Only Windows has the ladder: that is where starts died (BIMCraft's issue 14), macOS has Metal alone, and
/// OpenGL didn't get a surface on a Wayland desktop it was tried on, so a fallback there would only make a start fail.
fn choices() -> &'static [(&'static str, Option<Backends>)] {
    if cfg!(windows) { LADDER } else { &LADDER[..1] }
}

/// How long a window has to stay up before its start counts as finished.
pub const SURVIVE: Duration = Duration::from_secs(15);
/// The log is one that grew past this, and is started over.
const MAX_LOG: u64 = 64 << 10;
/// The mark is a few bytes. One over this isn't ours.
const MAX_MARK: u64 = 4 << 10;

/// Set once the window is open: a panic after that is the session's to catch, and needs no message box.
static UP: AtomicBool = AtomicBool::new(false);

/// The two files this keeps, beside the settings file.
#[derive(Clone, Debug)]
pub struct Files {
    pub log: PathBuf,
    mark: PathBuf,
}

impl Files {
    pub fn beside(settings: &Path) -> Files {
        Files { log: settings.with_file_name("lamina.log"), mark: settings.with_file_name("graphics.json") }
    }
}

/// Which graphics choice the starts are on, and whether the last of them finished.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mark {
    pub choice: usize,
    pub starting: bool,
}

impl Mark {
    /// The mark in the file at `path`; a missing, damaged, oversized or odd one is the default: automatic, finished.
    pub fn read(path: &Path) -> Mark {
        Mark::read_among(path, choices().len())
    }

    fn read_among(path: &Path, choices: usize) -> Mark {
        let text = fs::metadata(path).ok().filter(|m| m.is_file() && m.len() <= MAX_MARK).and_then(|_| fs::read_to_string(path).ok());
        let j: Value = text.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        let name = j.get("backend").and_then(Value::as_str);
        let choice = name.and_then(|n| LADDER.iter().take(choices).position(|(l, _)| *l == n));
        Mark { choice: choice.unwrap_or(0), starting: choice.is_some() && j.get("starting").and_then(Value::as_bool) == Some(true) }
    }

    /// Written to `path`, whole or not at all, its folder made if need be.
    pub fn write(self, path: &Path) -> Result<(), String> {
        let backend = LADDER[self.choice.min(LADDER.len() - 1)].0;
        let text = json!({ "backend": backend, "starting": self.starting }).to_string();
        if let Some(folder) = path.parent().filter(|f| !f.as_os_str().is_empty()) {
            fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        }
        let part = path.with_extension("json.part");
        fs::write(&part, text).map_err(|e| format!("{}: {e}", part.display()))?;
        fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The mark of the start after this one: the same choice, or the next when the last start never finished.
    /// shortcut: two starts within [`SURVIVE`] of each other count the first as unfinished; it only costs a start on another backend.
    pub fn next(self) -> Mark {
        self.next_among(choices().len())
    }

    fn next_among(self, choices: usize) -> Mark {
        let choice = if self.starting { (self.choice + 1) % choices } else { self.choice };
        Mark { choice, starting: true }
    }

    /// This start finished: its choice stays for the starts after it.
    pub fn finished(self) -> Mark {
        Mark { starting: false, ..self }
    }

    pub fn name(self) -> &'static str {
        choices()[self.choice.min(choices().len() - 1)].0
    }

    pub fn backends(self) -> Option<Backends> {
        choices()[self.choice.min(choices().len() - 1)].1
    }
}

/// Adds `line` to the log at `path`. Best effort: a log that can't be written never stops the app.
pub fn note(path: &Path, line: &str) {
    if let Some(folder) = path.parent().filter(|f| !f.as_os_str().is_empty()) {
        let _ = fs::create_dir_all(folder);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

/// The log, started over if it has grown too big.
fn trimmed(path: &Path) {
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = fs::remove_file(path);
    }
}

/// What a start tells the user: a message box on Windows, where a release build has no console to print to.
pub fn tell(what: &str, files: Option<&Files>) {
    let body = match files {
        Some(f) => format!("{what}\n\n{}", tf("The details are in {}.", &[&f.log.display().to_string()])),
        None => what.to_string(),
    };
    if cfg!(windows) {
        rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title(t("Lamina couldn't start")).set_description(body).show();
    }
}

/// Begins a start: the log's first lines, the panic hook, and the graphics choice for this start, marked as not finished.
/// Without a folder to keep them in, or with `WGPU_BACKEND` set, the choice is wgpu's own and nothing is kept.
pub fn begin(files: Option<&Files>) -> Mark {
    let Some(files) = files else { return Mark::default() };
    trimmed(&files.log);
    let user_chose = std::env::var_os("WGPU_BACKEND").is_some();
    let prior = Mark::read(&files.mark);
    let mark = if user_chose { Mark::default() } else { prior.next() };
    let graphics = match (user_chose, prior.starting) {
        (true, _) => "WGPU_BACKEND, as set".to_string(),
        (false, true) => format!("{} (the last start didn't finish)", mark.name()),
        (false, false) => mark.name().to_string(),
    };
    note(
        &files.log,
        &format!("lamina {} on {} {}; graphics: {graphics}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH),
    );
    if !user_chose && let Err(e) = mark.write(&files.mark) {
        note(&files.log, &format!("couldn't keep the graphics mark: {e}"));
    }
    let (log, files) = (files.log.clone(), files.clone());
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        note(&log, &format!("panic: {info}\n{}", Backtrace::force_capture()));
        if !UP.load(Ordering::Relaxed) {
            tell(&info.to_string(), Some(&files));
        }
        previous(info);
    }));
    mark
}

/// The window is open: what it runs on goes in the log, and once it has stayed up for [`SURVIVE`] the start has finished.
pub fn up(files: Option<&Files>, mark: Mark, graphics: &str) {
    UP.store(true, Ordering::Relaxed);
    let Some(files) = files.cloned() else { return };
    note(&files.log, &format!("window up: {graphics}"));
    if std::env::var_os("WGPU_BACKEND").is_some() {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(SURVIVE);
        let _ = mark.finished().write(&files.mark);
    });
}

/// The app was closed the ordinary way: this start didn't die, whatever its length. A start that failed with an error isn't
/// closed: it counts as unfinished too, so the next one tries another backend.
pub fn closed(files: Option<&Files>, mark: Mark) {
    if let Some(files) = files
        && std::env::var_os("WGPU_BACKEND").is_none()
    {
        let _ = mark.finished().write(&files.mark);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("lamina-startup-{}-{name}", std::process::id()))
    }

    #[test]
    fn a_start_that_never_finished_tries_the_next_choice_and_a_finished_one_keeps_its_own() {
        let n = LADDER.len();
        let first = Mark::default().next_among(n);
        assert_eq!(first, Mark { choice: 0, starting: true });
        // It never finished: the next start moves on, and on, and back to the first.
        let mut mark = first;
        for want in (1..n).chain([0]) {
            mark = mark.next_among(n);
            assert_eq!(mark, Mark { choice: want, starting: true });
        }
        // One that finished stays on its choice.
        let kept = Mark { choice: n - 1, starting: true }.finished();
        assert_eq!(kept.next_among(n), Mark { choice: n - 1, starting: true });
        // Where there is nothing to fall back to, every start is the first.
        assert_eq!(Mark { choice: 0, starting: true }.next_among(1), Mark { choice: 0, starting: true });
    }

    #[test]
    fn after_an_automatic_start_that_died_the_next_tries_directx_12_before_vulkan() {
        // BIMCraft's issue 14: on a laptop whose Intel graphics take the process down under Vulkan, wgpu's own choice is Vulkan too,
        // so trying Vulkan alone next only died again; DirectX 12 came up on the third start.
        let died = Mark { choice: 0, starting: true };
        let next = died.next_among(LADDER.len());
        assert_eq!(LADDER[next.choice].0, "DirectX 12");
        assert_eq!(LADDER[next.choice].1, Some(Backends::DX12));
        let names: Vec<&str> = LADDER.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["automatic", "DirectX 12", "Vulkan", "OpenGL"]);
    }

    #[test]
    fn the_mark_is_kept_in_a_file_and_a_missing_damaged_or_odd_one_is_the_default() {
        let file = scratch("mark.json");
        let _ = fs::remove_file(&file);
        let n = LADDER.len();
        assert_eq!(Mark::read_among(&file, n), Mark::default());
        let mark = Mark { choice: n - 1, starting: true };
        mark.write(&file).unwrap();
        assert_eq!(Mark::read_among(&file, n), mark);
        // A choice this system doesn't have is no mark.
        assert_eq!(Mark::read_among(&file, n - 1), Mark::default());
        for odd in
            ["", "not json", "[1]", r#"{"choice": -1, "starting": true}"#, r#"{"choice": 99, "starting": true}"#, r#"{"choice": "a"}"#]
        {
            fs::write(&file, odd).unwrap();
            assert_eq!(Mark::read_among(&file, n), Mark::default(), "{odd}");
        }
        // A file too big to be ours is left alone.
        fs::write(&file, format!(r#"{{"choice": 1, "starting": true, "pad": "{}"}}"#, "x".repeat(MAX_MARK as usize))).unwrap();
        assert_eq!(Mark::read_among(&file, n), Mark::default());
        let _ = fs::remove_file(file);
    }

    #[test]
    fn a_mark_names_its_backend_and_a_name_this_system_doesnt_have_is_no_mark() {
        let file = scratch("named.json");
        let n = LADDER.len();
        for (choice, (name, _)) in LADDER.iter().enumerate() {
            let mark = Mark { choice, starting: true };
            mark.write(&file).unwrap();
            let text = fs::read_to_string(&file).unwrap();
            assert!(text.contains(&format!("\"backend\":\"{name}\"")) && !text.contains("choice"), "{text}");
            assert_eq!(Mark::read_among(&file, n), mark);
        }
        // An unknown name, or one beyond `choices`, is the default.
        for odd in [r#"{"backend":"Metal","starting":true}"#, r#"{"backend":7,"starting":true}"#, r#"{"backend":"","starting":true}"#] {
            fs::write(&file, odd).unwrap();
            assert_eq!(Mark::read_among(&file, n), Mark::default(), "{odd}");
        }
        fs::write(&file, r#"{"backend":"OpenGL","starting":true}"#).unwrap();
        assert_eq!(Mark::read_among(&file, n - 1), Mark::default());
        assert_eq!(Mark::read_among(&file, 1), Mark::default());
        let _ = fs::remove_file(file);
    }

    #[cfg(not(windows))]
    #[test]
    fn off_windows_every_start_is_the_first_choice_whatever_the_mark_says() {
        let file = scratch("elsewhere.json");
        fs::write(&file, r#"{"backend":"DirectX 12","starting":true}"#).unwrap();
        assert_eq!(choices().len(), 1);
        assert_eq!(Mark::read(&file), Mark::default());
        assert_eq!(Mark::read(&file).next(), Mark { choice: 0, starting: true });
        let _ = fs::remove_file(file);
    }

    #[test]
    fn the_log_keeps_its_lines_and_starts_over_once_it_is_too_big() {
        let file = scratch("lamina.log");
        let _ = fs::remove_file(&file);
        note(&file, "one");
        note(&file, "two");
        assert_eq!(fs::read_to_string(&file).unwrap(), "one\ntwo\n");
        trimmed(&file);
        assert!(file.exists());
        fs::write(&file, "x".repeat(MAX_LOG as usize + 1)).unwrap();
        trimmed(&file);
        assert!(!file.exists());
        // A log in a folder that isn't there yet is made.
        let nested = scratch("folder").join("lamina.log");
        note(&nested, "made");
        assert_eq!(fs::read_to_string(&nested).unwrap(), "made\n");
        let _ = fs::remove_dir_all(nested.parent().unwrap());
    }

    #[test]
    fn the_files_sit_beside_the_settings() {
        let files = Files::beside(Path::new("/home/me/.config/lamina/settings.json"));
        assert_eq!(files.log, Path::new("/home/me/.config/lamina/lamina.log"));
        assert_eq!(files.mark, Path::new("/home/me/.config/lamina/graphics.json"));
    }
}
