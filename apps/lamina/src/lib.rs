//! Lamina's desktop window: the plate in 3D, the objects and their tools, the print settings, the slice and its preview, and the
//! assistant. A thin shell over the core's `Session`: it reads `session.scene` and changes it only through `Session::execute`, so
//! whatever a button does, a script, the MCP server or the assistant does the same way.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod autosave;
pub mod chat;
pub mod comfort;
pub mod control;
pub mod files;
pub mod gpu;
pub mod heights;
pub mod icon;
pub mod lang;
pub mod preview;
pub mod printer;
pub mod settings;
pub mod startup;
pub mod updates;
pub mod view3d;
pub mod work;

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, Key, Panel, RichText, ScrollArea, Ui};
use lamina_assistant::net::Net;
use lamina_core::{Scene, Session};
use lamina_i18n::ui_msg;
use lamina_slicer::presets::{self, FILAMENT_FIELDS, FILAMENTS, PRINTER_FIELDS, PRINTERS, PROCESS_FIELDS, PROCESSES};
use lamina_slicer::profile::Profile;
use serde_json::{Value, json};

use crate::lang::{t, tf};
use crate::settings::{Presets, Settings};
use crate::view3d::Pick;

/// A number setting with its label (English, translated here), its range from the profile's table, and on hover what it does and
/// its range.
fn field<N: egui::emath::Numeric>(ui: &mut Ui, label: &str, name: &str, value: &mut N, speed: f64) {
    let (lo, hi) = lamina_slicer::profile::limits(name).unwrap_or((0.0, 1e6));
    ui.label(t(label)).on_hover_text(hint(name));
    ui.add(crate::lang::number(value).speed(speed).range(N::from_f64(lo)..=N::from_f64(hi))).on_hover_text(hint(name));
    ui.end_row();
}

/// What a setting does, and its range when it is a number: every field's tooltip.
fn hint(name: &str) -> String {
    let about = t(lamina_slicer::profile::about(name));
    match lamina_slicer::profile::limits(name) {
        Some((lo, hi)) => format!("{about} {}", tf("From {} to {}.", &[&lang::mark(lo.to_string()), &lang::mark(hi.to_string())])),
        None => about.to_string(),
    }
}

