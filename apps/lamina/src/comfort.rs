//! Comfort: the keyboard shortcuts, the right-click menu on objects (in the 3D view and the plate list), the Edit menu with the undo
//! history, and the View menu with the theme and the language. Each runs the commands a button would.

use egui::{Key, KeyboardShortcut, Modifiers, Ui};
use lamina_i18n::ui_msg;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::App;
use crate::lang::{Lang, t, tf};
use crate::view3d::Tool;

/// The window's theme: dark (the default), light, or whichever the system uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Look {
    #[default]
    Dark,
    Light,
    System,
}

impl Look {
    fn preference(self) -> egui::ThemePreference {
        match self {
            Look::Dark => egui::ThemePreference::Dark,
            Look::Light => egui::ThemePreference::Light,
            Look::System => egui::ThemePreference::System,
        }
    }
}

/// The single-key shortcuts, as the View menu lists them (the keys as the keyboard names them, what they do translated).
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("G, R, S", ui_msg!("Over the 3D view: move, turn or scale the picked objects with the pointer, as Blender does")),
    ("X, Y, Z", ui_msg!("Then keep to one axis (again to free it); type an amount; Enter or a click applies, Escape puts back")),
    ("Ctrl", ui_msg!("While moving, turning or scaling: snap to steps")),
    ("1, 3, 7", ui_msg!("Over the 3D view: look from the front, the right, the top; with Ctrl, the back, the left, the bottom")),
    ("Home, .", ui_msg!("Frame everything, or the picked objects")),
    ("Alt+G, Alt+R, Alt+S", ui_msg!("Clear the picked objects' place, turn or scale")),
    ("H, Alt+H", ui_msg!("Hide the picked objects from the 3D view, or show every hidden one; they still print")),
    ("B", ui_msg!("Over the 3D view: drag a box, and the objects seen in it join the picked ones")),
    ("5", ui_msg!("Over the 3D view: orthographic or perspective")),
    ("G", ui_msg!("Move tool: pick and drag, turn with the rings, scale with the square")),
    ("L", ui_msg!("Lay flat tool: click a face to turn it down")),
    ("M", ui_msg!("Measure tool: click two points")),
    ("P", ui_msg!("Paint tool: click or drag over an object to paint seams, supports or blocks")),
    ("R", ui_msg!("Away from the 3D view: turn the picked objects 90 degrees about z")),
    ("F", ui_msg!("Frame the picked objects, or everything")),
    ("A", ui_msg!("Arrange the plate")),
    ("Ctrl+D", ui_msg!("Duplicate the picked objects")),
    ("Ctrl+A", ui_msg!("Pick every object")),
    ("Delete", ui_msg!("Delete the picked objects")),
    ("Ctrl+Z, Ctrl+Y", ui_msg!("Undo, redo")),
];

const DUPLICATE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::D);
const SELECT_ALL: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::A);

