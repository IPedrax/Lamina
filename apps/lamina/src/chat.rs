//! The assistant panel: a chat with a model of the user's choice (Anthropic, OpenAI or Google) that works on the plate through the
//! commands. The chat, its provider clients and its tools are `lamina-assistant`'s; this panel holds the provider, model and key, says
//! what goes to the provider, and runs the chat's turns in the frame ([`ChatPanel::poll`]), where what the model asks for runs on the
//! window's session as a click on a button does.
//!
//! The key comes from the provider's own environment variable, else from the box. It is kept for the session, and on this computer only
//! when the user ticks Remember: then the settings file holds it sealed (`lamina_assistant::seal`), scrambled under a secret built into
//! the program by default, which the panel says plainly is not protection from anyone who has Lamina, or encrypted under a passphrase
//! the user types once a session. The passphrase itself is kept nowhere.

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{Color32, Key, RichText, ScrollArea, TextEdit, Ui};
use lamina_assistant::net::Net;
use lamina_assistant::{Chat, Line, Provider, Status, key_ok, seal};
use lamina_core::Session;
use lamina_core::bridge::Access;

pub struct ChatPanel {
    pub chat: Chat,
    /// Keys in memory for this session, by provider: typed, or opened from `saved`.
    keys: BTreeMap<&'static str, String>,
    /// Keys the user asked to be remembered, sealed, by provider (`Provider::key`): what the settings file keeps.
    pub saved: BTreeMap<String, String>,
    /// A passphrase being typed, to seal a key or open one: cleared as soon as it is used.
    passphrase: String,
    problem: Option<String>,
    pub input: String,
    woke: bool,
}

impl ChatPanel {
    pub fn new(net: Arc<dyn Net>) -> ChatPanel {
        ChatPanel {
            chat: Chat::new(net),
            keys: BTreeMap::new(),
            saved: BTreeMap::new(),
            passphrase: String::new(),
            problem: None,
            input: String::new(),
            woke: false,
        }
    }

    /// Takes the keys the settings file kept: those sealed under the built-in secret open now, those under a passphrase wait for it.
    /// Returns a note for each that does not open (saved by another build of Lamina, or changed).
    pub fn restore(&mut self, saved: BTreeMap<String, String>) -> Vec<String> {
        let mut notes = Vec::new();
        for (provider, sealed) in saved {
            let Some(p) = Provider::from_key(&provider) else { continue };
            if seal::needs_passphrase(&sealed) {
                self.saved.insert(provider, sealed);
                continue;
            }
            match seal::open(&sealed) {
                Ok(key) => {
                    self.keys.insert(p.key(), key);
                    self.saved.insert(provider, sealed);
                }
                Err(e) => notes.push(format!("The saved {} key does not open ({e}): type it again.", p.name())),
            }
        }
        notes
    }

    /// Remembers the key in the box for the provider shown: sealed under the built-in secret, or under `passphrase` when one is given.
    pub fn remember(&mut self, passphrase: Option<&str>) -> Result<(), String> {
        let p = self.chat.provider;
        let key = self.keys.get(p.key()).map(|k| k.trim().to_string()).unwrap_or_default();
        if !key_ok(&key) {
            return Err("Type a valid key first.".into());
        }
        let sealed = match passphrase {
            None => seal::seal(&key)?,
            Some(phrase) if phrase.chars().count() < seal::PASSPHRASE_MIN => {
                return Err(format!("A passphrase has at least {} characters.", seal::PASSPHRASE_MIN));
            }
            Some(phrase) => seal::seal_passphrase(phrase, &key)?,
        };
        self.saved.insert(p.key().to_string(), sealed);
        Ok(())
    }

    /// Forgets the saved key of the provider shown; the key typed this session stays until the window closes.
    pub fn forget(&mut self) {
        self.saved.remove(self.chat.provider.key());
    }

    /// Opens the saved key of the provider shown with its passphrase, for this session.
    pub fn unlock(&mut self, passphrase: &str) -> Result<(), String> {
        let p = self.chat.provider;
        let sealed = self.saved.get(p.key()).ok_or("There is no saved key to unlock.")?;
        let key = seal::open_passphrase(passphrase, sealed)?;
        self.keys.insert(p.key(), key);
        Ok(())
    }

