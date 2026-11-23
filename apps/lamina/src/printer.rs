//! The printer connection: where the window sends a sliced plate (Moonraker or OctoPrint on the user's network), and the buttons that
//! send it. The address and kind are kept in the settings file; an API key only when the user ticks Remember, and then sealed by
//! `lamina_assistant::seal` as the assistant's key is (scrambled under the program's secret: no protection from anyone who has Lamina).
//! Sending runs the `slice` and `send_to_printer` commands, as the CLI does.

use egui::{RichText, Ui};
use lamina_assistant::seal;
use lamina_printer::Kind;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::App;
use crate::work::Done;
use lamina_core::Session;

/// What the settings file keeps of the connection: the kind and the address (the key is apart, sealed).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub kind: Kind,
    pub address: String,
}

#[derive(Default)]
pub struct PrinterPanel {
    pub connection: Connection,
    /// The key as typed, kept only in memory.
    pub key: String,
    pub remember: bool,
    /// The key as the settings file keeps it, when remembered.
    pub sealed: Option<String>,
    /// Whether the window asks the printer what it is doing, every [`EVERY`]; what it last said; the question on its way.
    pub watch: bool,
    pub status: Option<Result<lamina_printer::Status, String>>,
    asking: Option<std::sync::mpsc::Receiver<Result<lamina_printer::Status, String>>>,
    asked: Option<std::time::Instant>,
}

/// How often a watched printer is asked what it is doing.
pub const EVERY: std::time::Duration = std::time::Duration::from_secs(5);

impl PrinterPanel {
    /// The panel as the settings left it: the connection, and the key opened from its sealed form. A key that does not open is said.
    pub fn restore(connection: Option<Connection>, sealed: Option<String>) -> (PrinterPanel, Option<String>) {
        let mut p = PrinterPanel { connection: connection.unwrap_or_default(), ..PrinterPanel::default() };
        let mut note = None;
        if let Some(s) = sealed {
            match seal::open(&s) {
                Ok(k) => (p.key, p.remember, p.sealed) = (k, true, Some(s)),
                Err(e) => note = Some(format!("The saved printer key was left out: {e}")),
            }
        }
        (p, note)
    }

    /// Keeps `sealed` in step with the key and Remember.
    fn keep(&mut self) {
        let key = self.key.trim();
        let want = self.remember && !key.is_empty();
        let current = self.sealed.as_deref().and_then(|s| seal::open(s).ok());
        if !want {
            self.sealed = None;
        } else if current.as_deref() != Some(key) {
            self.sealed = seal::seal(key).ok();
        }
    }
}

impl App {
    /// Slices the plate with the settings shown and sends it to the printer; with `print`, the printer starts it.
    pub fn send_to_printer(&mut self, print: bool) {
        let c = self.link.connection.clone();
        if c.address.trim().is_empty() {
            (self.failed, self.status) = (true, "Give the printer's address in Printer connection first.".into());
            return;
        }
        let stem = self.project.as_ref().and_then(|p| p.file_stem()).map_or("plate".into(), |n| n.to_string_lossy().into_owned());
        let path = std::env::temp_dir().join(format!("lamina-send-{}-{}.gcode", std::process::id(), crate::files::clean_name(&stem)));
        let profile = serde_json::to_value(&self.profile).unwrap_or_default();
        let key = Some(self.link.key.trim()).filter(|k| !k.is_empty()).map(str::to_string);
        let slice = json!({ "path": path, "profile": profile });
        let send =
            json!({ "path": path, "printer": c.kind, "address": c.address, "key": key, "name": format!("{stem}.gcode"), "print": print });
        // Slicing and the upload both on the thread: a slow printer does not stop the window. (Cancel during the upload stops the
        // window waiting, not the upload.)
        let work = move |s: &mut Session| {
            let sent = s.execute("slice", &slice).and_then(|_| s.execute("send_to_printer", &send));
            let _ = std::fs::remove_file(&path);
            sent.map(Done::Value)
        };
        self.start("Sending to the printer", false, work, move |app, done| {
            if let Ok(Done::Value(v)) = done {
                let verb = if print { "is printing" } else { "has" };
                app.status = format!("Sent {} to {}: it {verb} it.", v["name"].as_str().unwrap_or("the plate"), c.kind.name());
            }
        });
    }

