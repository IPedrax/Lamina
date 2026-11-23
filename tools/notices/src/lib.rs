//! The licences of the crates Lamina is built from. [`allowed`] reads a crate's licence expression (SPDX, with the older `/` for
//! OR) and says whether Lamina may use it: some choice in it must be made only of permissive licences, never GPL, AGPL or LGPL
//! alone (CLAUDE.md). [`notice`] writes the third-party notice a release carries: every crate, its licence and its repository,
//! then the text of each licence file the crates ship, each distinct text once with the crates that ship it.
//!
//! What it reads is `cargo metadata`'s JSON (the crates and where their sources are), which `main` asks cargo for.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

/// The licences Lamina's dependencies may have, where a crate offers no other: permissive ones, and for the fonts egui carries,
/// the font licences.
pub const PERMISSIVE: &[&str] = &[
    "MIT",
    "MIT-0",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "0BSD",
    "Unlicense",
    "CC0-1.0",
    "BSL-1.0",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "CDLA-Permissive-2.0",
    "OFL-1.1",
    "Ubuntu-font-1.0",
];

/// The most bytes of one licence file put in the notice.
const MAX_TEXT: u64 = 256 << 10;

/// `expr` split at the top-level `word` (` OR ` or ` AND `), outside parentheses.
fn split_top<'a>(expr: &'a str, word: &str) -> Vec<&'a str> {
    let (mut parts, mut depth, mut start) = (Vec::new(), 0i32, 0);
    let bytes = expr.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ if depth == 0 && expr[i..].starts_with(word) => {
                parts.push(expr[start..i].trim());
                i += word.len();
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(expr[start..].trim());
    parts
}

/// `expr` without the parentheses round all of it, if there are any: "(A OR B)" is "A OR B", "(A) OR (B)" stays as it is.
fn unwrap(expr: &str) -> &str {
    let e = expr.trim();
    if !(e.starts_with('(') && e.ends_with(')')) {
        return e;
    }
    let mut depth = 0i32;
    for (i, c) in e.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && i + 1 < e.len() {
            return e; // the first group closes before the end
        }
    }
    unwrap(&e[1..e.len() - 1])
}

/// Whether Lamina may use a crate under `expr`: the choice it takes, or why none will do.
pub fn allowed(expr: &str) -> Result<String, String> {
    let expr = expr.replace(" / ", " OR ").replace('/', " OR ");
    if expr.trim().is_empty() {
        return Err("no licence given".into());
    }
    fn ok(e: &str) -> bool {
        let e = unwrap(e);
        let any = split_top(e, " OR ");
        if any.len() > 1 {
            return any.iter().any(|a| ok(a));
        }
        let all = split_top(e, " AND ");
        if all.len() > 1 {
            return all.iter().all(|a| ok(a));
        }
        PERMISSIVE.contains(&e)
    }
    let choices = split_top(unwrap(&expr), " OR ");
    match choices.iter().find(|c| ok(c)) {
        Some(c) => Ok(unwrap(c).to_string()),
        None => Err(format!("`{expr}` offers no permissive choice")),
    }
}

/// One crate as the notice lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Crate {
    pub name: String,
    pub version: String,
    pub license: String,
    pub repository: String,
    /// The folder its sources are in.
    pub dir: std::path::PathBuf,
}

/// The crates in `cargo metadata`'s JSON that are not Lamina's own, by name and version.
pub fn crates(meta: &Value) -> Vec<Crate> {
    let own: Vec<&str> = meta["workspace_members"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let mut out: Vec<Crate> = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| !own.contains(&p["id"].as_str().unwrap_or("")))
        .map(|p| Crate {
            name: p["name"].as_str().unwrap_or("").to_string(),
            version: p["version"].as_str().unwrap_or("").to_string(),
            license: p["license"].as_str().unwrap_or("").to_string(),
            repository: p["repository"].as_str().unwrap_or("").to_string(),
            dir: Path::new(p["manifest_path"].as_str().unwrap_or("")).parent().map(Path::to_path_buf).unwrap_or_default(),
        })
        .collect();
    out.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    out
}

/// Every crate whose licence Lamina may not use, with why.
pub fn refused(crates: &[Crate]) -> Vec<String> {
    crates.iter().filter_map(|c| allowed(&c.license).err().map(|e| format!("{} {}: {e}", c.name, c.version))).collect()
}

