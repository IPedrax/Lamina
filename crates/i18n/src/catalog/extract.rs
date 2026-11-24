//! Extract syntax, not text matches: English literals passed to the window's translation functions (`t`, `tf`, `tc`) and to its
//! `field` and `section` helpers, and `ui_msg!` markers for labels chosen indirectly. The window's own sources give both; the other crates give
//! their markers only. Test modules are excluded. Lifted from BIMCraft's `crates/i18n` (same author, MIT OR Apache-2.0).

use super::{Document, Entries, Entry};
use proc_macro2::{Span, TokenStream, TokenTree};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Expr, Lit, LitStr, Token};

/// The window's sources, every message in them.
const WINDOW: &str = "apps/lamina/src";
/// Crates whose `ui_msg!` markers the window shows: the slicer's setting and preset help.
const MARKED: &[&str] = &["crates/slicer/src"];

struct Extractor<'a> {
    entries: &'a mut Entries,
    path: String,
    lines: Vec<&'a str>,
    markers_only: bool,
}

impl Extractor<'_> {
    fn add(&mut self, context: &str, message: &LitStr) {
        let id = message.value();
        if id.is_empty() {
            return;
        }
        let entry =
            self.entries.entry((context.into(), id.clone())).or_insert_with(|| Entry { context: context.into(), id, ..Entry::default() });
        let line = message.span().start().line;
        let reference = format!("#: {}:{line}", self.path);
        if !entry.comments.contains(&reference) {
            entry.comments.push(reference);
        }
        if let Some(comment) = line.checked_sub(2).and_then(|i| self.lines.get(i)).and_then(|l| l.trim().strip_prefix("// TRANSLATORS:")) {
            let comment = format!("#. {}", comment.trim());
            if !entry.comments.contains(&comment) {
                entry.comments.push(comment);
            }
        }
    }

    fn arguments(&mut self, name: &str, args: &[&Expr]) {
        if self.markers_only {
            return;
        }
        let indices: &[usize] = match name {
            // `section(title)`: a collapsing section, its title translated and its id the English.
            "t" | "tf" | "section" => &[0],
            // `field(ui, label, name, value, speed)`: a setting's row, which translates its label.
            "field" => &[1],
            _ => &[],
        };
        for &index in indices {
            if let Some(Expr::Lit(literal)) = args.get(index).copied()
                && let Lit::Str(message) = &literal.lit
            {
                self.add("", message);
            }
        }
        if name == "tc"
            && let [Expr::Lit(context), Expr::Lit(message), ..] = args
            && let (Lit::Str(context), Lit::Str(message)) = (&context.lit, &message.lit)
        {
            self.add(&context.value(), message);
        }
    }

    fn tokens(&mut self, tokens: TokenStream) {
        let tokens: Vec<_> = tokens.into_iter().collect();
        for (index, token) in tokens.iter().enumerate() {
            if let TokenTree::Ident(name) = token {
                if name == "ui_msg"
                    && matches!(tokens.get(index + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!')
                    && let Some(TokenTree::Group(group)) = tokens.get(index + 2)
                    && let Ok(args) = Punctuated::<LitStr, Token![,]>::parse_terminated.parse2(group.stream())
                {
                    let args: Vec<_> = args.iter().collect();
                    match args.as_slice() {
                        [message] => self.add("", message),
                        [context, message] => self.add(&context.value(), message),
                        _ => {}
                    }
                } else if let Some(TokenTree::Group(group)) = tokens.get(index + 1)
                    && let Ok(args) = Punctuated::<Expr, Token![,]>::parse_terminated.parse2(group.stream())
                {
                    self.arguments(&name.to_string(), &args.iter().collect::<Vec<_>>());
                }
            }
            if let TokenTree::Group(group) = token {
                self.tokens(group.stream());
            }
        }
    }
}

fn test_attributes(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("test") || (a.path().is_ident("cfg") && a.parse_args::<syn::Ident>().is_ok_and(|i| i == "test")))
}

