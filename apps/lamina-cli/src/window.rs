//! `mcp --connect`: the MCP server's tools run on the plate of a window that lets agents in (its Agents section, or `lamina --control
//! PORT`), as the user's own, instead of on a headless session of its own. The protocol, and the file the window leaves its port and
//! token in, are `lamina_core::bridge`; this is the asking side. Nothing here listens, and the window is on this machine: the address
//! is its own. Lifted from BIMCraft's `apps/bimcraft-cli/src/window.rs` (same author, MIT OR Apache-2.0).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use lamina_core::bridge::{self, Entry, MAX_LINE, Request};
use serde_json::Value;

/// How long the window has to take the connection.
const CONNECT: Duration = Duration::from_secs(5);

/// A connection to a window's control port.
pub struct Window {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    token: String,
    /// Each request asks the window to only read the plate (`mcp --connect --read-only`).
    pub read_only: bool,
    /// The window said, when it was connected to, that it lets agents only read its plate.
    pub window_read_only: bool,
}

/// Whether an error of the connection says the window has gone: a close with the request unread is a reset, not an end.
fn gone(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{BrokenPipe, ConnectionAborted, ConnectionReset, NotConnected, UnexpectedEof};
    matches!(e.kind(), ConnectionReset | ConnectionAborted | BrokenPipe | UnexpectedEof | NotConnected)
}

impl Window {
    /// Connects to the window that left its bridge file, or, given `port`, to that one if the file is its.
    pub fn connect(port: Option<u16>) -> Result<Window, String> {
        let path = bridge::entry_path().ok_or("there is no folder for the bridge file here: set LAMINA_SETTINGS")?;
        Window::connect_with(&path, port)
    }

    /// [`Window::connect`] with the bridge file at `path`.
    pub(crate) fn connect_with(path: &Path, port: Option<u16>) -> Result<Window, String> {
        let entry = bridge::read_entry(path).ok_or_else(|| {
            format!(
                "{}: no window lets agents in (tick Let agents work on this plate in its Agents section, or start it with `lamina --control 0`)",
                path.display()
            )
        })?;
        if let Some(port) = port.filter(|p| *p != entry.port) {
            return Err(format!(
                "{} has the token of the window on port {}, not {port}: connect without a port",
                path.display(),
                entry.port
            ));
        }
        Window::connect_to(&entry)
    }

    /// Connects to the window `entry` names, and checks that it is Lamina that answers.
    pub fn connect_to(entry: &Entry) -> Result<Window, String> {
        let stream = TcpStream::connect_timeout(&SocketAddr::new(Ipv4Addr::LOCALHOST.into(), entry.port), CONNECT).map_err(|e| {
            format!("no window answers on port {}: {e} (is it still open? the bridge file may be left from one that closed)", entry.port)
        })?;
        stream.set_nodelay(true).map_err(|e| e.to_string())?;
        let reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut window = Window { reader, writer: stream, token: entry.token.clone(), read_only: false, window_read_only: false };
        let hello = window.ask(&Request::Hello)?;
        if hello["app"] != "lamina" {
            return Err(format!("port {} is not a Lamina window", entry.port));
        }
        window.window_read_only = hello["read_only"].as_bool().unwrap_or(false);
        Ok(window)
    }

