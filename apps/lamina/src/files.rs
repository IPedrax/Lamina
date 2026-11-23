//! The File menu: projects (new, open, save, save as, recent), meshes imported, files dropped on the window, and the question
//! before unsaved changes are lost. Every change to the plate is still a command (`new`, `open`, `save`, `import_mesh`); this
//! module only chooses the file and keeps track of which project is open and whether it changed since it was saved.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Key, KeyboardShortcut, Modifiers, Ui, ViewportCommand};
use lamina_core::project::EXTENSION;
use lamina_slicer::profile::Profile;
use serde_json::{Value, json};

use crate::App;

/// How many recent projects the menu keeps.
pub const MAX_RECENT: usize = 8;

/// The meshes the window imports (the extension, lower case).
pub const MESHES: &[&str] = &["stl", "3mf"];

const NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SAVE_AS: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);

/// What waits for an answer to "save the changes?".
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    New,
    Open(PathBuf),
    /// Open, with the file still to be chosen.
    Choose,
    Close,
}

/// Whether `path` names a project file.
pub fn is_project(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

fn is_mesh(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| MESHES.iter().any(|m| e.eq_ignore_ascii_case(m)))
}

impl App {
    /// The print settings and preset labels as the project keeps them.
    pub fn print_json(&self) -> Value {
        let mut v = json!({ "profile": self.profile });
        if let Some(p) = &self.printer {
            v["printer"] = json!(p);
        }
        if let Some(f) = &self.filament {
            v["filament"] = json!(f);
        }
        v
    }

    /// Whether there is something a close would lose: the plate changed since it was opened or saved, or the settings did in a
    /// project that has a file (an empty, unnamed plate with other settings loses nothing; the settings file keeps them).
    pub fn dirty(&self) -> bool {
        let plate = !Arc::ptr_eq(&self.session.scene, &self.saved_scene);
        let settings = self.print_json() != self.saved_print;
        plate || (settings && (self.project.is_some() || !self.session.scene.objects.is_empty()))
    }

    /// What is in the window now is what is saved.
    pub(crate) fn mark_saved(&mut self) {
        self.saved_scene = Arc::clone(&self.session.scene);
        self.saved_print = self.print_json();
    }

    /// The window's title: the project's name, and a star when it has changes not saved.
    pub fn title(&self) -> String {
        let name = self.project.as_ref().and_then(|p| p.file_stem()).map(|n| n.to_string_lossy().into_owned());
        format!("{}{} - Lamina", name.unwrap_or_else(|| "Untitled".into()), if self.dirty() { " *" } else { "" })
    }

    fn remember(&mut self, path: &Path) {
        let text = path.to_string_lossy().into_owned();
        self.recent.retain(|p| p != &text);
        self.recent.insert(0, text);
        self.recent.truncate(MAX_RECENT);
    }

    /// Does `action` now when nothing would be lost, or asks first.
    pub fn request(&mut self, action: Pending) {
        if self.dirty() { self.ask = Some(action) } else { self.carry_out(action) }
    }

    fn carry_out(&mut self, action: Pending) {
        match action {
            Pending::New => self.new_project(),
            Pending::Open(path) => self.open_project(&path),
            Pending::Choose => {
                if let Some(path) = rfd::FileDialog::new().add_filter("Lamina project", &[EXTENSION]).pick_file() {
                    self.open_project(&path);
                }
            }
            Pending::Close => {
                self.may_close = true;
                self.closing = true;
            }
        }
    }

    /// An empty plate and no file; the print settings stay.
    pub fn new_project(&mut self) {
        if self.run("new", json!({})).is_some() {
            self.project = None;
            (self.selected, self.sliced) = (Vec::new(), None);
            self.mark_saved();
            self.status = "New project.".into();
        }
    }

