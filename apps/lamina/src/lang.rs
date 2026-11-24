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

impl Lang {
    /// Whether numbers are written with a decimal comma: 0,2 mm in Portuguese and Spanish, 0.2 mm in English.
    pub fn decimal_comma(self) -> bool {
        matches!(self, Lang::Portuguese | Lang::Spanish)
    }
}

/// `value` as a number field shows it in `language`, with `decimals` as egui chooses them: egui's own text, its point a comma
/// where the language writes one.
pub fn format_number(language: Lang, value: f64, decimals: std::ops::RangeInclusive<usize>) -> String {
    let text = egui::emath::format_with_decimals_in_range(value, decimals);
    if language.decimal_comma() { text.replace('.', ",") } else { text }
}

/// What the user typed in a number field, read in `language`. English reads as egui does; Portuguese and Spanish take a comma or a
/// point as the decimal mark (a number pasted from elsewhere has a point), but not both, as Lamina writes no thousands separator and
/// 1.234,5 is more likely a mistake than a number. Spaces are left out and U+2212 is a minus, as egui does; what is not a finite
/// number is no number.
pub fn parse_number(language: Lang, typed: &str) -> Option<f64> {
    let text: String = typed.chars().filter(|c| !c.is_whitespace()).map(|c| if c == '\u{2212}' { '-' } else { c }).collect();
    let text = if language.decimal_comma() {
        if text.contains(',') && text.contains('.') {
            return None;
        }
        text.replace(',', ".")
    } else {
        text
    };
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A number written by Rust (`0.25`, `-1.50`, `+2.0`) with the window's decimal mark: a label or tooltip shows numbers as the
/// fields do. Give it the number alone, never a sentence or a name, whose points are not decimal marks.
pub fn mark(number: String) -> String {
    if get().decimal_comma() { number.replace('.', ",") } else { number }
}

/// A number field for `value`, written and read in the window's language (a decimal comma in Portuguese and Spanish). Every number
/// field in the window is made here rather than with `egui::DragValue::new`.
pub fn number<N: egui::emath::Numeric>(value: &mut N) -> egui::DragValue<'_> {
    let language = get();
    egui::DragValue::new(value)
        .custom_formatter(move |v, decimals| format_number(language, v, decimals))
        .custom_parser(move |typed| parse_number(language, typed))
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

    #[test]
    fn numbers_are_written_and_read_with_the_languages_decimal_mark() {
        for (language, shown) in [(Lang::English, "0.25"), (Lang::Portuguese, "0,25"), (Lang::Spanish, "0,25")] {
            assert_eq!(format_number(language, 0.25, 2..=2), shown);
            assert_eq!(format_number(language, 12.0, 0..=0), "12", "a whole number has no mark");
            assert_eq!(format_number(language, -1.5, 1..=3), shown.replace("0.25", "-1.5").replace("0,25", "-1,5"));
        }
        // English reads as egui does: a point, spaces left out, U+2212 a minus.
        assert_eq!(parse_number(Lang::English, "0.25"), Some(0.25));
        assert_eq!(parse_number(Lang::English, " \u{2212}1.5 "), Some(-1.5));
        assert_eq!(parse_number(Lang::English, "1 000"), Some(1000.0));
        assert_eq!(parse_number(Lang::English, "0,25"), None, "a comma is no decimal mark in English");
        for language in [Lang::Portuguese, Lang::Spanish] {
            assert_eq!(parse_number(language, "0,25"), Some(0.25));
            assert_eq!(parse_number(language, "0.25"), Some(0.25), "a point pasted from elsewhere");
            assert_eq!(parse_number(language, "\u{2212}1,5"), Some(-1.5));
            assert_eq!(parse_number(language, "12"), Some(12.0));
            assert_eq!(parse_number(language, "1.234,5"), None, "both marks: a mistake, not a number");
            assert_eq!(parse_number(language, "0,2,5"), None);
        }
        set(Lang::Portuguese);
        assert_eq!((mark(format!("{:.2}", 1.5)), mark(format!("{:+.1}", -0.25))), ("1,50".to_string(), "-0,2".to_string()));
        set(Lang::English);
        assert_eq!(mark(format!("{:.2}", 1.5)), "1.50");
        for language in Lang::ALL {
            for typed in ["", ",", "abc", "NaN", "inf", "-inf", "1e999"] {
                assert_eq!(parse_number(language, typed), None, "{language:?}: {typed:?}");
            }
            for value in [0.0, 0.2, 1.25, -3.5, 210.0, 0.001] {
                assert_eq!(parse_number(language, &format_number(language, value, 0..=3)), Some(value), "{language:?}: {value}");
            }
        }
    }
}
