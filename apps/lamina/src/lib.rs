//! Lamina's desktop window: the plate in 3D, the objects and their tools, the print settings, the slice and its preview, and the
//! assistant. A thin shell over the core's `Session`: it reads `session.scene` and changes it only through `Session::execute`, so
//! whatever a button does, a script, the MCP server or the assistant does the same way.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod chat;
pub mod preview;
pub mod view3d;

use std::sync::Arc;

use egui::{Color32, Key, Panel, RichText, ScrollArea, Ui};
use lamina_assistant::net::Net;
use lamina_core::{Mesh, Session};
use lamina_slicer::profile::Profile;
use serde_json::{Value, json};

use crate::view3d::Pick;

/// A session with every command: the core's, the slicer's and the booleans'.
pub fn session() -> Session {
    let mut s = Session::new();
    s.register(lamina_slicer::COMMANDS);
    s.register(lamina_csg::COMMANDS);
    s
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Prepare,
    Preview,
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
        }
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

    fn prune(&mut self) {
        let alive: Vec<u32> = self.session.scene.objects.iter().map(|o| o.id).collect();
        self.selected.retain(|id| alive.contains(id));
    }

    /// Puts an STL file on the plate and selects it.
    pub fn open(&mut self, path: &str) {
        if let Some(id) = self.run("import_mesh", json!({ "path": path })).and_then(|v| v["id"].as_u64()) {
            self.selected = vec![id as u32];
            self.frame_all();
        }
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
        if self.chat.poll(&ctx, &mut self.session) {
            self.prune();
        }
        self.keys(&ctx);
        Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
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
            for id in self.selected.clone() {
                self.run("delete", json!({ "id": id }));
            }
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
            if ui.button("Import STL...").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("STL", &["stl"]).pick_file()
            {
                self.open(&p.to_string_lossy());
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
        if rows.is_empty() {
            ui.label(RichText::new("Nothing on the plate yet: add a shape, import an STL, or ask the assistant.").weak());
        }
        for (id, text) in rows {
            if ui.selectable_label(self.selected.contains(&id), text).clicked() {
                self.pick(id, ui);
            }
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
            }
        }
        ui.separator();
        self.settings(ui);
    }

    fn boolean(&mut self, command: &str, params: Value) {
        if let Some(id) = self.run(command, params).and_then(|v| v["id"].as_u64()) {
            self.selected = vec![id as u32];
        }
    }

    fn one_object(&mut self, ui: &mut Ui, id: u32) {
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

    fn settings(&mut self, ui: &mut Ui) {
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
        match self.view.show(ui, &scene, &self.selected, self.profile.bed) {
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
                ui.label(RichText::new("The plate changed since this slice: slice again.").color(Color32::from_rgb(230, 180, 90)));
            }
        });
        for w in &sliced.warnings {
            ui.label(RichText::new(w).color(Color32::from_rgb(230, 180, 90)));
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
