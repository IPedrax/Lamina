//! Autosave: every few minutes, when the plate changed, the project is written to `autosave.lamina` beside the settings file, on a
//! thread of its own. A clean exit (or a save) removes it, so finding it at the next start means the window did not close cleanly, and
//! the window offers to bring the plate back. A simpler cousin of BIMCraft's `autosave.rs` (same author, MIT OR Apache-2.0): one
//! window, one file.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use lamina_core::{Scene, project};
use serde_json::{Value, json};

use crate::App;

/// How often the plate is written while it has changes.
pub const INTERVAL: Duration = Duration::from_secs(120);
/// The file's name, beside the settings file.
pub const FILE: &str = "autosave.lamina";

pub struct Autosave {
    pub path: PathBuf,
    last: Instant,
    /// The scene last written, so an unchanged plate is not written again.
    written: Option<Arc<Scene>>,
    writing: Option<JoinHandle<Result<(), String>>>,
}

impl Autosave {
    pub fn beside(settings: &Path) -> Autosave {
        let path = settings.parent().map_or_else(|| PathBuf::from(FILE), |d| d.join(FILE));
        Autosave { path, last: Instant::now(), written: None, writing: None }
    }

    /// Waits for a write in flight, so a removal after it is not undone by it.
    fn settle(&mut self) -> Option<String> {
        self.writing.take().and_then(|w| w.join().ok()).and_then(Result::err)
    }

    /// Writes `scene` with `print` now, on a thread; to a file beside it first, then over the old one.
    pub fn write(&mut self, scene: &Arc<Scene>, print: &Value) -> Option<String> {
        let note = self.settle();
        let (path, scene_now, print) = (self.path.clone(), Arc::clone(scene), print.clone());
        self.writing = Some(std::thread::spawn(move || {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let tmp = path.with_extension("lamina.tmp");
            std::fs::write(&tmp, project::write(&scene_now, &print)).map_err(|e| format!("{}: {e}", tmp.display()))?;
            std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
        }));
        self.written = Some(Arc::clone(scene));
        self.last = Instant::now();
        note
    }

    /// Removes the file: nothing is left to recover.
    pub fn remove(&mut self) {
        self.settle();
        self.written = None;
        let _ = std::fs::remove_file(&self.path);
    }

    /// Whether it is time to write `scene`: the interval has passed and the plate is not what was last written.
    pub fn due(&self, scene: &Arc<Scene>) -> bool {
        self.last.elapsed() >= INTERVAL && !self.written.as_ref().is_some_and(|w| Arc::ptr_eq(w, scene))
    }
}

impl App {
    /// Once a frame: writes the plate when it is due and has changes.
    pub(crate) fn autosave_tick(&mut self) {
        if !self.dirty() || !self.autosave.as_ref().is_some_and(|a| a.due(&self.session.scene)) {
            return;
        }
        let print = self.print_json();
        if let Some(a) = self.autosave.as_mut()
            && let Some(e) = a.write(&self.session.scene, &print)
        {
            (self.failed, self.status) = (true, format!("Autosave failed: {e}"));
        }
    }

    /// Writes the plate now (what the tick does when it is due).
    pub fn autosave_now(&mut self) {
        let print = self.print_json();
        if let Some(a) = self.autosave.as_mut() {
            a.write(&self.session.scene, &print);
            a.settle();
        }
    }

    /// Brings back the plate an unclean exit left, as an unsaved project.
    pub fn recover(&mut self) {
        let Some(path) = self.recovery.take() else { return };
        if self.run("open", json!({ "path": path })).is_some() {
            if let Some(p) = self.session.print.get("profile").map(lamina_slicer::profile::Profile::from_json).and_then(Result::ok) {
                self.profile = p;
            }
            self.project = None;
            // Nothing of it is saved: the window asks before it is lost again.
            self.saved_scene = Arc::new(Scene::default());
            self.status = "Brought back the plate from before Lamina closed. Save it to keep it.".into();
            self.frame_all();
        }
    }

    /// Forgets the plate an unclean exit left.
    pub fn discard_recovery(&mut self) {
        if self.recovery.take().is_some()
            && let Some(a) = self.autosave.as_mut()
        {
            a.remove();
        }
    }

    pub(crate) fn recovery_ui(&mut self, ctx: &egui::Context) {
        if self.recovery.is_none() {
            return;
        }
        let mut answer = None;
        egui::Modal::new(egui::Id::new("recover")).show(ctx, |ui| {
            ui.heading("Bring back the last plate?");
            ui.label("Lamina did not close cleanly last time. The plate it was working on was saved a few minutes before.");
            ui.horizontal(|ui| {
                if ui.button("Bring it back").clicked() {
                    answer = Some(true);
                }
                if ui.button("Discard it").clicked() {
                    answer = Some(false);
                }
            });
        });
        match answer {
            Some(true) => self.recover(),
            Some(false) => self.discard_recovery(),
            None => {}
        }
    }
}