    /// Once a frame: takes the printer's answer when it has come, and asks again every [`EVERY`] while the user watches. The
    /// question goes on a thread, to the address typed and nowhere else; it is not a command, so no agent can point it elsewhere.
    pub(crate) fn poll_printer(&mut self, ctx: &egui::Context) {
        let p = &mut self.link;
        if let Some(rx) = &p.asking
            && let Ok(answer) = rx.try_recv()
        {
            (p.status, p.asking) = (Some(answer), None);
        }
        if !p.watch || p.connection.address.trim().is_empty() {
            (p.asking, p.asked) = (None, None);
            return;
        }
        if p.asking.is_none() && p.asked.is_none_or(|t| t.elapsed() >= EVERY) {
            let printer = lamina_printer::Printer {
                kind: p.connection.kind,
                address: p.connection.address.clone(),
                key: Some(p.key.trim().to_string()).filter(|k| !k.is_empty()),
            };
            let (tx, rx) = std::sync::mpsc::channel();
            let wake = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(lamina_printer::status(&printer));
                wake.request_repaint();
            });
            (p.asking, p.asked) = (Some(rx), Some(std::time::Instant::now()));
        }
        ctx.request_repaint_after(EVERY);
    }

    pub(crate) fn printer_ui(&mut self, ui: &mut Ui) {
        egui::CollapsingHeader::new("Printer connection").show(ui, |ui| {
            let p = &mut self.link;
            egui::Grid::new("printer").num_columns(2).show(ui, |ui| {
                ui.label("Server");
                egui::ComboBox::from_id_salt("printer kind").selected_text(p.connection.kind.name()).show_ui(ui, |ui| {
                    for k in Kind::ALL {
                        ui.selectable_value(&mut p.connection.kind, k, k.name());
                    }
                });
                ui.end_row();
                ui.label("Address");
                ui.add(egui::TextEdit::singleline(&mut p.connection.address).hint_text("192.168.1.20 or http://printer.local:7125"));
                ui.end_row();
                ui.label("API key");
                ui.add(egui::TextEdit::singleline(&mut p.key).password(true).hint_text(match p.connection.kind {
                    Kind::Octoprint => "OctoPrint's settings, Application Keys",
                    Kind::PrusaLink => "the printer's Settings, Network, PrusaLink",
                    Kind::Duet => "the board's password, if it has one",
                    Kind::Moonraker => "only if Moonraker asks for one",
                }));
                ui.end_row();
            });
            ui.checkbox(&mut p.remember, "Remember the key").on_hover_text(
                "Kept in the settings file, sealed under a secret built into Lamina: scrambled, not protected from anyone who has Lamina.",
            );
            p.keep();
            ui.checkbox(&mut p.watch, "Show what it is doing").on_hover_text("Asks the printer every 5 seconds while ticked");
            if p.watch {
                match &p.status {
                    Some(Ok(s)) => ui.label(RichText::new(s.line()).small()),
                    Some(Err(e)) => ui.label(RichText::new(e).small().color(crate::WARNING)),
                    None => ui.label(RichText::new("Asking...").small().weak()),
                };
            }
            ui.label(RichText::new("Lamina talks only to this address: when you press Send, or watch it.").small().weak());
            ui.horizontal(|ui| {
                if ui.button("Send to printer").on_hover_text("Slice the plate and upload it").clicked() {
                    self.send_to_printer(false);
                }
                if ui.button("Send and print").on_hover_text("Slice the plate, upload it and start printing").clicked() {
                    self.send_to_printer(true);
                }
            });
        });
    }
}
