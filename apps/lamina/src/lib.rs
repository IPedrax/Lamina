//! Lamina's desktop window: the plate in 3D, the objects and their tools, the print settings, the slice and its preview, and the
//! assistant. A thin shell over the core's `Session`: it reads `session.scene` and changes it only through `Session::execute`, so
//! whatever a button does, a script, the MCP server or the assistant does the same way.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod chat;
pub mod files;
pub mod preview;
pub mod printer;
pub mod settings;
pub mod view3d;

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, Key, Panel, RichText, ScrollArea, Ui};
use lamina_assistant::net::Net;
use lamina_core::{Mesh, Scene, Session};
use lamina_slicer::presets::{self, FILAMENT_FIELDS, FILAMENTS, PRINTER_FIELDS, PRINTERS};
use lamina_slicer::profile::Profile;
use serde_json::{Value, json};

use crate::settings::{Presets, Settings};
use crate::view3d::Pick;

/// The colour of a warning: a mesh with problems, a plate changed since its slice.
const WARNING: Color32 = Color32::from_rgb(230, 180, 90);

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

/// The two kinds of preset: a printer sets the machine's fields, a filament its temperatures and fan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Printer,
    Filament,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Printer => "printer",
            Kind::Filament => "filament",
        }
    }

    fn fields(self) -> &'static [&'static str] {
        match self {
            Kind::Printer => PRINTER_FIELDS,
            Kind::Filament => FILAMENT_FIELDS,
        }
    }

    fn built_in(self) -> &'static [presets::Preset] {
        match self {
            Kind::Printer => PRINTERS,
            Kind::Filament => FILAMENTS,
        }
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
    pub tab: Tab,
    pub profile: Profile,
    pub sliced: Option<Sliced>,
    pub layer: usize,
    pub travels: bool,
    pub status: String,
    pub failed: bool,
    nudge: [f64; 3],
    axis: usize,
    degrees: f64,
    factor: f64,
    pub chat: chat::ChatPanel,
    /// The presets last applied, as labels, and the ones the user saved.
    pub printer: Option<String>,
    pub filament: Option<String>,
    pub my_printers: Presets,
    pub my_filaments: Presets,
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
    /// What is wrong with each mesh, by the address of its `Arc`, worked out once and kept while the mesh lives.
    problems: std::collections::HashMap<usize, lamina_core::repair::Problems>,
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
            axis: 2,
            degrees: 90.0,
            factor: 1.5,
            chat: chat::ChatPanel::new(net),
            printer: None,
            filament: None,
            my_printers: Presets::new(),
            my_filaments: Presets::new(),
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
        let (panel, note) = printer::PrinterPanel::restore(s.connection.clone(), s.connection_key.clone());
        app.link = panel;
        notes.extend(note);
        notes.extend(app.chat.restore(s.keys.clone()));
        if !notes.is_empty() {
            (app.failed, app.status) = (true, notes.join(" "));
        }
        app.saved = Some(app.settings());
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
            keys: self.chat.saved.clone(),
            recent: self.recent.clone(),
            connection: Some(self.link.connection.clone()).filter(|c| !c.address.trim().is_empty()),
            connection_key: self.link.sealed.clone(),
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
        }
    }

    /// Applies a preset by name, the user's first, then a built-in: its fields go over the settings shown, the others stay.
    pub fn apply_preset(&mut self, kind: Kind, name: &str) {
        let fields = match self.mine(kind).get(name) {
            Some(f) => Ok(f.clone()),
            None => presets::find(kind.built_in(), name).map(|p| p.fields()),
        };
        match fields.and_then(|f| presets::apply(&self.profile, &f)) {
            Ok(p) => {
                self.profile = p;
                let label = Some(name.to_string());
                match kind {
                    Kind::Printer => self.printer = label,
                    Kind::Filament => self.filament = label,
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
        let fields = presets::capture(&self.profile, kind.fields());
        self.mine(kind).insert(name.to_string(), fields);
        self.apply_preset(kind, name);
        self.status = format!("Saved the {} preset \"{name}\".", kind.word());
    }

    fn preset_picker(&mut self, ui: &mut Ui, kind: Kind) {
        let chosen = match kind {
            Kind::Printer => self.printer.clone(),
            Kind::Filament => self.filament.clone(),
        };
        let mut pick = None;
        let mine: Vec<String> = match kind {
            Kind::Printer => self.my_printers.keys().cloned().collect(),
            Kind::Filament => self.my_filaments.keys().cloned().collect(),
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
        });
    }

    /// Runs a command as a button does: an error goes to the status bar, and the selection forgets objects that are gone.
    pub fn run(&mut self, command: &str, params: Value) -> Option<Value> {
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

    /// Adds a shape `width` mm wide to the right of what is on the plate, so it does not land inside another, and selects it.
    fn add(&mut self, mut params: Value, width: f64) {
        let right = self.session.scene.objects.iter().filter_map(|o| o.bounds()).map(|(_, hi)| hi[0]).fold(f64::NEG_INFINITY, f64::max);
        if right.is_finite() {
            params["at"] = json!([right + 5.0 + width / 2.0, 0.0]);
        }
        if let Some(id) = self.run("add_primitive", params).and_then(|v| v["id"].as_u64()) {
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

    /// Slices the plate with the settings shown and opens the preview.
    pub fn slice(&mut self) {
        let objects: Vec<(u32, Mesh)> = self.session.scene.objects.iter().map(|o| (o.id, o.world())).collect();
        match lamina_slicer::print::gcode(&objects, &self.profile) {
            Ok(out) => {
                let layers = preview::parse(&out.gcode);
                let minutes = (out.seconds / 60.0).round() as u64;
                let summary = format!(
                    "{} layers, {:.2} m of filament, at least {}h {:02}m",
                    out.layers,
                    out.filament_mm / 1000.0,
                    minutes / 60,
                    minutes % 60
                );
                self.layer = layers.len() / 2;
                let scene = Arc::as_ptr(&self.session.scene) as usize;
                self.sliced = Some(Sliced { layers, summary, warnings: out.warnings, scene });
                (self.failed, self.status, self.tab) = (false, String::new(), Tab::Preview);
            }
            Err(e) => (self.failed, self.status) = (true, e),
        }
    }

    fn export(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("G-code", &["gcode"]).set_file_name("plate.gcode").save_file() else { return };
        let path = path.to_string_lossy().into_owned();
        let profile = serde_json::to_value(&self.profile).unwrap_or(Value::Null);
        if self.run("slice", json!({ "path": path, "profile": profile })).is_some() {
            self.status = format!("Wrote {path}");
        }
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
        Panel::top("toolbar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| self.file_menu(ui));
            self.toolbar(ui);
        });
        Panel::bottom("status").show(ui, |ui| {
            let color = if self.failed { Color32::from_rgb(230, 110, 100) } else { Color32::GRAY };
            ui.label(RichText::new(if self.status.is_empty() { "Ready" } else { &self.status }).color(color));
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
        let title = self.title();
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        self.persist(&ctx);
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
                self.add(json!({"kind": "box", "size": [20, 20, 20]}), 20.0);
            }
            if ui.button("Add cylinder").clicked() {
                self.add(json!({"kind": "cylinder", "radius": 10, "height": 20}), 20.0);
            }
            if ui.button("Add sphere").clicked() {
                self.add(json!({"kind": "sphere", "radius": 10}), 20.0);
            }
            if ui.button("Import mesh...").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("Mesh", files::MESHES).pick_file()
            {
                self.import(&p);
            }
            ui.separator();
            if ui.button("Undo").clicked() {
                self.run("undo", json!({}));
            }
            if ui.button("Redo").clicked() {
                self.run("redo", json!({}));
            }
            if ui.button("Frame all").clicked() {
                self.frame_all();
            }
            ui.separator();
            ui.selectable_value(&mut self.tab, Tab::Prepare, "Prepare");
            ui.selectable_value(&mut self.tab, Tab::Preview, "Preview");
            ui.separator();
            if ui.button("Slice").clicked() {
                self.slice();
            }
            if ui.button("Export G-code...").clicked() {
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
                if ui.selectable_label(self.selected.contains(&id), text).clicked() {
                    self.pick(id, ui);
                }
                if let Some(p) = problems {
                    ui.label(RichText::new("\u{26a0}").color(WARNING)).on_hover_text(format!(
                        "This mesh is not a clean solid: {}. Parts of it may not print; Repair mends what it can.",
                        p.describe()
                    ));
                }
            });
        }
        ui.label(RichText::new("Ctrl-click picks several.").small().weak());
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
    }

    fn boolean(&mut self, command: &str, params: Value) {
        if let Some(id) = self.run(command, params).and_then(|v| v["id"].as_u64()) {
            self.selected = vec![id as u32];
        }
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
        });
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut self.factor).speed(0.01).range(0.01..=100.0).prefix("x "));
            if ui.button("Scale").clicked() {
                self.run("scale", json!({ "id": id, "factor": self.factor }));
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Drop to plate").clicked() {
                self.run("drop_to_bed", json!({ "id": id }));
            }
            if ui.button("Delete").clicked() {
                self.run("delete", json!({ "id": id }));
            }
        });
    }

    fn print_settings(&mut self, ui: &mut Ui) {
        egui::CollapsingHeader::new("Print settings").default_open(true).show(ui, |ui| {
            let p = &mut self.profile;
            let max_layer = p.nozzle * 0.8;
            egui::Grid::new("settings").num_columns(2).show(ui, |ui| {
                ui.label("Layer height, mm");
                ui.add(egui::DragValue::new(&mut p.layer_height).speed(0.01).range(0.02..=max_layer));
                ui.end_row();
                ui.label("First layer, mm");
                ui.add(egui::DragValue::new(&mut p.first_layer_height).speed(0.01).range(0.02..=max_layer));
                ui.end_row();
                ui.label("Walls");
                ui.add(egui::DragValue::new(&mut p.perimeters).range(0..=10));
                ui.end_row();
                ui.label("Top / bottom layers");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p.top_layers).range(0..=50));
                    ui.add(egui::DragValue::new(&mut p.bottom_layers).range(0..=50));
                });
                ui.end_row();
                ui.label("Infill");
                let mut percent = p.infill_density * 100.0;
                if ui.add(egui::DragValue::new(&mut percent).speed(1.0).range(0.0..=100.0).suffix(" %")).changed() {
                    p.infill_density = percent / 100.0;
                }
                ui.end_row();
                ui.label("Nozzle / bed, C");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p.nozzle_temp).range(0.0..=400.0));
                    ui.add(egui::DragValue::new(&mut p.bed_temp).range(0.0..=400.0));
                });
                ui.end_row();
                ui.label("First layer nozzle / bed");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p.first_layer_nozzle_temp).range(0.0..=400.0));
                    ui.add(egui::DragValue::new(&mut p.first_layer_bed_temp).range(0.0..=400.0));
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
                        .on_hover_text("Where each wall starts and ends, which leaves a small mark");
                }
                ui.end_row();
                ui.label("Brim, mm / raft layers");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p.brim_width).speed(0.5).range(0.0..=50.0))
                        .on_hover_text("Loops round the first layer that hold its edges down; 0 for none");
                    ui.add(egui::DragValue::new(&mut p.raft_layers).range(0..=10))
                        .on_hover_text("Layers of lines under the part, which is lifted onto them; 0 for none");
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
                    egui::ComboBox::from_id_salt("support").selected_text(name(p.support)).show_ui(ui, |ui| {
                        for s in [Support::Off, Support::Everywhere, Support::FromPlate] {
                            ui.selectable_value(&mut p.support, s, name(s));
                        }
                    });
                    if p.support != Support::Off {
                        ui.add(egui::DragValue::new(&mut p.support_angle).range(10.0..=89.0).suffix(" deg"))
                            .on_hover_text("A surface leaning further than this from vertical is held up");
                    }
                });
                ui.end_row();
                ui.label("Plate, mm");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p.bed[0]).range(10.0..=2000.0));
                    ui.add(egui::DragValue::new(&mut p.bed[1]).range(10.0..=2000.0));
                });
                ui.end_row();
            });
            if ui.button("Back to the defaults").clicked() {
                self.profile = Profile::default();
            }
        });
    }

    fn prepare(&mut self, ui: &mut Ui) {
        let scene = Arc::clone(&self.session.scene);
        let shown = self.view.show(ui, &scene, &self.selected, self.profile.bed);
        if let Some((id, by)) = shown.moved {
            self.run("move", json!({ "id": id, "by": by }));
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
        });
        preview::show(ui, &sliced.layers, self.layer, self.profile.bed, self.profile.origin_center, self.travels);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
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

    fn press(h: &mut Harness<App>, label: &str) {
        h.get_by_label(label).click();
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
        assert!(app.failed && app.status.contains("API key"), "OctoPrint needs its key: {}", app.status);
        app.link.key = "ABCDEF0123456789".into();
        app.send_to_printer(true);
        assert!(!app.failed && app.status.contains("is printing"), "{}", app.status);
        let got = got.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].header("X-Api-Key"), Some("ABCDEF0123456789"));
        let body = got[0].body_text();
        assert!(body.contains("filename=\"plate.gcode\"") && body.contains(";LAYER:99"), "the whole slice went");
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
