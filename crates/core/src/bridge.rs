//! What a connected agent (the MCP server, the AI chat) may ask of a session, and the rules that go with it. A request is a
//! command, a script or the catalog; an agent that may only read can look at the scene and export, and never change it.
//!
//! The window's control port (roadmap 9.4) speaks it too: a window that lets agents in listens on this machine's own address, and
//! `lamina-cli mcp --connect` runs its tools on the window's plate. One JSON request a line, one JSON reply a line:
//! `{"token": T, "op": "hello" | "catalog"}`, `{"token": T, "op": "execute", "command": "move", "params": {...}}` and
//! `{"token": T, "op": "script", "text": "..."}`, each with `"read_only": true` when the agent's side asks to only read; the reply is
//! `{"ok": value}` or `{"error": text}`. The token is made when the window opens the port and kept in [`FILE`] beside the settings,
//! readable by the user alone ([`Entry`]), so another user's process, or a web page that tries the port, is turned away. Lifted from
//! BIMCraft's `crates/core/src/bridge.rs` (same author, MIT OR Apache-2.0); the threads and the window are in the apps.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::engine::Session;

/// What the front end lets an agent do to the scene: change it as the user does, or only look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Edit,
    Read,
}

/// The commands that put another plate in the user's place (and start a new history): an agent that may only read never runs
/// them, and neither does the window's assistant. BIMCraft's bridge keeps the same list.
pub const REPLACES: &[&str] = &["new", "open"];

/// What a refusal says when the agent may only read.
pub const READ_ONLY: &str =
    "this agent may only read the scene: the user lets it edit in the app's settings, or starts the MCP server without --read-only";

#[derive(Debug, PartialEq)]
pub enum Request {
    Hello,
    Execute {
        command: String,
        params: Value,
    },
    /// One `id {json}` a line.
    Script {
        text: String,
    },
    Catalog,
}

/// Runs a request on `session`, as `access` lets the agent.
pub fn run(session: &mut Session, request: &Request, access: Access) -> Result<Value, String> {
    match request {
        Request::Hello => Ok(json!({"app": "lamina", "version": env!("CARGO_PKG_VERSION"), "read_only": access == Access::Read})),
        Request::Catalog => Ok(session.catalog()),
        Request::Execute { command, params } => {
            may_run(session, command, access)?;
            session.execute(command, params)
        }
        Request::Script { text } => {
            // Read the whole script before any line runs, so a refused line stops it at the start.
            for (n, line) in text.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                may_run(session, line.split_whitespace().next().unwrap_or(""), access).map_err(|e| format!("line {}: {e}", n + 1))?;
            }
            session.script(text).map(Value::Array)
        }
    }
}

/// The longest request line read: a long script is a few megabytes at most.
pub const MAX_LINE: usize = 16 << 20;

/// The file the window leaves its port and token in, beside the settings file.
pub const FILE: &str = "bridge.json";

/// Why a request line was not taken.
#[derive(Debug, PartialEq)]
pub enum Error {
    /// The token is missing or wrong: the connection is dropped.
    Token,
    /// The line is no request: what is wrong with it.
    Request(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Token => f.write_str("the token is missing or wrong: the window's bridge file has it"),
            Error::Request(why) => f.write_str(why),
        }
    }
}

/// Whether two tokens are the same, in a time that does not say how much of one was right.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// Reads a request line for a window whose token is `token`: the request, and whether the agent's side asks to only read.
pub fn parse(line: &str, token: &str) -> Result<(Request, bool), Error> {
    let bad = |why: &str| Error::Request(why.to_string());
    let request: Value = serde_json::from_str(line).map_err(|e| Error::Request(format!("not JSON: {e}")))?;
    let Some(o) = request.as_object() else { return Err(bad("a request is an object")) };
    if !o.get("token").and_then(Value::as_str).is_some_and(|t| same(t, token)) {
        return Err(Error::Token);
    }
    let read_only = match o.get("read_only") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("`read_only` is true or false")),
    };
    let request = match o.get("op").and_then(Value::as_str) {
        Some("hello") => Request::Hello,
        Some("catalog") => Request::Catalog,
        Some("execute") => {
            let command = o.get("command").and_then(Value::as_str).ok_or_else(|| bad("`execute` needs a `command`, a string"))?;
            let params = match o.get("params") {
                None | Some(Value::Null) => json!({}),
                Some(p @ Value::Object(_)) => p.clone(),
                Some(_) => return Err(bad("`params` is an object")),
            };
            Request::Execute { command: command.to_string(), params }
        }
        Some("script") => Request::Script {
            text: o.get("text").and_then(Value::as_str).ok_or_else(|| bad("`script` needs `text`, a string"))?.to_string(),
        },
        Some(other) => return Err(Error::Request(format!("unknown op `{other}`: hello, execute, script or catalog"))),
        None => return Err(bad("a request has an `op`")),
    };
    Ok((request, read_only))
}

