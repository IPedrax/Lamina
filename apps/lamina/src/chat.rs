//! The assistant panel: a chat with a model of the user's choice (Anthropic, OpenAI or Google) that works on the plate through the
//! commands. The chat, its provider clients and its tools are `lamina-assistant`'s; this panel holds the provider, model and key, says
//! what goes to the provider, and runs the chat's turns in the frame ([`ChatPanel::poll`]), where what the model asks for runs on the
//! window's session as a click on a button does.
//!
//! The key comes from the provider's own environment variable, else from the box, and is kept for this session only, never written to
//! disk. shortcut: no "remember my key"; BIMCraft's sealed store (`seal.rs`, already ported) is the way to add one.

use std::sync::Arc;

use egui::{Color32, Key, RichText, ScrollArea, TextEdit, Ui};
use lamina_assistant::net::Net;
use lamina_assistant::{Chat, Line, Provider, Status};
use lamina_core::Session;
use lamina_core::bridge::Access;

pub struct ChatPanel {
    pub chat: Chat,
    key: String,
    pub input: String,
    woke: bool,
}

impl ChatPanel {
    pub fn new(net: Arc<dyn Net>) -> ChatPanel {
        ChatPanel { chat: Chat::new(net), key: String::new(), input: String::new(), woke: false }
    }

    /// Takes the chat's turn in this frame: a reply that came is read, and what it asks for runs on `session`. True when the scene may
    /// have changed.
    pub fn poll(&mut self, ctx: &egui::Context, session: &mut Session) -> bool {
        if !self.woke {
            let ctx = ctx.clone();
            self.chat.wake = Arc::new(move || ctx.request_repaint());
            self.woke = true;
        }
        self.chat.poll(session, Access::Edit)
    }

    /// The key: the provider's environment variable when it is set, else what was typed.
    fn key(&self) -> String {
        std::env::var(self.chat.provider.env()).ok().filter(|k| !k.trim().is_empty()).unwrap_or_else(|| self.key.trim().to_string())
    }

    pub fn show(&mut self, ui: &mut Ui) {
        ui.heading("Assistant");
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("provider").selected_text(self.chat.provider.name()).show_ui(ui, |ui| {
                for p in Provider::ALL {
                    if ui.selectable_label(self.chat.provider == p, p.name()).clicked() && self.chat.provider != p {
                        self.chat.provider = p;
                        self.chat.model = p.default_model().into();
                    }
                }
            });
            egui::ComboBox::from_id_salt("model").selected_text(self.chat.model.as_str()).show_ui(ui, |ui| {
                for m in self.chat.provider.models() {
                    ui.selectable_value(&mut self.chat.model, (*m).to_string(), *m);
                }
            });
        });
        let env = self.chat.provider.env();
        if std::env::var(env).is_ok_and(|k| !k.trim().is_empty()) {
            ui.label(RichText::new(format!("Using the key in {env}.")).small());
        } else {
            ui.add(TextEdit::singleline(&mut self.key).password(true).hint_text(format!("{} API key", self.chat.provider.name())));
        }
        ui.label(
            RichText::new(format!("Your messages and what is on the plate go to {} under your key.", self.chat.provider.name()))
                .small()
                .weak(),
        );
        ui.separator();
        let input_height = 96.0;
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .max_height((ui.available_height() - input_height).max(80.0))
            .show(ui, |ui| {
                if self.chat.log.is_empty() {
                    ui.label(
                        RichText::new("Ask for a part, a change or a print setting: \"a 30 mm cube with a 6 mm hole through it\".").weak(),
                    );
                }
                for line in &self.chat.log {
                    match line {
                        Line::You(t) => {
                            ui.label(RichText::new(t).strong());
                        }
                        Line::Files(names) => {
                            ui.label(RichText::new(format!("with {}", names.join(", "))).small().weak());
                        }
                        Line::Thinks(t) => {
                            ui.label(RichText::new(t).italics().weak());
                        }
                        Line::Says(t) => {
                            ui.label(t);
                        }
                        Line::Used { name, args, result, error } => {
                            let color = if *error { Color32::from_rgb(230, 110, 100) } else { Color32::GRAY };
                            ui.label(RichText::new(format!("{name} {args} -> {result}")).monospace().small().color(color));
                        }
                        Line::Note(t) => {
                            ui.label(RichText::new(t).color(Color32::from_rgb(230, 180, 90)));
                        }
                    }
                    ui.add_space(4.0);
                }
                if self.chat.busy() {
                    for todo in &self.chat.todos {
                        let mark = match todo.status {
                            Status::Completed => "[x]",
                            Status::InProgress => "[>]",
                            Status::Pending => "[ ]",
                        };
                        ui.label(RichText::new(format!("{mark} {}", todo.content)).small());
                    }
                    ui.spinner();
                }
            });
        ui.separator();
        let edit =
            ui.add(TextEdit::multiline(&mut self.input).desired_rows(2).desired_width(f32::INFINITY).hint_text("Message (Enter sends)"));
        let enter = edit.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
        ui.horizontal(|ui| {
            let busy = self.chat.busy();
            if (ui.add_enabled(!busy, egui::Button::new("Send")).clicked() || (enter && !busy)) && !self.input.trim().is_empty() {
                let text = std::mem::take(&mut self.input);
                self.chat.send(&text, &self.key());
            }
            if ui.add_enabled(busy, egui::Button::new("Stop")).clicked() {
                self.chat.stop();
            }
            if ui.add_enabled(!busy, egui::Button::new("Clear")).clicked() {
                self.chat.clear();
            }
        });
    }
}
