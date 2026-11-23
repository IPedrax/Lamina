//! `lamina-cli`: Lamina from the command line (the desktop app is `lamina`). Scripts, conversion and an MCP server for agents, all through the same commands.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod mcp;
mod window;

use std::io::Write;
use std::process::ExitCode;

use lamina_core::Session;
use lamina_core::bridge::Access;
use serde_json::{Value, json};

const USAGE: &str = "USAGE:
  lamina-cli run [FILE] [--cmd 'ID {json}']... [--script FILE]...
      Open FILE (a .lamina project, or a mesh put on the plate) or start empty, then run the commands and scripts in the order
      given (a script is one `ID {json}` per line). Each result is printed as a line of JSON. `export_mesh {\"path\": \"out.stl\"}`
      writes the plate, `save {\"path\": \"plate.lamina\"}` the project, and after `slice`, `send_to_printer` uploads the
      G-code to Moonraker or OctoPrint (see `lamina-cli commands`).
  lamina-cli slice FILE OUT.gcode [PROFILE.json]
                                 slice a mesh, or a project with its own print settings, to G-code. PROFILE.json holds only the
                                 settings that differ (`lamina-cli run --cmd slicer_profile` prints them all)
  lamina-cli info FILE               the plate at a glance: size, volume, triangles, whether each object is closed
  lamina-cli commands                every command and its parameters
  lamina-cli mcp [--read-only] [--connect [PORT]]
                                 an MCP server on stdio, for agents; with --read-only the agent may look at the scene, never change
                                 it; with --connect it works on the plate of the open window that lets agents in (its Agents section)
  lamina-cli --version";

/// A session with every command registered: the built-in ones, and each crate's.
fn session() -> Session {
    let mut s = Session::new();
    s.register(lamina_slicer::COMMANDS);
    s.register(lamina_csg::COMMANDS);
    s
}

/// Puts `file` in the session: a project is opened, a mesh is put on the plate.
fn load(s: &mut Session, file: &str) -> Result<Value, String> {
    let project = std::path::Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(lamina_core::project::EXTENSION));
    s.execute(if project { "open" } else { "import_mesh" }, &json!({"path": file}))
}

fn print(out: &mut impl Write, v: &Value) -> Result<(), String> {
    writeln!(out, "{v}").map_err(|e| format!("stdout: {e}"))
}

fn run(args: &[String], out: &mut impl Write) -> Result<(), String> {
    let mut s = session();
    // Sending to a printer is for the user's own runs and scripts; the MCP server's agents never get it.
    s.register(lamina_printer::COMMANDS);
    let mut it = args.iter().peekable();
    // A leading bare argument is a file to put on the plate.
    if let Some(file) = it.next_if(|a| !a.starts_with("--")) {
        print(out, &load(&mut s, file)?)?;
    }
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("`{flag}` needs a value"))?;
        match flag.as_str() {
            "--cmd" => {
                let (id, rest) = value.split_once(char::is_whitespace).unwrap_or((value, "{}"));
                let params: Value = serde_json::from_str(rest.trim()).map_err(|e| format!("--cmd `{id}`: parameters are not JSON: {e}"))?;
                print(out, &s.execute(id, &params)?)?;
            }
            "--script" => {
                let text = std::fs::read_to_string(value).map_err(|e| format!("{value}: {e}"))?;
                for v in s.script(&text)? {
                    print(out, &v)?;
                }
            }
            other => return Err(format!("unknown option `{other}`\n{USAGE}")),
        }
    }
    Ok(())
}