impl<'ast> Visit<'ast> for Extractor<'_> {
    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if !test_attributes(&module.attrs) {
            visit::visit_item_mod(self, module);
        }
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        if !test_attributes(&function.attrs) {
            visit::visit_item_fn(self, function);
        }
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Expr::Path(path) = &*call.func
            && let Some(name) = path.path.segments.last()
        {
            self.arguments(&name.ident.to_string(), &call.args.iter().collect::<Vec<_>>());
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if let Some(name) = mac.path.segments.last().filter(|s| s.ident == "ui_msg") {
            // Include the macro's name and ! so the same scanner handles outer and nested markers.
            let mut tokens = TokenStream::new();
            tokens.extend([
                TokenTree::Ident(proc_macro2::Ident::new(&name.ident.to_string(), Span::call_site())),
                TokenTree::Punct(proc_macro2::Punct::new('!', proc_macro2::Spacing::Alone)),
                TokenTree::Group(proc_macro2::Group::new(proc_macro2::Delimiter::Parenthesis, mac.tokens.clone())),
            ]);
            self.tokens(tokens);
        } else {
            self.tokens(mac.tokens.clone());
        }
    }
}

fn sources(directory: &Path, paths: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|e| format!("{}: {e}", directory.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.file_stem().is_some_and(|n| n == "tests" || n.to_string_lossy().ends_with("_tests")) {
            continue;
        }
        if path.is_dir() {
            sources(&path, paths)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            paths.insert(path);
        }
    }
    Ok(())
}

/// The interface's template: every message the window's sources and the marked crates hold, with where each is.
pub fn extract(root: &Path) -> Result<Document, String> {
    let mut paths = BTreeSet::new();
    for directory in std::iter::once(WINDOW).chain(MARKED.iter().copied()) {
        sources(&root.join(directory), &mut paths)?;
    }
    let mut entries = Entries::new();
    for path in paths {
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let syntax = syn::parse_file(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let relative = path.strip_prefix(root).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        let markers_only = !relative.starts_with(WINDOW);
        let mut extractor = Extractor { entries: &mut entries, path: relative, lines: text.lines().collect(), markers_only };
        extractor.visit_file(&syntax);
    }
    Ok(Document {
        entries,
        header: "Project-Id-Version: Lamina\nMIME-Version: 1.0\nContent-Type: text/plain; charset=UTF-8\nContent-Transfer-Encoding: 8bit\n"
            .into(),
        header_comments: vec!["# Lamina interface translation template.".into(), "# SPDX-License-Identifier: MIT".into()],
        ..Document::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(code: &str, markers_only: bool) -> BTreeSet<(String, String)> {
        let mut entries = Entries::new();
        Extractor { entries: &mut entries, path: "window.rs".into(), lines: code.lines().collect(), markers_only }
            .visit_file(&syn::parse_file(code).unwrap());
        entries.into_keys().collect()
    }

    #[test]
    fn extraction_covers_helpers_markers_macros_and_contexts_but_excludes_tests() {
        let code = r#"
            fn window() {
                t("Direct"); tf("Value {}", &["user data"]);
                field(ui, "Walls, mm/s", "perimeter_speed", &mut p.perimeter_speed, 1.0);
                section("Speeds").show(ui, |ui| {});
                let labels = [ui_msg!("Dynamic"), ui_msg!("verb", "Open")];
                let nested = vec![ui_msg!("Nested"), t("In macro")];
                tc("adjective", "Open");
                ui.label("Not marked");
            }
            #[cfg(test)] mod tests { fn test() { t("Test only"); } }
        "#;
        let found = keys(code, false);
        let want = [
            ("", "Direct"),
            ("", "Value {}"),
            ("", "Walls, mm/s"),
            ("", "Speeds"),
            ("", "Dynamic"),
            ("verb", "Open"),
            ("adjective", "Open"),
            ("", "Nested"),
            ("", "In macro"),
        ];
        assert_eq!(found, want.iter().map(|(c, s)| (c.to_string(), s.to_string())).collect());
        // Another crate gives only what it marks: its `t` and `field` are not the window's.
        let marked = keys(code, true);
        assert_eq!(marked.len(), 3, "{marked:?}");
        assert!(marked.iter().all(|(_, s)| ["Dynamic", "Open", "Nested"].contains(&s.as_str())));
    }
}
