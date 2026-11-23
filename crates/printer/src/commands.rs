//! The printer's command, which an app adds with `session.register(lamina_printer::COMMANDS)`. It reads a G-code file (so the
//! assistant, which refuses every command that touches a file, never runs it) and sends it to the address given.

use lamina_core::{Command, Files, Session};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Kind, MAX_GCODE, Printer, send};

pub static COMMANDS: &[Command] = &[Command {
    id: "send_to_printer",
    params: "{path, printer:\"moonraker\"|\"octoprint\"|\"prusalink\"|\"duet\", address, key?, name?, print?} sends the G-code file at \
             `path` to a printer on the user's network: Moonraker (Klipper), OctoPrint, PrusaLink or a Duet at `address` (such as \
             192.168.1.20 or http://printer.local:7125). OctoPrint needs its API `key`, PrusaLink usually does, and a Duet's `key` is its \
             password. `name` is the file's name on the printer (default the file's own); `print: true` starts \
             printing it (default false: upload only). Returns {name, printing}",
    run: send_to_printer,
    undoable: false,
    files: Files::Read,
}];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    path: String,
    printer: Kind,
    address: String,
    key: Option<String>,
    name: Option<String>,
    print: Option<bool>,
}

fn send_to_printer(_: &mut Session, p: &Value) -> Result<Value, String> {
    let a = Args::deserialize(p).map_err(|e| format!("bad parameters: {e}"))?;
    let len = std::fs::metadata(&a.path).map_err(|e| format!("{}: {e}", a.path))?.len();
    if len > MAX_GCODE {
        return Err(format!("{} is {len} bytes, more than the {MAX_GCODE} sent", a.path));
    }
    let gcode = std::fs::read(&a.path).map_err(|e| format!("{}: {e}", a.path))?;
    let name = a.name.unwrap_or_else(|| {
        std::path::Path::new(&a.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "lamina.gcode".into())
    });
    let sent = send(&Printer { kind: a.printer, address: a.address, key: a.key }, &name, &gcode, a.print.unwrap_or(false))?;
    Ok(json!({"name": sent.name, "printing": sent.printing}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sliced_plate_is_sent_through_the_command() {
        let (address, got) = crate::testing::printer(201, "{}");
        let path = std::env::temp_dir().join(format!("lamina-send-{}.gcode", std::process::id()));
        std::fs::write(&path, "G28\nM84\n").unwrap();
        let mut s = Session::new();
        s.register(COMMANDS);
        assert_eq!(s.find("send_to_printer").unwrap().files, Files::Read, "the assistant refuses it, as it reads a file");
        let r = s.execute("send_to_printer", &json!({"path": path, "printer": "moonraker", "address": address, "print": true})).unwrap();
        assert_eq!(r["printing"], true);
        assert!(r["name"].as_str().unwrap().starts_with("lamina-send-"));
        assert!(got.lock().unwrap()[0].body_text().contains("G28\nM84\n"));
        for bad in [
            json!({"path": path, "printer": "bambu", "address": address}),
            json!({"path": path, "printer": "moonraker", "address": address, "start": true}),
            json!({"path": "/nonexistent.gcode", "printer": "moonraker", "address": address}),
            json!({"path": path, "printer": "moonraker", "address": "ftp://x"}),
        ] {
            assert!(s.execute("send_to_printer", &bad).is_err(), "{bad}");
        }
        std::fs::remove_file(&path).unwrap();
        assert_eq!(got.lock().unwrap().len(), 1);
    }
}
