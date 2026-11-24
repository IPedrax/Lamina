//! The window's language: English, as the code writes it, Brazilian Portuguese or Spanish. What the window shows goes through [`t`],
//! [`tf`] or [`tc`], English being its own key and what shows when a catalog has no finished translation; a label chosen through a
//! table or an enum is marked with `lamina_i18n::ui_msg!` where its English is written, and so is the slicer's setting help, which the
//! settings show on hover. The catalogs are `translations/ui/*.po`, compiled in by `build.rs` (docs/TRANSLATING.md).
//!
//! What agents read stays English: command parameters, the errors `Session::execute` gives (the status bar shows them as they come),
//! the CLI and the MCP server, the assistant's skills and prompt, the G-code and the log's own lines. What the user named (objects,
//! files, presets) is never translated. Lifted from BIMCraft's `crates/ui-egui/src/lang.rs` (same author, MIT OR Apache-2.0), with
//! three languages and no project domain.

use std::cell::Cell;

use serde::{Deserialize, Serialize};

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

/// The interface's language; the settings file keeps its code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "pt")]
    Portuguese,
    #[serde(rename = "es")]
    Spanish,
}

impl Lang {
    pub const ALL: [Lang; 3] = [Lang::English, Lang::Portuguese, Lang::Spanish];

    /// Its code in the settings file, which is also its catalog's name: en, pt, es.
    pub fn code(self) -> &'static str {
        match self {
            Lang::English => "en",
            Lang::Portuguese => "pt",
            Lang::Spanish => "es",
        }
    }

    /// Its name, in itself.
    pub fn label(self) -> &'static str {
        match self {
            Lang::English => "English",
            Lang::Portuguese => "Português (Brasil)",
            Lang::Spanish => "Español",
        }
    }

    /// A language by its code, or from a locale as systems give one: pt-BR, pt_PT.UTF-8, es_419, en-US. None for the others (C,
    /// POSIX, a language without a catalog).
    pub fn from_code(code: &str) -> Option<Lang> {
        let lower = code.trim().to_lowercase();
        match lower.split(['-', '_', '.', '@']).next().unwrap_or_default() {
            "en" => Some(Lang::English),
            "pt" => Some(Lang::Portuguese),
            "es" => Some(Lang::Spanish),
            _ => None,
        }
    }
}

thread_local! {
    /// The language this frame is drawn in: the window sets it as each frame starts, and a test's window, on a thread of its own,
    /// is English unless the test says otherwise.
    static LANG: Cell<Lang> = const { Cell::new(Lang::English) };
}

pub fn set(language: Lang) {
    LANG.with(|l| l.set(language));
}

pub fn get() -> Lang {
    LANG.with(Cell::get)
}

/// The language the system asks for (on Unix `LANGUAGE`, `LC_ALL`, `LC_MESSAGES` or `LANG`; the user's language on Windows and
/// macOS), when there is a catalog for it; else English.
pub fn from_env() -> Lang {
    sys_locale::get_locale().and_then(|l| Lang::from_code(&l)).unwrap_or_default()
}

/// `english` in the window's language.
pub fn t(english: &str) -> &str {
    lamina_i18n::translated(CATALOGS, get().code(), "", english).unwrap_or(english)
}

/// A collapsing section titled `english` in the window's language. Its id is the English, so which sections are open stays as it
/// was when the language changes (egui keys a section by its title otherwise).
pub fn section(english: &str) -> egui::CollapsingHeader {
    egui::CollapsingHeader::new(t(english)).id_salt(english)
}

/// A message whose meaning a PO `msgctxt` tells apart.
pub fn tc<'a>(context: &str, english: &'a str) -> &'a str {
    lamina_i18n::translated(CATALOGS, get().code(), context, english).unwrap_or(english)
}

/// `template` translated, its `{}`s filled with `args` in order.
pub fn tf(template: &str, args: &[&str]) -> String {
    args.iter().fold(t(template).to_string(), |s, a| s.replacen("{}", a, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_its_own_key_and_each_catalog_has_its_words() {
        assert_eq!(get(), Lang::English, "a test thread starts in English, whatever the machine's locale");
        assert_eq!((t("Slice"), tf("Opened {}.", &["a.lamina"]).as_str()), ("Slice", "Opened a.lamina."));
        set(Lang::Portuguese);
        assert_eq!((t("Slice"), t("Supports"), t("Infill")), ("Fatiar", "Suportes", "Preenchimento"));
        assert_eq!(tf("Opened {}.", &["a.lamina"]), "a.lamina aberto.");
        set(Lang::Spanish);
        assert_eq!((t("Slice"), t("Supports"), t("Infill")), ("Laminar", "Soportes", "Relleno"));
        set(Lang::English);
    }

    #[test]
    fn missing_messages_and_contexts_fall_back_to_english() {
        for language in Lang::ALL {
            set(language);
            assert_eq!(t("A label no catalog has"), "A label no catalog has");
            assert_eq!(tf("Untranslated {} and {}", &["a", "b"]), "Untranslated a and b");
            assert_eq!(tc("missing-context", "Slice"), "Slice");
        }
        set(Lang::English);
    }

    #[test]
    fn every_finished_translation_keeps_the_english_placeholders() {
        for &(code, messages) in CATALOGS {
            for &(_, english, translated) in messages {
                assert_eq!(english.matches("{}").count(), translated.matches("{}").count(), "{code}: {english:?} -> {translated:?}");
            }
        }
        let codes: Vec<&str> = CATALOGS.iter().map(|&(code, _)| code).collect();
        assert_eq!(codes, ["es", "pt"], "a catalog for each language but English, which is the code's own");
    }

    #[test]
    fn languages_by_their_codes_and_the_locales_systems_give() {
        for (code, language) in [
            ("pt", Some(Lang::Portuguese)),
            ("pt-BR", Some(Lang::Portuguese)),
            ("pt_BR.UTF-8", Some(Lang::Portuguese)),
            ("PT_pt", Some(Lang::Portuguese)),
            ("es_419", Some(Lang::Spanish)),
            ("es-ES", Some(Lang::Spanish)),
            ("en_US.UTF-8", Some(Lang::English)),
            ("C", None),
            ("POSIX", None),
            ("fr_FR", None),
            ("", None),
        ] {
            assert_eq!(Lang::from_code(code), language, "{code:?}");
        }
        assert_eq!(Lang::ALL.map(|l| Lang::from_code(l.code())), Lang::ALL.map(Some));
        assert_eq!(Lang::ALL.map(Lang::label), ["English", "Português (Brasil)", "Español"]);
    }
}