impl App {
    /// The single-key shortcuts, while no text field has the keyboard.
    pub(crate) fn shortcuts(&mut self, ctx: &egui::Context) {
        // While a keyed transform lasts its keys are its own (the view reads them).
        if ctx.egui_wants_keyboard_input() || self.view.modal.is_some() {
            return;
        }
        let plain = |k: Key| ctx.input(|i| i.modifiers.is_none() && i.key_pressed(k));
        // H hides the picked objects from the view, Alt+H shows them all again, as Blender's do.
        if ctx.input(|i| i.modifiers.alt && !i.modifiers.command && !i.modifiers.shift && i.key_pressed(Key::H)) {
            self.view.hidden.clear();
            return;
        }
        if plain(Key::H) {
            self.hide_selected();
        }
        // Alt+G, Alt+R and Alt+S clear the picked objects' place, turn or scale, as Blender's do.
        for (key, what) in [(Key::G, "location"), (Key::R, "rotation"), (Key::S, "scale")] {
            if ctx.input(|i| i.modifiers.alt && !i.modifiers.command && !i.modifiers.shift && i.key_pressed(key)) {
                let commands = self.selected.iter().map(|&id| ("reset_transform", json!({ "id": id, "what": what }))).collect();
                self.run_all("reset_transform", commands);
                return;
            }
        }
        // G, R and S with the pointer over the 3D view and objects picked: a transform that follows the pointer, as Blender's.
        let over = ctx.input(|i| i.pointer.hover_pos()).filter(|p| self.view.rect.is_some_and(|r| r.contains(*p)));
        if let Some(start) = over
            && !self.selected.is_empty()
        {
            use crate::view3d::Keyed;
            for (key, kind) in [(Key::G, Keyed::Move), (Key::R, Keyed::Turn), (Key::S, Keyed::Scale)] {
                if plain(key)
                    && let Some(at) = self.middle_of_selected()
                {
                    self.view.start_keyed(kind, self.selected.clone(), at, start);
                    return;
                }
            }
        }
        if ctx.input_mut(|i| i.consume_shortcut(&DUPLICATE)) {
            self.duplicate_selected();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&SELECT_ALL)) {
            self.selected = self.session.scene.objects.iter().map(|o| o.id).collect();
        }
        for (key, tool) in [(Key::G, Tool::Move), (Key::L, Tool::LayFlat), (Key::M, Tool::Measure), (Key::P, Tool::Paint)] {
            if plain(key) && self.view.tool != tool {
                self.view.tool = tool;
                self.view.marks.clear();
            }
        }
        // Blender's view keys (on the number row too, as Blender's numpad emulation has them): front, right and top, and with Ctrl
        // the other side; the camera keeps its target and distance.
        if over.is_some() && plain(Key::B) {
            self.view.boxing = Some(None);
            self.status = t("Drag a box over the objects to pick; Escape gives up.").into();
        }
        if over.is_some() && plain(Key::Num5) {
            self.view.camera.ortho = !self.view.camera.ortho;
        }
        if over.is_some() {
            for (key, azimuth, elevation, opposite) in
                [(Key::Num1, -90.0, 0.0, (90.0, 0.0)), (Key::Num3, 0.0, 0.0, (180.0, 0.0)), (Key::Num7, -90.0, 89.0, (-90.0, -89.0))]
            {
                let (plain_key, ctrl_key) = ctx.input(|i| {
                    let down = i.key_pressed(key);
                    (down && i.modifiers.is_none(), down && i.modifiers.command && !i.modifiers.shift && !i.modifiers.alt)
                });
                let set = if plain_key {
                    Some((azimuth, elevation))
                } else if ctrl_key {
                    Some(opposite)
                } else {
                    None
                };
                if let Some((a, e)) = set {
                    (self.view.camera.azimuth, self.view.camera.elevation) = (a, e);
                }
            }
        }
        if plain(Key::Home) {
            self.frame_all();
        }
        if plain(Key::Period) {
            self.frame_selected();
        }
        if plain(Key::R) {
            self.turn_selected(90.0);
        }
        if plain(Key::F) {
            self.frame_selected();
        }
        if plain(Key::A) {
            self.arrange();
        }
    }

    /// The middle of the picked objects' bounds, where a transform turns and scales them about.
    fn middle_of_selected(&self) -> Option<[f64; 3]> {
        self.selected
            .iter()
            .filter_map(|&id| self.session.scene.get(id).ok().and_then(|o| o.bounds()))
            .reduce(|(alo, ahi), (blo, bhi)| {
                ([alo[0].min(blo[0]), alo[1].min(blo[1]), alo[2].min(blo[2])], [ahi[0].max(bhi[0]), ahi[1].max(bhi[1]), ahi[2].max(bhi[2])])
            })
            .map(|(lo, hi)| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0])
    }

    /// Turns every picked object `degrees` about z, as one undo step.
    pub fn turn_selected(&mut self, degrees: f64) {
        let commands = self.selected.iter().map(|&id| ("rotate", json!({ "id": id, "axis": "z", "degrees": degrees }))).collect();
        self.run_all("rotate", commands);
    }

    /// A copy of each picked object, as one undo step; the copies are picked.
    pub fn duplicate_selected(&mut self) {
        if self.selected.is_empty() || self.waits() {
            return;
        }
        let ids = self.selected.clone();
        let result = self.session.transaction("duplicate", |s| {
            let mut copies = Vec::new();
            for id in ids {
                let v = s.execute("duplicate", &json!({ "id": id }))?;
                copies.extend(v["ids"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(|i| i as u32));
            }
            Ok(copies)
        });
        match result {
            Ok(copies) => (self.selected, self.failed, self.status) = (copies, false, String::new()),
            Err(e) => (self.failed, self.status) = (true, e),
        }
    }

    /// Looks at the picked objects, or at everything when none is picked.
    pub fn frame_selected(&mut self) {
        let bounds = self.selected.iter().filter_map(|&id| self.session.scene.get(id).ok().and_then(|o| o.bounds())).reduce(
            |(alo, ahi), (blo, bhi)| {
                ([alo[0].min(blo[0]), alo[1].min(blo[1]), alo[2].min(blo[2])], [ahi[0].max(bhi[0]), ahi[1].max(bhi[1]), ahi[2].max(bhi[2])])
            },
        );
        match bounds {
            Some(b) => self.view.camera.frame_all(b),
            None => self.frame_all(),
        }
    }

    /// Lays every object out on the plate, none touching.
    pub fn arrange(&mut self) {
        let bed = self.profile.bed;
        if let Some(v) = self.run("arrange", json!({ "bed": bed })) {
            if v["fits"] == false {
                (self.failed, self.status) = (true, t("They do not all fit the plate: some stand off it.").into());
            }
            self.frame_all();
        }
    }

    /// Takes back `steps` steps (or brings back, with `redo`), each one the `undo` or `redo` command.
    pub fn step_history(&mut self, steps: usize, redo: bool) {
        for _ in 0..steps {
            if self.run(if redo { "redo" } else { "undo" }, json!({})).is_none() {
                break;
            }
        }
    }

    pub(crate) fn edit_menu(&mut self, ui: &mut Ui) {
        ui.menu_button(t("Edit"), |ui| {
            let (undo, redo) = self.session.history();
            let (undo, redo): (Vec<String>, Vec<String>) =
                (undo.into_iter().map(str::to_string).collect(), redo.into_iter().map(str::to_string).collect());
            let undo_text = undo.first().map_or(t("Undo").to_string(), |l| tf("Undo {}", &[l]));
            if ui.add_enabled(!undo.is_empty(), egui::Button::new(undo_text).shortcut_text("Ctrl+Z")).clicked() {
                self.step_history(1, false);
            }
            let redo_text = redo.first().map_or(t("Redo").to_string(), |l| tf("Redo {}", &[l]));
            if ui.add_enabled(!redo.is_empty(), egui::Button::new(redo_text).shortcut_text("Ctrl+Y")).clicked() {
                self.step_history(1, true);
            }
            ui.add_enabled_ui(!undo.is_empty() || !redo.is_empty(), |ui| {
                ui.menu_button(t("History"), |ui| {
                    // Newest at the top: what redo would bring back, then the steps undo would take back.
                    for (k, label) in redo.iter().enumerate().rev() {
                        if ui.button(format!("\u{21b7} {label}")).on_hover_text(t("Bring back up to this step")).clicked() {
                            self.step_history(k + 1, true);
                        }
                    }
                    ui.label(egui::RichText::new(t("now")).weak());
                    for (k, label) in undo.iter().enumerate() {
                        if ui.button(format!("\u{21b6} {label}")).on_hover_text(t("Take back this step and those after it")).clicked() {
                            self.step_history(k + 1, false);
                        }
                    }
                });
            });
            ui.separator();
            let any = !self.selected.is_empty();
            if ui.add_enabled(any, egui::Button::new(t("Duplicate")).shortcut_text(ui.ctx().format_shortcut(&DUPLICATE))).clicked() {
                self.duplicate_selected();
            }
            if ui.add_enabled(any, egui::Button::new(t("Delete")).shortcut_text("Delete")).clicked() {
                self.delete_selected();
            }
            if ui.add(egui::Button::new(t("Pick all")).shortcut_text(ui.ctx().format_shortcut(&SELECT_ALL))).clicked() {
                self.selected = self.session.scene.objects.iter().map(|o| o.id).collect();
            }
        });
    }

    /// Hides the picked objects from the 3D view, and unpicks them.
    pub(crate) fn hide_selected(&mut self) {
        for id in std::mem::take(&mut self.selected) {
            if !self.view.hidden.contains(&id) {
                self.view.hidden.push(id);
            }
        }
        if !self.view.hidden.is_empty() {
            self.status = tf("{} hidden; Alt+H shows them.", &[&self.view.hidden.len().to_string()]);
        }
    }

    pub(crate) fn view_menu(&mut self, ui: &mut Ui) {
        ui.menu_button(t("View"), |ui| {
            if ui.add(egui::Button::new(t("Frame")).shortcut_text("F")).on_hover_text(t("The picked objects, or everything")).clicked() {
                self.frame_selected();
            }
            ui.checkbox(&mut self.view.camera.ortho, t("Orthographic"))
                .on_hover_text(t("Parallel lines stay parallel, as on a drawing (5)"));
            let any = !self.selected.is_empty();
            if ui.add_enabled(any, egui::Button::new(t("Hide picked")).shortcut_text("H")).clicked() {
                self.hide_selected();
            }
            if ui.add_enabled(!self.view.hidden.is_empty(), egui::Button::new(t("Show all")).shortcut_text("Alt+H")).clicked() {
                self.view.hidden.clear();
            }
            ui.checkbox(&mut self.strip.open, t("Layer heights by hand"))
                .on_hover_text(t("A strip beside the 3D view to make layers thinner or thicker by height"));
            ui.separator();
            ui.label(t("Theme"));
            for (look, name) in [(Look::Dark, t("Dark")), (Look::Light, t("Light")), (Look::System, t("As the system"))] {
                ui.radio_value(&mut self.look, look, name);
            }
            ui.separator();
            // Each language by its own name, so whoever reads it finds theirs.
            ui.label(t("Language"));
            for language in Lang::ALL {
                ui.radio_value(&mut self.lang, language, language.label());
            }
            ui.separator();
            use crate::view3d::{Orientation, Pivot};
            ui.label(t("Turn and scale about"));
            ui.radio_value(&mut self.view.pivot, Pivot::Each, t("Each object's own middle"));
            ui.radio_value(&mut self.view.pivot, Pivot::Middle, t("The middle of all picked"));
            ui.label(t("Move and turn along"));
            ui.radio_value(&mut self.view.orientation, Orientation::Plate, t("The plate's axes"));
            ui.radio_value(&mut self.view.orientation, Orientation::Own, t("The picked object's own axes"));
            ui.separator();
            let processor = ui
                .checkbox(&mut self.view.card.cpu, t("Draw the 3D view on the processor"))
                .on_hover_text(t("Slower with large meshes. The graphics card draws it otherwise, where the window has one."));
            if processor.changed() {
                self.view.card.retry();
            }
            ui.separator();
            ui.menu_button(t("Keyboard shortcuts"), |ui| {
                egui::Grid::new("shortcuts").num_columns(2).show(ui, |ui| {
                    for (keys, what) in SHORTCUTS {
                        ui.label(egui::RichText::new(*keys).strong());
                        ui.label(t(what));
                        ui.end_row();
                    }
                });
            });
        });
    }

    /// Sets the theme chosen, when it changed.
    pub(crate) fn apply_look(&mut self, ctx: &egui::Context) {
        if self.applied_look != Some(self.look) {
            ctx.set_theme(self.look.preference());
            self.applied_look = Some(self.look);
        }
    }

    /// The right-click menu: what can be done to the picked objects, or to the plate when none is picked.
    pub(crate) fn object_menu(&mut self, ui: &mut Ui) {
        if self.selected.is_empty() {
            if ui.button(t("Add box")).clicked() {
                self.add_box();
                ui.close();
            }
            if ui.add(egui::Button::new(t("Arrange")).shortcut_text("A")).clicked() {
                self.arrange();
                ui.close();
            }
            if ui.add(egui::Button::new(t("Frame all")).shortcut_text("F")).clicked() {
                self.frame_all();
                ui.close();
            }
            return;
        }
        let ids = self.selected.clone();
        let did = |ui: &mut Ui, app: &mut App, label: &str, keys: &str, f: &dyn Fn(&mut App)| {
            if ui.add(egui::Button::new(label).shortcut_text(keys)).clicked() {
                f(app);
                ui.close();
            }
        };
        did(ui, self, t("Duplicate"), "Ctrl+D", &|app| app.duplicate_selected());
        did(ui, self, t("Turn 90 degrees"), "R", &|app| app.turn_selected(90.0));
        did(ui, self, t("Mirror left to right"), "", &|app| {
            let commands = app.selected.iter().map(|&id| ("mirror", json!({ "id": id, "axis": "x" }))).collect();
            app.run_all("mirror", commands);
        });
        did(ui, self, t("Drop to plate"), "", &|app| {
            let commands = app.selected.iter().map(|&id| ("drop_to_bed", json!({ "id": id }))).collect();
            app.run_all("drop_to_bed", commands);
        });
        did(ui, self, t("Lay flat on a face..."), "L", &|app| app.view.tool = Tool::LayFlat);
        did(ui, self, t("Frame"), "F", &|app| app.frame_selected());
        if ids.iter().any(|&id| self.problems_of(id).is_some()) {
            did(ui, self, t("Repair"), "", &|app| {
                let commands = app.selected.iter().map(|&id| ("repair", json!({ "id": id }))).collect();
                app.run_all("repair", commands);
            });
        }
        ui.separator();
        did(ui, self, t("Delete"), "Delete", &|app| app.delete_selected());
    }

    /// A right click on object `id` (in the view or the list) picks it, unless it is picked already with others.
    pub(crate) fn context_pick(&mut self, id: Option<u32>) {
        match id {
            Some(id) if !self.selected.contains(&id) => self.selected = vec![id],
            Some(_) => {}
            None => self.selected.clear(),
        }
    }
}