fn main_with(args: &[String], out: &mut impl Write) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("run") => run(&args[1..], out),
        Some("info") => {
            let file = args.get(1).ok_or("`info` needs a file")?;
            let mut s = session();
            load(&mut s, file)?;
            print(out, &s.execute("info", &json!({}))?)
        }
        Some("slice") => {
            let (file, to) =
                (args.get(1).ok_or("`slice` needs a file and an output path")?, args.get(2).ok_or("`slice` needs an output path")?);
            let mut s = session();
            load(&mut s, file)?;
            print(out, &s.execute("slice", &json!({"path": to, "profile_path": args.get(3)}))?)
        }
        Some("commands") => print(out, &session().catalog()),
        Some("mcp") => {
            let access = if args.iter().any(|a| a == "--read-only") { Access::Read } else { Access::Edit };
            let mut server = match args.iter().position(|a| a == "--connect") {
                Some(at) => {
                    let port = match args.get(at + 1).filter(|p| !p.starts_with("--")) {
                        Some(p) => Some(p.parse::<u16>().map_err(|_| format!("`--connect {p}`: a port is a number"))?),
                        None => None,
                    };
                    mcp::Server::window(window::Window::connect(port)?, access)
                }
                None => mcp::Server::new(session(), access),
            };
            server.serve(std::io::stdin().lock(), std::io::stdout().lock()).map_err(|e| format!("mcp: {e}"))
        }
        Some("--version") => print(out, &json!(env!("CARGO_PKG_VERSION"))),
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Started as the child that runs one boolean: do that and nothing else.
    if let Some(code) = lamina_csg::child_main(&args) {
        return code;
    }
    // Every boolean runs in a copy of this program, so a crash in the engine is an error and not the end of the run.
    if let Ok(exe) = std::env::current_exe() {
        lamina_csg::run_in_child(exe);
    }
    match main_with(&args, &mut std::io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lamina-cli: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(args: &[&str]) -> Result<Vec<Value>, String> {
        let mut out = Vec::new();
        main_with(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>(), &mut out)?;
        Ok(String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect())
    }

    #[test]
    fn run_builds_a_plate_from_commands_and_prints_each_result() {
        let r = lines(&["run", "--cmd", r#"add_primitive {"kind":"box","size":[10,10,10]}"#, "--cmd", "info"]).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[1]["objects"][0]["watertight"], true);
    }

    #[test]
    fn a_run_slices_and_sends_to_a_printer_but_an_agent_cannot() {
        let (address, got) = lamina_printer::testing::printer(201, "{}");
        let gcode = std::env::temp_dir().join(format!("lamina-cli-send-{}.gcode", std::process::id())).to_string_lossy().into_owned();
        let r = lines(&[
            "run",
            "--cmd",
            r#"add_primitive {"kind":"box","size":[10,10,4]}"#,
            "--cmd",
            &format!("slice {}", json!({"path": gcode})),
            "--cmd",
            &format!("send_to_printer {}", json!({"path": gcode, "printer": "moonraker", "address": address})),
        ])
        .unwrap();
        std::fs::remove_file(&gcode).unwrap();
        assert_eq!(r[2]["printing"], false);
        assert!(got.lock().unwrap()[0].body_text().contains(";LAYER:19"));
        assert!(session().find("send_to_printer").is_none(), "the MCP server's session has no such command");
    }

    #[test]
    fn info_reads_an_stl_and_a_bad_command_fails_the_run() {
        let path = std::env::temp_dir().join(format!("lamina-cli-{}.stl", std::process::id()));
        let path = path.to_string_lossy().into_owned();
        lines(&[
            "run",
            "--cmd",
            r#"add_primitive {"kind":"cylinder","radius":3,"height":8}"#,
            "--cmd",
            &format!("export_mesh {}", json!({"path": path})),
        ])
        .unwrap();
        let r = lines(&["info", &path]).unwrap();
        assert!((r[0]["objects"][0]["size"][2].as_f64().unwrap() - 8.0).abs() < 1e-6);
        let gcode = std::env::temp_dir().join(format!("lamina-cli-{}.gcode", std::process::id())).to_string_lossy().into_owned();
        let r = lines(&["slice", &path, &gcode]).unwrap();
        assert_eq!(r[0]["layers"], 40);
        // A project made by one run is opened by the next, with its print settings, and sliced with them.
        let project = std::env::temp_dir().join(format!("lamina-cli-{}.lamina", std::process::id())).to_string_lossy().into_owned();
        lines(&["run", &path, "--cmd", &format!("save {}", json!({"path": project}))]).unwrap();
        let mut text: Value = serde_json::from_str(&std::fs::read_to_string(&project).unwrap()).unwrap();
        text["print"] = json!({"profile": {"layer_height": 0.4, "first_layer_height": 0.2, "nozzle": 0.6}});
        std::fs::write(&project, text.to_string()).unwrap();
        assert_eq!(lines(&["info", &project]).unwrap()[0]["objects"].as_array().unwrap().len(), 1);
        assert_eq!(lines(&["slice", &project, &gcode]).unwrap()[0]["layers"], 21);
        std::fs::remove_file(&gcode).unwrap();
        std::fs::remove_file(&project).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(lines(&["run", "--cmd", "nope"]).is_err());
        assert!(lines(&["run", "--wat", "x"]).is_err());
        assert!(lines(&[]).is_err());
    }
}