    /// Opens a project: its plate, and its print settings in the panel.
    pub fn open_project(&mut self, path: &Path) {
        let Some(r) = self.run("open", json!({ "path": path })) else { return };
        let print = self.session.print.clone();
        let mut notes = Vec::new();
        match print.get("profile").map(Profile::from_json) {
            Some(Ok(p)) => self.profile = p,
            Some(Err(e)) => notes.push(format!("its print settings were left out ({e}), so the ones shown stay")),
            None => {}
        }
        let label = |k: &str| print.get(k).and_then(Value::as_str).filter(|t| t.len() <= 200).map(str::to_string);
        (self.printer, self.filament) = (label("printer"), label("filament"));
        let ignored: Vec<&str> = r["ignored"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        if !ignored.is_empty() {
            notes.push(format!("this version left out {}", ignored.join(", ")));
        }
        self.project = Some(path.to_path_buf());
        self.remember(path);
        (self.selected, self.sliced) = (Vec::new(), None);
        self.mark_saved();
        self.frame_all();
        self.status = format!("Opened {}.", path.display());
        if !notes.is_empty() {
            self.status = format!("{} Note: {}.", self.status, notes.join("; "));
        }
    }

    /// Saves the project to `path` (given the extension if it has none). Returns whether it was written.
    pub fn save_project(&mut self, path: &Path) -> bool {
        let path = if path.extension().is_none() { path.with_extension(EXTENSION) } else { path.to_path_buf() };
        self.session.print = self.print_json();
        if self.run("save", json!({ "path": path })).is_none() {
            return false;
        }
        self.project = Some(path.clone());
        self.remember(&path);
        self.mark_saved();
        self.status = format!("Saved {}.", path.display());
        true
    }

    /// Saves where the project already is, or asks where. Returns whether it was written.
    pub fn save(&mut self) -> bool {
        match self.project.clone() {
            Some(path) => self.save_project(&path),
            None => self.save_as(),
        }
    }

    fn save_as(&mut self) -> bool {
        let name = self.project.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        let dialog = rfd::FileDialog::new().add_filter("Lamina project", &[EXTENSION]).set_file_name(name.unwrap_or("plate.lamina".into()));
        match dialog.save_file() {
            Some(path) => self.save_project(&path),
            None => false,
        }
    }

    /// Opens a file the way its kind asks: a project replaces the plate (asking first if there are changes), a mesh is put on it.
    pub fn open_file(&mut self, path: &Path) {
        if is_project(path) {
            self.request(Pending::Open(path.to_path_buf()));
        } else if is_mesh(path) {
            self.import(path);
        } else {
            (self.failed, self.status) =
                (true, format!("{}: Lamina opens .lamina projects and imports .stl and .3mf meshes", path.display()));
        }
    }

    /// Puts a mesh file on the plate (a 3MF may hold several objects) and selects what came in.
    pub fn import(&mut self, path: &Path) {
        if let Some(v) = self.run("import_mesh", json!({ "path": path })) {
            self.selected = v["ids"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(|id| id as u32).collect();
            self.frame_all();
            if let Some(r) = v.get("repaired") {
                self.status = format!(
                    "Imported and repaired: {} faces dropped, {} turned, {} holes filled.",
                    r["dropped"], r["flipped"], r["holes_filled"]
                );
            }
        }
    }

    fn export_mesh(&mut self) {
        let dialog = rfd::FileDialog::new().add_filter("3MF", &["3mf"]).add_filter("STL", &["stl"]).set_file_name("plate.3mf");
        let Some(path) = dialog.save_file() else { return };
        let path = if path.extension().is_none() { path.with_extension("3mf") } else { path };
        if let Some(v) = self.run("export_mesh", json!({ "path": path })) {
            self.status = format!("Wrote {} ({} triangles).", path.display(), v["triangles"]);
        }
    }

    /// Files dropped on the window: each opened or imported.
    pub(crate) fn dropped(&mut self, ctx: &egui::Context) {
        let files: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for path in files {
            self.open_file(&path);
        }
    }

    /// The shortcuts of the File menu.
    pub(crate) fn file_keys(&mut self, ctx: &egui::Context) {
        let (new, open, save_as, save) = ctx
            .input_mut(|i| (i.consume_shortcut(&NEW), i.consume_shortcut(&OPEN), i.consume_shortcut(&SAVE_AS), i.consume_shortcut(&SAVE)));
        if new {
            self.request(Pending::New);
        }
        if open {
            self.request(Pending::Choose);
        }
        if save_as {
            self.save_as();
        } else if save {
            self.save();
        }
    }

    pub(crate) fn file_menu(&mut self, ui: &mut Ui) {
        ui.menu_button("File", |ui| {
            let ctx = ui.ctx().clone();
            if ui.add(egui::Button::new("New").shortcut_text(ctx.format_shortcut(&NEW))).clicked() {
                self.request(Pending::New);
            }
            if ui.add(egui::Button::new("Open...").shortcut_text(ctx.format_shortcut(&OPEN))).clicked() {
                self.request(Pending::Choose);
            }
            ui.add_enabled_ui(!self.recent.is_empty(), |ui| {
                ui.menu_button("Open recent", |ui| {
                    for path in self.recent.clone() {
                        if ui.button(&path).clicked() {
                            self.request(Pending::Open(PathBuf::from(path)));
                        }
                    }
                    ui.separator();
                    if ui.button("Clear the list").clicked() {
                        self.recent.clear();
                    }
                });
            });
            if ui.add(egui::Button::new("Save").shortcut_text(ctx.format_shortcut(&SAVE))).clicked() {
                self.save();
            }
            if ui.add(egui::Button::new("Save as...").shortcut_text(ctx.format_shortcut(&SAVE_AS))).clicked() {
                self.save_as();
            }
            ui.separator();
            if ui.button("Import mesh...").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("Mesh", MESHES).pick_file()
            {
                self.import(&p);
            }
            if ui.button("Export mesh...").on_hover_text("The plate as a 3MF, each object its own, or as one STL").clicked() {
                self.export_mesh();
            }
            if ui.button("Export G-code...").clicked() {
                self.export();
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                self.request(Pending::Close);
            }
        });
    }

    /// The close button asks first when there are changes, and the question itself.
    pub(crate) fn closing(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.may_close && self.dirty() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.ask = Some(Pending::Close);
        }
        if std::mem::take(&mut self.closing) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        let Some(action) = self.ask.clone() else { return };
        let name = self.project.as_ref().and_then(|p| p.file_name()).map_or("Untitled".into(), |n| n.to_string_lossy().into_owned());
        let mut answer = None;
        egui::Modal::new(egui::Id::new("unsaved")).show(ctx, |ui| {
            ui.heading("Save the changes?");
            ui.label(format!("{name} has changes that are not saved. They are lost unless you save them."));
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    answer = Some(true);
                }
                if ui.button("Don't save").clicked() {
                    answer = Some(false);
                }
                if ui.button("Cancel").clicked() {
                    self.ask = None;
                }
            });
        });
        if let Some(save) = answer {
            self.ask = None;
            if !save || self.save() {
                self.carry_out(action);
            }
        }
    }
}
