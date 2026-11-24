//! What the model may read on demand with `read_skill`: short Markdown notes on modelling for a printer, written for Lamina's own
//! commands and compiled into the app. Nothing is read from disk at run time, so the rule that the assistant never touches a file
//! holds. A skill file is `---`, a `name:` line and a `description:` line, `---`, then the text. The model sees only the list
//! (names and descriptions) until it asks for one, so the notes cost nothing in the requests that do not need them.

use serde_json::{Value, json};

use crate::clip;

pub struct Skill {
    pub name: &'static str,
    pub description: &'static str,
    pub text: &'static str,
}

const FILES: &[&str] = &[
    include_str!("../skills/modelling-with-lamina.md"),
    include_str!("../skills/design-for-fdm.md"),
    include_str!("../skills/slicing-with-lamina.md"),
    include_str!("../skills/work-from-a-picture.md"),
    include_str!("../skills/troubleshooting-prints.md"),
    include_str!("../skills/calibration.md"),
    include_str!("../skills/materials.md"),
    include_str!("../skills/supports.md"),
    include_str!("../skills/objects-heights-and-extruders.md"),
];

fn parse(file: &'static str) -> Option<Skill> {
    // A checkout on Windows may end its lines with `\r\n` (`.gitattributes` asks for `\n`, but a copy made without git ignores it).
    let file = file.strip_prefix("---")?;
    let file = file.strip_prefix("\r\n").or_else(|| file.strip_prefix('\n'))?;
    let (head, text) = file.split_once("\n---\n").or_else(|| file.split_once("\n---\r\n"))?;
    let field = |key: &str| head.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':')).map(str::trim);
    Some(Skill { name: field("name")?, description: field("description")?, text: text.trim() })
}

pub fn all() -> Vec<Skill> {
    FILES.iter().filter_map(|f| parse(f)).collect()
}