/// The licence files a crate ships in its folder: LICENSE*, LICENCE*, COPYING*, NOTICE* and UNLICENSE*, by name.
fn texts(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let upper = name.to_uppercase();
        let wanted = ["LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE"].iter().any(|p| upper.starts_with(p));
        let small = e.metadata().is_ok_and(|m| m.is_file() && m.len() <= MAX_TEXT);
        if wanted
            && small
            && let Ok(text) = std::fs::read_to_string(e.path())
        {
            out.push((name, text.replace("\r\n", "\n").trim_end().to_string()));
        }
    }
    out.sort();
    out
}

/// The third-party notice for `crates`.
pub fn notice(crates: &[Crate]) -> String {
    let mut out = String::from(
        "Lamina is built from the crates below, each under the licence it names. Their licence texts follow, each text once, with the\n\
         crates that ship it. A crate that ships no licence file is under the licence it names, whose text is given for another crate.\n\n",
    );
    let mut by_text: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in crates {
        let repo = if c.repository.is_empty() { String::new() } else { format!(" <{}>", c.repository) };
        out.push_str(&format!("{} {}: {}{repo}\n", c.name, c.version, c.license));
        for (_, text) in texts(&c.dir) {
            by_text.entry(text).or_default().push(format!("{} {}", c.name, c.version));
        }
    }
    for (text, users) in by_text {
        out.push_str(&format!("\n{}\nUsed by: {}\n{}\n\n{text}\n", "=".repeat(78), users.join(", "), "=".repeat(78)));
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_crate_is_allowed_when_some_choice_is_permissive() {
        assert_eq!(allowed("MIT OR Apache-2.0").unwrap(), "MIT");
        assert_eq!(allowed("Apache-2.0/MIT").unwrap(), "Apache-2.0");
        assert_eq!(allowed("MIT / Apache-2.0").unwrap(), "MIT");
        assert_eq!(allowed("Apache-2.0 OR GPL-2.0-only").unwrap(), "Apache-2.0");
        assert_eq!(allowed("MIT OR Apache-2.0 OR LGPL-2.1-or-later").unwrap(), "MIT");
        assert_eq!(
            allowed("(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0").unwrap(),
            "(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0"
        );
        assert_eq!(allowed("Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT").unwrap(), "Apache-2.0 WITH LLVM-exception");
        assert_eq!(allowed("(MIT OR Apache-2.0) AND Unicode-3.0").unwrap(), "(MIT OR Apache-2.0) AND Unicode-3.0");
    }

    #[test]
    fn copyleft_alone_or_bound_in_is_refused() {
        for bad in [
            "GPL-3.0-only",
            "AGPL-3.0-or-later",
            "LGPL-2.1-or-later",
            "MIT AND GPL-2.0-only",
            "(MIT OR Apache-2.0) AND LGPL-3.0",
            "",
            "MPL-2.0",
        ] {
            assert!(allowed(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_notice_lists_every_crate_and_each_licence_text_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut list = Vec::new();
        for (name, text) in [("alpha", "MIT text"), ("beta", "MIT text"), ("gamma", "Apache text")] {
            let d = dir.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("LICENSE-X"), text).unwrap();
            std::fs::write(d.join("README.md"), "not a licence").unwrap();
            list.push(Crate { name: name.into(), version: "1.0.0".into(), license: "MIT".into(), repository: String::new(), dir: d });
        }
        let n = notice(&list);
        assert!(n.contains("alpha 1.0.0: MIT\nbeta 1.0.0: MIT\ngamma 1.0.0: MIT"), "{n}");
        assert_eq!(n.matches("MIT text").count(), 1);
        assert!(n.contains("Used by: alpha 1.0.0, beta 1.0.0"));
        assert!(!n.contains("not a licence"));
        assert!(refused(&list).is_empty());
        list[0].license = "GPL-3.0-only".into();
        assert_eq!(refused(&list).len(), 1);
    }

    #[test]
    fn crates_are_read_from_cargo_metadata_without_lamina_itself() {
        let meta = serde_json::json!({
            "workspace_members": ["lamina-core 0.1.0"],
            "packages": [
                {"id": "lamina-core 0.1.0", "name": "lamina-core", "version": "0.1.0", "license": "MIT", "manifest_path": "/x/Cargo.toml"},
                {"id": "zeta", "name": "zeta", "version": "2.0.0", "license": "MIT", "repository": "https://example.org/zeta", "manifest_path": "/r/zeta/Cargo.toml"},
                {"id": "beta", "name": "beta", "version": "1.0.0", "license": null, "manifest_path": "/r/beta/Cargo.toml"}
            ]
        });
        let list = crates(&meta);
        assert_eq!(list.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["beta", "zeta"]);
        assert_eq!(list[1].dir, Path::new("/r/zeta"));
        assert_eq!(refused(&list), ["beta 1.0.0: no licence given"]);
    }
}