    /// Sends a request and waits for its reply: the value it gives, or the error it says.
    pub fn ask(&mut self, request: &Request) -> Result<Value, String> {
        writeln!(self.writer, "{}", bridge::line(request, &self.token, self.read_only))
            .and_then(|()| self.writer.flush())
            .map_err(|e| format!("the window closed the connection ({e})"))?;
        let mut line = String::new();
        let read = self.reader.by_ref().take(MAX_LINE as u64 + 1).read_line(&mut line).map_err(|e| {
            if gone(&e) { format!("the window closed the connection ({e})") } else { format!("the window did not answer: {e}") }
        })?;
        if read == 0 {
            return Err("the window closed the connection".into());
        }
        let reply: Value = serde_json::from_str(&line).map_err(|e| format!("the window's reply is not JSON: {e}"))?;
        match (reply.get("ok"), reply["error"].as_str()) {
            (Some(ok), _) => Ok(ok.clone()),
            (None, Some(error)) => Err(error.to_string()),
            _ => Err("the window's reply says neither ok nor error".into()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lamina_core::Session;
    use lamina_core::bridge::Access;
    use serde_json::json;
    use std::net::TcpListener;

    /// A window's control port for a test: a thread that answers each line as the window does, on a session of its own, until the
    /// agent hangs up or `after` requests have been answered (the window closing).
    pub(crate) fn stand_in(after: usize) -> Entry {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let entry = Entry { port: listener.local_addr().unwrap().port(), token: "test-token".into() };
        let token = entry.token.clone();
        std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else { return };
            let mut session = Session::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            for _ in 0..after {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let result = bridge::parse(&line, &token)
                    .map_err(|e| e.to_string())
                    .and_then(|(r, read_only)| bridge::run(&mut session, &r, if read_only { Access::Read } else { Access::Edit }));
                if writeln!(writer, "{}", bridge::reply(result)).is_err() {
                    break;
                }
            }
        });
        entry
    }

    fn execute(command: &str, params: Value) -> Request {
        Request::Execute { command: command.into(), params }
    }

    #[test]
    fn a_window_is_asked_and_what_it_says_is_a_value_or_an_error() {
        let mut w = Window::connect_to(&stand_in(usize::MAX)).unwrap();
        let made = w.ask(&execute("add_primitive", json!({"kind": "box", "size": [5, 5, 5]}))).unwrap();
        assert_eq!(made["id"], 1);
        assert!(w.ask(&execute("move", json!({"id": 9, "by": [1, 0, 0]}))).is_err());
        w.read_only = true;
        assert!(w.ask(&execute("delete", json!({"id": 1}))).unwrap_err().contains("only read"));
        assert_eq!(w.ask(&execute("info", json!({}))).unwrap()["objects"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn a_wrong_token_a_closed_window_and_a_port_nobody_has_are_said_in_words() {
        let mut entry = stand_in(usize::MAX);
        entry.token = "guess".into();
        assert!(Window::connect_to(&entry).err().is_some_and(|e| e.contains("token")), "a wrong token");
        // A window that closes after the hello: the next request finds the connection closed.
        let mut w = Window::connect_to(&stand_in(1)).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(w.ask(&Request::Hello).unwrap_err().starts_with("the window closed the connection"));
        // A port nothing listens on.
        assert!(Window::connect_to(&Entry { port: 1, token: "x".into() }).err().is_some_and(|e| e.contains("no window answers")));
        // Something that answers and is no Lamina.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let other = Entry { port: listener.local_addr().unwrap().port(), token: "x".into() };
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut line = String::new();
                let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
                let _ = writeln!(stream, "HTTP/1.1 400 Bad Request");
            }
        });
        assert!(Window::connect_to(&other).err().is_some_and(|e| e.contains("not JSON")));
    }

    #[test]
    fn the_bridge_file_names_the_window_and_a_port_that_is_not_its_own_is_refused() {
        let dir = std::env::temp_dir().join(format!("lamina-cli-bridge-{}", std::process::id()));
        let file = dir.join(bridge::FILE);
        assert!(Window::connect_with(&file, None).err().is_some_and(|e| e.contains("no window lets agents in")));
        let entry = stand_in(usize::MAX);
        bridge::write_entry(&file, &entry).unwrap();
        let other = if entry.port == 9 { 10 } else { 9 };
        assert!(Window::connect_with(&file, Some(other)).err().is_some_and(|e| e.contains(&format!("not {other}"))));
        assert!(Window::connect_with(&file, Some(entry.port)).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
