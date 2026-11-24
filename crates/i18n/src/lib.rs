//! The window's translations: a lookup with no runtime parser, no file reads and no system gettext. English is the key and the
//! fallback; each language's PO file in `translations/ui` is compiled into a sorted static table by the app's build script
//! ([`catalog::compile`], the `catalog` feature), and `cargo i18n` (the `tooling` feature) keeps the template and the catalogs in
//! step with the code. Lifted from BIMCraft's `crates/i18n` (same author, MIT OR Apache-2.0), with its one domain: Lamina has no
//! project text to translate, only the interface.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

#[cfg(any(test, feature = "catalog"))]
pub mod catalog;

/// Sorted (context, English message, translation) entries compiled from one PO file.
pub type Messages = &'static [(&'static str, &'static str, &'static str)];
/// Language codes and their message tables, sorted by code.
pub type Catalogs = &'static [(&'static str, Messages)];

/// Marks an English message the window shows but that is chosen indirectly (a table entry, an enum's label, a setting's help in
/// another crate). `cargo i18n` extracts it; at run time it is the literal itself, so a crate that marks its texts gains nothing else.
#[macro_export]
macro_rules! ui_msg {
    ($english:literal) => {
        $english
    };
    ($context:literal, $english:literal) => {
        $english
    };
}

/// A translation in exactly this context, or none when the language, its catalog or the message is missing.
pub fn translated(catalogs: Catalogs, language: &str, context: &str, english: &str) -> Option<&'static str> {
    if language == "en" {
        return None;
    }
    let index = catalogs.binary_search_by_key(&language, |&(code, _)| code).ok()?;
    let messages = catalogs[index].1;
    let index = messages.binary_search_by_key(&(context, english), |&(context, id, _)| (context, id)).ok()?;
    Some(messages[index].2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_keeps_contexts_separate_and_missing_messages_fall_back() {
        static CATALOGS: Catalogs =
            &[("en", &[("", "Save", "Do not replace English")]), ("pt", &[("", "Save", "Salvar"), ("verb", "Open", "Abrir")])];
        assert_eq!(translated(CATALOGS, "pt", "", "Save"), Some("Salvar"));
        assert_eq!(translated(CATALOGS, "pt", "verb", "Open"), Some("Abrir"));
        for (language, context, english) in [("en", "", "Save"), ("fr", "", "Save"), ("pt", "", "Open"), ("pt", "verb", "Missing")] {
            assert_eq!(translated(CATALOGS, language, context, english).unwrap_or(english), english);
        }
        assert_eq!(ui_msg!("verb", "Open"), "Open");
        assert_eq!(ui_msg!("Slice"), "Slice");
    }
}