/// The `read_skill` tool: no name lists the skills, a name returns that skill's text.
pub fn read(name: Option<&str>) -> Result<Value, String> {
    let skills = all();
    let Some(name) = name else {
        return Ok(json!({"skills": skills.iter().map(|s| json!({"name": s.name, "description": s.description})).collect::<Vec<_>>()}));
    };
    match skills.iter().find(|s| s.name.eq_ignore_ascii_case(name)) {
        Some(s) => Ok(json!({"name": s.name, "text": s.text})),
        None => {
            Err(format!("no skill `{}`; the skills are: {}", clip(name, 40), skills.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_RESULT;
    use lamina_core::Session;

    #[test]
    fn every_skill_file_is_well_formed() {
        let skills = all();
        assert_eq!(skills.len(), FILES.len(), "a skill file lacks its `---` header with a name and a description");
        let mut names: Vec<_> = skills.iter().map(|s| s.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), skills.len(), "two skills share a name");
        for s in &skills {
            assert!((20..=220).contains(&s.description.len()), "{}: a description is one short line", s.name);
            assert!(s.text.len() < MAX_RESULT / 2, "{}: {} bytes would crowd a tool result", s.name, s.text.len());
            assert!(!s.text.contains('\u{2014}') && !s.text.contains('\u{2013}'), "{}: no dashes in prose", s.name);
        }
    }

    /// A skill that names a command the app no longer has would teach the model to call something that fails.
    #[test]
    fn every_command_a_skill_names_exists() {
        let mut session = Session::new();
        session.register(lamina_slicer::COMMANDS);
        session.register(lamina_csg::COMMANDS);
        let tools: Vec<&str> = crate::tools::all().iter().map(|t| t.name).collect();
        let mut seen = 0;
        for s in all() {
            // Code spans are the odd pieces between backticks; `name {params}` is a call.
            for span in s.text.split('`').skip(1).step_by(2) {
                let Some((head, _)) = span.split_once('{') else { continue };
                let head = head.trim();
                if head.is_empty() || !head.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                    continue;
                }
                seen += 1;
                assert!(
                    session.find(head).is_some() || tools.contains(&head),
                    "{} names `{head}`, which is neither a command nor a tool",
                    s.name
                );
            }
        }
        assert!(seen >= 4, "found only {seen} calls: has the way skills write them changed?");
    }

    /// A skill that says a command is missing when it exists would have the model refuse what it can do (the modelling skill once
    /// listed text after `text` was added). Each claim of something missing is its first sentence, from "missing:" or under a
    /// "What is not there yet" heading; what it offers instead, in parentheses or a later sentence, may name commands.
    #[test]
    fn no_skill_calls_a_command_missing() {
        let mut session = Session::new();
        session.register(lamina_slicer::COMMANDS);
        session.register(lamina_csg::COMMANDS);
        let mut claims = 0;
        for s in all() {
            let mut lines = s.text.lines();
            let mut found = Vec::new();
            while let Some(line) = lines.next() {
                if let Some((_, rest)) = line.split_once("missing:") {
                    let next = lines.clone().take_while(|l| l.starts_with("  ")).collect::<Vec<_>>().join(" ");
                    found.push(format!("{rest} {next}"));
                } else if line.starts_with("## What is not there yet") {
                    found.push(
                        lines
                            .clone()
                            .skip_while(|l| l.trim().is_empty())
                            .take_while(|l| !l.trim().is_empty())
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
            }
            for claim in found {
                claims += 1;
                let sentence = claim.split(". ").next().unwrap_or_default();
                let mut plain = String::new();
                let mut depth = 0;
                for c in sentence.chars() {
                    match c {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ if depth == 0 => plain.push(c),
                        _ => {}
                    }
                }
                for word in plain.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                    assert!(session.find(word).is_none(), "{} says `{word}` is missing, but it is a command: {sentence}", s.name);
                }
            }
        }
        assert!(claims >= 3, "found only {claims} claims of something missing: has the way skills write them changed?");
    }

    /// The slicing skill promises this split: the model checks a slice, the user writes the file.
    #[test]
    fn the_model_may_check_a_slice_but_never_write_one() {
        use crate::Call;
        use crate::tools::run;
        use lamina_core::bridge::Access;

        let mut s = Session::new();
        s.register(lamina_slicer::COMMANDS);
        let call = |name: &str, args: Value| Call { id: "1".into(), name: name.into(), args };
        let make = call("execute", json!({"command": "add_primitive", "params": {"kind": "box", "size": [10, 10, 10]}}));
        run(&mut s, &make, Access::Edit).unwrap();
        let checked = run(&mut s, &call("execute", json!({"command": "slice_check"})), Access::Edit).unwrap();
        assert_eq!(checked["layers"], 50);
        let path = std::env::temp_dir().join(format!("lamina-never-{}.gcode", std::process::id()));
        let write = call("execute", json!({"command": "slice", "params": {"path": path.to_string_lossy()}}));
        assert!(run(&mut s, &write, Access::Edit).unwrap_err().contains("never does"));
        assert!(!path.exists());
    }

    #[test]
    fn a_skill_with_windows_line_ends_reads_the_same() {
        let unix = "---\nname: one\ndescription: a note on one thing\n---\n\nThe text.\nMore.\n";
        let windows: &'static str = unix.replace('\n', "\r\n").leak();
        let (a, b) = (parse(unix).unwrap(), parse(windows).unwrap());
        assert_eq!((a.name, a.description), (b.name, b.description));
        assert_eq!((b.name, b.description), ("one", "a note on one thing"));
        assert_eq!(b.text.replace("\r\n", "\n"), a.text);
        assert!(parse("name: one\n").is_none() && parse("---").is_none() && parse("---\r").is_none() && parse("--").is_none());
    }

    #[test]
    fn the_tool_lists_reads_and_refuses_what_it_does_not_have() {
        let list = read(None).unwrap();
        assert_eq!(list["skills"].as_array().unwrap().len(), FILES.len());
        let one = read(Some("Design-For-FDM")).unwrap();
        assert!(one["text"].as_str().unwrap().contains("45 degrees"));
        let err = read(Some("../../etc/passwd")).unwrap_err();
        assert!(err.contains("the skills are: modelling-with-lamina"), "{err}");
    }
}
