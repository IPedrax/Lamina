//! Lamina's desktop window: the plate in 3D, the objects and their tools, the print settings, the slice and its preview, and the
//! assistant. A thin shell over the core's `Session`: it reads `session.scene` and changes it only through `Session::execute`, so
//! whatever a button does, a script, the MCP server or the assistant does the same way.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod autosave;
pub mod chat;
pub mod comfort;
pub mod control;
pub mod files;
pub mod icon;
pub mod preview;
pub mod printer;
pub mod settings;
pub mod startup;
pub mod view3d;
pub mod work;

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, Key, Panel, RichText, ScrollArea, Ui};
use lamina_assistant::net::Net;
use lamina_core::{Mesh, Scene, Session};
use lamina_slicer::presets::{self, FILAMENT_FIELDS, FILAMENTS, PRINTER_FIELDS, PRINTERS, PROCESS_FIELDS, PROCESSES};
use lamina_slicer::profile::Profile;
use serde_json::{Value, json};

use crate::settings::{Presets, Settings};
use crate::view3d::Pick;

/// A number setting with its label, its range from the profile's table, and on hover what it does and its range.
fn field<N: egui::emath::Numeric>(ui: &mut Ui, label: &str, name: &str, value: &mut N, speed: f64) {
    let (lo, hi) = lamina_slicer::profile::limits(name).unwrap_or((0.0, 1e6));
    ui.label(label).on_hover_text(hint(name));
    ui.add(egui::DragValue::new(value).speed(speed).range(N::from_f64(lo)..=N::from_f64(hi))).on_hover_text(hint(name));
    ui.end_row();
}

/// What a setting does, and its range when it is a number: every field's tooltip.
fn hint(name: &str) -> String {
    let about = lamina_slicer::profile::about(name);
    match lamina_slicer::profile::limits(name) {
        Some((lo, hi)) => format!("{about} From {lo} to {hi}."),
        None => about.to_string(),
    }
}

/// The colour of a warning: a mesh with problems, a plate changed since its slice.
pub(crate) const WARNING: Color32 = Color32::from_rgb(230, 180, 90);

/// A session with every command: the core's, the slicer's, the booleans' and the printer connection's.
pub fn session() -> Session {
    let mut s = Session::new();
    s.register(lamina_slicer::COMMANDS);
    s.register(lamina_csg::COMMANDS);
    s.register(lamina_printer::COMMANDS);
    s
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Prepare,
    Preview,
}

/// The kinds of preset: a printer sets the machine's fields, a filament its temperatures and fan, a process the layers and strength.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Printer,
    Filament,
    Process,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Printer => "printer",
            Kind::Filament => "filament",
            Kind::Process => "process",
        }
    }

    fn fields(self) -> &'static [&'static str] {
        match self {
            Kind::Printer => PRINTER_FIELDS,
            Kind::Filament => FILAMENT_FIELDS,
            Kind::Process => PROCESS_FIELDS,
        }
    }

    fn built_in(self) -> &'static [presets::Preset] {
        match self {
            Kind::Printer => PRINTERS,
            Kind::Filament => FILAMENTS,
            Kind::Process => PROCESSES,
        }
    }
}

/// The position (the middle of the footprint, and the bottom) and size fields of the picked object.
#[derive(Clone, Debug, PartialEq)]
pub struct Fields {
    id: u32,
    scene: usize,
    pub at: [f64; 3],
    pub size: [f64; 3],
}

/// A slice of the plate, read back from its G-code, and which scene it was made of (so the preview can say it is out of date).
pub struct Sliced {
    pub layers: Vec<preview::Layer>,
    pub summary: String,
    pub warnings: Vec<String>,
    scene: usize,
}

pub struct App {
    pub session: Session,
    pub selected: Vec<u32>,
    pub view: view3d::View,
    pub tab: Tab,
    pub profile: Profile,
    pub sliced: Option<Sliced>,
    pub layer: usize,
    pub travels: bool,
    pub status: String,
    pub failed: bool,
    nudge: [f64; 3],
    /// The height the Cut button cuts the picked object at.
    cut_z: f64,
    /// The picked object's position and size as the fields show them while they are edited: the object and the scene they were
    /// read from, so they are read again when either changes.
    fields: Option<Fields>,
    /// Whether a size typed keeps the proportions.
    pub keep_proportions: bool,
    axis: usize,
    degrees: f64,
    factor: f64,
    /// The share of a heavy mesh's triangles Simplify keeps, %.
    keep: f64,
    pub chat: chat::ChatPanel,
    /// The presets last applied, as labels, and the ones the user saved.
    pub printer: Option<String>,
    pub filament: Option<String>,
    pub process: Option<String>,
    pub my_printers: Presets,
    pub my_filaments: Presets,
    pub my_processes: Presets,
    preset_name: String,
    /// Where the settings are kept between runs, and what was last written there; none in a test that does not ask for it.
    pub settings_path: Option<PathBuf>,
    saved: Option<Settings>,
    /// The project file open, if it has one; the plate and the print settings as it was last opened or saved.
    pub project: Option<PathBuf>,
    saved_scene: Arc<Scene>,
    saved_print: Value,
    /// Recent projects, newest first.
    pub recent: Vec<String>,
    /// What waits for an answer to "save the changes?", and whether the window may now close.
    pub ask: Option<files::Pending>,
    may_close: bool,
    closing: bool,
    title: String,
    /// Where a sliced plate is sent.
    pub link: printer::PrinterPanel,
    /// The autosave beside the settings file (none in a test that does not ask), and a plate an unclean exit left, to offer back.
    pub autosave: Option<autosave::Autosave>,
    pub recovery: Option<PathBuf>,
    /// What is wrong with each mesh, by the address of its `Arc`, worked out once and kept while the mesh lives.
    problems: std::collections::HashMap<usize, lamina_core::repair::Problems>,
    /// The long work running on a thread, if any.
    pub job: Option<work::Job>,
    /// The preview in 3D (every layer up to the chosen one) or the chosen layer alone from above.
    pub preview_3d: bool,
    pub preview: preview::Preview3d,
    /// The theme chosen, and the one last set on the window.
    pub look: comfort::Look,
    applied_look: Option<comfort::Look>,
    /// The log beside the settings file, where each error the status bar shows is written once.
    pub log: Option<PathBuf>,
    logged: String,
    /// The control port agents reach the plate through, when it is open; the Agents section as the settings keep it; why the port
    /// could not be opened; and where the bridge file goes (beside the settings; none in a test that does not ask).
    pub control: Option<control::Control>,
    pub agents: control::Agents,
    pub agents_failed: Option<String>,
    pub bridge_file: Option<PathBuf>,
}

impl App {
    /// A window on `session`; the assistant reaches its provider through `net` (a test passes one that never leaves the machine).
    pub fn new(session: Session, net: Arc<dyn Net>) -> App {
        App {
            session,
            selected: Vec::new(),
            view: view3d::View::default(),
            tab: Tab::Prepare,
            profile: Profile::default(),
            sliced: None,
            layer: 0,
            travels: false,
            status: String::new(),
            failed: false,
            nudge: [10.0, 0.0, 0.0],
            cut_z: 10.0,
            fields: None,
            keep_proportions: true,
            axis: 2,
            degrees: 90.0,
            factor: 1.5,
            keep: 25.0,
            chat: chat::ChatPanel::new(net),
            printer: None,
            filament: None,
            process: None,
            my_printers: Presets::new(),
            my_filaments: Presets::new(),
            my_processes: Presets::new(),
            preset_name: String::new(),
            settings_path: None,
            saved: None,
            project: None,
            saved_scene: Arc::new(Scene::default()),
            saved_print: Value::Null,
            recent: Vec::new(),
            ask: None,
            may_close: false,
            closing: false,
            title: String::new(),
            problems: std::collections::HashMap::new(),
            link: printer::PrinterPanel::default(),
            autosave: None,
            recovery: None,
            job: None,
            preview_3d: true,
            preview: preview::Preview3d::default(),
            look: comfort::Look::default(),
            applied_look: None,
            log: None,
            logged: String::new(),
            control: None,
            agents: control::Agents::default(),
            agents_failed: None,
            bridge_file: None,
        }
        .ready()
    }

    /// The window as it starts: what it shows is what is saved.
    fn ready(mut self) -> App {
        self.mark_saved();
        self
    }

    /// A window that keeps its settings in the file at `path`: what is there now is taken (the print settings, the presets, a
    /// remembered key), and what changes is written back as it changes. What does not read is left out and said on the status bar.
    pub fn with_settings(session: Session, net: Arc<dyn Net>, path: PathBuf) -> App {
        let mut app = App::new(session, net);
        let (s, mut notes) = Settings::read(&path);
        if let Some(p) = &s.profile {
            app.profile = p.clone();
        }
        (app.printer, app.filament, app.my_printers, app.my_filaments) =
            (s.printer.clone(), s.filament.clone(), s.printers.clone(), s.filaments.clone());
        app.recent = s.recent.clone();
        app.look = s.look.unwrap_or_default();
        if let Some(u) = &s.local_url {
            app.chat.chat.local_url.clone_from(u);
        }
        if let Some(m) = &s.local_model {
            app.chat.local_model.clone_from(m);
        }
        (app.process, app.my_processes) = (s.process.clone(), s.processes.clone());
        let (panel, note) = printer::PrinterPanel::restore(s.connection.clone(), s.connection_key.clone());
        app.link = panel;
        notes.extend(note);
        notes.extend(app.chat.restore(s.keys.clone()));
        if !notes.is_empty() {
            (app.failed, app.status) = (true, notes.join(" "));
        }
        app.saved = Some(app.settings());
        let a = autosave::Autosave::beside(&path);
        app.recovery = a.path.exists().then(|| a.path.clone());
        app.autosave = Some(a);
        app.log = Some(startup::Files::beside(&path).log);
        app.bridge_file = Some(path.with_file_name(lamina_core::bridge::FILE));
        if let Some(a) = s.agents.clone() {
            app.agents = a;
        }
        if app.agents.listen {
            app.open_control();
        }
        app.settings_path = Some(path);
        app.mark_saved();
        app
    }

    /// What the settings file would hold now.
    fn settings(&self) -> Settings {
        Settings {
            profile: Some(self.profile.clone()),
            printer: self.printer.clone(),
            filament: self.filament.clone(),
            printers: self.my_printers.clone(),
            filaments: self.my_filaments.clone(),
            process: self.process.clone(),
            processes: self.my_processes.clone(),
            keys: self.chat.saved.clone(),
            recent: self.recent.clone(),
            connection: Some(self.link.connection.clone()).filter(|c| !c.address.trim().is_empty()),
            connection_key: self.link.sealed.clone(),
            look: Some(self.look).filter(|l| *l != comfort::Look::default()),
            local_url: Some(self.chat.chat.local_url.clone()).filter(|u| u != lamina_assistant::local::DEFAULT_URL),
            local_model: Some(self.chat.local_model.clone()).filter(|m| m != lamina_assistant::Provider::Local.default_model()),
            agents: Some(self.agents.clone()).filter(|a| *a != control::Agents::default()),
        }
    }

    /// Writes the settings when they changed, but not while a value is being dragged, so a drag is one write and not one a frame.
    fn persist(&mut self, ctx: &egui::Context) {
        let Some(path) = self.settings_path.clone() else { return };
        let now = self.settings();
        if self.saved.as_ref() == Some(&now) || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        match now.write(&path) {
            Ok(()) => self.saved = Some(now),
            Err(e) => {
                (self.failed, self.status) = (true, e);
                self.saved = Some(now); // said once, not every frame
            }
        }
    }

    fn mine(&mut self, kind: Kind) -> &mut Presets {
        match kind {
            Kind::Printer => &mut self.my_printers,
            Kind::Filament => &mut self.my_filaments,
            Kind::Process => &mut self.my_processes,
        }
    }

    /// Applies a preset by name, the user's first, then a built-in, with the presets it inherits: its fields go over the settings
    /// shown, the others stay.
    pub fn apply_preset(&mut self, kind: Kind, name: &str) {
        let mine = self.mine(kind).clone();
        let fields = presets::resolve(name, |n| mine.get(n), kind.built_in());
        match fields.and_then(|f| presets::apply(&self.profile, &f)) {
            Ok(p) => {
                self.profile = p;
                let label = Some(name.to_string());
                match kind {
                    Kind::Printer => self.printer = label,
                    Kind::Filament => self.filament = label,
                    Kind::Process => self.process = label,
                }
                (self.failed, self.status) = (false, String::new());
            }
            Err(e) => (self.failed, self.status) = (true, e),
        }
    }