/// What is wrong with a mesh, in the window's language (`Problems::describe` says it in English, for agents).
fn problems_text(p: &lamina_core::repair::Problems) -> String {
    let counts = [
        (p.open_edges, ui_msg!("{} open edges")),
        (p.nonmanifold_edges, ui_msg!("{} edges shared by more than two faces")),
        (p.flipped_faces, ui_msg!("{} flipped faces")),
    ];
    counts.iter().filter(|(n, _)| *n > 0).map(|(n, what)| tf(what, &[&n.to_string()])).collect::<Vec<_>>().join(", ")
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
    /// Its name in a sentence, in English (also the id of its picker).
    fn word(self) -> &'static str {
        match self {
            Kind::Printer => ui_msg!("printer"),
            Kind::Filament => ui_msg!("filament"),
            Kind::Process => ui_msg!("process"),
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

/// A height range being written in the window: where it is and what it changes (each optional).
#[derive(Clone, Debug, PartialEq)]
pub struct RangeForm {
    pub from: f64,
    pub to: f64,
    pub nozzle_temp: Option<f64>,
    pub infill: Option<f64>,
    pub walls: Option<u32>,
}

impl Default for RangeForm {
    fn default() -> RangeForm {
        RangeForm { from: 5.0, to: 10.0, nozzle_temp: Some(220.0), infill: None, walls: None }
    }
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
    /// The layer heights strip beside the 3D view.
    pub strip: heights::Strip,
    pub tab: Tab,
    pub profile: Profile,
    pub sliced: Option<Sliced>,
    pub layer: usize,
    pub travels: bool,
    pub status: String,
    pub failed: bool,
    nudge: [f64; 3],
    /// The Cut tools: the axis (0 x, 1 y, 2 z), where along it, and the connectors the user chose: the kind ("pin", "dowel" or
    /// "none"), the shape ("round" or "square"), how many and how big (none: chosen from the cut face), and the fit, mm a side.
    cut_axis: usize,
    cut_at: f64,
    connector: &'static str,
    connector_shape: &'static str,
    connector_count: Option<u32>,
    connector_size: Option<f64>,
    connector_fit: f64,
    /// The picked object's position and size as the fields show them while they are edited: the object and the scene they were
    /// read from, so they are read again when either changes.
    fields: Option<Fields>,
    /// Whether a size typed keeps the proportions.
    pub keep_proportions: bool,
    /// The form for a new height range in This object's settings: from, to, and the settings it may give.
    pub range: RangeForm,
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
    /// The interface's language, as the settings file keeps it (`main` picks the system's at the first start).
    pub lang: lang::Lang,
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
    /// Where each project's chat is kept (`chats` beside the settings; none in a test that does not ask), and the chat's revision when
    /// it was last written there.
    pub chats: Option<PathBuf>,
    chat_saved: usize,
    /// The updater, which `main` attaches (none in a test, or with `LAMINA_NO_UPDATE_CHECK`), and whether it looks at the start,
    /// as the settings file keeps it.
    pub updates: Option<lamina_update::Updater>,
    pub update_check: bool,
}

impl App {
    /// A window on `session`; the assistant reaches its provider through `net` (a test passes one that never leaves the machine).
    pub fn new(session: Session, net: Arc<dyn Net>) -> App {
        App {
            session,
            selected: Vec::new(),
            view: view3d::View::default(),
            strip: heights::Strip::default(),
            tab: Tab::Prepare,
            profile: Profile::default(),
            sliced: None,
            layer: 0,
            travels: false,
            status: String::new(),
            failed: false,
            nudge: [10.0, 0.0, 0.0],
            cut_axis: 2,
            cut_at: f64::NAN,
            connector: "pin",
            connector_shape: "round",
            connector_count: None,
            connector_size: None,
            connector_fit: 0.2,
            fields: None,
            keep_proportions: true,
            range: RangeForm::default(),
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
            lang: lang::Lang::English,
            applied_look: None,
            log: None,
            logged: String::new(),
            control: None,
            agents: control::Agents::default(),
            agents_failed: None,
            bridge_file: None,
            chats: None,
            chat_saved: 0,
            updates: None,
            update_check: true,
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
        app.lang = s.language.unwrap_or(app.lang);
        app.view.card.cpu = s.cpu_view;
        app.update_check = !s.no_update_check;
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
        app.chats = Some(path.with_file_name("chats"));
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
            language: Some(self.lang),
            local_url: Some(self.chat.chat.local_url.clone()).filter(|u| u != lamina_assistant::local::DEFAULT_URL),
            local_model: Some(self.chat.local_model.clone()).filter(|m| m != lamina_assistant::Provider::Local.default_model()),
            agents: Some(self.agents.clone()).filter(|a| *a != control::Agents::default()),
            cpu_view: self.view.card.cpu,
            no_update_check: !self.update_check,
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
            (self.failed, self.status) = (true, t("A preset needs a name of up to 60 characters.").into());
            return;
        }
        if kind.built_in().iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
            (self.failed, self.status) = (true, tf("\"{}\" is a built-in {}: choose another name.", &[name, t(kind.word())]));
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
        self.status = tf("Saved the {} preset \"{}\".", &[t(kind.word()), name]);
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
        egui::ComboBox::from_id_salt(kind.word())
            .selected_text(chosen.clone().unwrap_or_else(|| t("Choose...").into()))
            .width(170.0)
            .show_ui(ui, |ui| {
                for p in kind.built_in() {
                    if ui.selectable_label(chosen.as_deref() == Some(p.name), p.name).on_hover_text(t(p.about)).clicked() {
                        pick = Some(p.name.to_string());
                    }
                }
                if !mine.is_empty() {
                    ui.separator();
                }
                for name in &mine {
                    if ui.selectable_label(chosen.as_deref() == Some(name), tf("{} (mine)", &[name])).clicked() {
                        pick = Some(name.clone());
                    }
                }
            });
        if let Some(name) = chosen.filter(|n| mine.contains(n))
            && ui.small_button(t("Delete")).on_hover_text(t("Forget this preset of yours")).clicked()
        {
            self.mine(kind).remove(&name);
        }
        if let Some(name) = pick {
            self.apply_preset(kind, &name);
        }
    }

    fn presets_ui(&mut self, ui: &mut Ui) {
        egui::Grid::new("presets").num_columns(2).show(ui, |ui| {
            ui.label(t("Printer"));
            ui.horizontal(|ui| self.preset_picker(ui, Kind::Printer));
            ui.end_row();
            ui.label(t("Filament"));
            ui.horizontal(|ui| self.preset_picker(ui, Kind::Filament));
            ui.end_row();
            ui.label(t("Process"));
            ui.horizontal(|ui| self.preset_picker(ui, Kind::Process));
            ui.end_row();
        });
        // The name on a row of its own and the buttons wrapping, so longer words in a translation neither cut the hint short nor
        // widen the panel.
        ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text(t("Preset name")).desired_width(f32::INFINITY));
        ui.horizontal_wrapped(|ui| {
            let name = self.preset_name.clone();
            if ui.button(t("Save printer")).on_hover_text(t("Save the plate, nozzle, G-code and retraction shown")).clicked() {
                self.save_preset(Kind::Printer, &name);
            }
            if ui.button(t("Save filament")).on_hover_text(t("Save the temperatures and fan shown")).clicked() {
                self.save_preset(Kind::Filament, &name);
            }
            if ui.button(t("Save process")).on_hover_text(t("Save the layers, walls, skins and infill shown")).clicked() {
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
        if result.is_ok() && lamina_core::bridge::REPLACES.contains(&command) {
            // A new plate's ids start again: what was hidden on the old one is not hidden on it.
            self.view.hidden.clear();
        }
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
        (self.failed, self.status) = (true, tf("Wait for {} to finish, or cancel it.", &[t(&job.label)]));
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
        self.view.hidden.retain(|id| alive.contains(id));
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
            let n = |k: &str, signed: bool| lang::mark(if signed { format!("{:+.2}", f(k)) } else { format!("{:.2}", f(k)) });
            self.status = format!("{} mm (x {}, y {}, z {})", n("distance", false), n("dx", true), n("dy", true), n("dz", true));
        } else if self.view.marks.len() == 1 {
            self.status = t("Click a second point to measure to.").into();
        }
    }

    /// Slices the plate with the settings shown, on a thread, and opens the preview when it is done.
    pub fn slice(&mut self) {
        let all: Vec<&lamina_core::Object> = self.session.scene.objects.iter().collect();
        let job =
            match lamina_slicer::commands::Job::of(&all, self.session.scene.layer_heights.as_deref().map(Vec::as_slice), &self.profile) {
                Ok(job) => job,
                Err(e) => return (self.failed, self.status) = (true, e),
            };
        let profile = self.profile.clone();
        let scene = Arc::as_ptr(&self.session.scene) as usize;
        let work = move |_: &mut Session| {
            let out = job.gcode(&profile)?;
            let layers = preview::parse(&out.gcode);
            let minutes = (out.seconds / 60.0).round() as u64;
            let summary = tf(
                "{} layers, {} m of filament, about {}h {}m",
                &[
                    &out.layers.to_string(),
                    &lang::mark(format!("{:.2}", out.filament_mm / 1000.0)),
                    &(minutes / 60).to_string(),
                    &format!("{:02}", minutes % 60),
                ],
            );
            Ok(work::Done::Sliced(Sliced { layers, summary, warnings: out.warnings, scene }))
        };
        self.start(ui_msg!("Slicing"), false, work, |app, done| {
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
                let (took, from) = (fields.len().to_string(), path.display().to_string());
                self.status = match left.len() {
                    0 => tf("Took {} settings from {}.", &[&took, &from]),
                    n => tf("Took {} settings from {}; left out {}: {}.", &[&took, &from, &n.to_string(), &left.join(", ")]),
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
            ui_msg!("Writing the G-code"),
            false,
            move |s| s.execute("slice", &params).map(work::Done::Value),
            move |app, done| {
                if done.is_ok() {
                    app.status = tf("Wrote {}", &[&path]);
                }
            },
        );
    }

    /// The whole window, one frame of it.
    pub fn show(&mut self, ui: &mut Ui) {
        lang::set(self.lang);
        let ctx = ui.ctx().clone();
        // While an update is put in place nothing may change: the window closes once it is in.
        if self.poll_updates(&ctx) {
            return;
        }
        // The window's print settings are the session's, so the assistant's `slice_check` and a saved project use what is shown.
        let print = self.print_json();
        if self.session.print != print {
            self.session.print = print;
        }
        if self.chat.poll(&ctx, &mut self.session) {
            self.prune();
        }
        self.keep_chat();
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
        if self.has_update_banner() {
            Panel::top("update").show(ui, |ui| self.update_banner(ui));
        }
        self.poll_work(&ctx);
        self.answer_agents(&ctx);
        self.poll_printer(&ctx);
        Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                self.work_ui(ui);
                let color = if self.failed { Color32::from_rgb(230, 110, 100) } else { Color32::GRAY };
                let idle = if self.busy() { "" } else { t("Ready") };
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

    /// Writes the open project's chat beside the settings when a turn is over and it changed.
    fn keep_chat(&mut self) {
        let chat = &self.chat.chat;
        if chat.busy() || chat.revision() == self.chat_saved {
            return;
        }
        if let (Some(dir), Some(project)) = (&self.chats, &self.project)
            && let Err(e) = lamina_assistant::saved::write(dir, project, chat)
        {
            (self.failed, self.status) = (true, tf("The chat could not be kept: {}", &[&e.to_string()]));
        }
        self.chat_saved = chat.revision();
    }

    /// The chat of `project`, as it was left, in place of the one shown; an empty one when it has none.
    pub(crate) fn chat_for(&mut self, project: Option<&std::path::Path>) {
        self.chat.chat.reset();
        if let (Some(dir), Some(project)) = (&self.chats, project)
            && let Some(v) = lamina_assistant::saved::read(dir, project)
        {
            let provider = self.chat.chat.provider;
            self.chat.chat.restore(&v, provider);
        }
        self.chat_saved = self.chat.chat.revision();
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
        // Wrapped: in a narrow window, or a language with longer words, the last buttons go to a second row, not off the edge.
        ui.horizontal_wrapped(|ui| {
            if ui.button(t("Add box")).clicked() {
                self.add_box();
            }
            if ui.button(t("Add cylinder")).clicked() {
                self.add(json!({"kind": "cylinder", "radius": 10, "height": 20}), 20.0);
            }
            if ui.button(t("Add sphere")).clicked() {
                self.add(json!({"kind": "sphere", "radius": 10}), 20.0);
            }
            ui.menu_button(t("More shapes"), |ui| {
                let shapes = [
                    (ui_msg!("Rounded box"), json!({"kind": "rounded_box", "size": [20, 20, 20], "corner": 3}), 20.0),
                    (ui_msg!("Cone"), json!({"kind": "cone", "radius": 10, "height": 20}), 20.0),
                    (ui_msg!("Tube"), json!({"kind": "tube", "radius": 10, "inner": 7, "height": 20}), 20.0),
                    (ui_msg!("Torus"), json!({"kind": "torus", "radius": 10, "tube": 3}), 26.0),
                    (ui_msg!("Hexagonal prism"), json!({"kind": "prism", "radius": 10, "height": 20, "sides": 6}), 20.0),
                    (ui_msg!("Pyramid"), json!({"kind": "pyramid", "radius": 12, "height": 15, "sides": 4}), 24.0),
                    (ui_msg!("Thread, M8"), json!({"kind": "thread", "metric": "M8", "height": 20, "name": "M8 thread"}), 8.0),
                ];
                for (label, params, width) in shapes {
                    if ui.button(t(label)).clicked() {
                        self.add(params, width);
                    }
                }
                ui.separator();
                if ui
                    .button(t("Text"))
                    .on_hover_text(t("Lettering, 10 mm capitals, 1 mm deep; the assistant or `text` writes any"))
                    .clicked()
                {
                    self.add_with("text", json!({"text": "Lamina", "height": 10, "depth": 1}), 50.0);
                }
                if ui
                    .button(t("Plate with four holes"))
                    .on_hover_text(t("A rounded plate, extruded; the assistant or `extrude` makes any outline"))
                    .clicked()
                {
                    let plate = json!({"shape": "rounded_rect", "size": [60, 40], "corner": 5, "height": 4, "name": "plate",
                        "round_holes": [[-24, -14, 3.4], [24, -14, 3.4], [24, 14, 3.4], [-24, 14, 3.4]]});
                    self.add_with("extrude", plate, 60.0);
                }
                ui.separator();
                ui.menu_button(t("Calibration"), |ui| {
                    let parts = [
                        (
                            ui_msg!("Temperature tower, 230 to 190 C"),
                            "temperature",
                            ui_msg!("Hottest at the bottom, 5 C a block: take the coolest block that bonds well and stays clean."),
                        ),
                        (
                            ui_msg!("Retraction towers, 0.4 to 2 mm"),
                            "retraction",
                            ui_msg!("0.4 mm more every 5 mm up: take the lowest band with no hairs between the towers."),
                        ),
                        (
                            ui_msg!("Pressure advance tower, 0 to 0.1"),
                            "pressure_advance",
                            ui_msg!("0.005 more every millimetre up: the height of the sharpest corners, in mm, times 0.005."),
                        ),
                        (
                            ui_msg!("Flow blocks, 0.90 to 1.10"),
                            "flow",
                            ui_msg!("0.90, 0.95, 1.00, 1.05 and 1.10 from left to right: take the block with the smoothest top."),
                        ),
                    ];
                    for (label, kind, look) in parts {
                        if ui
                            .button(t(label))
                            .on_hover_text(t("Its settings change with height; the assistant can change the range"))
                            .clicked()
                            && let Some(v) = self.run("calibration", json!({ "kind": kind }))
                        {
                            self.selected = v["ids"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(|i| i as u32).collect();
                            self.status = t(look).into();
                            self.frame_all();
                        }
                    }
                });
            });
            if ui.button(t("Import mesh...")).clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter(t("Mesh"), files::MESHES).pick_file()
            {
                self.import(&p);
            }
            ui.separator();
            let undo = self.session.undo_label().map_or(t("Nothing to undo").to_string(), |l| tf("Undo {} (Ctrl+Z)", &[l]));
            if ui.button(t("Undo")).on_hover_text(undo).clicked() {
                self.run("undo", json!({}));
            }
            if ui.button(t("Redo")).on_hover_text(t("Bring back what undo took (Ctrl+Y)")).clicked() {
                self.run("redo", json!({}));
            }
            if ui.button(t("Frame all")).on_hover_text(t("Look at everything on the plate (F frames the picked objects)")).clicked() {
                self.frame_all();
            }
            if ui.button(t("Arrange")).on_hover_text(t("Lay every object out on the plate, none touching (A)")).clicked() {
                self.arrange();
            }
            ui.separator();
            {
                use crate::view3d::Tool;
                let before = self.view.tool;
                ui.selectable_value(&mut self.view.tool, Tool::Move, t("Move"))
                    .on_hover_text(t("Pick objects and drag them with the arrows (G)"));
                ui.selectable_value(&mut self.view.tool, Tool::LayFlat, t("Lay flat"))
                    .on_hover_text(t("Click a face to turn it down onto the plate (L)"));
                ui.selectable_value(&mut self.view.tool, Tool::Measure, t("Measure"))
                    .on_hover_text(t("Click two points to measure between them (M)"));
                ui.selectable_value(&mut self.view.tool, Tool::Paint, t("Paint"))
                    .on_hover_text(t("Paint where seams go and where supports must or must not go: click or drag over an object (P)"));
                if self.view.tool != before {
                    self.view.marks.clear();
                }
            }
            ui.separator();
            ui.selectable_value(&mut self.tab, Tab::Prepare, t("Prepare"));
            ui.selectable_value(&mut self.tab, Tab::Preview, t("Preview"));
            ui.separator();
            if ui.button(t("Slice")).on_hover_text(t("Slice the plate with these settings and show the layers")).clicked() {
                self.slice();
            }
            if ui.button(t("Export G-code...")).on_hover_text(t("Slice the plate into a G-code file")).clicked() {
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
        self.view.plane = None;
        ui.heading(t("Plate"));
        let rows: Vec<(u32, String)> = self
            .session
            .scene
            .objects
            .iter()
            .map(|o| {
                let (lo, hi) = o.bounds().unwrap_or_default();
                let side = |k: usize| lang::mark(format!("{:.1}", hi[k] - lo[k]));
                (o.id, format!("{}  {} x {} x {}", o.name, side(0), side(1), side(2)))
            })
            .collect();
        let rows: Vec<(u32, String, Option<lamina_core::repair::Problems>)> =
            rows.into_iter().map(|(id, text)| (id, text, self.problems_of(id))).collect();
        if rows.is_empty() {
            ui.label(
                RichText::new(t("Nothing on the plate yet: add a shape, import or drop a mesh, open a project, or ask the assistant."))
                    .weak(),
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
                if self.view.hidden.contains(&id)
                    && ui.small_button(t("Show")).on_hover_text(t("Hidden from the 3D view (H); it still prints")).clicked()
                {
                    self.view.hidden.retain(|&h| h != id);
                }
                if let Some(p) = problems {
                    ui.label(RichText::new("\u{26a0}").color(WARNING)).on_hover_text(tf(
                        "This mesh is not a clean solid: {}. Parts of it may not print; Repair mends what it can.",
                        &[&problems_text(&p)],
                    ));
                }
            });
        }
        ui.label(RichText::new(t("Ctrl-click picks several; right-click for more.")).small().weak());
        ui.separator();
        match self.selected.as_slice() {
            [] => {}
            &[id] => self.one_object(ui, id),
            many => {
                let ids = many.to_vec();
                ui.label(tf("{} objects picked", &[&ids.len().to_string()]));
                ui.horizontal(|ui| {
                    if ui.button(t("Join")).on_hover_text(t("One solid of all of them (union)")).clicked() {
                        self.boolean("union", json!({ "ids": ids }));
                    }
                    if ui.button(t("Subtract")).on_hover_text(t("Cut the others out of the first one picked")).clicked() {
                        self.boolean("difference", json!({ "id": ids[0], "minus": ids[1..] }));
                    }
                    if ui.button(t("Intersect")).on_hover_text(t("Keep only what they all share")).clicked() {
                        self.boolean("intersect", json!({ "ids": ids }));
                    }
                });
                if ui.button(t("Delete all")).on_hover_text(t("Delete the picked objects, as one undo step")).clicked() {
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
                    if let Some(note) = v["note"].as_str() {
                        app.status = tf("Done; {}.", &[note]);
                    }
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
                ui.label(RichText::new(format!("\u{26a0} {}", problems_text(&p))).color(WARNING));
                if ui.button(t("Repair")).on_hover_text(t("Drop doubled faces, turn flipped ones, close cracks and small holes")).clicked()
                    && let Some(r) = self.run("repair", json!({ "id": id }))
                {
                    let closed = if r["watertight"] == true {
                        t("it is closed now")
                    } else {
                        t("it is still open: a hole is too big to fill without guessing")
                    };
                    let n = |k: &str| r[k].to_string();
                    self.status = tf(
                        "Repaired: {} faces dropped, {} turned, {} holes filled; {}.",
                        &[&n("dropped"), &n("flipped"), &n("holes_filled"), closed],
                    );
                }
            });
        }
        self.position_fields(ui, id);
        self.object_settings(ui, id);
        ui.label(t("Move by, mm"));
        ui.horizontal(|ui| {
            for (k, axis) in ["x", "y", "z"].into_iter().enumerate() {
                ui.add(crate::lang::number(&mut self.nudge[k]).speed(0.5).prefix(format!("{axis} ")));
            }
            if ui.button(t("Move")).clicked() {
                self.run("move", json!({ "id": id, "by": self.nudge }));
            }
        });
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("axis").selected_text(["x", "y", "z"][self.axis]).width(40.0).show_ui(ui, |ui| {
                for (k, axis) in ["x", "y", "z"].into_iter().enumerate() {
                    ui.selectable_value(&mut self.axis, k, axis);
                }
            });
            ui.add(crate::lang::number(&mut self.degrees).speed(1.0).suffix(t(" deg")));
            if ui.button(t("Rotate")).clicked() {
                let axis = ["x", "y", "z"][self.axis];
                self.run("rotate", json!({ "id": id, "axis": axis, "degrees": self.degrees }));
            }
            if ui.button(t("Mirror")).on_hover_text(t("Mirror across the plane square to the axis chosen")).clicked() {
                let axis = ["x", "y", "z"][self.axis];
                self.run("mirror", json!({ "id": id, "axis": axis }));
            }
        });
        ui.horizontal(|ui| {
            ui.add(crate::lang::number(&mut self.factor).speed(0.01).range(0.01..=100.0).prefix("x "));
            if ui.button(t("Scale")).clicked() {
                self.run("scale", json!({ "id": id, "factor": self.factor }));
            }
        });
        // A part, or a modifier the slicer reads and does not print.
        ui.horizontal(|ui| {
            use lamina_core::Modifier;
            let now = self.session.scene.get(id).ok().and_then(|o| o.modifier);
            let name = |m: Option<Modifier>| match m {
                None => t("A part"),
                Some(Modifier::SupportEnforcer) => t("Support enforcer"),
                Some(Modifier::SupportBlocker) => t("Support blocker"),
                Some(Modifier::Settings) => t("Settings modifier"),
            };
            ui.label(t("Prints as"));
            let mut chosen = now;
            egui::ComboBox::from_id_salt("modifier")
                .selected_text(name(now))
                .show_ui(ui, |ui| {
                    for m in [None, Some(Modifier::SupportEnforcer), Some(Modifier::SupportBlocker), Some(Modifier::Settings)] {
                        ui.selectable_value(&mut chosen, m, name(m));
                    }
                })
                .response
                .on_hover_text(t(
                    "A modifier is not printed. The overhangs of a part inside a support enforcer get supports, inside a blocker none; \
                     a settings modifier gives the infill of a part inside it its own density (in This object's settings)",
                ));
            if chosen != now {
                let kind = match chosen {
                    None => "none",
                    Some(Modifier::SupportEnforcer) => "support_enforcer",
                    Some(Modifier::SupportBlocker) => "support_blocker",
                    Some(Modifier::Settings) => "settings",
                };
                self.run("set_modifier", json!({ "id": id, "kind": kind }));
            }
        });
        crate::lang::section("Cut into pieces").show(ui, |ui| self.cut_ui(ui, id));
        // A heavy mesh (a scan, say) can be given fewer triangles; on a thread, as it can take seconds.
        let triangles = self.session.scene.get(id).map_or(0, |o| o.mesh.triangles.len());
        if triangles > 1000 {
            ui.horizontal(|ui| {
                ui.add(crate::lang::number(&mut self.keep).speed(1.0).range(1.0..=99.0).suffix(" %"))
                    .on_hover_text(t("The share of its triangles to keep"));
                if ui
                    .button(t("Simplify"))
                    .on_hover_text(tf("{} triangles now; its shape is kept as well as it can be", &[&triangles.to_string()]))
                    .clicked()
                {
                    let params = json!({ "id": id, "ratio": self.keep / 100.0 });
                    self.start(
                        "simplify",
                        true,
                        move |s| s.execute("simplify", &params).map(work::Done::Value),
                        move |app, done| {
                            if let Ok(work::Done::Value(v)) = done {
                                let (before, after) = (v["before"].to_string(), v["triangles"].to_string());
                                app.status = tf("Simplified from {} to {} triangles.", &[&before, &after]);
                            }
                        },
                    );
                }
            });
        }
        ui.horizontal(|ui| {
            if ui.button(t("Duplicate")).on_hover_text(t("A copy beside it; the plate is laid out again")).clicked()
                && let Some(v) = self.run("duplicate", json!({ "id": id }))
                && let Some(copy) = v["ids"][0].as_u64()
            {
                self.selected = vec![copy as u32];
            }
            if ui.button(t("Drop to plate")).clicked() {
                self.run("drop_to_bed", json!({ "id": id }));
            }
            if ui.button(t("Delete")).clicked() {
                self.run("delete", json!({ "id": id }));
            }
        });
    }

    /// The Cut tools for the picked object: a plane square to an axis, shown in the 3D view, where the slider puts it; the connectors;
    /// Cut, and Split to fit plate. The default, pins, is left to the command to place, so a face too small for them is cut without and said.
    fn cut_ui(&mut self, ui: &mut Ui, id: u32) {
        let Some((lo, hi)) = self.session.scene.get(id).ok().and_then(|o| o.bounds()) else { return };
        ui.horizontal(|ui| {
            ui.label(t("Across"));
            for (k, axis) in ["x", "y", "z"].into_iter().enumerate() {
                ui.selectable_value(&mut self.cut_axis, k, axis);
            }
        });
        let k = self.cut_axis.min(2);
        if !(lo[k]..=hi[k]).contains(&self.cut_at) {
            self.cut_at = (lo[k] + hi[k]) / 2.0;
        }
        ui.add(egui::Slider::new(&mut self.cut_at, lo[k]..=hi[k]).text(t("at")).suffix(" mm").max_decimals(2));
        let (ku, kv, m) = ((k + 1) % 3, (k + 2) % 3, 5.0);
        self.view.plane = Some([(-m, -m), (m, -m), (m, m), (-m, m)].map(|(du, dv)| {
            let mut p = [0.0; 3];
            p[k] = self.cut_at;
            p[ku] = if du < 0.0 { lo[ku] + du } else { hi[ku] + du };
            p[kv] = if dv < 0.0 { lo[kv] + dv } else { hi[kv] + dv };
            p
        }));
        let connectors = self.connectors_ui(ui);
        ui.horizontal(|ui| {
            if ui.button(t("Cut")).on_hover_text(t("Cut here; each piece is turned to print and the plate laid out")).clicked() {
                let axis = ["x", "y", "z"][k];
                self.boolean("cut", json!({ "id": id, "axis": axis, "at": self.cut_at, "connectors": connectors, "lay_out": true }));
            }
            if ui
                .button(t("Split to fit plate"))
                .on_hover_text(t("Cut it into as few pieces as fit the printer, with connectors at every cut"))
                .clicked()
            {
                self.boolean("split_to_fit", json!({ "id": id, "connectors": connectors }));
            }
        });
    }

    /// The connectors' choices, and what they ask the command for. What is left on Auto is chosen from the cut face; the default
    /// (pins, round, Auto, 0.2 mm) asks for nothing by name, so a face too small for them is cut without, and said, not refused.
    fn connectors_ui(&mut self, ui: &mut Ui) -> Value {
        ui.horizontal(|ui| {
            ui.label(t("Connectors"));
            ui.selectable_value(&mut self.connector, "pin", t("Pins"))
                .on_hover_text(t("Pegs on one piece and sockets in the other: nothing loose"));
            ui.selectable_value(&mut self.connector, "dowel", t("Dowels"))
                .on_hover_text(t("Holes in both pieces and a loose pin for each pair: both pieces lie on their cut faces"));
            ui.selectable_value(&mut self.connector, "none", t("None")).on_hover_text(t("No connectors: for parts you glue"));
        });
        if self.connector == "none" {
            return json!({ "kind": "none" });
        }
        egui::Grid::new("connector-options").num_columns(2).show(ui, |ui| {
            ui.label(t("Shape"));
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.connector_shape, "round", t("Round"));
                ui.selectable_value(&mut self.connector_shape, "square", t("Square"))
                    .on_hover_text(t("A square peg or hole cannot turn in its socket"));
            });
            ui.end_row();
            ui.label(t("How many"));
            egui::ComboBox::from_id_salt("connector-count")
                .selected_text(self.connector_count.map_or(t("Auto").to_string(), |n| n.to_string()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.connector_count, None, t("Auto"));
                    for n in 1..=8 {
                        ui.selectable_value(&mut self.connector_count, Some(n), n.to_string());
                    }
                });
            ui.end_row();
            ui.label(t("Size"));
            ui.horizontal(|ui| {
                let mut auto = self.connector_size.is_none();
                if ui.checkbox(&mut auto, t("Auto")).on_hover_text(t("Sized from the cut face, 3 to 8 mm across")).changed() {
                    self.connector_size = if auto { None } else { Some(5.0) };
                }
                if let Some(d) = &mut self.connector_size {
                    ui.add(crate::lang::number(d).speed(0.1).range(2.0..=30.0).suffix(" mm"));
                }
            });
            ui.end_row();
            ui.label(t("Fit"));
            ui.add(crate::lang::number(&mut self.connector_fit).speed(0.01).range(0.0..=1.0).suffix(t(" mm a side")))
                .on_hover_text(t("The gap between a peg or pin and its hole: more is looser. 0.2 mm suits most printers"));
            ui.end_row();
        });
        let mut c = serde_json::Map::new();
        if self.connector != "pin" {
            c.insert("kind".into(), json!(self.connector));
        }
        if self.connector_shape != "round" {
            c.insert("shape".into(), json!(self.connector_shape));
        }
        if let Some(n) = self.connector_count {
            c.insert("count".into(), json!(n));
        }
        if let Some(d) = self.connector_size {
            c.insert("diameter".into(), json!(d));
        }
        if (self.connector_fit - 0.2).abs() > 1e-9 {
            c.insert("clearance".into(), json!(self.connector_fit));
        }
        Value::Object(c)
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
            ui.label(t("Position"));
            for (k, axis) in ["x", "y", "z"].into_iter().enumerate() {
                let r = ui.add(crate::lang::number(&mut f.at[k]).speed(0.5).prefix(format!("{axis} ")).max_decimals(3));
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    moved = Some(k);
                }
            }
            ui.end_row();
            ui.label(t("Size"));
            for (k, axis) in ["x", "y", "z"].into_iter().enumerate() {
                let r = ui
                    .add(crate::lang::number(&mut f.size[k]).speed(0.5).range(0.01..=10_000.0).prefix(format!("{axis} ")).max_decimals(3));
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    sized = Some(k);
                }
            }
            ui.end_row();
        });
        ui.checkbox(&mut self.keep_proportions, t("Keep proportions"));
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
        // A settings modifier gives only the infill inside it.
        let zone = o.modifier == Some(lamina_core::Modifier::Settings);
        let mut changed = None;
        crate::lang::section("This object's settings").show(ui, |ui| {
            if zone {
                ui.label(t("The infill of the parts inside this modifier:"));
            }
            let mut infill = mine.get("infill_density").and_then(Value::as_f64);
            let mut walls = mine.get("perimeters").and_then(Value::as_u64).map(|w| w as u32);
            let mut own_infill = infill.is_some();
            let mut own_walls = walls.is_some();
            ui.horizontal(|ui| {
                ui.checkbox(&mut own_infill, t("Infill"));
                let mut percent = infill.unwrap_or(self.profile.infill_density) * 100.0;
                if ui.add_enabled(own_infill, crate::lang::number(&mut percent).range(0.0..=100.0).suffix(" %")).changed()
                    || own_infill != infill.is_some()
                {
                    infill = own_infill.then_some(percent / 100.0);
                    changed = Some(());
                }
            });
            let mut extruder = mine.get("extruder").and_then(Value::as_u64).map(|e| e as u32);
            if !zone {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut own_walls, t("Walls"));
                    let mut w = walls.unwrap_or(self.profile.perimeters);
                    if ui.add_enabled(own_walls, crate::lang::number(&mut w).range(0..=10)).changed() || own_walls != walls.is_some() {
                        walls = own_walls.then_some(w);
                        changed = Some(());
                    }
                });
                let mut own_extruder = extruder.is_some();
                ui.horizontal(|ui| {
                    ui.checkbox(&mut own_extruder, t("Extruder")).on_hover_text(hint("extruder"));
                    let mut e = extruder.unwrap_or(self.profile.extruder);
                    let range = 1..=self.profile.extruders.max(1);
                    if ui.add_enabled(own_extruder, crate::lang::number(&mut e).range(range)).changed()
                        || own_extruder != extruder.is_some()
                    {
                        extruder = own_extruder.then_some(e);
                        changed = Some(());
                    }
                });
            }
            if changed.is_some() {
                let mut fields = mine.clone();
                let own = [
                    ("infill_density", infill.map(Value::from)),
                    ("perimeters", walls.map(Value::from)),
                    ("extruder", extruder.map(Value::from)),
                ];
                for (k, v) in own {
                    match v {
                        Some(v) => fields.insert(k.into(), v),
                        None => fields.remove(k),
                    };
                }
                changed = None;
                self.run("set_object_settings", json!({ "id": id, "profile": fields }));
            }
            if !zone {
                self.ranges_ui(ui, id);
            }
        });
    }

    /// The object's settings by height: each range with what it sets and Remove, and a form to add one.
    fn ranges_ui(&mut self, ui: &mut Ui, id: u32) {
        ui.separator();
        ui.label(t("By height")).on_hover_text(t(
            "Layers between two heights print with settings of their own: a temperature tower, denser infill near a load",
        ));
        let ranges = self.session.scene.get(id).ok().and_then(|o| o.ranges.as_deref().cloned()).unwrap_or(Value::Null);
        let mut remove = None;
        for r in ranges.as_array().into_iter().flatten() {
            let (from, to) = (r["from"].as_f64().unwrap_or(0.0), r["to"].as_f64().unwrap_or(0.0));
            let what = r["profile"]
                .as_object()
                .map(|m| m.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(tf("{} to {} mm: {}", &[&lang::mark(from.to_string()), &lang::mark(to.to_string()), &what]));
                if ui.small_button(t("Remove")).clicked() {
                    remove = Some((from, to));
                }
            });
        }
        if let Some((from, to)) = remove {
            self.run("set_height_range", json!({ "id": id, "from": from, "to": to, "profile": {} }));
        }
        let f = &mut self.range;
        ui.horizontal(|ui| {
            ui.label(t("From"));
            ui.add(crate::lang::number(&mut f.from).speed(0.5).range(0.0..=10_000.0).suffix(" mm"));
            ui.label(t("to"));
            ui.add(crate::lang::number(&mut f.to).speed(0.5).range(0.0..=10_000.0).suffix(" mm"));
        });
        let row = |ui: &mut Ui, label: &str, on: bool| -> bool {
            let mut on = on;
            ui.checkbox(&mut on, label);
            on
        };
        ui.horizontal(|ui| {
            let on = row(ui, t("Nozzle, C"), f.nozzle_temp.is_some());
            let mut v = f.nozzle_temp.unwrap_or(self.profile.nozzle_temp);
            ui.add_enabled(on, crate::lang::number(&mut v).range(0.0..=400.0));
            f.nozzle_temp = on.then_some(v);
        });
        ui.horizontal(|ui| {
            let on = row(ui, t("Infill"), f.infill.is_some());
            let mut v = f.infill.unwrap_or(self.profile.infill_density) * 100.0;
            ui.add_enabled(on, crate::lang::number(&mut v).range(0.0..=100.0).suffix(" %"));
            f.infill = on.then_some(v / 100.0);
        });
        ui.horizontal(|ui| {
            let on = row(ui, t("Walls"), f.walls.is_some());
            let mut v = f.walls.unwrap_or(self.profile.perimeters);
            ui.add_enabled(on, crate::lang::number(&mut v).range(0..=10));
            f.walls = on.then_some(v);
        });
        let mut profile = serde_json::Map::new();
        for (k, v) in [
            ("nozzle_temp", f.nozzle_temp.map(Value::from)),
            ("infill_density", f.infill.map(Value::from)),
            ("perimeters", f.walls.map(Value::from)),
        ] {
            if let Some(v) = v {
                profile.insert(k.into(), v);
            }
        }
        let (from, to) = (f.from, f.to);
        if ui.add_enabled(!profile.is_empty(), egui::Button::new(t("Add range"))).clicked() {
            self.run("set_height_range", json!({ "id": id, "from": from, "to": to, "profile": profile }));
        }
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
        crate::lang::section("Print settings").default_open(true).show(ui, |ui| {
            let p = &mut self.profile;
            let max_layer = p.nozzle * 0.8;
            egui::Grid::new("settings").num_columns(2).show(ui, |ui| {
                ui.label(t("Layer height, mm"));
                ui.add(crate::lang::number(&mut p.layer_height).speed(0.01).range(0.02..=max_layer)).on_hover_text(hint("layer_height"));
                ui.end_row();
                ui.label("");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut p.adaptive_layers, t("Adaptive, from")).on_hover_text(hint("adaptive_layers"));
                    ui.add_enabled(p.adaptive_layers, crate::lang::number(&mut p.min_layer_height).speed(0.01).range(0.02..=max_layer))
                        .on_hover_text(hint("min_layer_height"));
                });
                ui.end_row();
                ui.label(t("First layer, mm"));
                ui.add(crate::lang::number(&mut p.first_layer_height).speed(0.01).range(0.02..=max_layer))
                    .on_hover_text(hint("first_layer_height"));
                ui.end_row();
                ui.label(t("Walls"));
                ui.horizontal(|ui| {
                    ui.add(crate::lang::number(&mut p.perimeters).range(0..=10)).on_hover_text(hint("perimeters"));
                    use lamina_slicer::walls::WallGenerator;
                    let name = |g: WallGenerator| match g {
                        WallGenerator::Classic => t("Classic"),
                        WallGenerator::Variable => t("Variable width"),
                    };
                    egui::ComboBox::from_id_salt("wall_generator")
                        .selected_text(name(p.wall_generator))
                        .show_ui(ui, |ui| {
                            for g in [WallGenerator::Classic, WallGenerator::Variable] {
                                ui.selectable_value(&mut p.wall_generator, g, name(g));
                            }
                        })
                        .response
                        .on_hover_text(hint("wall_generator"));
                });
                ui.end_row();
                ui.label(t("Top / bottom layers"));
                ui.horizontal(|ui| {
                    ui.add(crate::lang::number(&mut p.top_layers).range(0..=50)).on_hover_text(hint("top_layers"));
                    ui.add(crate::lang::number(&mut p.bottom_layers).range(0..=50)).on_hover_text(hint("bottom_layers"));
                });
                ui.end_row();
                ui.label(t("Infill"));
                let mut percent = p.infill_density * 100.0;
                if ui
                    .add(crate::lang::number(&mut percent).speed(1.0).range(0.0..=100.0).suffix(" %"))
                    .on_hover_text(t(lamina_slicer::profile::about("infill_density")))
                    .changed()
                {
                    p.infill_density = percent / 100.0;
                }
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut p.vase, t("Vase mode")).on_hover_text(hint("vase"));
                ui.end_row();
                ui.label(t("Infill pattern"));
                {
                    use lamina_slicer::infill::Pattern;
                    let name = |x: Pattern| match x {
                        Pattern::Lines => t("Lines"),
                        Pattern::Grid => t("Grid"),
                        Pattern::Triangles => t("Triangles"),
                        Pattern::Gyroid => t("Gyroid"),
                        Pattern::Honeycomb => t("Honeycomb"),
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
                ui.label(t("Nozzle / bed, C"));
                ui.horizontal(|ui| {
                    ui.add(crate::lang::number(&mut p.nozzle_temp).range(0.0..=400.0)).on_hover_text(hint("nozzle_temp"));
                    ui.add(crate::lang::number(&mut p.bed_temp).range(0.0..=400.0)).on_hover_text(hint("bed_temp"));
                });
                ui.end_row();
                ui.label(t("First layer nozzle / bed"));
                ui.horizontal(|ui| {
                    ui.add(crate::lang::number(&mut p.first_layer_nozzle_temp).range(0.0..=400.0))
                        .on_hover_text(hint("first_layer_nozzle_temp"));
                    ui.add(crate::lang::number(&mut p.first_layer_bed_temp).range(0.0..=400.0)).on_hover_text(hint("first_layer_bed_temp"));
                });
                ui.end_row();
                ui.label(t("Seam"));
                {
                    use lamina_slicer::profile::Seam;
                    let name = |s: Seam| match s {
                        Seam::Aligned => t("Aligned"),
                        Seam::Rear => t("At the back"),
                        Seam::Sharpest => t("Sharpest corner"),
                        Seam::Nearest => t("Nearest"),
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
                ui.label(t("Brim, mm / raft layers"));
                ui.horizontal(|ui| {
                    ui.add(crate::lang::number(&mut p.brim_width).speed(0.5).range(0.0..=50.0)).on_hover_text(hint("brim_width"));
                    ui.add(crate::lang::number(&mut p.raft_layers).range(0..=10)).on_hover_text(hint("raft_layers"));
                });
                ui.end_row();
                ui.label(t("Supports"));
                ui.horizontal(|ui| {
                    use lamina_slicer::profile::Support;
                    let name = |s: Support| match s {
                        Support::Off => t("Off"),
                        Support::Everywhere => t("Everywhere"),
                        Support::FromPlate => t("From the plate only"),
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
                        ui.add(crate::lang::number(&mut p.support_angle).range(10.0..=89.0).suffix(t(" deg")))
                            .on_hover_text(hint("support_angle"));
                        use lamina_slicer::profile::SupportStyle;
                        let style = |s: SupportStyle| match s {
                            SupportStyle::Columns => t("Columns"),
                            SupportStyle::Tree => t("Tree"),
                        };
                        egui::ComboBox::from_id_salt("support_style")
                            .selected_text(style(p.support_style))
                            .show_ui(ui, |ui| {
                                for s in [SupportStyle::Columns, SupportStyle::Tree] {
                                    ui.selectable_value(&mut p.support_style, s, style(s));
                                }
                            })
                            .response
                            .on_hover_text(hint("support_style"));
                    }
                });
                ui.end_row();
                ui.label(t("Plate, mm"));
                ui.horizontal(|ui| {
                    ui.add(crate::lang::number(&mut p.bed[0]).range(10.0..=2000.0)).on_hover_text(hint("bed"));
                    ui.add(crate::lang::number(&mut p.bed[1]).range(10.0..=2000.0)).on_hover_text(hint("bed"));
                });
                ui.end_row();
            });
            self.more_settings(ui);
            ui.horizontal(|ui| {
                if ui.button(t("Back to the defaults")).clicked() {
                    self.profile = Profile::default();
                }
                if ui
                    .button(t("Import profile..."))
                    .on_hover_text(t("A PrusaSlicer .ini, or an OrcaSlicer or Bambu Studio .json preset"))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new().add_filter(t("Profile"), &["ini", "json"]).pick_file()
                {
                    self.import_profile(&path);
                }
            });
        });
    }

    /// Every other setting, in groups, each number's limits on hover; and the start and end G-code with their placeholders.
    fn more_settings(&mut self, ui: &mut Ui) {
        let p = &mut self.profile;
        // `title` is English (marked with `ui_msg!`): the section's id and its grid's, whatever the language shown.
        let group = |ui: &mut Ui, title: &str, body: &mut dyn FnMut(&mut Ui)| {
            crate::lang::section(title).show(ui, |ui| egui::Grid::new(title).num_columns(2).show(ui, |ui| body(ui)));
        };
        group(ui, ui_msg!("Speeds"), &mut |ui| {
            field(ui, "Walls, mm/s", "perimeter_speed", &mut p.perimeter_speed, 1.0);
            field(ui, "Infill, mm/s", "infill_speed", &mut p.infill_speed, 1.0);
            field(ui, "First layer, mm/s", "first_layer_speed", &mut p.first_layer_speed, 1.0);
            field(ui, "Travel, mm/s", "travel_speed", &mut p.travel_speed, 1.0);
            field(ui, "Bridges, mm/s", "bridge_speed", &mut p.bridge_speed, 1.0);
            field(ui, "Slowest when cooling, mm/s", "min_print_speed", &mut p.min_print_speed, 1.0);
            field(ui, "Most plastic, mm3/s", "max_volumetric_speed", &mut p.max_volumetric_speed, 0.5);
            field(ui, "Flow ratio", "flow_ratio", &mut p.flow_ratio, 0.005);
        });
        group(ui, ui_msg!("Retraction and travel"), &mut |ui| {
            field(ui, "Retract, mm", "retract_length", &mut p.retract_length, 0.1);
            field(ui, "Retract speed, mm/s", "retract_speed", &mut p.retract_speed, 1.0);
            field(ui, "Not under, mm of travel", "retract_min_travel", &mut p.retract_min_travel, 0.1);
            field(ui, "Z hop, mm", "z_hop", &mut p.z_hop, 0.05);
            field(ui, "Wipe, mm", "wipe_distance", &mut p.wipe_distance, 0.1);
            ui.checkbox(&mut p.combing, t("Travel inside the part without retracting")).on_hover_text(hint("combing"));
            ui.end_row();
            ui.checkbox(&mut p.firmware_retraction, t("Firmware retraction (G10, G11)")).on_hover_text(hint("firmware_retraction"));
            ui.end_row();
        });
        group(ui, ui_msg!("Skirt"), &mut |ui| {
            field(ui, "Loops", "skirt_loops", &mut p.skirt_loops, 0.1);
            field(ui, "Distance, mm", "skirt_distance", &mut p.skirt_distance, 0.5);
        });
        group(ui, ui_msg!("Cooling"), &mut |ui| {
            field(ui, "Fan, 0 to 255", "fan_speed", &mut p.fan_speed, 1.0);
            field(ui, "Full fan from layer", "fan_full_layer", &mut p.fan_full_layer, 0.1);
            field(ui, "Shortest layer, s", "min_layer_time", &mut p.min_layer_time, 0.5);
            field(ui, "Fan on bridges", "bridge_fan", &mut p.bridge_fan, 1.0);
            field(ui, "Bridge flow", "bridge_flow", &mut p.bridge_flow, 0.01);
        });
        group(ui, ui_msg!("Surface"), &mut |ui| {
            ui.checkbox(&mut p.ironing, t("Iron the top surfaces")).on_hover_text(hint("ironing"));
            ui.end_row();
            field(ui, "Ironing flow", "ironing_flow", &mut p.ironing_flow, 0.01);
            field(ui, "Ironing spacing, mm", "ironing_spacing", &mut p.ironing_spacing, 0.01);
            field(ui, "Ironing speed, mm/s", "ironing_speed", &mut p.ironing_speed, 1.0);
            ui.checkbox(&mut p.fuzzy_skin, t("Fuzzy skin on the outer wall")).on_hover_text(hint("fuzzy_skin"));
            ui.end_row();
            field(ui, "Fuzzy thickness, mm", "fuzzy_thickness", &mut p.fuzzy_thickness, 0.01);
            field(ui, "Fuzzy spacing, mm", "fuzzy_spacing", &mut p.fuzzy_spacing, 0.05);
        });
        group(ui, ui_msg!("Support details"), &mut |ui| {
            field(ui, "Density", "support_density", &mut p.support_density, 0.01);
            field(ui, "Gap above, mm", "support_z_gap", &mut p.support_z_gap, 0.05);
            field(ui, "Gap beside, mm", "support_xy_gap", &mut p.support_xy_gap, 0.05);
            field(ui, "Interface layers", "support_interface_layers", &mut p.support_interface_layers, 0.1);
            field(ui, "Tree tips apart, mm", "tree_tip_spacing", &mut p.tree_tip_spacing, 0.1);
            field(ui, "Tree branch lean, deg", "tree_branch_angle", &mut p.tree_branch_angle, 1.0);
            field(ui, "Tree trunk radius, mm", "tree_max_radius", &mut p.tree_max_radius, 0.1);
        });
        group(ui, ui_msg!("Extruders"), &mut |ui| {
            field(ui, "How many", "extruders", &mut p.extruders, 0.05);
            // Fewer extruders: what named the ones gone goes with them, so the settings stay valid.
            p.extruder = p.extruder.min(p.extruders);
            p.extruder_temps.truncate(p.extruders.saturating_sub(1) as usize);
            for i in 1..p.extruders as usize {
                let mut t = p.extruder_temps.get(i - 1).copied().unwrap_or(p.nozzle_temp);
                ui.label(tf("Extruder {} nozzle, C", &[&(i + 1).to_string()])).on_hover_text(hint("extruder_temps"));
                if ui.add(crate::lang::number(&mut t).range(0.0..=400.0)).on_hover_text(hint("extruder_temps")).changed() {
                    p.extruder_temps.resize(p.extruder_temps.len().max(i), p.nozzle_temp);
                    p.extruder_temps[i - 1] = t;
                }
                ui.end_row();
            }
            field(ui, "Standby, C below", "standby_temp_delta", &mut p.standby_temp_delta, 1.0);
            field(ui, "Heating, C/s", "heating_rate", &mut p.heating_rate, 0.1);
            field(ui, "Tower x, mm", "tower_x", &mut p.tower_x, 1.0);
            field(ui, "Tower y, mm", "tower_y", &mut p.tower_y, 1.0);
            field(ui, "Tower size, mm", "tower_size", &mut p.tower_size, 0.5);
            field(ui, "Purge, mm3", "purge_volume", &mut p.purge_volume, 1.0);
        });
        group(ui, ui_msg!("One at a time"), &mut |ui| {
            ui.checkbox(&mut p.sequential, t("Print each object whole, one after another")).on_hover_text(hint("sequential"));
            ui.end_row();
            field(ui, "Head clearance radius, mm", "clearance_radius", &mut p.clearance_radius, 1.0);
            field(ui, "Gantry height, mm", "gantry_height", &mut p.gantry_height, 1.0);
        });
        group(ui, ui_msg!("Machine"), &mut |ui| {
            field(ui, "Nozzle, mm", "nozzle", &mut p.nozzle, 0.05);
            field(ui, "Filament, mm", "filament_diameter", &mut p.filament_diameter, 0.05);
            field(ui, "Height, mm", "max_height", &mut p.max_height, 1.0);
            field(ui, "Acceleration, mm/s2", "max_acceleration", &mut p.max_acceleration, 10.0);
            field(ui, "Junction deviation, mm", "junction_deviation", &mut p.junction_deviation, 0.001);
            field(ui, "Pressure advance", "pressure_advance", &mut p.pressure_advance, 0.001);
            ui.label(t("Firmware"));
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
            ui.label(t("Thumbnail, pixels"));
            ui.add(crate::lang::number(&mut p.thumbnail).range(0..=512)).on_hover_text(format!(
                "{} {}",
                t(lamina_slicer::profile::about("thumbnail")),
                t("0, or from 16 to 512.")
            ));
            ui.end_row();
            ui.checkbox(&mut p.origin_center, t("Zero in the middle (delta)")).on_hover_text(hint("origin_center"));
            ui.end_row();
            ui.checkbox(&mut p.arc_fitting, t("Curves as arcs (G2, G3)")).on_hover_text(hint("arc_fitting"));
            ui.end_row();
            field(ui, "Arc tolerance, mm", "arc_tolerance", &mut p.arc_tolerance, 0.005);
            ui.end_row();
        });
        crate::lang::section("Start and end G-code").show(ui, |ui| {
            let names = lamina_slicer::profile::PLACEHOLDERS.iter().map(|n| format!("[{n}]")).collect::<Vec<_>>().join(" ");
            ui.label(RichText::new(tf("Placeholders: {}", &[&names])).small());
            ui.label(
                RichText::new(t("In braces, an expression of them: {first_layer_bed_temp - 5}, {if(bed_temp > 90, 1, 0)}, with min, max and round; {{ for a brace."))
                    .small(),
            );
            ui.label(t("Start"));
            ui.add(egui::TextEdit::multiline(&mut p.start_gcode).code_editor().desired_rows(4));
            ui.label(t("End"));
            ui.add(egui::TextEdit::multiline(&mut p.end_gcode).code_editor().desired_rows(4));
            let tool = lamina_slicer::profile::TOOL_PLACEHOLDERS;
            let names = tool.iter().map(|(n, _)| format!("[{n}]")).collect::<Vec<_>>().join(" ");
            ui.label(tf("Tool change, before the T; also {}", &[&names])).on_hover_text(hint("toolchange_gcode"));
            ui.add(egui::TextEdit::multiline(&mut p.toolchange_gcode).code_editor().desired_rows(2));
            for (text, extra) in [(&p.start_gcode, &[][..]), (&p.end_gcode, &[][..]), (&p.toolchange_gcode, tool)] {
                if let Err(e) = p.expand_with(text, extra) {
                    ui.label(RichText::new(e).color(WARNING));
                }
            }
        });
    }

    /// The brush's settings, over the 3D view while the paint tool is in hand.
    fn brush_ui(&mut self, ui: &mut Ui) {
        use lamina_core::Paint;
        ui.horizontal_wrapped(|ui| {
            let b = &mut self.view.brush;
            ui.label(t("Paint:"));
            ui.selectable_value(&mut b.kind, Paint::Seam, t("Seam")).on_hover_text(t("Each wall's seam starts on the painted faces"));
            ui.selectable_value(&mut b.kind, Paint::Support, t("Support"))
                .on_hover_text(t("Painted faces looking down are held up, even with supports off"));
            ui.selectable_value(&mut b.kind, Paint::Block, t("Block")).on_hover_text(t("No support under the painted faces"));
            ui.selectable_value(&mut b.kind, Paint::None, t("Erase")).on_hover_text(t("Rub paint out"));
            ui.separator();
            ui.label(t("Brush"));
            ui.add(crate::lang::number(&mut b.radius).speed(0.1).range(0.2..=50.0).suffix(" mm"));
            ui.separator();
            let painted: Vec<u32> =
                self.selected.iter().copied().filter(|&id| self.session.scene.get(id).is_ok_and(|o| o.paint.is_some())).collect();
            if ui
                .add_enabled(!painted.is_empty(), egui::Button::new(t("Clear the picked objects' paint")))
                .on_hover_text(t("Take all paint off the picked objects"))
                .clicked()
            {
                let commands = painted.iter().map(|&id| ("paint", json!({ "id": id, "kind": "none" }))).collect();
                self.run_all("paint", commands);
            }
        });
    }

    fn prepare(&mut self, ui: &mut Ui) {
        if self.strip.open {
            Panel::right("layer-heights").resizable(false).exact_size(170.0).show(ui, |ui| self.heights_ui(ui));
        }
        if self.view.tool == view3d::Tool::Paint {
            self.brush_ui(ui);
        }
        let scene = Arc::clone(&self.session.scene);
        let shown = self.view.show(ui, &scene, &self.selected, self.profile.bed);
        if let Some(ids) = &shown.boxed {
            // As Blender's B: what the box covers joins what is picked.
            for &id in ids {
                if !self.selected.contains(&id) {
                    self.selected.push(id);
                }
            }
            self.status = tf("{} picked.", &[&self.selected.len().to_string()]);
        }
        if let Some((id, points)) = &shown.painted {
            // A stroke is one step to undo: each point painted round, in one transaction.
            let (kind, radius) = (self.view.brush.command_kind(), self.view.brush.radius);
            let commands = points.iter().map(|p| ("paint", json!({ "id": id, "kind": kind, "at": p, "radius": radius }))).collect();
            self.run_all("paint", commands);
        }
        if let Some(note) = self.view.card.note.take() {
            (self.failed, self.status) = (true, note);
        }
        if let Some((ids, change)) = &shown.moved {
            // One command (or two, about the middle of them all) for each picked object, in one transaction, so one undo takes the
            // whole drag back.
            let (label, commands) = view3d::commands_for(*change, ids, &self.session.scene, self.view.pivot, &self.view.axes);
            self.run_all(label, commands);
        }
        if let Some((id, face)) = shown.face
            && self.run("lay_flat", json!({ "id": id, "face": face })).is_some()
        {
            self.selected = vec![id];
            self.status = t("Laid flat on the face clicked.").into();
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
            ui.centered_and_justified(|ui| ui.label(t("Slice the plate to see its layers.")));
            return;
        };
        ui.horizontal(|ui| {
            ui.label(&sliced.summary);
            if sliced.scene != current {
                ui.label(RichText::new(t("The plate changed since this slice: slice again.")).color(WARNING));
            }
        });
        for w in &sliced.warnings {
            ui.label(RichText::new(w).color(WARNING));
        }
        let last = sliced.layers.len().saturating_sub(1);
        let z = sliced.layers.get(self.layer).map_or(0.0, |l| l.z);
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut self.layer, 0..=last).text(t("layer")));
            ui.label(format!("z = {} mm", lang::mark(format!("{z:.2}"))));
            ui.checkbox(&mut self.travels, t("Travels"));
            ui.separator();
            ui.selectable_value(&mut self.preview_3d, true, "3D")
                .on_hover_text(t("Every layer up to this one, coloured by what it prints"));
            ui.selectable_value(&mut self.preview_3d, false, t("Layer")).on_hover_text(t("This layer alone, from above"));
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
    use lamina_core::Mesh;

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
    fn the_cut_tools_show_their_plane_cut_with_connectors_and_split_to_fit() {
        let mut h = window();
        h.state_mut().run("add_primitive", json!({"kind": "box", "size": [40, 40, 60]}));
        h.state_mut().selected = vec![1];
        h.run();
        assert!(h.state().view.plane.is_none(), "no plane while the Cut tools are closed");
        press(&mut h, "Cut into pieces");
        let plane = h.state().view.plane.unwrap();
        assert!(plane.iter().all(|p| (p[2] - 30.0).abs() < 1e-9), "level, through the middle: {plane:?}");
        // Pins by default: two pieces, pegs on one and sockets in the other, nothing loose, one undo step.
        press(&mut h, "Cut");
        let app = h.state();
        assert!(!app.failed, "{}", app.status);
        assert_eq!((app.session.scene.objects.len(), app.selected.len(), app.session.undo_label()), (2, 2, Some("cut")));
        assert!(app.session.scene.objects.iter().all(|o| o.bounds().unwrap().0[2].abs() < 1e-6), "laid out on the plate");
        press(&mut h, "Undo");
        // Across x, with dowels: the plane stands upright through the middle, and a loose dowel is made for each pair of holes.
        let app = h.state_mut();
        (app.selected, app.cut_axis) = (vec![1], 0);
        h.run();
        let plane = h.state().view.plane.unwrap();
        assert!(plane.iter().all(|p| p[0].abs() < 1e-9), "{plane:?}");
        press(&mut h, "Dowels");
        press(&mut h, "Cut");
        assert_eq!(h.state().session.scene.objects.len(), 4, "{}", h.state().status);
        // What the user chooses is what is made: three square dowels.
        press(&mut h, "Undo");
        h.state_mut().selected = vec![1];
        h.run();
        press(&mut h, "Square");
        h.state_mut().connector_count = Some(3);
        h.run();
        press(&mut h, "Cut");
        let app = h.state();
        assert!(!app.failed, "{}", app.status);
        let dowels: Vec<_> = app.session.scene.objects.iter().filter(|o| o.name.ends_with("dowel")).collect();
        assert_eq!(dowels.len(), 3, "{}", app.status);
        // A part too tall for the printer is split into pieces that each fit.
        press(&mut h, "Undo");
        h.state_mut().run("add_primitive", json!({"kind": "cylinder", "radius": 25, "height": 300, "at": [80, 0]}));
        h.state_mut().selected = vec![2];
        h.run();
        press(&mut h, "Pins");
        press(&mut h, "Split to fit plate");
        let app = h.state();
        assert!(!app.failed, "{}", app.status);
        assert_eq!(app.session.undo_label(), Some("split_to_fit"));
        assert!(app.session.scene.objects.len() >= 3, "two pieces and the box, no loose dowels");
        assert!(app.session.scene.objects.iter().all(|o| o.bounds().unwrap().1[2] <= 250.0));
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
            ("Extruders", "Purge, mm3"),
            ("One at a time", "Gantry height, mm"),
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
    fn the_assistants_answer_shows_as_it_is_written() {
        let (fake, body) = lamina_assistant::testing::Fake::default().live(200);
        let mut app = App::new(session(), Arc::new(fake));
        app.chat.chat.provider = lamina_assistant::Provider::Anthropic;
        let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        h.state_mut().chat.chat.send("hi", "sk-test-0123456789abcdef");
        // Anthropic's events, as its streaming guide shows them.
        let event = |kind: &str, data: &str| format!("event: {kind}\ndata: {{\"type\": \"{kind}\"{data}}}\n\n");
        let text =
            |piece: &str| event("content_block_delta", &format!(r#", "index": 0, "delta": {{"type": "text_delta", "text": "{piece}"}}"#));
        let first = [
            event("message_start", r#", "message": {"content": [], "usage": {"input_tokens": 9, "output_tokens": 1}}"#),
            event("content_block_start", r#", "index": 0, "content_block": {"type": "text", "text": ""}"#),
            text("A cube,"),
        ];
        body.send(first.concat()).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while h.query_by_label("A cube,").is_none() {
            assert!(std::time::Instant::now() < until, "{:?}", h.state().chat.chat.log);
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(h.state().chat.chat.busy(), "the words show before the turn ends");
        let rest = [
            text(" 20 mm."),
            event("content_block_stop", r#", "index": 0"#),
            event("message_delta", r#", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 4}"#),
            event("message_stop", ""),
        ];
        body.send(rest.concat()).unwrap();
        drop(body);
        while h.state().chat.chat.busy() {
            assert!(std::time::Instant::now() < until, "{:?}", h.state().chat.chat.log);
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        h.run();
        h.get_by_label("A cube, 20 mm.");
        assert!(h.query_by_label("A cube,").is_none(), "the whole answer took the place of the growing one");
    }

    #[test]
    fn a_projects_chat_is_kept_and_comes_back_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let net = Arc::new(
            lamina_assistant::testing::Fake::default()
                .reply(200, r#"{"content": [{"type": "text", "text": "Hello."}], "stop_reason": "end_turn"}"#),
        );
        let mut app = App::with_settings(session(), net, dir.path().join("settings.json"));
        app.chat.chat.provider = lamina_assistant::Provider::Anthropic;
        let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        let project = dir.path().join("plate.lamina");
        assert!(h.state_mut().save_project(&project));
        h.state_mut().chat.chat.send("hi", "sk-test-0123456789abcdef");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while h.state().chat.chat.busy() {
            assert!(std::time::Instant::now() < until);
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        h.run();
        let log = h.state().chat.chat.log.clone();
        assert!(log.iter().any(|l| matches!(l, lamina_assistant::Line::Says(t) if t == "Hello.")), "{log:?}");
        // A new plate has a new chat; opening the project again brings its chat back.
        h.state_mut().new_project();
        assert!(h.state().chat.chat.log.is_empty());
        h.state_mut().open_project(&project);
        assert_eq!(h.state().chat.chat.log, log);
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
    fn the_window_speaks_the_language_chosen_in_the_view_menu_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let app = App::with_settings(session(), Arc::new(lamina_assistant::testing::Fake::default()), path.clone());
        let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        h.run();
        assert_eq!(h.state().lang, lang::Lang::English, "a test's window is English, whatever the machine's locale");
        h.get_by_label("Slice");
        press(&mut h, "View");
        press(&mut h, "Português (Brasil)");
        h.get_by_label("Fatiar");
        h.get_by_label("Configurações de impressão");
        assert!(h.query_by_label("Slice").is_none(), "nothing of the toolbar is left in English");
        assert_eq!(Settings::read(&path).0.language, Some(lang::Lang::Portuguese));
        assert!(h.state().title().starts_with("Sem título"), "{}", h.state().title());
        // What the user named stays as it is.
        h.state_mut().run("add_primitive", json!({"kind": "box", "size": [10, 10, 10], "name": "Bracket"}));
        h.run();
        h.get_by_label_contains("Bracket");
        // Back to English, from the same menu (now called Exibir).
        h.key_press(Key::Escape);
        h.run();
        press(&mut h, "Exibir");
        press(&mut h, "English");
        h.get_by_label("Slice");
        assert!(h.query_by_label("Fatiar").is_none());
        let again = App::with_settings(session(), Arc::new(lamina_assistant::testing::Fake::default()), path);
        assert_eq!(again.lang, lang::Lang::English, "kept for the next start");
    }

    /// Types `text` into the number field that shows `shown`, and presses Enter.
    fn type_number(h: &mut Harness<App>, shown: &str, text: &str) {
        let role = egui::accesskit::Role::SpinButton;
        h.get_by(|n| n.role() == role && n.value().as_deref() == Some(shown)).focus();
        h.run();
        h.get_by(|n| n.role() == role && n.value().as_deref() == Some(shown)).type_text(text);
        h.run();
        h.key_press(Key::Enter);
        h.run();
    }

    #[test]
    fn number_fields_write_and_read_a_decimal_comma_in_portuguese_and_spanish() {
        let mut h = window();
        press(&mut h, "Add box");
        h.get_by_label_contains("20.0 x 20.0 x 20.0");
        h.state_mut().profile.layer_height = 0.17;
        h.run();
        let spin = |h: &Harness<App>, text: &str| {
            h.query_by(|n| n.role() == egui::accesskit::Role::SpinButton && n.value().as_deref() == Some(text)).is_some()
        };
        assert!(spin(&h, "0.17") && !spin(&h, "0,17"), "English writes a point");
        // English reads as before: a point, and a comma is no number, so the value stays.
        type_number(&mut h, "0.17", "0,3");
        assert_eq!(h.state().profile.layer_height, 0.17);
        type_number(&mut h, "0.17", "0.15");
        assert_eq!(h.state().profile.layer_height, 0.15);
        for (language, typed, want) in [(lang::Lang::Portuguese, "0,25", 0.25), (lang::Lang::Spanish, "0,12", 0.12)] {
            h.state_mut().lang = language;
            h.state_mut().profile.layer_height = 0.17;
            h.run();
            assert!(spin(&h, "0,17") && !spin(&h, "0.17"), "{language:?} writes a comma");
            // So do the tooltips' ranges and the sizes in the plate's list.
            assert!(hint("layer_height").contains("0,02"), "{}", hint("layer_height"));
            h.get_by_label_contains("20,0 x 20,0 x 20,0");
            type_number(&mut h, "0,17", typed);
            assert_eq!(h.state().profile.layer_height, want, "{language:?} reads {typed}");
            // A point, as a number pasted from elsewhere has, is read too.
            type_number(&mut h, typed, "0.2");
            assert_eq!(h.state().profile.layer_height, 0.2, "{language:?} reads a point");
        }
    }

    /// The plate panel grows to its widest row, so a translation's longer words take room from the 3D view: in Portuguese and
    /// Spanish it keeps at least 90 % of the width it has in English, with a picked object's tools and the print settings shown.
    #[test]
    fn translations_leave_the_3d_view_most_of_its_width() {
        let width = |language: lang::Lang| {
            let mut h = window();
            h.state_mut().lang = language;
            h.run();
            press(&mut h, t("Add box"));
            h.state().view.rect.unwrap().width()
        };
        let english = width(lang::Lang::English);
        for language in [lang::Lang::Portuguese, lang::Lang::Spanish] {
            let w = width(language);
            assert!(w >= 0.9 * english, "{language:?}: the 3D view is {w} wide, against {english} in English");
        }
    }

    #[test]
    fn g_r_and_s_move_turn_and_scale_with_an_axis_and_a_typed_amount_as_blender_does() {
        let mut h = window();
        press(&mut h, "Add box");
        h.state_mut().run("scale", json!({"id": 1, "by": [2, 1, 1]})); // 40 x 20 x 20
        h.state_mut().selected = vec![1];
        h.run();
        let middle = h.state().view.rect.unwrap().center();
        let keys = |h: &mut Harness<App>, text: &str, end: Key| {
            h.hover_at(middle);
            h.run();
            for c in text.chars() {
                match c {
                    'G' | 'R' | 'S' | 'X' | 'Y' | 'Z' => {
                        h.key_press([Key::G, Key::R, Key::S, Key::X, Key::Y, Key::Z]["GRSXYZ".find(c).unwrap()])
                    }
                    c => h.event(egui::Event::Text(c.to_string())),
                }
                h.run();
            }
            h.key_press(end);
            h.run();
        };
        let bounds = |h: &Harness<App>| h.state().session.scene.get(1).unwrap().bounds().unwrap();
        let (lo, _) = bounds(&h);
        keys(&mut h, "GX12.5", Key::Enter);
        assert!((bounds(&h).0[0] - lo[0] - 12.5).abs() < 1e-6, "moved 12.5 mm along x: {:?}", bounds(&h));
        assert!(h.state().view.modal.is_none(), "Enter ends it");
        press(&mut h, "Undo");
        assert!((bounds(&h).0[0] - lo[0]).abs() < 1e-6, "one undo takes it back");
        // A quarter turn about z typed, as R then 90 (z is the axis when none is chosen).
        keys(&mut h, "R90", Key::Enter);
        let (a, b) = bounds(&h);
        assert!((b[0] - a[0] - 20.0).abs() < 1e-3 && (b[1] - a[1] - 40.0).abs() < 1e-3, "{a:?} {b:?}");
        // Twice as tall: S, Z, 2.
        keys(&mut h, "SZ2", Key::Enter);
        let (a, b) = bounds(&h);
        assert!((b[2] - a[2] - 40.0).abs() < 1e-3 && (b[0] - a[0] - 20.0).abs() < 1e-3, "only z: {a:?} {b:?}");
        // Escape puts it back, whatever was typed.
        let before = bounds(&h);
        keys(&mut h, "GY30", Key::Escape);
        assert_eq!(bounds(&h), before);
        assert!(h.state().view.modal.is_none());
        // In Portuguese a comma is the decimal mark.
        h.state_mut().lang = lang::Lang::Portuguese;
        h.run();
        keys(&mut h, "GX1,5", Key::Enter);
        assert!((bounds(&h).0[0] - before.0[0] - 1.5).abs() < 1e-6, "{:?}", bounds(&h));
        // With nothing picked, G picks the move tool as before.
        h.state_mut().selected.clear();
        h.state_mut().view.tool = view3d::Tool::Measure;
        h.run();
        h.hover_at(middle);
        h.key_press(Key::G);
        h.run();
        assert!(h.state().view.modal.is_none() && h.state().view.tool == view3d::Tool::Move);
    }

    #[test]
    fn a_turn_goes_about_each_middle_or_the_middle_of_all_and_moves_go_along_an_objects_own_axes() {
        let mut h = window();
        let app = h.state_mut();
        app.run("add_primitive", json!({"kind": "box", "size": [20, 10, 10], "at": [-20, 0]}));
        app.run("add_primitive", json!({"kind": "box", "size": [20, 10, 10], "at": [20, 0]}));
        app.selected = vec![1, 2];
        h.run();
        let middle = h.state().view.rect.unwrap().center();
        let keyed = |h: &mut Harness<App>, keys: &[Key], text: &str| {
            h.hover_at(middle);
            h.run();
            for &k in keys {
                h.key_press(k);
                h.run();
            }
            h.event(egui::Event::Text(text.to_string()));
            h.run();
            h.key_press(Key::Enter);
            h.run();
        };
        let centre = |h: &Harness<App>, id: u32| {
            let (lo, hi) = h.state().session.scene.get(id).unwrap().bounds().unwrap();
            [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, hi[0] - lo[0]]
        };
        let near = |a: [f64; 3], b: [f64; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-4);
        // Each about its own middle: they turn where they stand.
        keyed(&mut h, &[Key::R], "90");
        assert!(near(centre(&h, 1), [-20.0, 0.0, 10.0]) && near(centre(&h, 2), [20.0, 0.0, 10.0]), "{:?}", centre(&h, 1));
        press(&mut h, "Undo");
        // About the middle of both: they swing round it, in one undo step.
        h.state_mut().view.pivot = view3d::Pivot::Middle;
        keyed(&mut h, &[Key::R], "90");
        assert!(
            near(centre(&h, 1), [0.0, -20.0, 10.0]) && near(centre(&h, 2), [0.0, 20.0, 10.0]),
            "{:?} {:?}",
            centre(&h, 1),
            centre(&h, 2)
        );
        press(&mut h, "Undo");
        assert!(near(centre(&h, 1), [-20.0, 0.0, 20.0]), "one step: {:?}", centre(&h, 1));
        // Along its own axes: a box turned 30 degrees moves 10 mm along its own x.
        h.state_mut().view.pivot = view3d::Pivot::Each;
        h.state_mut().run("rotate", json!({"id": 1, "axis": "z", "degrees": 30}));
        h.state_mut().selected = vec![1];
        h.state_mut().view.orientation = view3d::Orientation::Own;
        h.run();
        let before = centre(&h, 1);
        keyed(&mut h, &[Key::G, Key::X], "10");
        let after = centre(&h, 1);
        let (c, s) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
        assert!((after[0] - before[0] - 10.0 * c).abs() < 1e-4 && (after[1] - before[1] - 10.0 * s).abs() < 1e-4, "{before:?} {after:?}");
        // Alt+R takes its turn away, Alt+G puts it in the middle of the plate.
        h.key_press_modifiers(egui::Modifiers::ALT, Key::R);
        h.run();
        assert!(near([0.0, 0.0, centre(&h, 1)[2]], [0.0, 0.0, 20.0]), "{:?}", centre(&h, 1));
        h.key_press_modifiers(egui::Modifiers::ALT, Key::G);
        h.run();
        assert!(near(centre(&h, 1), [0.0, 0.0, 20.0]), "{:?}", centre(&h, 1));
    }

    #[test]
    fn the_number_keys_look_from_the_front_the_right_and_the_top_and_home_frames_all() {
        let mut h = window();
        press(&mut h, "Add box");
        let middle = h.state().view.rect.unwrap().center();
        h.hover_at(middle);
        h.run();
        let look = |h: &Harness<App>| (h.state().view.camera.azimuth, h.state().view.camera.elevation);
        for (key, modifiers, want) in [
            (Key::Num1, egui::Modifiers::NONE, (-90.0, 0.0)),
            (Key::Num3, egui::Modifiers::NONE, (0.0, 0.0)),
            (Key::Num7, egui::Modifiers::NONE, (-90.0, 89.0)),
            (Key::Num1, egui::Modifiers::COMMAND, (90.0, 0.0)),
            (Key::Num3, egui::Modifiers::COMMAND, (180.0, 0.0)),
            (Key::Num7, egui::Modifiers::COMMAND, (-90.0, -89.0)),
        ] {
            h.key_press_modifiers(modifiers, key);
            h.run();
            assert_eq!(look(&h), want, "{key:?} {modifiers:?}");
        }
        // From the front, the box is in the middle of the picture.
        h.key_press(Key::Num1);
        h.key_press(Key::Home);
        h.run();
        let target = h.state().view.camera.target;
        assert!(target[0].abs() < 1e-6 && (target[2] - 10.0).abs() < 1e-6, "framed on the box: {target:?}");
        h.state_mut().view.camera.distance = 2000.0;
        h.key_press(Key::Period);
        h.run();
        assert!(h.state().view.camera.distance < 500.0, "framed on the picked box again");
    }

    #[test]
    fn h_hides_the_picked_from_the_view_alt_h_shows_them_and_b_picks_what_a_box_covers() {
        let mut h = window();
        press(&mut h, "Add box");
        press(&mut h, "Add cylinder");
        press(&mut h, "Add sphere");
        h.state_mut().view.camera.frame_all(([-15.0, -15.0, 0.0], [80.0, 15.0, 20.0]));
        h.state_mut().view.card.cpu = true;
        h.state_mut().selected = vec![2];
        let rect = h.state().view.rect.unwrap();
        h.hover_at(rect.center());
        h.run();
        let seen = |h: &Harness<App>| -> Vec<u32> {
            let p = h.state().view.picture.as_ref().expect("the processor's picture");
            let mut ids: Vec<u32> =
                (0..p.size[1]).flat_map(|y| (0..p.size[0]).map(move |x| (x, y))).filter_map(|(x, y)| p.id_at(x, y)).collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };
        assert_eq!(seen(&h), [1, 2, 3]);
        // H: the cylinder leaves the picture and the pick, and the plate list offers to show it; it still prints.
        h.key_press(Key::H);
        h.run();
        h.run();
        assert_eq!((h.state().view.hidden.clone(), h.state().selected.clone()), (vec![2], vec![]));
        assert_eq!(seen(&h), [1, 3]);
        h.get_by_label("Show");
        assert_eq!(h.state().session.scene.objects.len(), 3);
        // B, then a drag over the whole view: what it sees is picked, the hidden cylinder not.
        h.key_press(Key::B);
        h.run();
        assert!(h.state().view.boxing.is_some());
        drag(&mut h, rect.left_top() + egui::vec2(2.0, 2.0), rect.right_bottom() - egui::vec2(2.0, 2.0));
        assert_eq!(h.state().selected, [1, 3], "{}", h.state().status);
        assert!(h.state().view.boxing.is_none());
        // A box over empty plate picks nothing more; Escape gives one up.
        h.state_mut().selected.clear();
        h.hover_at(rect.center());
        h.key_press(Key::B);
        h.run();
        drag(&mut h, rect.left_top() + egui::vec2(2.0, 2.0), rect.left_top() + egui::vec2(12.0, 12.0));
        assert!(h.state().selected.is_empty());
        h.hover_at(rect.center());
        h.key_press(Key::B);
        h.run();
        h.key_press(Key::Escape);
        h.run();
        assert!(h.state().view.boxing.is_none());
        // Alt+H: everything back.
        h.key_press_modifiers(egui::Modifiers::ALT, Key::H);
        h.run();
        h.run();
        assert!(h.state().view.hidden.is_empty());
        assert_eq!(seen(&h), [1, 2, 3]);
        // 5 over the view: orthographic, and still drawn and picked.
        h.hover_at(rect.center());
        h.key_press(Key::Num5);
        h.run();
        h.run();
        assert!(h.state().view.camera.ortho);
        assert_eq!(seen(&h), [1, 2, 3]);
        h.key_press(Key::Num5);
        h.run();
        assert!(!h.state().view.camera.ortho);
    }

    #[test]
    fn the_calibration_menu_puts_a_tower_on_the_plate_and_says_what_to_look_for() {
        let mut h = window();
        press(&mut h, "More shapes");
        h.get_by_label_contains("Calibration").hover();
        h.run();
        press(&mut h, "Temperature tower, 230 to 190 C");
        let s = &h.state().session.scene;
        assert_eq!(s.objects.len(), 1);
        assert_eq!(s.objects[0].ranges.as_deref().and_then(Value::as_array).map(Vec::len), Some(9));
        assert!(h.state().status.contains("coolest block"), "{}", h.state().status);
        assert_eq!(h.state().selected, vec![1]);
    }

    #[test]
    fn a_height_range_is_added_and_removed_from_the_objects_settings() {
        let app = App::new(session(), Arc::new(lamina_assistant::testing::Fake::default()));
        let mut h = Harness::builder().with_size([1400.0, 2400.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        h.run();
        press(&mut h, "Add box");
        press(&mut h, "This object's settings");
        press(&mut h, "Add range");
        let ranges = h.state().session.scene.get(1).unwrap().ranges.as_deref().cloned().unwrap();
        assert_eq!(ranges, json!([{"from": 5.0, "to": 10.0, "profile": {"nozzle_temp": 220.0}}]));
        h.get_by_label_contains("5 to 10 mm: nozzle_temp 220");
        press(&mut h, "Remove");
        assert!(h.state().session.scene.get(1).unwrap().ranges.is_none());
    }

    #[test]
    fn prints_as_makes_the_picked_object_a_support_blocker_in_one_step() {
        let mut h = window();
        press(&mut h, "Add box");
        // The picker shows what the object is now as its value.
        h.get_by(|n| n.role() == egui::accesskit::Role::ComboBox && n.value().as_deref() == Some("A part")).click();
        h.run();
        press(&mut h, "Support blocker");
        assert_eq!(h.state().session.scene.get(1).unwrap().modifier, Some(lamina_core::Modifier::SupportBlocker));
        h.get_by(|n| n.role() == egui::accesskit::Role::ComboBox && n.value().as_deref() == Some("Support blocker"));
        press(&mut h, "Undo");
        assert_eq!(h.state().session.scene.get(1).unwrap().modifier, None);
    }

    #[test]
    fn a_settings_modifier_shows_only_the_infill_in_its_settings() {
        let mut h = window();
        press(&mut h, "Add box");
        h.get_by(|n| n.role() == egui::accesskit::Role::ComboBox && n.value().as_deref() == Some("A part")).click();
        h.run();
        press(&mut h, "Settings modifier");
        assert_eq!(h.state().session.scene.get(1).unwrap().modifier, Some(lamina_core::Modifier::Settings));
        press(&mut h, "This object's settings");
        h.get_by_label("The infill of the parts inside this modifier:");
        assert!(h.query_by_label("By height").is_none(), "a modifier has no settings by height");
        // The object's own Infill is a checkbox; the print settings' Infill is a label.
        h.get_by(|n| n.role() == egui::accesskit::Role::CheckBox && n.label().as_deref() == Some("Infill")).click();
        h.run();
        let fields = h.state().session.scene.get(1).unwrap().settings.as_deref().cloned().unwrap();
        assert_eq!(fields["infill_density"], h.state().profile.infill_density, "{fields}");
    }

    #[test]
    fn open_sections_stay_open_and_closed_ones_closed_when_the_language_changes() {
        // Tall, so the plate panel needs no scrolling to the headers clicked.
        let app = App::new(session(), Arc::new(lamina_assistant::testing::Fake::default()));
        let mut h = Harness::builder().with_size([1400.0, 2400.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        h.run();
        press(&mut h, "Add box");
        press(&mut h, "Speeds");
        press(&mut h, "Cut into pieces");
        h.get_by_label("Walls, mm/s");
        h.get_by_label("Connectors");
        assert!(h.query_by_label("Retract, mm").is_none(), "a section not opened is closed");
        for (language, open, closed) in [
            (lang::Lang::Portuguese, ["Paredes, mm/s", "Conectores"], "Retração, mm"),
            (lang::Lang::Spanish, ["Paredes, mm/s", "Conectores"], "Retracción, mm"),
            (lang::Lang::English, ["Walls, mm/s", "Connectors"], "Retract, mm"),
        ] {
            h.state_mut().lang = language;
            h.run();
            for label in open {
                h.get_by_label(label);
            }
            assert!(h.query_by_label(closed).is_none(), "{language:?}: {closed} stays closed");
        }
        // Closed in Spanish, closed in English: the state is one, whatever the title says.
        h.state_mut().lang = lang::Lang::Spanish;
        h.run();
        press(&mut h, "Velocidades");
        assert!(h.query_by_label("Paredes, mm/s").is_none(), "closed in Spanish");
        h.state_mut().lang = lang::Lang::English;
        h.run();
        assert!(h.query_by_label("Walls, mm/s").is_none());
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
        // Trees are a style of support, kept with the settings, and their own settings are in the details.
        h.state_mut().profile.support_style = lamina_slicer::profile::SupportStyle::Tree;
        h.run();
        assert_eq!(h.state().print_json()["profile"]["support_style"], "tree");
        press(&mut h, "Support details");
        h.get_by_label("Tree tips apart, mm");
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
        app.view.camera = view3d::Camera { target: [0.0, 0.0, 10.0], azimuth: -90.0, elevation: 0.0, distance: 200.0, ortho: false };
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
    fn the_layer_heights_strip_sets_a_band_by_dragging_and_its_buttons_fill_smooth_and_reset() {
        let mut h = window();
        press(&mut h, "Add box");
        h.state_mut().strip.open = true;
        h.run();
        let top = h.state().session.scene.get(1).unwrap().bounds().unwrap().1[2];
        press(&mut h, "Adaptive");
        assert!(h.state().session.scene.layer_heights.is_some(), "{}", h.state().status);
        press(&mut h, "Reset");
        assert!(h.state().session.scene.layer_heights.is_none());
        // A drag from a quarter of the way up to halfway, a quarter of the way across: those heights at a quarter of the most the
        // nozzle prints (0.32 mm for 0.4).
        let plot = h.state().strip.plot.unwrap();
        let at = |x: f32, y: f32| egui::pos2(plot.left() + x * plot.width(), plot.bottom() - y * plot.height());
        drag(&mut h, at(0.25, 0.25), at(0.25, 0.5));
        let curve = h.state().session.scene.layer_heights.clone().expect("a band set");
        let height = |z: f64| lamina_slicer::heights::at(&curve, z);
        assert!((height(top * 0.375) - 0.08).abs() < 0.011, "{curve:?}");
        assert!((height(top * 0.1) - h.state().profile.layer_height).abs() < 1e-9, "below the band as it was");
        press(&mut h, "Smooth");
        assert_ne!(h.state().session.scene.layer_heights.as_ref(), Some(&curve), "smoothed");
        // Each is one step to undo.
        h.state_mut().run("undo", json!({}));
        assert_eq!(h.state().session.scene.layer_heights.as_ref(), Some(&curve));
    }

    #[test]
    fn the_paint_tool_paints_where_it_is_clicked_and_dragged_and_each_stroke_is_one_undo() {
        use lamina_core::Paint;
        let mut h = window();
        press(&mut h, "Add box");
        press(&mut h, "Paint");
        assert_eq!(h.state().view.tool, view3d::Tool::Paint);
        // Looking at the box's front face (y = -10) square on, its middle in the middle of the view.
        h.state_mut().view.camera =
            view3d::Camera { target: [0.0, 0.0, 10.0], azimuth: -90.0, elevation: 0.0, distance: 200.0, ortho: false };
        h.run();
        h.run();
        let rect = h.state().view.rect.unwrap();
        let tags = |h: &Harness<App>| h.state().session.scene.get(1).unwrap().paint.as_deref().cloned();
        let click = |h: &mut Harness<App>, at: egui::Pos2| {
            h.hover_at(at);
            h.run();
            for pressed in [true, false] {
                let (button, modifiers) = (egui::PointerButton::Primary, egui::Modifiers::NONE);
                h.event(egui::Event::PointerButton { pos: at, button, pressed, modifiers });
                h.run();
            }
        };
        // A click paints the seam on the front face's two triangles, and nowhere else.
        click(&mut h, rect.center());
        let seam = tags(&h).expect("painted");
        assert_eq!(seam.iter().filter(|&&t| t == Paint::Seam as u8).count(), 2, "{seam:?}");
        let front: Vec<usize> = (0..seam.len()).filter(|&i| seam[i] != 0).collect();
        let world = h.state().session.scene.get(1).unwrap().world();
        assert!(front.iter().all(|&i| world.tri(i).unwrap().iter().all(|q| (q[1] + 10.0).abs() < 1e-6)));
        // A drag across the face with the support brush: the same faces, now support, in one step.
        let steps = h.state().session.history().0.len();
        press(&mut h, "Erase");
        assert_eq!(h.state().view.brush.kind, Paint::None);
        h.state_mut().view.brush.kind = Paint::Support;
        let camera = h.state().view.camera;
        drag(&mut h, rect.center() - egui::vec2(20.0, 0.0), rect.center() + egui::vec2(20.0, 0.0));
        let support = tags(&h).unwrap();
        assert!(front.iter().all(|&i| support[i] == Paint::Support as u8), "{support:?}");
        assert_eq!(h.state().session.history().0.len(), steps + 1, "one stroke, one step");
        assert_eq!(h.state().view.camera, camera, "a stroke on the object does not turn the view");
        h.state_mut().run("undo", json!({}));
        assert_eq!(tags(&h).unwrap(), seam);
        // A drag that starts off the object turns the view and paints nothing.
        drag(&mut h, rect.left_top() + egui::vec2(10.0, 10.0), rect.left_top() + egui::vec2(60.0, 10.0));
        assert_ne!(h.state().view.camera, camera);
        assert_eq!(tags(&h).unwrap(), seam);
        // Clearing the picked object's paint takes it all off.
        h.state_mut().selected = vec![1];
        press(&mut h, "Clear the picked objects' paint");
        assert!(tags(&h).is_none());
    }

    /// A window taller than the graphics card's largest texture (2048 pixels in the test harness) draws the 3D view smaller and
    /// stretches it, and a click still finds what is under the pointer.
    #[test]
    fn a_window_taller_than_the_largest_texture_still_draws_and_picks() {
        let app = App::new(session(), Arc::new(lamina_assistant::testing::Fake::default()));
        let mut h = Harness::builder().with_size([1400.0, 2400.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        h.run();
        press(&mut h, "Add box");
        h.state_mut().selected.clear();
        h.state_mut().view.camera =
            view3d::Camera { target: [0.0, 0.0, 10.0], azimuth: -90.0, elevation: 0.0, distance: 120.0, ortho: false };
        h.run();
        h.run();
        let rect = h.state().view.rect.unwrap();
        assert!(rect.height() > 2048.0, "the view itself is taller than a texture may be: {rect:?}");
        let most = h.ctx.input(|i| i.max_texture_side);
        let picture = h.state().view.picture.as_ref().expect("the processor draws in a test");
        assert!(picture.size.iter().all(|&side| side <= most), "{:?} within {most}", picture.size);
        let click = |h: &mut Harness<App>, at: egui::Pos2| {
            h.hover_at(at);
            h.run();
            for pressed in [true, false] {
                h.event(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
                h.run();
            }
        };
        click(&mut h, rect.center());
        assert_eq!(h.state().selected, vec![1], "the box in the middle of the view is picked");
        click(&mut h, rect.center() + egui::vec2(0.0, rect.height() * 0.4));
        assert!(h.state().selected.is_empty(), "the plate below it is not the box");
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
        let p = h.state_mut().problems_of(1).unwrap();
        assert_eq!((p.open_edges, problems_text(&p)), (3, "3 open edges".to_string()));
        h.get_by_label("Repair");
        press(&mut h, "Repair");
        assert!(h.state().status.contains("closed now"), "{}", h.state().status);
        assert!(h.state_mut().problems_of(1).is_none());
        assert!(h.state().session.scene.get(1).unwrap().mesh.is_watertight());
    }

    #[test]
    fn a_watched_printer_says_what_it_is_doing() {
        let moon = r#"{"result": {"status": {"print_stats": {"state": "printing", "filename": "part.gcode"}, "virtual_sdcard": {"progress": 0.5}}}}"#;
        let (address, _) = lamina_printer::testing::printer(200, moon);
        let mut h = window();
        let app = h.state_mut();
        app.link.connection = printer::Connection { kind: lamina_printer::Kind::Moonraker, address, ..Default::default() };
        app.link.watch = true;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !matches!(h.state().link.status, Some(Ok(_))) {
            assert!(std::time::Instant::now() < until, "no answer: {:?}", h.state().link.status);
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(h.state().link.status.as_ref().unwrap().as_ref().unwrap().line(), "printing part.gcode, 50 %");
    }

    #[test]
    fn the_plate_is_sliced_and_sent_to_the_printer() {
        let (address, got) = lamina_printer::testing::printer(201, "{}");
        let mut h = window();
        press(&mut h, "Add box");
        let app = h.state_mut();
        app.send_to_printer(true);
        assert!(app.failed && app.status.contains("address"), "no address yet: {}", app.status);
        app.link.connection = printer::Connection { kind: lamina_printer::Kind::Octoprint, address: address.clone(), ..Default::default() };
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

    #[test]
    fn a_prusalink_with_a_user_name_is_sent_its_digest_answer() {
        let (address, got) = lamina_printer::testing::serve(|g| match g.header("Authorization") {
            Some(_) => (201, Vec::new(), String::new()),
            None => (401, vec![("WWW-Authenticate".into(), r#"Digest realm="Printer API", nonce="n1""#.into())], String::new()),
        });
        // Tall enough that Printer connection is on the screen, where its labels can be found.
        let app = App::new(session(), Arc::new(lamina_assistant::testing::Fake::default()));
        let mut h = Harness::builder().with_size([1400.0, 2000.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        press(&mut h, "Add box");
        h.get_by_label("Printer connection").click();
        h.run();
        h.get_by_label("Address");
        assert!(h.query_by_label("User").is_none(), "Moonraker asks for no user name");
        let app = h.state_mut();
        app.link.connection =
            printer::Connection { kind: lamina_printer::Kind::PrusaLink, address, user: "maker".into(), ..Default::default() };
        app.link.key = "pass word".into();
        h.run();
        h.get_by_label("User");
        h.get_by_label("Password");
        let app = h.state_mut();
        app.send_to_printer(false);
        app.wait();
        assert!(!app.failed, "{}", app.status);
        let got = got.lock().unwrap();
        let auth = got[1].header("Authorization").unwrap();
        assert!(auth.contains(r#"username="maker""#) && !auth.contains("pass word"), "{auth}");
    }

    #[test]
    fn a_printer_on_a_usb_cable_is_only_printed_on_at_a_port() {
        let mut h = window();
        press(&mut h, "Add box");
        let app = h.state_mut();
        let kind = lamina_printer::Kind::Serial;
        app.link.connection = printer::Connection { kind, address: "printer.local".into(), baud: Some(250_000), ..Default::default() };
        app.send_to_printer(false);
        app.wait();
        assert!(app.failed && app.status.contains("Send and print"), "{}", app.status);
        app.send_to_printer(true);
        app.wait();
        assert!(app.failed && app.status.contains("not a serial port"), "{}", app.status);
        let printer = lamina_printer::Printer { kind, address: "/dev/ttyUSB7".into(), ..Default::default() };
        assert_eq!(lamina_printer::status(&printer).unwrap().state, "not printing from Lamina", "the port is not opened to ask");
    }

    /// The window's 3D view on a real graphics card, through egui's own renderer: the card's picture shown in the window, a click
    /// picked by reading the card back, the processor when View asks for it, and the card again after.
    #[test]
    #[ignore = "needs a graphics card: cargo test -p lamina -- --ignored the_window_draws_on_the_card"]
    fn the_window_draws_on_the_card() {
        let setup = eframe::egui_wgpu::WgpuSetup::CreateNew(eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle());
        let state = egui_kittest::wgpu::create_render_state(setup, Default::default());
        let mut app = App::new(session(), Arc::new(lamina_assistant::testing::Fake::default()));
        app.view.card.state = Some(state.clone());
        let mut h = Harness::builder()
            .with_size([1400.0, 860.0])
            .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(state))
            .build_ui_state(|ui, app: &mut App| app.show(ui), app);
        h.run();
        press(&mut h, "Add box");
        h.state_mut().selected.clear();
        h.state_mut().view.camera =
            view3d::Camera { target: [0.0, 0.0, 10.0], azimuth: -60.0, elevation: 30.0, distance: 200.0, ortho: false };
        h.run();
        assert!(h.state().view.on_card() && !h.state().failed, "{}", h.state().status);
        let middle = h.state().view.rect.unwrap().center();
        let colour = |h: &mut Harness<App>| {
            let image = h.render().unwrap();
            // The box's left face, clear of the pointer and the gizmo at the middle.
            let p = image.get_pixel(middle.x as u32 - 60, middle.y as u32 + 60).0;
            [p[0], p[1], p[2]]
        };
        assert!(colour(&mut h)[2] > colour(&mut h)[0], "the box in the model colour: {:?}", colour(&mut h));
        h.hover_at(middle);
        h.run();
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: middle,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
            h.run();
        }
        assert_eq!(h.state().selected, [1], "picked by reading the card back");
        let picked = colour(&mut h);
        assert!(picked[0] > picked[2], "drawn in the highlight: {picked:?}");
        h.state_mut().view.card.cpu = true;
        h.run();
        assert!(!h.state().view.on_card());
        let cpu = colour(&mut h);
        assert!(picked.iter().zip(cpu).all(|(a, b)| a.abs_diff(b) <= 6), "the processor draws it alike: {picked:?} {cpu:?}");
        h.state_mut().view.card.cpu = false;
        h.run();
        assert!(h.state().view.on_card());
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
        // The same window in each translation, to see that its longer words still fit.
        for language in lang::Lang::ALL.into_iter().skip(1) {
            h.state_mut().lang = language;
            h.run();
            h.render().unwrap().save(out.replace(".png", &format!("-{}.png", language.code()))).unwrap();
        }
        h.state_mut().lang = lang::Lang::English;
        h.run();
        // The layer heights strip with adaptive layers, and the paint tool with the box painted.
        h.state_mut().strip.open = true;
        h.run();
        press(&mut h, "Adaptive");
        press(&mut h, "Paint");
        h.state_mut().run("paint", json!({"id": 1, "kind": "seam", "at": [0, -10, 10], "radius": 4}));
        h.state_mut().run("paint", json!({"id": 1, "kind": "block", "at": [0, 0, 20], "radius": 4}));
        h.run();
        h.render().unwrap().save(out.replace(".png", "-tools.png")).unwrap();
        h.state_mut().strip.open = false;
        h.run();
        press(&mut h, "Slice");
        h.render().unwrap().save(out.replace(".png", "-preview.png")).unwrap();
    }

    /// Pictures of the menus, the dialogs and every section open, in each language, to see that the longer words of a translation
    /// still fit: `target/lamina-ui/<language>-<what>.png`, or under `LAMINA_PICTURES`. Ignored, as it needs a graphics adapter
    /// (Mesa's lavapipe will do: `cargo test -p lamina --lib -- --ignored pictures_of_menus_and_dialogs`).
    #[test]
    #[ignore = "needs a graphics adapter"]
    fn pictures_of_menus_and_dialogs() {
        let dir = std::path::PathBuf::from(std::env::var("LAMINA_PICTURES").unwrap_or_else(|_| "target/lamina-ui".into()));
        std::fs::create_dir_all(&dir).unwrap();
        for language in lang::Lang::ALL {
            let app = App::new(session(), Arc::new(lamina_assistant::testing::Fake::default()));
            let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
            h.state_mut().lang = language;
            h.run();
            let shot = |h: &mut Harness<App>, what: &str| {
                h.run();
                h.render().unwrap().save(dir.join(format!("{}-{what}.png", language.code()))).unwrap();
            };
            // `t` reads the language the window last drew in, so each label is clicked by its translation.
            let click = |h: &mut Harness<App>, english: &str| {
                let label = t(english).to_string();
                h.get_by_label(&label).click();
                h.run();
            };
            let close = |h: &mut Harness<App>| {
                h.key_press(Key::Escape);
                h.run();
            };
            click(&mut h, "Add box");
            click(&mut h, "Add cylinder");
            for menu in ["File", "Edit", "View"] {
                click(&mut h, menu);
                shot(&mut h, &menu.to_lowercase());
                if menu == "View" {
                    h.get_by_label_contains(t("Keyboard shortcuts")).hover();
                    shot(&mut h, "shortcuts");
                }
                close(&mut h);
            }
            click(&mut h, "More shapes");
            shot(&mut h, "shapes");
            close(&mut h);
            let row = h.state().session.scene.objects[0].name.clone();
            h.get_by_label_contains(&format!("{row}  ")).click_secondary();
            shot(&mut h, "object-menu");
            close(&mut h);
            // Every section of the plate panel open, scrolled down a screen at a time.
            for section in ["This object's settings", "Cut into pieces", "Printer connection", "Agents"] {
                click(&mut h, section);
            }
            for (k, group) in [
                "Speeds",
                "Retraction and travel",
                "Skirt",
                "Cooling",
                "Surface",
                "Support details",
                "Extruders",
                "One at a time",
                "Machine",
            ]
            .into_iter()
            .chain(["Start and end G-code"])
            .enumerate()
            {
                h.get_by_label(t(group)).scroll_to_me();
                h.run();
                click(&mut h, group);
                if k % 3 == 2 {
                    shot(&mut h, &format!("settings-{}", k / 3));
                }
            }
            shot(&mut h, "settings-end");
            h.state_mut().ask = Some(files::Pending::Close);
            shot(&mut h, "unsaved");
            h.state_mut().ask = None;
            h.state_mut().recovery = Some(PathBuf::from("autosave.lamina"));
            shot(&mut h, "recovery");
            h.state_mut().recovery = None;
            h.get_by_label(t("Slice")).click();
            h.step();
            h.state_mut().wait();
            shot(&mut h, "preview");
        }
    }
}
