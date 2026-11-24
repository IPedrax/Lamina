# Translating Lamina

The window's English stays in the Rust sources, where it is both the key and the fallback. Translations live in UTF-8 gettext PO
files, one for each language:

| Template | Catalogs | Used by |
|---|---|---|
| `translations/ui/lamina-ui.pot` | `pt.po` (Brazilian Portuguese), `es.po` (Spanish) | `lang::t`, `lang::tf`, `lang::tc` in `apps/lamina` |

The settings file keeps the language chosen in View, Language by its code (`en`, `pt`, `es`); a code this version has no catalog
for reads as English. The first start takes the system's language (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES` or `LANG` on Unix, the
user's language on Windows and macOS), else English.

Only the window is translated. What agents read stays English: command parameters and the errors commands give, the CLI, the MCP
server, the assistant's skills, prompt and notes, the G-code and the log's own lines. Names the user gave (objects, files,
presets) are never translated.

The way it works is lifted from BIMCraft's `crates/i18n` (same author, MIT OR Apache-2.0), with one domain, the interface.

## Translating

Edit the language's `.po` file in a text editor or a gettext editor such as Poedit. Change `msgstr`, and keep `msgid` and
`msgctxt` as they are. Keep every `{}` placeholder, in the same order: `tf` fills them one after another. Keep deliberate spaces
(` deg`, ` mm a side` start with one), line breaks and punctuation. Entries may be left unfinished.

```po
#: apps/lamina/src/files.rs:332
msgid "{} has changes that are not saved. They are lost unless you save them."
msgstr "{} tem alterações não salvas. Elas se perdem se você não as salvar."
```

An unknown language, a missing message, an empty translation or one marked `#, fuzzy` shows the English. Obsolete entries
(`#~`) are kept for translators and left out of the program. Coverage is reported, not enforced.

## Adding or changing English messages

Use `t("English")` for a label, `tf("Value {}", &[value])` to fill placeholders in order, and `tc("context", "English")` for a word
that needs a `msgctxt` to tell two meanings apart. A setting's row, `field(ui, "Label, mm", "name", ...)`, translates its label
itself. A label chosen through a table, an enum or a condition is marked where its English is written:

```rust
use lamina_i18n::ui_msg;

let shapes = [(ui_msg!("Cone"), cone), (ui_msg!("Tube"), tube)];
ui.button(t(label));
```

`ui_msg!` is the English literal itself, so a crate that marks its texts (the slicer marks `profile::ABOUT` and the presets'
help) gains nothing else. A `// TRANSLATORS:` comment on the line before a literal becomes an extracted comment. Test code is
left out. Do not mark user data, JSON keys or a command's errors.

From the repository root:

```sh
cargo i18n extract  # Write the English template only.
cargo i18n update   # Write the template and merge it into every PO file.
cargo i18n check    # Check the template is current, the PO syntax, duplicate keys and placeholders.
cargo i18n stats    # Count the finished translations in each catalog.
```

Run `update` after `cargo fmt`, since the references hold line numbers. Merging matches the exact English and context, keeps
translator comments, flags and the header, refreshes references, and keeps removed translations as obsolete entries; a message
that comes back gets its old translation again. Changed English gets an empty translation: the tool does not guess one. CI runs
`cargo i18n check`.

## Building, and the PO syntax

`apps/lamina/build.rs` compiles every `translations/ui/*.po` into sorted static tables through `lamina-i18n`'s `catalog`
feature. Lookup is a binary search: no parsing at run time, no file reads, no gettext or `libintl`. A change to a catalog takes
effect when the app is built again.

The compiler reads UTF-8, contexts, translator and developer comments, references, fuzzy flags, obsolete entries, multiline
strings and the usual escapes. Duplicate keys, malformed syntax and a finished translation whose `{}` count differs from the
English fail the build, naming the file and the message. Plural forms (`msgid_plural`, `msgstr[n]`) are refused: write the
English so one form fits every count.

## Adding a language

1. Copy `translations/ui/lamina-ui.pot` to `translations/ui/<code>.po` and add a `Language:` line to its header.
2. Add the language to `Lang` in `apps/lamina/src/lang.rs`: its variant, `ALL`, its code, its name in itself, and the codes
   `from_code` takes. The build finds the PO file by itself.
3. Translate, run `cargo i18n check` and `cargo test -p lamina`, and look at the window: `cargo test -p lamina -- --ignored
   picture_of_the_window` writes it to `target/` in English and in each translation.