/// A request as a line says it, with `token` (and `read_only` when asked): what the CLI sends a window.
pub fn line(request: &Request, token: &str, read_only: bool) -> Value {
    let mut v = match request {
        Request::Hello => json!({"op": "hello"}),
        Request::Catalog => json!({"op": "catalog"}),
        Request::Execute { command, params } => json!({"op": "execute", "command": command, "params": params}),
        Request::Script { text } => json!({"op": "script", "text": text}),
    };
    v["token"] = json!(token);
    if read_only {
        v["read_only"] = json!(true);
    }
    v
}

/// The reply line for a result.
pub fn reply(result: Result<Value, String>) -> String {
    match result {
        Ok(v) => json!({"ok": v}),
        Err(e) => json!({"error": e}),
    }
    .to_string()
}

/// The first command among `ids` that `request` runs (by id or in any case), for the window to refuse: an agent never replaces the
/// user's plate or sends it to a printer.
pub fn mentions(session: &Session, request: &Request, ids: &[&str]) -> Option<&'static str> {
    let named = |command: &str| session.find(command).map(|c| c.id).filter(|id| ids.contains(id));
    match request {
        Request::Execute { command, .. } => named(command),
        Request::Script { text } => text.lines().find_map(|l| named(l.split_whitespace().next().unwrap_or(""))),
        Request::Hello | Request::Catalog => None,
    }
}

/// A window's way in, as [`FILE`] keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub port: u16,
    pub token: String,
}

/// Where the settings file is: `LAMINA_SETTINGS` when set, else `settings.json` in the user's settings folder (`%APPDATA%\Lamina`,
/// `~/Library/Application Support/Lamina`, or `$XDG_CONFIG_HOME/lamina`, `~/.config/lamina`, as BIMCraft finds its own).
pub fn settings_file() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("LAMINA_SETTINGS").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let home = || std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    let folder = if cfg!(target_os = "windows") {
        PathBuf::from(std::env::var_os("APPDATA").filter(|p| !p.is_empty())?).join("Lamina")
    } else if cfg!(target_os = "macos") {
        home()?.join("Library").join("Application Support").join("Lamina")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config")))?
            .join("lamina")
    };
    Some(folder.join("settings.json"))
}

/// Where [`FILE`] is: beside the settings file.
pub fn entry_path() -> Option<PathBuf> {
    settings_file().map(|p| p.with_file_name(FILE))
}

/// Writes the file for the window listening on `entry.port`, readable by the user alone where the system has such a thing.
pub fn write_entry(path: &Path, entry: &Entry) -> io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(json!({"port": entry.port, "token": entry.token}).to_string().as_bytes())
}

/// Reads the file, if it is there and says a port and a token.
pub fn read_entry(path: &Path) -> Option<Entry> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path).ok()?.take(4096).read_to_string(&mut text).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    Some(Entry { port: u16::try_from(v["port"].as_u64()?).ok()?, token: v["token"].as_str().filter(|t| !t.is_empty())?.to_string() })
}

/// Removes the file, when it is still the one `entry` wrote: another window's is not this one's to remove.
pub fn remove_entry(path: &Path, entry: &Entry) {
    if read_entry(path).as_ref() == Some(entry) {
        let _ = std::fs::remove_file(path);
    }
}