    /// Saves the settings shown that a preset of `kind` holds, under `name`.
    pub fn save_preset(&mut self, kind: Kind, name: &str) {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            (self.failed, self.status) = (true, "A preset needs a name of up to 60 characters.".into());
            return;
        }
        if kind.built_in().iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
            (self.failed, self.status) = (true, format!("\"{name}\" is a built-in {}: choose another name.", kind.word()));
            return;
        }
        let mut fields = presets::capture(&self.profile, kind.fields());
        // Saved over the preset chosen now, it keeps only what differs from it and names it, so a change to that one carries over.
        let base = match kind {
            Kind::Printer => self.printer.clone(),
            Kind::Filament => self.filament.clone(),
            Kind::Process => self.process.clone(),
        };
        let mine = self.mine(kind).clone();
        if let Some(base) = base.filter(|b| !b.eq_ignore_ascii_case(name))
            && let Ok(inherited) = presets::resolve(&base, |n| mine.get(n), kind.built_in())
            && let Ok(under) = presets::apply(&Profile::default(), &inherited)
        {
            // Only what the base itself sets can be left to it; read back through a profile, so 240 and 240.0 compare equal.
            let theirs = presets::capture(&under, kind.fields());
            fields.retain(|k, v| !(inherited.contains_key(k) && theirs.get(k) == Some(v)));
            fields.insert(presets::INHERITS.into(), Value::String(base));
        }
        self.mine(kind).insert(name.to_string(), fields);
        self.apply_preset(kind, name);
        self.status = format!("Saved the {} preset \"{name}\".", kind.word());
    }

    fn preset_picker(&mut self, ui: &mut Ui, kind: Kind) {
        let chosen = match kind {
            Kind::Printer => self.printer.clone(),
            Kind::Filament => self.filament.clone(),
            Kind::Process => self.process.clone(),
        };
        let mut pick = None;
        let mine: Vec<String> = match kind {
            Kind::Printer => self.my_printers.keys().cloned().collect(),
            Kind::Filament => self.my_filaments.keys().cloned().collect(),
            Kind::Process => self.my_processes.keys().cloned().collect(),
        };
        egui::ComboBox::from_id_salt(kind.word()).selected_text(chosen.clone().unwrap_or_else(|| "Choose...".into())).width(170.0).show_ui(
            ui,
            |ui| {
                for p in kind.built_in() {
                    if ui.selectable_label(chosen.as_deref() == Some(p.name), p.name).on_hover_text(p.about).clicked() {
                        pick = Some(p.name.to_string());
                    }
                }
                if !mine.is_empty() {
                    ui.separator();
                }
                for name in &mine {
                    if ui.selectable_label(chosen.as_deref() == Some(name), format!("{name} (mine)")).clicked() {
                        pick = Some(name.clone());
                    }
                }
            },
        );
        if let Some(name) = chosen.filter(|n| mine.contains(n))
            && ui.small_button("Delete").on_hover_text("Forget this preset of yours").clicked()
        {
            self.mine(kind).remove(&name);
        }
        if let Some(name) = pick {
            self.apply_preset(kind, &name);
        }
    }

    fn presets_ui(&mut self, ui: &mut Ui) {
        egui::Grid::new("presets").num_columns(2).show(ui, |ui| {
            ui.label("Printer");
            ui.horizontal(|ui| self.preset_picker(ui, Kind::Printer));
            ui.end_row();
            ui.label("Filament");
            ui.horizontal(|ui| self.preset_picker(ui, Kind::Filament));
            ui.end_row();
            ui.label("Process");
            ui.horizontal(|ui| self.preset_picker(ui, Kind::Process));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text("Preset name").desired_width(110.0));
            let name = self.preset_name.clone();
            if ui.button("Save printer").on_hover_text("Save the plate, nozzle, G-code and retraction shown").clicked() {
                self.save_preset(Kind::Printer, &name);
            }
            if ui.button("Save filament").on_hover_text("Save the temperatures and fan shown").clicked() {
                self.save_preset(Kind::Filament, &name);
            }
            if ui.button("Save process").on_hover_text("Save the layers, walls, skins and infill shown").clicked() {
                self.save_preset(Kind::Process, &name);
            }
        });
    }

    /// Runs a command as a button does: an error goes to the status bar, and the selection forgets objects that are gone.
    pub fn run(&mut self, command: &str, params: Value) -> Option<Value> {
        if self.waits() {
            return None;
        }
        let result = self.session.execute(command, &params);
        self.prune();
        match result {
            Ok(v) => {
                (self.failed, self.status) = (false, String::new());
                Some(v)
            }
            Err(e) => {
                (self.failed, self.status) = (true, e);
                None
            }
        }
    }

    /// Runs several commands as one undo step, as a button does: all of them or none.
    pub fn run_all(&mut self, label: &str, commands: Vec<(&str, Value)>) -> bool {
        if self.waits() {
            return false;
        }
        let result = self.session.transaction(label, |s| commands.iter().try_for_each(|(c, p)| s.execute(c, p).map(|_| ())));
        self.prune();
        match result {
            Ok(()) => {
                (self.failed, self.status) = (false, String::new());
                true
            }
            Err(e) => {
                (self.failed, self.status) = (true, e);
                false
            }
        }
    }

    /// Whether a command must wait for a job that changes the plate (said on the status bar), so neither undoes the other.
    fn waits(&mut self) -> bool {
        let Some(job) = self.job.as_ref().filter(|j| j.changes_plate) else { return false };
        (self.failed, self.status) = (true, format!("Wait for {} to finish, or cancel it.", job.label));
        true
    }

    /// Deletes every picked object, as one undo step.
    pub fn delete_selected(&mut self) {
        let commands = self.selected.iter().map(|&id| ("delete", json!({ "id": id }))).collect();
        self.run_all("delete", commands);
    }

    fn prune(&mut self) {
        let alive: Vec<u32> = self.session.scene.objects.iter().map(|o| o.id).collect();
        self.selected.retain(|id| alive.contains(id));
    }

    /// Opens a project or imports a mesh, by its extension (the file named on the command line).
    pub fn open(&mut self, path: &str) {
        self.open_file(std::path::Path::new(path));
    }

    fn add_box(&mut self) {
        self.add(json!({"kind": "box", "size": [20, 20, 20]}), 20.0);
    }

    /// Adds a shape `width` mm wide to the right of what is on the plate, so it does not land inside another, and selects it.
    fn add(&mut self, params: Value, width: f64) {
        self.add_with("add_primitive", params, width);
    }

    /// The same, with any command that makes one object and takes `at`.
    fn add_with(&mut self, command: &str, mut params: Value, width: f64) {
        let right = self.session.scene.objects.iter().filter_map(|o| o.bounds()).map(|(_, hi)| hi[0]).fold(f64::NEG_INFINITY, f64::max);
        if right.is_finite() {
            params["at"] = json!([right + 5.0 + width / 2.0, 0.0]);
        }
        if let Some(id) = self.run(command, params).and_then(|v| v["id"].as_u64()) {
            self.selected = vec![id as u32];
        }
    }

    fn frame_all(&mut self) {
        let bounds = self.session.scene.objects.iter().filter_map(|o| o.bounds()).reduce(|(alo, ahi), (blo, bhi)| {
            ([alo[0].min(blo[0]), alo[1].min(blo[1]), alo[2].min(blo[2])], [ahi[0].max(bhi[0]), ahi[1].max(bhi[1]), ahi[2].max(bhi[2])])
        });
        let [bx, by] = self.profile.bed;
        self.view.camera.frame_all(bounds.unwrap_or(([-bx / 2.0, -by / 2.0, 0.0], [bx / 2.0, by / 2.0, 0.0])));
    }

    /// A point clicked with the measure tool: the first of two, or the second, which measures between them.
    pub fn measure(&mut self, p: [f64; 3]) {
        if self.view.marks.len() >= 2 {
            self.view.marks.clear();
        }
        self.view.marks.push(p);
        if let [a, b] = self.view.marks[..]
            && let Some(m) = self.run("measure", json!({ "a": a, "b": b }))
        {
            let f = |k: &str| m[k].as_f64().unwrap_or(0.0);
            self.status = format!("{:.2} mm (x {:+.2}, y {:+.2}, z {:+.2})", f("distance"), f("dx"), f("dy"), f("dz"));
        } else if self.view.marks.len() == 1 {
            self.status = "Click a second point to measure to.".into();
        }
    }

    /// Slices the plate with the settings shown, on a thread, and opens the preview when it is done.
    pub fn slice(&mut self) {
        let objects: Vec<(u32, Mesh)> = self.session.scene.objects.iter().map(|o| (o.id, o.world())).collect();
        let all: Vec<&lamina_core::Object> = self.session.scene.objects.iter().collect();
        let own = match lamina_slicer::commands::own(&all, &self.profile) {
            Ok(own) => own,
            Err(e) => return (self.failed, self.status) = (true, e),
        };
        let profile = self.profile.clone();
        let scene = Arc::as_ptr(&self.session.scene) as usize;
        let work = move |_: &mut Session| {
            let out = lamina_slicer::print::gcode_each(&objects, &own, &profile)?;
            let layers = preview::parse(&out.gcode);
            let minutes = (out.seconds / 60.0).round() as u64;
            let summary = format!(
                "{} layers, {:.2} m of filament, about {}h {:02}m",
                out.layers,
                out.filament_mm / 1000.0,
                minutes / 60,
                minutes % 60
            );
            Ok(work::Done::Sliced(Sliced { layers, summary, warnings: out.warnings, scene }))
        };
        self.start("Slicing", false, work, |app, done| {
            if let Ok(work::Done::Sliced(sliced)) = done {
                app.layer = sliced.layers.len() / 2;
                app.sliced = Some(sliced);
                app.tab = Tab::Preview;
            }
        });
    }

    /// Takes another slicer's profile into the settings shown, and says what was left out.
    pub fn import_profile(&mut self, path: &std::path::Path) {
        let Some(r) = self.run("import_profile", json!({ "path": path })) else { return };
        let fields = r["fields"].as_object().cloned().unwrap_or_default();
        match presets::apply(&self.profile, &fields) {
            Ok(p) => {
                self.profile = p;
                let left: Vec<String> = r["left_out"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|l| l.split(':').next().unwrap_or(l).to_string())
                    .collect();
                self.status = match left.len() {
                    0 => format!("Took {} settings from {}.", fields.len(), path.display()),
                    n => format!("Took {} settings from {}; left out {n}: {}.", fields.len(), path.display(), left.join(", ")),
                };
            }
            Err(e) => (self.failed, self.status) = (true, e),
        }
    }

    fn export(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("G-code", &["gcode"]).set_file_name("plate.gcode").save_file() else { return };
        self.export_to(path.to_string_lossy().into_owned());
    }

    /// Slices the plate with the settings shown into the G-code file at `path`, on a thread.
    pub fn export_to(&mut self, path: String) {
        let profile = serde_json::to_value(&self.profile).unwrap_or(Value::Null);
        let params = json!({ "path": path, "profile": profile });
        self.start(
            "Writing the G-code",
            false,
            move |s| s.execute("slice", &params).map(work::Done::Value),
            move |app, done| {
                if done.is_ok() {
                    app.status = format!("Wrote {path}");
                }
            },
        );
    }

    /// The whole window, one frame of it.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        // The window's print settings are the session's, so the assistant's `slice_check` and a saved project use what is shown.
        let print = self.print_json();
        if self.session.print != print {
            self.session.print = print;
        }
        if self.chat.poll(&ctx, &mut self.session) {
            self.prune();
        }
        self.dropped(&ctx);
        self.file_keys(&ctx);
        self.keys(&ctx);
        self.shortcuts(&ctx);
        self.apply_look(&ctx);
        Panel::top("toolbar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                self.file_menu(ui);
                self.edit_menu(ui);
                self.view_menu(ui);
            });
            self.toolbar(ui);
        });
        self.poll_work(&ctx);
        self.answer_agents(&ctx);
        Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                self.work_ui(ui);
                let color = if self.failed { Color32::from_rgb(230, 110, 100) } else { Color32::GRAY };
                let idle = if self.busy() { "" } else { "Ready" };
                ui.label(RichText::new(if self.status.is_empty() { idle } else { &self.status }).color(color));
            });
        });
        Panel::left("plate").resizable(true).default_size(270.0).show(ui, |ui| {
            ScrollArea::vertical().show(ui, |ui| self.plate_panel(ui));
        });
        Panel::right("assistant").resizable(true).default_size(330.0).show(ui, |ui| self.chat.show(ui));
        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Prepare => self.prepare(ui),
            Tab::Preview => self.preview(ui),
        });
        self.closing(&ctx);
        self.recovery_ui(&ctx);
        self.autosave_tick();
        let title = self.title();
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        self.persist(&ctx);
        self.log_error();
    }

    /// Writes the error on the status bar to the log, once.
    fn log_error(&mut self) {
        if self.failed && !self.status.is_empty() && self.status != self.logged {
            if let Some(log) = &self.log {
                startup::note(log, &format!("error: {}", self.status));
            }
            self.logged = self.status.clone();
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (undo, redo, delete) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (
                cmd && !i.modifiers.shift && i.key_pressed(Key::Z),
                cmd && (i.key_pressed(Key::Y) || (i.modifiers.shift && i.key_pressed(Key::Z))),
                i.key_pressed(Key::Delete),
            )
        });
        if undo {
            self.run("undo", json!({}));
        }
        if redo {
            self.run("redo", json!({}));
        }
        if delete {
            self.delete_selected();
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui.button("Add box").clicked() {
                self.add_box();
            }
            if ui.button("Add cylinder").clicked() {
                self.add(json!({"kind": "cylinder", "radius": 10, "height": 20}), 20.0);
            }
            if ui.button("Add sphere").clicked() {
                self.add(json!({"kind": "sphere", "radius": 10}), 20.0);
            }
            ui.menu_button("More shapes", |ui| {
                let shapes = [
                    ("Rounded box", json!({"kind": "rounded_box", "size": [20, 20, 20], "corner": 3}), 20.0),
                    ("Cone", json!({"kind": "cone", "radius": 10, "height": 20}), 20.0),
                    ("Tube", json!({"kind": "tube", "radius": 10, "inner": 7, "height": 20}), 20.0),
                    ("Torus", json!({"kind": "torus", "radius": 10, "tube": 3}), 26.0),
                    ("Hexagonal prism", json!({"kind": "prism", "radius": 10, "height": 20, "sides": 6}), 20.0),
                    ("Pyramid", json!({"kind": "pyramid", "radius": 12, "height": 15, "sides": 4}), 24.0),
                ];
                for (label, params, width) in shapes {
                    if ui.button(label).clicked() {
                        self.add(params, width);
                    }
                }
                ui.separator();
                if ui.button("Text").on_hover_text("Lettering, 10 mm capitals, 1 mm deep; the assistant or `text` writes any").clicked() {
                    self.add_with("text", json!({"text": "Lamina", "height": 10, "depth": 1}), 50.0);
                }
                if ui
                    .button("Plate with four holes")
                    .on_hover_text("A rounded plate, extruded; the assistant or `extrude` makes any outline")
                    .clicked()
                {
                    let plate = json!({"shape": "rounded_rect", "size": [60, 40], "corner": 5, "height": 4, "name": "plate",
                        "round_holes": [[-24, -14, 3.4], [24, -14, 3.4], [24, 14, 3.4], [-24, 14, 3.4]]});
                    self.add_with("extrude", plate, 60.0);
                }
            });
            if ui.button("Import mesh...").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("Mesh", files::MESHES).pick_file()
            {
                self.import(&p);
            }
            ui.separator();
            let undo = self.session.undo_label().map_or("Nothing to undo".to_string(), |l| format!("Undo {l} (Ctrl+Z)"));
            if ui.button("Undo").on_hover_text(undo).clicked() {
                self.run("undo", json!({}));
            }
            if ui.button("Redo").on_hover_text("Bring back what undo took (Ctrl+Y)").clicked() {
                self.run("redo", json!({}));
            }
            if ui.button("Frame all").on_hover_text("Look at everything on the plate (F frames the picked objects)").clicked() {
                self.frame_all();
            }
            if ui.button("Arrange").on_hover_text("Lay every object out on the plate, none touching (A)").clicked() {
                self.arrange();
            }
            ui.separator();
            {
                use crate::view3d::Tool;
                let before = self.view.tool;
                ui.selectable_value(&mut self.view.tool, Tool::Move, "Move")
                    .on_hover_text("Pick objects and drag them with the arrows (G)");
                ui.selectable_value(&mut self.view.tool, Tool::LayFlat, "Lay flat")
                    .on_hover_text("Click a face to turn it down onto the plate (L)");
                ui.selectable_value(&mut self.view.tool, Tool::Measure, "Measure")
                    .on_hover_text("Click two points to measure between them (M)");
                if self.view.tool != before {
                    self.view.marks.clear();
                }
            }
            ui.separator();
            ui.selectable_value(&mut self.tab, Tab::Prepare, "Prepare");
            ui.selectable_value(&mut self.tab, Tab::Preview, "Preview");
            ui.separator();
            if ui.button("Slice").on_hover_text("Slice the plate with these settings and show the layers").clicked() {
                self.slice();
            }
            if ui.button("Export G-code...").on_hover_text("Slice the plate into a G-code file").clicked() {
                self.export();
            }
        });
    }

    fn pick(&mut self, id: u32, ui: &Ui) {
        if ui.input(|i| i.modifiers.command || i.modifiers.shift) {
            match self.selected.iter().position(|&s| s == id) {
                Some(at) => {
                    self.selected.remove(at);
                }
                None => self.selected.push(id),
            }
        } else {
            self.selected = vec![id];
        }
    }

    /// What is wrong with object `id`'s mesh (none for a sound one), worked out once per mesh.
    pub fn problems_of(&mut self, id: u32) -> Option<lamina_core::repair::Problems> {
        let o = self.session.scene.get(id).ok()?;
        let key = Arc::as_ptr(&o.mesh) as usize;
        if !self.problems.contains_key(&key) {
            // Forget meshes no longer on the plate before adding one.
            let alive: Vec<usize> = self.session.scene.objects.iter().map(|o| Arc::as_ptr(&o.mesh) as usize).collect();
            self.problems.retain(|k, _| alive.contains(k));
            self.problems.insert(key, lamina_core::repair::problems(&o.mesh));
        }
        self.problems.get(&key).copied().filter(|p| p.any())
    }

    fn plate_panel(&mut self, ui: &mut Ui) {
        ui.heading("Plate");
        let rows: Vec<(u32, String)> = self
            .session
            .scene
            .objects
            .iter()
            .map(|o| {
                let (lo, hi) = o.bounds().unwrap_or_default();
                (o.id, format!("{}  {:.1} x {:.1} x {:.1}", o.name, hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]))
            })
            .collect();
        let rows: Vec<(u32, String, Option<lamina_core::repair::Problems>)> =
            rows.into_iter().map(|(id, text)| (id, text, self.problems_of(id))).collect();
        if rows.is_empty() {
            ui.label(
                RichText::new("Nothing on the plate yet: add a shape, import or drop a mesh, open a project, or ask the assistant.").weak(),
            );
        }
        for (id, text, problems) in rows {
            ui.horizontal(|ui| {
                let row = ui.selectable_label(self.selected.contains(&id), text);
                if row.clicked() {
                    self.pick(id, ui);
                }
                if row.secondary_clicked() {
                    self.context_pick(Some(id));
                }
                row.context_menu(|ui| self.object_menu(ui));
                if let Some(p) = problems {
                    ui.label(RichText::new("\u{26a0}").color(WARNING)).on_hover_text(format!(
                        "This mesh is not a clean solid: {}. Parts of it may not print; Repair mends what it can.",
                        p.describe()
                    ));
                }
            });
        }
        ui.label(RichText::new("Ctrl-click picks several; right-click for more.").small().weak());
        ui.separator();
        match self.selected.as_slice() {
            [] => {}
            &[id] => self.one_object(ui, id),
            many => {
                let ids = many.to_vec();
                ui.label(format!("{} objects picked", ids.len()));
                ui.horizontal(|ui| {
                    if ui.button("Join").on_hover_text("One solid of all of them (union)").clicked() {
                        self.boolean("union", json!({ "ids": ids }));
                    }
                    if ui.button("Subtract").on_hover_text("Cut the others out of the first one picked").clicked() {
                        self.boolean("difference", json!({ "id": ids[0], "minus": ids[1..] }));
                    }
                    if ui.button("Intersect").on_hover_text("Keep only what they all share").clicked() {
                        self.boolean("intersect", json!({ "ids": ids }));
                    }
                });
                if ui.button("Delete all").on_hover_text("Delete the picked objects, as one undo step").clicked() {
                    self.delete_selected();
                }
            }
        }
        ui.separator();
        self.presets_ui(ui);
        self.print_settings(ui);
        self.printer_ui(ui);
        self.agents_ui(ui);
    }

    /// A boolean or a cut, on a thread: its result replaces the plate as one undo step, and what it made is picked.
    fn boolean(&mut self, command: &'static str, params: Value) {
        self.start(
            command,
            true,
            move |s| s.execute(command, &params).map(work::Done::Value),
            |app, done| {
                if let Ok(work::Done::Value(v)) = done {
                    let one = v["id"].as_u64().into_iter();
                    let many = v["ids"].as_array().into_iter().flatten().filter_map(Value::as_u64);
                    app.selected = one.chain(many).map(|i| i as u32).collect();
                }
            },
        );
    }

    fn one_object(&mut self, ui: &mut Ui, id: u32) {
        if let Some(p) = self.problems_of(id) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("\u{26a0} {}", p.describe())).color(WARNING));
                if ui.button("Repair").on_hover_text("Drop doubled faces, turn flipped ones, close cracks and small holes").clicked()
                    && let Some(r) = self.run("repair", json!({ "id": id }))
                {
                    self.status = format!(
                        "Repaired: {} faces dropped, {} turned, {} holes filled; {}.",
                        r["dropped"],
                        r["flipped"],
                        r["holes_filled"],
                        if r["watertight"] == true {
                            "it is closed now"
                        } else {
                            "it is still open: a hole is too big to fill without guessing"
                        }
                    );
                }
            });
        }
        self.position_fields(ui, id);
        self.object_settings(ui, id);
        ui.label("Move by, mm");
        ui.horizontal(|ui| {
            for (k, axis) in ["x", "y", "z"].into_iter().enumerate() {
                ui.add(egui::DragValue::new(&mut self.nudge[k]).speed(0.5).prefix(format!("{axis} ")));
            }
            if ui.button("Move").clicked() {
                self.run("move", json!({ "id": id, "by": self.nudge }));
            }
        });
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("axis").selected_text(["x", "y", "z"][self.axis]).width(40.0).show_ui(ui, |ui| {
                for (k, axis) in ["x", "y", "z"].into_iter().enumerate() {
                    ui.selectable_value(&mut self.axis, k, axis);
                }
            });
            ui.add(egui::DragValue::new(&mut self.degrees).speed(1.0).suffix(" deg"));
            if ui.button("Rotate").clicked() {
                let axis = ["x", "y", "z"][self.axis];
                self.run("rotate", json!({ "id": id, "axis": axis, "degrees": self.degrees }));
            }
            if ui.button("Mirror").on_hover_text("Mirror across the plane square to the axis chosen").clicked() {
                let axis = ["x", "y", "z"][self.axis];
                self.run("mirror", json!({ "id": id, "axis": axis }));
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut self.factor).speed(0.01).range(0.01..=100.0).prefix("x "));
            if ui.button("Scale").clicked() {
                self.run("scale", json!({ "id": id, "factor": self.factor }));
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut self.cut_z).speed(0.2).prefix("z ").suffix(" mm"));
            if ui
                .button("Cut")
                .on_hover_text("Cut at this height: the top part is turned onto its cut face beside the bottom one")
                .clicked()
            {
                self.boolean("cut", json!({ "id": id, "z": self.cut_z }));
            }
        });
        // A heavy mesh (a scan, say) can be given fewer triangles; on a thread, as it can take seconds.
        let triangles = self.session.scene.get(id).map_or(0, |o| o.mesh.triangles.len());
        if triangles > 1000 {
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.keep).speed(1.0).range(1.0..=99.0).suffix(" %"))
                    .on_hover_text("The share of its triangles to keep");
                if ui
                    .button("Simplify")
                    .on_hover_text(format!("{triangles} triangles now; its shape is kept as well as it can be"))
                    .clicked()
                {
                    let params = json!({ "id": id, "ratio": self.keep / 100.0 });
                    self.start(
                        "simplify",
                        true,
                        move |s| s.execute("simplify", &params).map(work::Done::Value),
                        move |app, done| {
                            if let Ok(work::Done::Value(v)) = done {
                                app.status = format!("Simplified from {} to {} triangles.", v["before"], v["triangles"]);
                            }
                        },
                    );
                }
            });
        }
        ui.horizontal(|ui| {
            if ui.button("Duplicate").on_hover_text("A copy beside it; the plate is laid out again").clicked()
                && let Some(v) = self.run("duplicate", json!({ "id": id }))
                && let Some(copy) = v["ids"][0].as_u64()
            {
                self.selected = vec![copy as u32];
            }
            if ui.button("Drop to plate").clicked() {
                self.run("drop_to_bed", json!({ "id": id }));
            }
            if ui.button("Delete").clicked() {
                self.run("delete", json!({ "id": id }));
            }
        });
    }

    /// The picked object's position and size, as fields: a value typed (or dragged and let go) moves or scales it to that.
    fn position_fields(&mut self, ui: &mut Ui, id: u32) {
        let scene = Arc::as_ptr(&self.session.scene) as usize;
        let Some((lo, hi)) = self.session.scene.get(id).ok().and_then(|o| o.bounds()) else { return };
        if self.fields.as_ref().is_none_or(|f| f.id != id || f.scene != scene) {
            let at = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]];
            self.fields = Some(Fields { id, scene, at, size: [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]] });
        }
        let Some(mut f) = self.fields.clone() else { return };
        let (mut moved, mut sized) = (None, None);
        egui::Grid::new("fields").num_columns(4).show(ui, |ui| {
            ui.label("Position");
            for (k, axis) in ["x", "y", "z"].into_iter().enumerate() {
                let r = ui.add(egui::DragValue::new(&mut f.at[k]).speed(0.5).prefix(format!("{axis} ")).max_decimals(3));
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    moved = Some(k);
                }
            }
            ui.end_row();
            ui.label("Size");
            for (k, axis) in ["x", "y", "z"].into_iter().enumerate() {
                let r = ui
                    .add(egui::DragValue::new(&mut f.size[k]).speed(0.5).range(0.01..=10_000.0).prefix(format!("{axis} ")).max_decimals(3));
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    sized = Some(k);
                }
            }
            ui.end_row();
        });
        ui.checkbox(&mut self.keep_proportions, "Keep proportions");
        self.fields = Some(f.clone());
        let now = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]];
        if moved.is_some() {
            self.set_position(id, f.at, now);
        }
        if let Some(k) = sized {
            self.set_size(id, k, f.size[k], hi[k] - lo[k]);
        }
    }

    /// The picked object's own infill and walls, over the plate's.
    fn object_settings(&mut self, ui: &mut Ui, id: u32) {
        let Ok(o) = self.session.scene.get(id) else { return };
        let mine = o.settings.as_deref().and_then(Value::as_object).cloned().unwrap_or_default();
        let mut changed = None;
        egui::CollapsingHeader::new("This object's settings").show(ui, |ui| {
            let mut infill = mine.get("infill_density").and_then(Value::as_f64);
            let mut walls = mine.get("perimeters").and_then(Value::as_u64).map(|w| w as u32);
            let mut own_infill = infill.is_some();
            let mut own_walls = walls.is_some();
            ui.horizontal(|ui| {
                ui.checkbox(&mut own_infill, "Infill");
                let mut percent = infill.unwrap_or(self.profile.infill_density) * 100.0;
                if ui.add_enabled(own_infill, egui::DragValue::new(&mut percent).range(0.0..=100.0).suffix(" %")).changed()
                    || own_infill != infill.is_some()
                {
                    infill = own_infill.then_some(percent / 100.0);
                    changed = Some(());
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut own_walls, "Walls");
                let mut w = walls.unwrap_or(self.profile.perimeters);
                if ui.add_enabled(own_walls, egui::DragValue::new(&mut w).range(0..=10)).changed() || own_walls != walls.is_some() {
                    walls = own_walls.then_some(w);
                    changed = Some(());
                }
            });
            if changed.is_some() {
                let mut fields = mine.clone();
                for (k, v) in [("infill_density", infill.map(Value::from)), ("perimeters", walls.map(Value::from))] {
                    match v {
                        Some(v) => fields.insert(k.into(), v),
                        None => fields.remove(k),
                    };
                }
                changed = None;
                self.run("set_object_settings", json!({ "id": id, "profile": fields }));
            }
        });
    }

    /// Moves object `id` so its footprint's middle and bottom are `at` (they are `now`).
    pub fn set_position(&mut self, id: u32, at: [f64; 3], now: [f64; 3]) {
        let by = [at[0] - now[0], at[1] - now[1], at[2] - now[2]];
        if by.iter().any(|c| c.abs() > 1e-9) {
            self.run("move", json!({ "id": id, "by": by }));
        }
    }

    /// Scales object `id` so its size on axis `k` is `want` (it is `now`): that axis alone, or all three when keeping proportions.
    pub fn set_size(&mut self, id: u32, k: usize, want: f64, now: f64) {
        if now <= 0.0 || (want - now).abs() < 1e-9 {
            return;
        }
        let ratio = want / now;
        let mut by = if self.keep_proportions { [ratio; 3] } else { [1.0; 3] };
        by[k] = ratio;
        self.run("scale", json!({ "id": id, "by": by }));
    }

    fn print_settings(&mut self, ui: &mut Ui) {
        egui::CollapsingHeader::new("Print settings").default_open(true).show(ui, |ui| {
            let p = &mut self.profile;
            let max_layer = p.nozzle * 0.8;
            egui::Grid::new("settings").num_columns(2).show(ui, |ui| {
                ui.label("Layer height, mm");
                ui.add(egui::DragValue::new(&mut p.layer_height).speed(0.01).range(0.02..=max_layer)).on_hover_text(hint("layer_height"));
                ui.end_row();
                ui.label("");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut p.adaptive_layers, "Adaptive, from").on_hover_text(hint("adaptive_layers"));
                    ui.add_enabled(p.adaptive_layers, egui::DragValue::new(&mut p.min_layer_height).speed(0.01).range(0.02..=max_layer))
                        .on_hover_text(hint("min_layer_height"));
                });
                ui.end_row();
                ui.label("First layer, mm");
                ui.add(egui::DragValue::new(&mut p.first_layer_height).speed(0.01).range(0.02..=max_layer))
                    .on_hover_text(hint("first_layer_height"));
                ui.end_row();
                ui.label("Walls");
                ui.add(egui::DragValue::new(&mut p.perimeters).range(0..=10)).on_hover_text(hint("perimeters"));
                ui.end_row();
                ui.label("Top / bottom layers");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p.top_layers).range(0..=50)).on_hover_text(hint("top_layers"));
                    ui.add(egui::DragValue::new(&mut p.bottom_layers).range(0..=50)).on_hover_text(hint("bottom_layers"));
                });
                ui.end_row();
                ui.label("Infill");
                let mut percent = p.infill_density * 100.0;
                if ui
                    .add(egui::DragValue::new(&mut percent).speed(1.0).range(0.0..=100.0).suffix(" %"))
                    .on_hover_text(lamina_slicer::profile::about("infill_density"))
                    .changed()
                {
                    p.infill_density = percent / 100.0;
                }
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut p.vase, "Vase mode").on_hover_text(hint("vase"));
                ui.end_row();
                ui.label("Infill pattern");
                {
                    use lamina_slicer::infill::Pattern;
                    let name = |x: Pattern| match x {
                        Pattern::Lines => "Lines",
                        Pattern::Grid => "Grid",
                        Pattern::Triangles => "Triangles",
                        Pattern::Gyroid => "Gyroid",
                        Pattern::Honeycomb => "Honeycomb",
                    };
                    egui::ComboBox::from_id_salt("infill pattern")
                        .selected_text(name(p.infill_pattern))
                        .show_ui(ui, |ui| {
                            for x in [Pattern::Lines, Pattern::Grid, Pattern::Triangles, Pattern::Gyroid, Pattern::Honeycomb] {
                                ui.selectable_value(&mut p.infill_pattern, x, name(x));
                            }
                        })
                        .response
                        .on_hover_text(hint("infill_pattern"));
                }
                ui.end_row();
                ui.label("Nozzle / bed, C");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p.nozzle_temp).range(0.0..=400.0)).on_hover_text(hint("nozzle_temp"));
                    ui.add(egui::DragValue::new(&mut p.bed_temp).range(0.0..=400.0)).on_hover_text(hint("bed_temp"));
                });
                ui.end_row();
                ui.label("First layer nozzle / bed");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p.first_layer_nozzle_temp).range(0.0..=400.0))
                        .on_hover_text(hint("first_layer_nozzle_temp"));
                    ui.add(egui::DragValue::new(&mut p.first_layer_bed_temp).range(0.0..=400.0))
                        .on_hover_text(hint("first_layer_bed_temp"));
                });
                ui.end_row();
                ui.label("Seam");
                {
                    use lamina_slicer::profile::Seam;
                    let name = |s: Seam| match s {
                        Seam::Aligned => "Aligned",
                        Seam::Rear => "At the back",
                        Seam::Sharpest => "Sharpest corner",
                        Seam::Nearest => "Nearest",
                    };
                    egui::ComboBox::from_id_salt("seam")
                        .selected_text(name(p.seam))
                        .show_ui(ui, |ui| {
                            for s in [Seam::Aligned, Seam::Rear, Seam::Sharpest, Seam::Nearest] {
                                ui.selectable_value(&mut p.seam, s, name(s));
                            }
                        })
                        .response
                        .on_hover_text(hint("seam"));
                }
                ui.end_row();
                ui.label("Brim, mm / raft layers");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p.brim_width).speed(0.5).range(0.0..=50.0)).on_hover_text(hint("brim_width"));
                    ui.add(egui::DragValue::new(&mut p.raft_layers).range(0..=10)).on_hover_text(hint("raft_layers"));
                });
                ui.end_row();
                ui.label("Supports");
                ui.horizontal(|ui| {
                    use lamina_slicer::profile::Support;
                    let name = |s: Support| match s {
                        Support::Off => "Off",
                        Support::Everywhere => "Everywhere",
                        Support::FromPlate => "From the plate only",
                    };
                    egui::ComboBox::from_id_salt("support")
                        .selected_text(name(p.support))
                        .show_ui(ui, |ui| {
                            for s in [Support::Off, Support::Everywhere, Support::FromPlate] {
                                ui.selectable_value(&mut p.support, s, name(s));
                            }
                        })
                        .response
                        .on_hover_text(hint("support"));
                    if p.support != Support::Off {
                        ui.add(egui::DragValue::new(&mut p.support_angle).range(10.0..=89.0).suffix(" deg"))
                            .on_hover_text(hint("support_angle"));
                    }
                });
                ui.end_row();
                ui.label("Plate, mm");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p.bed[0]).range(10.0..=2000.0)).on_hover_text(hint("bed"));
                    ui.add(egui::DragValue::new(&mut p.bed[1]).range(10.0..=2000.0)).on_hover_text(hint("bed"));
                });
                ui.end_row();
            });
            self.more_settings(ui);
            ui.horizontal(|ui| {
                if ui.button("Back to the defaults").clicked() {
                    self.profile = Profile::default();
                }
                if ui
                    .button("Import profile...")
                    .on_hover_text("A PrusaSlicer .ini, or an OrcaSlicer or Bambu Studio .json preset")
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new().add_filter("Profile", &["ini", "json"]).pick_file()
                {
                    self.import_profile(&path);
                }
            });
        });
    }

    /// Every other setting, in groups, each number's limits on hover; and the start and end G-code with their placeholders.
    fn more_settings(&mut self, ui: &mut Ui) {
        let p = &mut self.profile;
        let group = |ui: &mut Ui, title: &str, body: &mut dyn FnMut(&mut Ui)| {
            egui::CollapsingHeader::new(title).show(ui, |ui| egui::Grid::new(title).num_columns(2).show(ui, |ui| body(ui)));
        };
        group(ui, "Speeds", &mut |ui| {
            field(ui, "Walls, mm/s", "perimeter_speed", &mut p.perimeter_speed, 1.0);
            field(ui, "Infill, mm/s", "infill_speed", &mut p.infill_speed, 1.0);
            field(ui, "First layer, mm/s", "first_layer_speed", &mut p.first_layer_speed, 1.0);
            field(ui, "Travel, mm/s", "travel_speed", &mut p.travel_speed, 1.0);
            field(ui, "Bridges, mm/s", "bridge_speed", &mut p.bridge_speed, 1.0);
            field(ui, "Slowest when cooling, mm/s", "min_print_speed", &mut p.min_print_speed, 1.0);
        });
        group(ui, "Retraction and travel", &mut |ui| {
            field(ui, "Retract, mm", "retract_length", &mut p.retract_length, 0.1);
            field(ui, "Retract speed, mm/s", "retract_speed", &mut p.retract_speed, 1.0);
            field(ui, "Not under, mm of travel", "retract_min_travel", &mut p.retract_min_travel, 0.1);
            field(ui, "Z hop, mm", "z_hop", &mut p.z_hop, 0.05);
            field(ui, "Wipe, mm", "wipe_distance", &mut p.wipe_distance, 0.1);
            ui.checkbox(&mut p.combing, "Travel inside the part without retracting").on_hover_text(hint("combing"));
            ui.end_row();
            ui.checkbox(&mut p.firmware_retraction, "Firmware retraction (G10, G11)").on_hover_text(hint("firmware_retraction"));
            ui.end_row();
        });
        group(ui, "Skirt", &mut |ui| {
            field(ui, "Loops", "skirt_loops", &mut p.skirt_loops, 0.1);
            field(ui, "Distance, mm", "skirt_distance", &mut p.skirt_distance, 0.5);
        });
        group(ui, "Cooling", &mut |ui| {
            field(ui, "Fan, 0 to 255", "fan_speed", &mut p.fan_speed, 1.0);
            field(ui, "Full fan from layer", "fan_full_layer", &mut p.fan_full_layer, 0.1);
            field(ui, "Shortest layer, s", "min_layer_time", &mut p.min_layer_time, 0.5);
            field(ui, "Fan on bridges", "bridge_fan", &mut p.bridge_fan, 1.0);
            field(ui, "Bridge flow", "bridge_flow", &mut p.bridge_flow, 0.01);
        });
        group(ui, "Surface", &mut |ui| {
            ui.checkbox(&mut p.ironing, "Iron the top surfaces").on_hover_text(hint("ironing"));
            ui.end_row();
            field(ui, "Ironing flow", "ironing_flow", &mut p.ironing_flow, 0.01);
            field(ui, "Ironing spacing, mm", "ironing_spacing", &mut p.ironing_spacing, 0.01);
            field(ui, "Ironing speed, mm/s", "ironing_speed", &mut p.ironing_speed, 1.0);
            ui.checkbox(&mut p.fuzzy_skin, "Fuzzy skin on the outer wall").on_hover_text(hint("fuzzy_skin"));
            ui.end_row();
            field(ui, "Fuzzy thickness, mm", "fuzzy_thickness", &mut p.fuzzy_thickness, 0.01);
            field(ui, "Fuzzy spacing, mm", "fuzzy_spacing", &mut p.fuzzy_spacing, 0.05);
        });
        group(ui, "Support details", &mut |ui| {
            field(ui, "Density", "support_density", &mut p.support_density, 0.01);
            field(ui, "Gap above, mm", "support_z_gap", &mut p.support_z_gap, 0.05);
            field(ui, "Gap beside, mm", "support_xy_gap", &mut p.support_xy_gap, 0.05);
            field(ui, "Interface layers", "support_interface_layers", &mut p.support_interface_layers, 0.1);
        });
        group(ui, "Machine", &mut |ui| {
            field(ui, "Nozzle, mm", "nozzle", &mut p.nozzle, 0.05);
            field(ui, "Filament, mm", "filament_diameter", &mut p.filament_diameter, 0.05);
            field(ui, "Height, mm", "max_height", &mut p.max_height, 1.0);
            field(ui, "Acceleration, mm/s2", "max_acceleration", &mut p.max_acceleration, 10.0);
            field(ui, "Junction deviation, mm", "junction_deviation", &mut p.junction_deviation, 0.001);
            field(ui, "Pressure advance", "pressure_advance", &mut p.pressure_advance, 0.001);
            ui.label("Firmware");
            {
                use lamina_slicer::profile::Firmware;
                let name = |f: Firmware| match f {
                    Firmware::Marlin => "Marlin",
                    Firmware::Klipper => "Klipper",
                    Firmware::Reprap => "RepRapFirmware",
                };
                egui::ComboBox::from_id_salt("firmware").selected_text(name(p.firmware)).show_ui(ui, |ui| {
                    for f in [Firmware::Marlin, Firmware::Klipper, Firmware::Reprap] {
                        ui.selectable_value(&mut p.firmware, f, name(f));
                    }
                });
            }
            ui.end_row();
            ui.label("Thumbnail, pixels");
            ui.add(egui::DragValue::new(&mut p.thumbnail).range(0..=512))
                .on_hover_text(format!("{} 0, or from 16 to 512.", lamina_slicer::profile::about("thumbnail")));
            ui.end_row();
            ui.checkbox(&mut p.origin_center, "Zero in the middle (delta)").on_hover_text(hint("origin_center"));
            ui.end_row();
        });
        egui::CollapsingHeader::new("Start and end G-code").show(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "Placeholders: {}",
                    lamina_slicer::profile::PLACEHOLDERS.iter().map(|n| format!("[{n}]")).collect::<Vec<_>>().join(" ")
                ))
                .small(),
            );
            ui.label(
                RichText::new("In braces, an expression of them: {first_layer_bed_temp - 5}, {if(bed_temp > 90, 1, 0)}, with min, max and round; {{ for a brace.")
                    .small(),
            );
            ui.label("Start");
            ui.add(egui::TextEdit::multiline(&mut p.start_gcode).code_editor().desired_rows(4));
            ui.label("End");
            ui.add(egui::TextEdit::multiline(&mut p.end_gcode).code_editor().desired_rows(4));
            for text in [&p.start_gcode, &p.end_gcode] {
                if let Err(e) = p.expand(text) {
                    ui.label(RichText::new(e).color(WARNING));
                }
            }
        });
    }

    fn prepare(&mut self, ui: &mut Ui) {
        let scene = Arc::clone(&self.session.scene);
        let shown = self.view.show(ui, &scene, &self.selected, self.profile.bed);
        if let Some((ids, change)) = &shown.moved {
            use crate::view3d::Change;
            // One command for each picked object, in one transaction, so one undo takes the whole drag back.
            let (label, commands): (&str, Vec<(&str, Value)>) = match *change {
                Change::Move(by) => ("move", ids.iter().map(|&id| ("move", json!({ "id": id, "by": by }))).collect()),
                Change::Turn(axis, degrees) => {
                    let axis = ["x", "y", "z"][axis.min(2)];
                    ("rotate", ids.iter().map(|&id| ("rotate", json!({ "id": id, "axis": axis, "degrees": degrees }))).collect())
                }
                Change::Scale(f) => ("scale", ids.iter().map(|&id| ("scale", json!({ "id": id, "factor": f }))).collect()),
            };
            self.run_all(label, commands);
        }
        if let Some((id, face)) = shown.face
            && self.run("lay_flat", json!({ "id": id, "face": face })).is_some()
        {
            self.selected = vec![id];
            self.status = "Laid flat on the face clicked.".into();
        }
        if let Some(p) = shown.point {
            self.measure(p);
        }
        if let Some(p) = shown.context {
            self.context_pick(match p {
                Pick::Object(id) => Some(id),
                Pick::Nothing => None,
            });
        }
        if let Some(r) = self.view.response.clone() {
            r.context_menu(|ui| self.object_menu(ui));
        }
        match shown.pick {
            Some(Pick::Object(id)) => self.pick(id, ui),
            Some(Pick::Nothing) if !ui.input(|i| i.modifiers.command || i.modifiers.shift) => self.selected.clear(),
            _ => {}
        }
    }

    fn preview(&mut self, ui: &mut Ui) {
        let current = Arc::as_ptr(&self.session.scene) as usize;
        let Some(sliced) = self.sliced.as_ref() else {
            ui.centered_and_justified(|ui| ui.label("Slice the plate to see its layers."));
            return;
        };
        ui.horizontal(|ui| {
            ui.label(&sliced.summary);
            if sliced.scene != current {
                ui.label(RichText::new("The plate changed since this slice: slice again.").color(WARNING));
            }
        });
        for w in &sliced.warnings {
            ui.label(RichText::new(w).color(WARNING));
        }
        let last = sliced.layers.len().saturating_sub(1);
        let z = sliced.layers.get(self.layer).map_or(0.0, |l| l.z);
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut self.layer, 0..=last).text("layer"));
            ui.label(format!("z = {z:.2} mm"));
            ui.checkbox(&mut self.travels, "Travels");
            ui.separator();
            ui.selectable_value(&mut self.preview_3d, true, "3D").on_hover_text("Every layer up to this one, coloured by what it prints");
            ui.selectable_value(&mut self.preview_3d, false, "Layer").on_hover_text("This layer alone, from above");
        });
        let (bed, centred) = (self.profile.bed, self.profile.origin_center);
        if self.preview_3d {
            let machine = preview::Machine { bed, centred, width: self.profile.line_width() };
            preview::show_3d(ui, &mut self.preview, &sliced.layers, self.layer, machine, self.travels);
        } else {
            preview::show(ui, &sliced.layers, self.layer, bed, centred, self.travels);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    /// A clean exit leaves no autosave behind, unless a plate it left is still waiting for an answer.
    fn on_exit(&mut self) {
        if self.recovery.is_none()
            && let Some(a) = self.autosave.as_mut()
        {
            a.remove();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn window() -> Harness<'static, App> {
        let app = App::new(session(), Arc::new(lamina_assistant::testing::Fake::default()));
        let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        h.run();
        h
    }

    /// Clicks a button and lets the window settle, waiting for any job the click started.
    fn press(h: &mut Harness<App>, label: &str) {
        h.get_by_label(label).click();
        h.step();
        h.state_mut().wait();
        h.run();
    }

    #[test]
    fn shapes_added_from_the_toolbar_land_beside_each_other() {
        let mut h = window();
        press(&mut h, "Add box");
        press(&mut h, "Add sphere");
        let s = &h.state().session.scene;
        assert_eq!(s.objects.len(), 2);
        let (box_hi, ball_lo) = (s.objects[0].bounds().unwrap().1[0], s.objects[1].bounds().unwrap().0[0]);
        assert!(ball_lo >= box_hi + 4.9, "the sphere stands clear of the box: {box_hi} {ball_lo}");
        assert_eq!(h.state().selected, vec![2], "the new shape is picked");
    }

    #[test]
    fn delete_and_undo_work_from_the_panel_and_the_keys() {
        let mut h = window();
        press(&mut h, "Add box");
        press(&mut h, "Delete");
        assert!(h.state().session.scene.objects.is_empty());
        press(&mut h, "Undo");
        assert_eq!(h.state().session.scene.objects.len(), 1);
        h.state_mut().selected = vec![1];
        h.key_press(Key::Delete);
        h.run();
        assert!(h.state().session.scene.objects.is_empty());
    }

    #[test]
    fn deleting_several_objects_is_one_undo_step() {
        let mut h = window();
        press(&mut h, "Add box");
        press(&mut h, "Add cylinder");
        press(&mut h, "Add sphere");
        h.state_mut().selected = vec![1, 3];
        h.run();
        press(&mut h, "Delete all");
        assert_eq!(h.state().session.scene.objects.len(), 1);
        press(&mut h, "Undo");
        assert_eq!(h.state().session.scene.objects.len(), 3, "one undo brings both back");
    }

    #[test]
    fn long_work_runs_on_a_thread_with_a_spinner_and_cancel() {
        let mut h = window();
        press(&mut h, "Add box");
        press(&mut h, "Add cylinder");
        // Slicing: the window draws while it runs, says so, and Cancel forgets it whether or not it had finished.
        h.get_by_label("Slice").click();
        h.step();
        assert!(h.state().busy());
        h.get_by_label("Cancel");
        h.get_by_label_contains("Slicing...");
        h.get_by_label("Cancel").click();
        h.step();
        h.run();
        assert!(!h.state().busy() && h.state().sliced.is_none() && h.state().status == "Cancelled Slicing.", "{}", h.state().status);
        // A boolean changes the plate: while it runs, the window's own commands wait, and a second job is refused.
        let app = h.state_mut();
        app.selected = vec![1, 2];
        app.boolean("union", json!({ "ids": [1, 2] }));
        assert!(app.run("delete", json!({ "id": 1 })).is_none() && app.status.contains("Wait for union"), "{}", app.status);
        app.slice();
        assert!(app.status.contains("Wait for union"));
        app.wait();
        assert_eq!((app.session.scene.objects.len(), app.selected.clone(), app.session.undo_label()), (1, vec![3], Some("union")));
        // The plate changed under a boolean (the assistant did something meanwhile): its result is not applied, and that is said.
        app.run("add_primitive", json!({"kind": "box", "size": [5, 5, 5], "at": [60, 0]}));
        app.boolean("union", json!({ "ids": [3, 4] }));
        app.session.execute("add_primitive", &json!({"kind": "box", "size": [5, 5, 5], "at": [-60, 0]})).unwrap();
        app.wait();
        assert_eq!(app.session.scene.objects.len(), 3, "nothing lost and nothing applied");
        assert!(app.failed && app.status.contains("plate changed"), "{}", app.status);
    }

    #[test]
    fn a_heavy_mesh_is_simplified_from_its_panel() {
        let mut h = window();
        h.state_mut().run("add_primitive", json!({"kind": "sphere", "radius": 20, "segments": 96}));
        h.state_mut().selected = vec![1];
        h.run();
        let before = h.state().session.scene.get(1).unwrap().mesh.triangles.len();
        press(&mut h, "Simplify");
        let after = h.state().session.scene.get(1).unwrap().mesh.triangles.len();
        assert!(after <= before / 4 + 1, "{before} to {after}: {}", h.state().status);
        assert!(h.state().status.starts_with("Simplified"), "{}", h.state().status);
        assert_eq!(h.state().session.undo_label(), Some("simplify"));
    }

    #[test]
    fn two_picked_objects_can_be_subtracted() {
        let mut h = window();
        press(&mut h, "Add box");
        press(&mut h, "Add cylinder");
        // Put the cylinder through the middle of the box, taller than it, then cut it out.
        let app = h.state_mut();
        app.run("move", json!({"id": 2, "by": [-25, 0, -2]}));
        app.run("scale", json!({"id": 2, "by": [0.4, 0.4, 1.3]}));
        app.selected = vec![1, 2];
        h.run();
        press(&mut h, "Subtract");
        let app = h.state();
        assert_eq!(app.session.scene.objects.len(), 1, "{}", app.status);
        assert_eq!(app.selected, vec![3]);
        assert!(!app.failed);
    }

    #[test]
    fn slicing_opens_the_preview_with_its_layers() {
        let mut h = window();
        press(&mut h, "Add cylinder");
        press(&mut h, "Slice");
        let app = h.state();
        assert_eq!(app.tab, Tab::Preview);
        let sliced = app.sliced.as_ref().unwrap();
        assert_eq!(sliced.layers.len(), 100);
        assert!(sliced.layers[50].moves.iter().any(|m| m.extrude));
        assert!(sliced.summary.contains("100 layers"), "{}", sliced.summary);
        h.get_by_label("Travels");
        // In 3D, every layer up to the chosen one, with a legend of what they print; the layer alone from above is a click away.
        assert!(app.preview_3d);
        h.get_by_label("outer wall");
        h.get_by_label("sparse infill");
        let kinds = h.state().preview.kinds.len();
        h.state_mut().layer = 0;
        h.run();
        assert!(h.state().preview.kinds.len() < kinds, "the first layer has no sparse infill");
        press(&mut h, "Layer");
        assert!(!h.state().preview_3d);
    }

    #[test]
    fn a_slice_that_cannot_be_made_says_why() {
        let mut h = window();
        press(&mut h, "Slice");
        assert!(h.state().failed && h.state().status.contains("empty"), "{}", h.state().status);
    }

    #[test]
    fn the_assistant_asks_for_a_key_before_it_sends_anything() {
        let mut h = window();
        h.state_mut().chat.input = "a 20 mm cube".into();
        // The environment may hold a real key; this test must never use it.
        h.state_mut().chat.chat.provider = lamina_assistant::Provider::Gemini;
        if std::env::var(lamina_assistant::Provider::Gemini.env()).is_ok() {
            return;
        }
        h.run();
        press(&mut h, "Send");
        let log = &h.state().chat.chat.log;
        assert!(matches!(log.last(), Some(lamina_assistant::Line::Note(n)) if n.contains("key")), "{log:?}");
    }

    /// A primary drag from `a` to `b` in steps, as a hand makes it.
    fn drag(h: &mut Harness<App>, a: egui::Pos2, b: egui::Pos2) {
        use egui::{Event, Modifiers, PointerButton};
        h.hover_at(a);
        h.run();
        h.event(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.run();
        for t in [0.25, 0.5, 0.75, 1.0] {
            h.hover_at(a + (b - a) * t);
            h.run();
        }
        h.event(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run();
    }

    fn middle(app: &App, id: u32) -> [f64; 3] {
        let (lo, hi) = app.session.scene.get(id).unwrap().bounds().unwrap();
        [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]]
    }

    #[test]
    fn dragging_the_gizmo_moves_the_object_in_one_undo_step() {
        use crate::view3d::Handle;
        let mut h = window();
        press(&mut h, "Add box");
        let g = h.state().view.gizmo.expect("a picked object has a gizmo");
        let tip = g.grip(Handle::X);
        drag(&mut h, tip, tip + (g.tips[0] - g.origin));
        let after = middle(h.state(), 1);
        assert!(after[0] > 5.0 && after[1].abs() < 1e-9 && after[2].abs() < 1e-9, "moved along x only: {after:?}");
        assert_eq!(after[0], (after[0] / 0.5).round() * 0.5, "snapped to half a millimetre");
        press(&mut h, "Undo");
        assert_eq!(middle(h.state(), 1), [0.0, 0.0, 0.0], "one undo takes the whole drag back");
        // The plate square slides it on the plate, never up or down.
        let g = h.state().view.gizmo.unwrap();
        let grip = g.grip(Handle::Plate);
        drag(&mut h, grip, grip + egui::vec2(-40.0, 30.0));
        let slid = middle(h.state(), 1);
        assert!(slid[0].abs() + slid[1].abs() > 2.0 && slid[2].abs() < 1e-9, "{slid:?}");
        // A drag that starts away from the gizmo still turns the camera and moves nothing.
        let before = h.state().view.camera.azimuth;
        drag(&mut h, egui::pos2(700.0, 750.0), egui::pos2(780.0, 750.0));
        assert_ne!(h.state().view.camera.azimuth, before);
        assert_eq!(middle(h.state(), 1), slid);
    }

    #[test]
    fn dragging_the_gizmo_with_several_objects_picked_moves_them_all_in_one_undo_step() {
        use crate::view3d::Handle;
        let mut h = window();
        press(&mut h, "Add box");
        press(&mut h, "Add sphere");
        press(&mut h, "Add cylinder");
        h.state_mut().selected = vec![1, 3];
        h.run();
        let (a, b, c) = (middle(h.state(), 1), middle(h.state(), 2), middle(h.state(), 3));
        let g = h.state().view.gizmo.expect("several picked objects share a gizmo");
        let tip = g.grip(Handle::X);
        drag(&mut h, tip, tip + (g.tips[0] - g.origin));
        let by = middle(h.state(), 1)[0] - a[0];
        assert!(by > 5.0, "moved along x: {by}");
        assert_eq!(middle(h.state(), 3)[0] - c[0], by, "both picked objects moved alike");
        assert_eq!(middle(h.state(), 2), b, "the one not picked stayed");
        press(&mut h, "Undo");
        assert_eq!((middle(h.state(), 1), middle(h.state(), 3)), (a, c), "one undo takes the whole drag back");
    }

    #[test]
    fn a_ring_drag_turns_the_object_and_the_corner_square_scales_it_each_in_one_undo_step() {
        use crate::view3d::Handle;
        let mut h = window();
        press(&mut h, "Add box");
        h.state_mut().run("scale", json!({"id": 1, "by": [2, 1, 1]})); // 40 x 20 x 20, so a quarter turn about z shows
        h.run();
        let g = h.state().view.gizmo.unwrap();
        // Sweep the pointer a quarter turn round the middle, starting on the z ring.
        let start = g.grip(Handle::Ring(2));
        let r = start - g.origin;
        let end = g.origin + egui::vec2(-r.y, r.x) * if g.facing[2] { -1.0 } else { 1.0 };
        drag(&mut h, start, end);
        let (lo, hi) = h.state().session.scene.get(1).unwrap().bounds().unwrap();
        assert!((hi[0] - lo[0] - 20.0).abs() < 0.5 && (hi[1] - lo[1] - 40.0).abs() < 0.5, "turned a quarter: {lo:?} {hi:?}");
        press(&mut h, "Undo");
        let (lo, hi) = h.state().session.scene.get(1).unwrap().bounds().unwrap();
        assert!((hi[0] - lo[0] - 40.0).abs() < 1e-6, "one undo takes the turn back");
        let g = h.state().view.gizmo.unwrap();
        let s = g.grip(Handle::Scale);
        drag(&mut h, s, g.origin + (s - g.origin) * 1.5);
        let (lo, hi) = h.state().session.scene.get(1).unwrap().bounds().unwrap();
        assert!((hi[2] - lo[2] - 30.0).abs() < 1.5 && lo[2].abs() < 1e-9, "scaled by half again, still on the plate: {lo:?} {hi:?}");
    }

    #[test]
    fn a_printer_and_a_filament_preset_set_their_own_fields() {
        let mut h = window();
        h.get_by_label("Printer");
        let app = h.state_mut();
        app.profile.infill_density = 0.4;
        app.apply_preset(Kind::Printer, "Generic 350 mm, Klipper");
        app.apply_preset(Kind::Filament, "ABS");
        assert_eq!((app.profile.bed, app.profile.bed_temp, app.profile.infill_density), ([350.0, 350.0], 100.0, 0.4));
        assert_eq!((app.printer.as_deref(), app.filament.as_deref()), (Some("Generic 350 mm, Klipper"), Some("ABS")));
        app.apply_preset(Kind::Filament, "wood");
        assert!(app.failed && app.status.contains("there are"));
    }

    #[test]
    fn a_process_preset_sets_the_layers_and_keeps_the_rest() {
        let mut h = window();
        h.get_by_label("Process");
        let app = h.state_mut();
        app.apply_preset(Kind::Filament, "PETG");
        app.apply_preset(Kind::Process, "0.12 mm Fine");
        assert_eq!((app.profile.layer_height, app.profile.nozzle_temp), (0.12, 240.0));
        assert_eq!(app.print_json()["process"], "0.12 mm Fine", "the project keeps the label");
        app.profile.perimeters = 5;
        app.save_preset(Kind::Process, "Five walls");
        app.apply_preset(Kind::Process, "Strong");
        app.apply_preset(Kind::Process, "Five walls");
        assert_eq!(app.profile.perimeters, 5);
    }

    #[test]
    fn every_setting_has_a_place_and_bad_gcode_is_said() {
        let mut h = window();
        // Each group opened on its own (the panel scrolls), one of its fields found, and closed again.
        for (group, one) in [
            ("Speeds", "Walls, mm/s"),
            ("Retraction and travel", "Z hop, mm"),
            ("Skirt", "Loops"),
            ("Cooling", "Shortest layer, s"),
            ("Support details", "Gap above, mm"),
            ("Machine", "Junction deviation, mm"),
        ] {
            press(&mut h, group);
            h.get_by_label(one);
            press(&mut h, group);
        }
        press(&mut h, "Start and end G-code");
        h.state_mut().profile.start_gcode = "M104 S[nope]".into();
        h.run();
        h.get_by_label_contains("unknown placeholder");
    }

    #[test]
    fn shortcuts_drive_the_tools_and_the_picked_objects() {
        use crate::view3d::Tool;
        let mut h = window();
        press(&mut h, "Add box");
        press(&mut h, "Add cylinder");
        for (key, tool) in [(Key::L, Tool::LayFlat), (Key::M, Tool::Measure), (Key::G, Tool::Move)] {
            h.key_press(key);
            h.run();
            assert_eq!(h.state().view.tool, tool);
        }
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.run();
        assert_eq!(h.state().selected, vec![1, 2], "Ctrl+A picks everything");
        h.state_mut().selected = vec![1];
        h.run();
        h.key_press(Key::R);
        h.run();
        let app = h.state();
        assert_eq!(app.session.undo_label(), Some("rotate"));
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::D);
        h.run();
        assert_eq!(h.state().session.scene.objects.len(), 3);
        assert_eq!(h.state().selected, vec![3], "the copy is picked");
        let before = h.state().view.camera;
        h.key_press(Key::F);
        h.run();
        assert_ne!(h.state().view.camera, before, "F frames the picked copy");
        // Typing in a field is not a shortcut.
        h.state_mut().view.tool = Tool::Move;
        h.get_all_by_role(egui::accesskit::Role::TextInput).next().unwrap().click();
        h.run();
        h.key_press(Key::L);
        h.run();
        assert_eq!(h.state().view.tool, Tool::Move);
    }

    #[test]
    fn the_history_steps_back_and_forth_and_the_right_click_menu_acts_on_the_object() {
        let mut h = window();
        press(&mut h, "Add box");
        let app = h.state_mut();
        app.run("move", json!({"id": 1, "by": [5, 0, 0]}));
        app.run("scale", json!({"id": 1, "factor": 2}));
        app.step_history(2, false);
        assert_eq!(app.session.history(), (vec!["add_primitive"], vec!["move", "scale"]));
        app.step_history(1, true);
        assert_eq!(app.session.undo_label(), Some("move"));
        h.run();
        press(&mut h, "Edit");
        h.get_by_label_contains("Undo move");
        h.get_by_label_contains("History").click();
        h.run();
        h.get_by_label("\u{21b6} move");
        press(&mut h, "\u{21b7} scale");
        assert_eq!(h.state().session.undo_label(), Some("scale"));
        // A right click on the object in the list picks it and opens what can be done to it.
        h.state_mut().selected.clear();
        h.run();
        h.get_by_label_contains("40.0 x 40.0 x 40.0").click_secondary();
        h.run();
        assert_eq!(h.state().selected, vec![1]);
        press(&mut h, "Duplicate");
        assert_eq!(h.state().session.scene.objects.len(), 2, "{}", h.state().status);
    }

    #[test]
    fn an_error_on_the_status_bar_goes_to_the_log_once() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::with_settings(session(), Arc::new(lamina_assistant::testing::Fake::default()), dir.path().join("settings.json"));
        let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        h.state_mut().run("move", json!({"id": 99, "by": [1, 0, 0]}));
        h.run();
        h.run();
        let log = std::fs::read_to_string(dir.path().join("lamina.log")).unwrap();
        assert_eq!(log.matches("error: ").count(), 1, "{log}");
        assert!(log.contains("99"), "{log}");
    }

    #[test]
    fn a_local_model_is_chosen_with_its_server_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let app = App::with_settings(session(), Arc::new(lamina_assistant::testing::Fake::default()), path.clone());
        let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        h.run();
        let app = h.state_mut();
        app.chat.chat.provider = lamina_assistant::Provider::Local;
        app.chat.chat.model = app.chat.local_model.clone();
        h.run();
        h.get_by_label_contains("nothing leaves it");
        h.get_by_label("Server");
        let app = h.state_mut();
        assert_eq!(app.chat.chat.model, "qwen3");
        app.chat.chat.model = "llama3.2:3b".into();
        app.chat.chat.local_url = "http://192.168.1.20:11434".into();
        h.run();
        h.get_by_label_contains("must be on this computer");
        h.state_mut().chat.chat.local_url = "localhost:1234".into();
        h.run();
        let (s, notes) = Settings::read(&path);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!((s.local_url.as_deref(), s.local_model.as_deref()), (Some("localhost:1234"), Some("llama3.2:3b")));
        // The next window has them back.
        let next = App::with_settings(session(), Arc::new(lamina_assistant::testing::Fake::default()), path);
        assert_eq!((next.chat.chat.local_url.as_str(), next.chat.local_model.as_str()), ("localhost:1234", "llama3.2:3b"));
    }

    #[test]
    fn an_agent_works_on_the_window_s_plate_through_its_port() {
        use std::io::{BufRead, BufReader, Write};
        let dir = tempfile::tempdir().unwrap();
        let mut h = window();
        h.state_mut().bridge_file = Some(dir.path().join(lamina_core::bridge::FILE));
        h.state_mut().open_control();
        let entry = lamina_core::bridge::read_entry(&dir.path().join(lamina_core::bridge::FILE)).expect("the bridge file");
        assert_eq!(Some(entry.port), h.state().control.as_ref().map(|c| c.entry.port));
        // The agent, on a thread of its own, as lamina-cli would be; the window answers in its frames.
        let agent = std::thread::spawn(move || {
            let stream = std::net::TcpStream::connect(("127.0.0.1", entry.port)).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut ask = |request: Value| {
                let mut request = request;
                request["token"] = json!(entry.token);
                writeln!(writer, "{request}").unwrap();
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                serde_json::from_str::<Value>(&line).unwrap()
            };
            vec![
                ask(json!({"op": "hello"})),
                ask(json!({"op": "execute", "command": "add_primitive", "params": {"kind": "box", "size": [10, 10, 10]}})),
                ask(json!({"op": "execute", "command": "new"})),
                ask(json!({"op": "script", "text": "info\nsend_to_printer {}"})),
                ask(json!({"op": "execute", "command": "delete", "params": {"id": 1}, "read_only": true})),
                ask(json!({"op": "execute", "command": "info"})),
            ]
        });
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !agent.is_finished() {
            assert!(std::time::Instant::now() < until, "the agent was not answered");
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let said = agent.join().unwrap();
        assert_eq!(said[0]["ok"]["app"], "lamina");
        assert_eq!(said[1]["ok"]["id"], 1, "{}", said[1]);
        assert!(said[2]["error"].as_str().unwrap().contains("not for an agent"), "{}", said[2]);
        assert!(said[3]["error"].as_str().unwrap().contains("send_to_printer"), "{}", said[3]);
        assert!(said[4]["error"].as_str().unwrap().contains("only read"), "{}", said[4]);
        assert_eq!(said[5]["ok"]["objects"].as_array().map(Vec::len), Some(1), "{}", said[5]);
        // What the agent made is on the window's plate, one undo step the user can take back.
        assert_eq!(h.state().session.scene.objects.len(), 1);
        assert_eq!(h.state().session.undo_label(), Some("add_primitive"));
        // Closing the port removes the bridge file.
        h.state_mut().close_control();
        assert!(!dir.path().join(lamina_core::bridge::FILE).exists());
    }

    #[test]
    fn the_light_theme_is_chosen_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let app = App::with_settings(session(), Arc::new(lamina_assistant::testing::Fake::default()), path.clone());
        let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        h.run();
        assert_eq!(h.ctx.theme(), egui::Theme::Dark);
        press(&mut h, "View");
        press(&mut h, "Light");
        assert_eq!(h.ctx.theme(), egui::Theme::Light);
        let (s, _) = Settings::read(&path);
        assert_eq!(s.look, Some(comfort::Look::Light));
        // Every setting says what it does.
        assert!(hint("layer_height").contains("thinner is finer") && hint("layer_height").contains("From 0.02"));
        assert_eq!(hint("seam"), lamina_slicer::profile::about("seam"));
    }

    #[test]
    fn a_preset_saved_over_another_keeps_only_what_differs_and_follows_it() {
        let mut h = window();
        let app = h.state_mut();
        app.apply_preset(Kind::Filament, "PETG");
        app.profile.nozzle_temp = 235.0;
        app.save_preset(Kind::Filament, "My PETG");
        let saved = app.my_filaments["My PETG"].clone();
        assert_eq!(saved.get("inherits").and_then(Value::as_str), Some("PETG"));
        // What PETG sets and the settings match is left to it; the nozzle differs, and pressure advance, which PETG leaves alone, is kept.
        assert_eq!(saved.get("nozzle_temp").and_then(Value::as_f64), Some(235.0));
        assert!(!saved.contains_key("bed_temp") && !saved.contains_key("fan_speed"), "{saved:?}");
        // Applied on top of PLA's, it gives PETG's other fields with its own nozzle.
        app.apply_preset(Kind::Filament, "PLA");
        app.apply_preset(Kind::Filament, "My PETG");
        assert_eq!((app.profile.nozzle_temp, app.profile.bed_temp, app.profile.fan_speed), (235.0, 80.0, 128.0));
        // A preset saved with nothing chosen holds every field of its kind.
        app.filament = None;
        app.save_preset(Kind::Filament, "Whole");
        assert!(!app.my_filaments["Whole"].contains_key("inherits"));
    }

    #[test]
    fn another_slicer_s_profile_comes_into_the_settings_shown() {
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("prusa.ini");
        std::fs::write(
            &ini,
            "layer_height = 0.16
perimeters = 4
fill_angle = 45
",
        )
        .unwrap();
        let mut h = window();
        let app = h.state_mut();
        app.profile.nozzle_temp = 230.0;
        app.import_profile(&ini);
        assert_eq!((app.profile.layer_height, app.profile.perimeters, app.profile.nozzle_temp), (0.16, 4, 230.0));
        assert!(app.status.contains("Took 2 settings") && app.status.contains("fill_angle"), "{}", app.status);
    }

    #[test]
    fn a_preset_of_your_own_is_saved_and_applied_again() {
        let mut h = window();
        let app = h.state_mut();
        app.profile.nozzle_temp = 236.0;
        app.save_preset(Kind::Filament, "My PETG");
        app.save_preset(Kind::Filament, "PLA");
        assert!(app.failed, "a built-in's name is refused");
        app.apply_preset(Kind::Filament, "PLA");
        assert_eq!(app.profile.nozzle_temp, 210.0);
        app.apply_preset(Kind::Filament, "My PETG");
        assert_eq!(app.profile.nozzle_temp, 236.0);
    }

    #[test]
    fn settings_and_a_remembered_key_outlive_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let net = || -> Arc<dyn Net> { Arc::new(lamina_assistant::testing::Fake::default()) };
        let mut first = App::with_settings(session(), net(), path.clone());
        first.apply_preset(Kind::Printer, "Generic 250 x 210 mm, Marlin");
        first.profile.layer_height = 0.12;
        first.my_filaments.insert("Mine".into(), presets::capture(&first.profile, FILAMENT_FIELDS));
        first.chat.chat.provider = lamina_assistant::Provider::Gemini;
        let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), first);
        h.run();
        let app = h.state_mut();
        app.chat.saved.insert("gemini".into(), lamina_assistant::seal::seal("AIza-test-0123456789abcdef").unwrap());
        h.run();
        assert!(path.exists());
        let again = App::with_settings(session(), net(), path);
        assert!(!again.failed, "{}", again.status);
        assert_eq!((again.profile.bed, again.profile.layer_height), ([250.0, 210.0], 0.12));
        assert_eq!(again.printer.as_deref(), Some("Generic 250 x 210 mm, Marlin"));
        assert!(again.my_filaments.contains_key("Mine"));
        assert_eq!(again.chat.saved.len(), 1);
    }

    #[test]
    fn a_project_keeps_the_plate_and_the_print_settings() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = window();
        press(&mut h, "Add box");
        press(&mut h, "Add cylinder");
        let app = h.state_mut();
        app.apply_preset(Kind::Printer, "Generic 300 mm, Klipper");
        app.profile.infill_density = 0.33;
        assert!(app.dirty() && app.title().starts_with("Untitled *"), "{}", app.title());
        assert!(app.save_project(&dir.path().join("bracket")), "{}", app.status);
        let path = dir.path().join("bracket.lamina");
        assert!(path.exists() && !app.dirty(), "saved with the extension added");
        assert_eq!(app.title(), "bracket - Lamina");
        let before: Vec<_> = app.session.scene.objects.iter().map(|o| (o.id, o.name.clone(), o.xf, o.mesh.clone())).collect();

        let mut other = App::new(session(), Arc::new(lamina_assistant::testing::Fake::default()));
        other.open_project(&path);
        assert!(!other.failed, "{}", other.status);
        let after: Vec<_> = other.session.scene.objects.iter().map(|o| (o.id, o.name.clone(), o.xf, o.mesh.clone())).collect();
        assert_eq!(before, after);
        assert_eq!((other.profile.infill_density, other.profile.bed), (0.33, [300.0, 300.0]));
        assert_eq!(other.printer.as_deref(), Some("Generic 300 mm, Klipper"));
        assert_eq!(other.project.as_deref(), Some(path.as_path()));
        assert_eq!(other.recent, [path.to_string_lossy().into_owned()]);
        assert!(!other.dirty());
        other.run("delete", json!({"id": 1}));
        assert!(other.dirty());
        other.run("undo", json!({}));
        assert!(!other.dirty(), "undoing back to what was saved is not a change");
    }

    #[test]
    fn unsaved_changes_are_asked_about_before_they_are_lost() {
        let mut h = window();
        assert!(!h.state().dirty(), "an empty window has nothing to lose");
        h.state_mut().request(files::Pending::New);
        assert_eq!(h.state().ask, None);
        press(&mut h, "Add box");
        h.state_mut().request(files::Pending::New);
        h.run();
        h.get_by_label("Save the changes?");
        press(&mut h, "Cancel");
        assert_eq!((h.state().ask.clone(), h.state().session.scene.objects.len()), (None, 1), "cancel keeps everything");
        h.state_mut().request(files::Pending::New);
        h.run();
        press(&mut h, "Don't save");
        assert!(h.state().session.scene.objects.is_empty() && !h.state().dirty());
        press(&mut h, "Add sphere");
        h.state_mut().request(files::Pending::Close);
        h.run();
        press(&mut h, "Don't save");
        assert!(h.state().may_close, "the window may close once the user said so");
    }

    /// A file as the system drops it on the window.
    #[derive(Debug)]
    struct Dropped(std::path::PathBuf);

    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &std::path::Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    fn drop_file(h: &mut Harness<App>, path: std::path::PathBuf) {
        h.input_mut().dropped_files.push(Arc::new(Dropped(path)));
        h.run();
    }

    #[test]
    fn files_dropped_on_the_window_are_imported_or_opened() {
        let dir = tempfile::tempdir().unwrap();
        let stl = dir.path().join("part.stl");
        std::fs::write(&stl, lamina_core::stl::write(&Mesh::cuboid([10.0, 10.0, 10.0]))).unwrap();
        let mut h = window();
        drop_file(&mut h, stl.clone());
        assert_eq!(h.state().session.scene.objects.len(), 1, "{}", h.state().status);
        assert_eq!(h.state().session.scene.objects[0].name, "part");
        let project = dir.path().join("two.lamina");
        let mut s = session();
        s.execute("import_mesh", &json!({"path": stl})).unwrap();
        s.execute("import_mesh", &json!({"path": stl, "at": [30, 0]})).unwrap();
        s.execute("save", &json!({"path": project})).unwrap();
        drop_file(&mut h, project.clone());
        h.run();
        press(&mut h, "Don't save");
        assert_eq!(h.state().session.scene.objects.len(), 2, "{}", h.state().status);
        assert_eq!(h.state().project.as_deref(), Some(project.as_path()));
        // A 3MF of two objects comes in as two objects, both picked.
        let three = dir.path().join("pair.3mf");
        h.state_mut().run("export_mesh", json!({"path": three}));
        drop_file(&mut h, three);
        assert_eq!(h.state().session.scene.objects.len(), 4, "{}", h.state().status);
        assert_eq!(h.state().selected.len(), 2);
        drop_file(&mut h, dir.path().join("notes.txt"));
        assert!(h.state().failed && h.state().status.contains(".lamina"), "{}", h.state().status);
        // A picture goes to the assistant, listed above the message box with a button to take it off.
        let photo = dir.path().join("my part.png");
        std::fs::write(&photo, lamina_core::render::png_rgba([2, 2], &[[200, 100, 50, 255]; 4])).unwrap();
        drop_file(&mut h, photo);
        assert_eq!(h.state().chat.chat.attached.len(), 1, "{}", h.state().status);
        h.get_by_label_contains("my part.png (picture");
        press(&mut h, "x");
        assert!(h.state().chat.chat.attached.is_empty());
    }

    #[test]
    fn supports_are_chosen_in_the_print_settings() {
        let mut h = window();
        h.get_by_label("Supports");
        h.state_mut().profile.support = lamina_slicer::profile::Support::FromPlate;
        h.run();
        assert_eq!(h.state().print_json()["profile"]["support"], "from_plate");
    }

    #[test]
    fn the_measure_tool_measures_between_two_clicks_and_lay_flat_turns_a_face_down() {
        let mut h = window();
        press(&mut h, "Add box");
        press(&mut h, "Measure");
        let app = h.state_mut();
        assert_eq!(app.view.tool, view3d::Tool::Measure);
        app.measure([0.0, 0.0, 0.0]);
        app.measure([3.0, 4.0, 0.0]);
        assert!(app.status.starts_with("5.00 mm"), "{}", app.status);
        app.measure([1.0, 1.0, 1.0]);
        assert_eq!(app.view.marks.len(), 1, "a third click starts again");
        // Lay flat: a click in the middle of the view, looking at the box, turns the face there down.
        press(&mut h, "Lay flat");
        let app = h.state_mut();
        app.view.camera = view3d::Camera { target: [0.0, 0.0, 10.0], azimuth: -90.0, elevation: 0.0, distance: 200.0 };
        h.run();
        h.run();
        let middle = h.state().view.rect.unwrap().center();
        h.hover_at(middle);
        h.run();
        h.event(egui::Event::PointerButton {
            pos: middle,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
        h.event(egui::Event::PointerButton {
            pos: middle,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
        assert!(h.state().status.contains("Laid flat"), "{}", h.state().status);
        assert!(h.state().session.can_undo());
    }

    #[test]
    fn typing_a_position_or_a_size_moves_or_scales_the_object_to_it() {
        let mut h = window();
        press(&mut h, "Add box");
        h.get_by_label("Keep proportions");
        let app = h.state_mut();
        app.set_position(1, [30.0, -12.5, 0.0], [0.0, 0.0, 0.0]);
        assert_eq!(middle(app, 1), [30.0, -12.5, 0.0]);
        app.set_size(1, 0, 40.0, 20.0);
        let (lo, hi) = app.session.scene.get(1).unwrap().bounds().unwrap();
        assert!((hi[0] - lo[0] - 40.0).abs() < 1e-6 && (hi[2] - lo[2] - 40.0).abs() < 1e-6, "kept its proportions");
        app.keep_proportions = false;
        app.set_size(1, 2, 10.0, 40.0);
        let (lo, hi) = app.session.scene.get(1).unwrap().bounds().unwrap();
        assert!((hi[0] - lo[0] - 40.0).abs() < 1e-6 && (hi[2] - lo[2] - 10.0).abs() < 1e-6 && lo[2].abs() < 1e-9);
    }

    #[test]
    fn a_broken_mesh_is_flagged_and_repaired_from_the_panel() {
        let mut h = window();
        let mut open = Mesh::cuboid([10.0, 10.0, 10.0]);
        open.triangles.pop();
        let app = h.state_mut();
        app.session.scene_mut().add("open box", open).unwrap();
        app.selected = vec![1];
        h.run();
        assert!(h.state_mut().problems_of(1).is_some_and(|p| p.open_edges == 3));
        h.get_by_label("Repair");
        press(&mut h, "Repair");
        assert!(h.state().status.contains("closed now"), "{}", h.state().status);
        assert!(h.state_mut().problems_of(1).is_none());
        assert!(h.state().session.scene.get(1).unwrap().mesh.is_watertight());
    }

    #[test]
    fn the_plate_is_sliced_and_sent_to_the_printer() {
        let (address, got) = lamina_printer::testing::printer(201, "{}");
        let mut h = window();
        press(&mut h, "Add box");
        let app = h.state_mut();
        app.send_to_printer(true);
        assert!(app.failed && app.status.contains("address"), "no address yet: {}", app.status);
        app.link.connection = printer::Connection { kind: lamina_printer::Kind::Octoprint, address: address.clone() };
        app.send_to_printer(true);
        app.wait();
        assert!(app.failed && app.status.contains("API key"), "OctoPrint needs its key: {}", app.status);
        app.link.key = "ABCDEF0123456789".into();
        app.send_to_printer(true);
        app.wait();
        assert!(!app.failed && app.status.contains("is printing"), "{}", app.status);
        let got = got.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].header("X-Api-Key"), Some("ABCDEF0123456789"));
        let body = got[0].body_text();
        assert!(body.contains("filename=\"plate.gcode\"") && body.contains(";LAYER:99"), "the whole slice went");
    }

    #[test]
    fn an_unclean_exit_leaves_a_plate_that_is_offered_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let net = || -> Arc<dyn Net> { Arc::new(lamina_assistant::testing::Fake::default()) };
        let mut first = App::with_settings(session(), net(), path.clone());
        assert!(first.recovery.is_none());
        first.run("add_primitive", json!({"kind": "box", "size": [12, 12, 12]}));
        first.profile.infill_density = 0.42;
        first.autosave_now();
        let file = dir.path().join(autosave::FILE);
        assert!(file.exists());
        drop(first); // the window is gone without a clean exit
        let mut again = App::with_settings(session(), net(), path.clone());
        assert_eq!(again.recovery.as_deref(), Some(file.as_path()));
        let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), again);
        h.run();
        h.get_by_label("Bring back the last plate?");
        press(&mut h, "Bring it back");
        let app = h.state_mut();
        assert_eq!(app.session.scene.objects.len(), 1);
        assert_eq!(app.profile.infill_density, 0.42);
        assert!(app.project.is_none() && app.dirty(), "an unsaved project, which the window asks about before losing");
        // A clean exit removes it; the next start offers nothing.
        eframe::App::on_exit(app);
        assert!(!file.exists());
        again = App::with_settings(session(), net(), path);
        assert!(again.recovery.is_none());
        again.discard_recovery();
    }

    #[test]
    fn the_printer_key_is_kept_sealed_only_when_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let net = || -> Arc<dyn Net> { Arc::new(lamina_assistant::testing::Fake::default()) };
        let mut first = App::with_settings(session(), net(), path.clone());
        first.link.connection.address = "printer.local:7125".into();
        first.link.key = "secret-key-123".into();
        let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), first);
        h.get_by_label("Printer connection").click();
        h.run();
        h.run();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("secret-key"), "not remembered, not written");
        h.state_mut().link.remember = true;
        h.run();
        h.run();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("printer.local:7125") && !text.contains("secret-key-123"), "sealed, never as typed");
        let again = App::with_settings(session(), net(), path);
        assert_eq!((again.link.key.as_str(), again.link.remember), ("secret-key-123", true));
    }

    /// The whole window as a picture, for looking at: `cargo test -p lamina -- --ignored picture_of_the_window`.
    #[test]
    #[ignore = "needs a graphics adapter; run by hand to look at the window"]
    fn picture_of_the_window() {
        let mut h = window();
        press(&mut h, "Add box");
        press(&mut h, "Add cylinder");
        press(&mut h, "Add sphere");
        h.state_mut().view.camera.frame_all(([-15.0, -15.0, 0.0], [80.0, 15.0, 20.0]));
        h.run();
        let out = std::env::var("LAMINA_PICTURE").unwrap_or_else(|_| "target/lamina-window.png".into());
        h.render().unwrap().save(&out).unwrap();
        press(&mut h, "Slice");
        h.render().unwrap().save(out.replace(".png", "-preview.png")).unwrap();
    }
}