    /// Whether the provider shown has a key saved under a passphrase that is not opened yet.
    pub fn locked(&self) -> bool {
        let p = self.chat.provider;
        self.saved.get(p.key()).is_some_and(|s| seal::needs_passphrase(s)) && self.keys.get(p.key()).is_none_or(|k| k.is_empty())
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

    /// The key: the provider's environment variable when it is set, else the one in memory.
    pub fn key(&self) -> String {
        let p = self.chat.provider;
        std::env::var(p.env())
            .ok()
            .filter(|k| !k.trim().is_empty())
            .unwrap_or_else(|| self.keys.get(p.key()).map(|k| k.trim().to_string()).unwrap_or_default())
    }

    /// The key box, Remember, and what goes with them: the passphrase to protect a saved key, or to open one.
    fn key_ui(&mut self, ui: &mut Ui) {
        let p = self.chat.provider;
        let env = p.env();
        if std::env::var(env).is_ok_and(|k| !k.trim().is_empty()) {
            ui.label(RichText::new(format!("Using the key in {env}.")).small());
            return;
        }
        if self.locked() {
            ui.label(RichText::new(format!("The saved {} key is locked: type its passphrase to use it this session.", p.name())).small());
            ui.horizontal(|ui| {
                let go = ui.add(TextEdit::singleline(&mut self.passphrase).password(true).hint_text("Passphrase").desired_width(150.0));
                let enter = go.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if ui.button("Unlock").clicked() || enter {
                    let phrase = std::mem::take(&mut self.passphrase);
                    self.problem = self.unlock(&phrase).err();
                }
                if ui.button("Forget it").clicked() {
                    self.forget();
                }
            });
        } else {
            let typed = self.keys.entry(p.key()).or_default();
            let edited = ui.add(TextEdit::singleline(typed).password(true).hint_text(format!("{} API key", p.name()))).changed();
            let sealed = self.saved.get(p.key()).cloned();
            if edited && let Some(sealed) = &sealed {
                // The key changed under a saved one: seal the new one the same way, or ask for the passphrase again.
                if seal::needs_passphrase(sealed) {
                    self.forget();
                    self.problem = Some("The key changed: tick Remember again to save it.".into());
                } else {
                    self.problem = self.remember(None).err();
                }
            }
            let mut remember = self.saved.contains_key(p.key());
            if ui.checkbox(&mut remember, "Remember on this computer").changed() {
                self.problem = if remember { self.remember(None).err() } else { (self.forget(), None).1 };
            }
            match self.saved.get(p.key()) {
                Some(s) if seal::needs_passphrase(s) => {
                    ui.label(
                        RichText::new(
                            "Saved encrypted with your passphrase (AES-256-GCM): you type it once a session. A forgotten passphrase cannot be \
                             recovered; type the key again.",
                        )
                        .small()
                        .weak(),
                    );
                }
                Some(_) => {
                    ui.label(
                        RichText::new(
                            "Saved scrambled with a secret built into Lamina: not readable at a glance, but anyone who has Lamina can \
                             unscramble it. Add a passphrase to encrypt it, or leave Remember off on a shared computer.",
                        )
                        .small()
                        .weak(),
                    );
                    ui.horizontal(|ui| {
                        ui.add(TextEdit::singleline(&mut self.passphrase).password(true).hint_text("Passphrase").desired_width(150.0));
                        let long = self.passphrase.chars().count() >= seal::PASSPHRASE_MIN;
                        if ui.add_enabled(long, egui::Button::new("Encrypt")).clicked() {
                            let phrase = std::mem::take(&mut self.passphrase);
                            self.problem = self.remember(Some(&phrase)).err();
                        }
                    });
                }
                None => {}
            }
        }
        if let Some(problem) = &self.problem {
            ui.label(RichText::new(problem).small().color(Color32::from_rgb(230, 180, 90)));
        }
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
        self.key_ui(ui);
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
        // The files that go with the next message, each with a button to take it off.
        let mut off = None;
        for (i, a) in self.chat.attached.iter().enumerate() {
            ui.horizontal(|ui| {
                let kind = if a.image { "picture" } else { "text" };
                ui.label(RichText::new(format!("{} ({kind}, {} KB)", a.name, a.size.div_ceil(1024))).small());
                if ui.small_button("x").on_hover_text("Do not send this file").clicked() {
                    off = Some(i);
                }
            });
        }
        if let Some(i) = off {
            self.chat.detach(i);
        }
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
            let attach = ui.add_enabled(!busy, egui::Button::new("Attach...")).on_hover_text(
                "Send a photo or sketch of the part (or a text note) with the next message; dropping a picture on the window does too",
            );
            if attach.clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Picture", &["png", "jpg", "jpeg", "gif", "webp"])
                    .add_filter("Text", &["txt", "md"])
                    .pick_file()
            {
                self.chat.attach(&path);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> ChatPanel {
        let mut p = ChatPanel::new(Arc::new(lamina_assistant::testing::Fake::default()));
        // A provider whose environment variable the tests never set, so a real key in the environment is never used.
        p.chat.provider = Provider::Gemini;
        p
    }

    #[test]
    fn a_remembered_key_comes_back_in_the_next_window() {
        let mut a = panel();
        assert!(a.remember(None).is_err(), "nothing to remember yet");
        a.keys.insert(Provider::Gemini.key(), "AIza-test-0123456789abcdef".into());
        a.remember(None).unwrap();
        let sealed = a.saved["gemini"].clone();
        assert!(sealed.starts_with("lm1:") && !sealed.contains("AIza"));
        let mut b = panel();
        assert!(b.restore(a.saved.clone()).is_empty());
        assert_eq!(b.keys.get("gemini").map(String::as_str), Some("AIza-test-0123456789abcdef"));
        b.forget();
        assert!(b.saved.is_empty());
    }

    #[test]
    fn a_key_under_a_passphrase_waits_for_it() {
        let mut a = panel();
        a.keys.insert(Provider::Gemini.key(), "AIza-test-0123456789abcdef".into());
        assert!(a.remember(Some("short")).is_err());
        a.remember(Some("correct horse battery")).unwrap();
        assert!(a.saved["gemini"].starts_with("lm2:"));
        let mut b = panel();
        b.restore(a.saved.clone());
        assert!(b.locked() && b.key().is_empty());
        assert!(b.unlock("wrong passphrase").is_err());
        assert!(b.locked());
        b.unlock("correct horse battery").unwrap();
        assert!(!b.locked());
        assert_eq!(b.key(), "AIza-test-0123456789abcdef");
    }

    #[test]
    fn a_saved_key_that_does_not_open_is_dropped_and_said() {
        let mut b = panel();
        let other_build = seal::seal_with("another-secret", "AIza-test-0123456789abcdef", &[7; 16], &[9; 12], seal::ITERATIONS).unwrap();
        let notes = b.restore(BTreeMap::from([("gemini".to_string(), other_build), ("nobody".to_string(), "x".to_string())]));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(b.saved.is_empty());
    }
}