/// Refuses a command that would change the scene or its history when the agent may only read.
fn may_run(session: &Session, command: &str, access: Access) -> Result<(), String> {
    let Some(c) = session.find(command) else { return Ok(()) }; // `execute` says it is unknown
    if access == Access::Read && (c.undoable || REPLACES.contains(&c.id) || matches!(c.id, "undo" | "redo")) {
        return Err(format!("`{}` would change the scene, and {READ_ONLY}", c.id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn execute(command: &str, params: Value) -> Request {
        Request::Execute { command: command.into(), params }
    }

    #[test]
    fn an_agent_that_may_only_read_looks_but_never_changes() {
        let mut s = Session::new();
        run(&mut s, &execute("add_primitive", json!({"kind": "box", "size": [1, 1, 1]})), Access::Edit).unwrap();
        assert!(run(&mut s, &execute("info", json!({})), Access::Read).is_ok());
        for (c, p) in [
            ("add_primitive", json!({"kind": "box", "size": [1, 1, 1]})),
            ("delete", json!({"id": 1})),
            ("undo", json!({})),
            ("new", json!({})),
            ("open", json!({"path": "/nonexistent.lamina"})),
        ] {
            assert!(run(&mut s, &execute(c, p), Access::Read).unwrap_err().contains("only read"), "{c}");
        }
        let script = Request::Script { text: "info\ndelete {\"id\":1}\n".into() };
        assert!(run(&mut s, &script, Access::Read).unwrap_err().starts_with("line 2:"));
        assert_eq!(s.scene.objects.len(), 1);
    }

    const T: &str = "0123-token";

    #[test]
    fn a_request_is_read_with_its_token_and_what_is_wrong_with_one_is_said() {
        let line = |v: Value| v.to_string();
        assert_eq!(parse(&line(json!({"token": T, "op": "hello"})), T), Ok((Request::Hello, false)));
        assert_eq!(
            parse(&line(json!({"token": T, "op": "execute", "command": "move", "params": {"id": 1}, "read_only": true})), T),
            Ok((execute("move", json!({"id": 1})), true))
        );
        assert_eq!(parse(&line(json!({"token": T, "op": "execute", "command": "undo"})), T), Ok((execute("undo", json!({})), false)));
        assert_eq!(
            parse(&line(json!({"token": T, "op": "script", "text": "info"})), T),
            Ok((Request::Script { text: "info".into() }, false))
        );
        // The token first: a request without it, or with another, says nothing else.
        for request in
            [json!({"op": "hello"}), json!({"token": "x", "op": "hello"}), json!({"token": 5}), json!({"token": "", "op": "bogus"})]
        {
            assert_eq!(parse(&line(request.clone()), T), Err(Error::Token), "{request}");
        }
        for (request, says) in [
            (json!({"token": T}), "`op`"),
            (json!({"token": T, "op": "format"}), "unknown op `format`"),
            (json!({"token": T, "op": "execute"}), "`command`"),
            (json!({"token": T, "op": "execute", "command": "move", "params": [1]}), "`params`"),
            (json!({"token": T, "op": "script"}), "`text`"),
            (json!({"token": T, "op": "hello", "read_only": "yes"}), "`read_only`"),
        ] {
            assert!(matches!(parse(&line(request.clone()), T), Err(Error::Request(why)) if why.contains(says)), "{request}");
        }
        assert!(matches!(parse("[1]", T), Err(Error::Request(_))) && matches!(parse("{not json", T), Err(Error::Request(_))));
        // What the CLI sends reads back as it was.
        for (r, ro) in [(execute("scale", json!({"id": 2, "factor": 2})), true), (Request::Catalog, false)] {
            let sent = super::line(&r, T, ro).to_string();
            assert_eq!(parse(&sent, T), Ok((r, ro)));
        }
        assert!(same("abc", "abc") && !same("abc", "abd") && !same("abc", "ab"));
        assert_eq!(reply(Ok(json!(1))), r#"{"ok":1}"#);
        assert_eq!(reply(Err("no".into())), r#"{"error":"no"}"#);
    }

    #[test]
    fn the_window_refuses_by_id_whatever_the_name_or_where_in_a_script() {
        let s = Session::new();
        let no = &["new", "open"];
        assert_eq!(mentions(&s, &execute("NEW", json!({})), no), Some("new"));
        assert_eq!(
            mentions(
                &s,
                &Request::Script {
                    text: "info
open {\"path\":\"x\"}"
                        .into()
                },
                no
            ),
            Some("open")
        );
        assert_eq!(mentions(&s, &execute("info", json!({})), no), None);
        assert_eq!(mentions(&s, &Request::Catalog, no), None);
    }

    #[test]
    fn the_entry_file_holds_the_port_and_token_for_the_user_alone() {
        let dir = std::env::temp_dir().join(format!("lamina-bridge-{}", std::process::id()));
        let path = dir.join(FILE);
        let entry = Entry { port: 4242, token: "secret-token".into() };
        write_entry(&path, &entry).unwrap();
        assert_eq!(read_entry(&path), Some(entry.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // Another window's file is left alone; this window's is removed.
        remove_entry(&path, &Entry { port: 1, token: "other".into() });
        assert!(path.exists());
        remove_entry(&path, &entry);
        assert!(!path.exists());
        for bad in ["", "{}", r#"{"port": 70000, "token": "x"}"#, r#"{"port": 1, "token": ""}"#] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(read_entry(&path), None, "{bad}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_catalog_lists_every_command_with_its_parameters() {
        let mut s = Session::new();
        let all = run(&mut s, &Request::Catalog, Access::Read).unwrap();
        assert!(all.as_array().unwrap().iter().any(|c| c["id"] == "import_mesh" && c["files"] == "read"));
    }
}
