//! The control port (roadmap 9.4): a window whose user lets agents in (the Agents section of the plate panel, or `lamina --control
//! PORT`) listens on this machine's own address, and an agent that runs `lamina-cli mcp --connect` works on the plate the window
//! has open, as the user does, instead of on a headless one of its own. The protocol, and the file with the port and the token, are
//! `lamina_core::bridge`; this is the threads that listen, the frame that answers, and what the settings keep of it ([`Agents`]).
//! Lifted from BIMCraft's `crates/ui-egui/src/control.rs` (same author, MIT OR Apache-2.0).
//!
//! The requests come from outside, so they are hostile: only 127.0.0.1 is bound, a connection that does not say the token in its
//! first line is dropped, a line is read to a limit, only a few connections are held, and a request that is no request is an error
//! reply. A request runs on the window's thread, in the frame, as a button's command does ([`App::answer_agents`]), so what the agent
//! makes is the user's to see, undo and save. While the user lets agents only read, or the agent asks to, it runs read-only. An agent
//! never replaces the plate (`new`, `open`) or sends it to a printer. Dropping the port stops it listening and hangs up on its agents.

use std::io::{self, BufRead, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use egui::{Context, RichText, Ui};
use lamina_core::bridge::{self, Access, Entry, Error, MAX_LINE, Request};
use serde::{Deserialize, Serialize};

use crate::App;
use crate::lang::{t, tf};

/// The most agents connected at once.
const MAX_CONNECTIONS: usize = 8;
/// How long a connection has, from the start, to say its first line whole, with the token in it.
const FIRST_LINE: Duration = Duration::from_secs(10);
/// How long a request waits for the window to answer: a slice or a boolean of a big plate takes a while.
const ANSWER: Duration = Duration::from_secs(600);
/// How many requests a frame answers.
const PER_FRAME: usize = 16;
/// What an agent of the window never runs: they replace the user's plate, or send it to a printer.
pub const REFUSED: &[&str] = &["new", "open", "send_to_printer"];

/// The Agents section as the settings file keeps it: whether the window opens the port, on which (0: any free one), and whether
/// agents may change the plate or only read it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Agents {
    pub listen: bool,
    pub port: u16,
    pub edit: bool,
}

impl Default for Agents {
    fn default() -> Self {
        Agents { listen: false, port: 0, edit: true }
    }
}

/// A request on its way to the window, and where its reply goes. An agent that gave up waiting says so, and the window does not do
/// a request nobody waits for any more.
pub struct Asked {
    pub request: Request,
    pub read_only: bool,
    pub reply: Sender<String>,
    pub abandoned: Arc<AtomicBool>,
}

/// What the threads of a port share with it: whether it is closing, and the agents connected.
#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    agents: Mutex<Vec<(u64, TcpStream)>>,
    next: AtomicU64,
}

impl Shared {
    fn arrived(&self, stream: &TcpStream) -> io::Result<u64> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let held = stream.try_clone()?;
        if let Ok(mut agents) = self.agents.lock() {
            agents.push((id, held));
        }
        Ok(id)
    }

    fn left(&self, id: u64) {
        if let Ok(mut agents) = self.agents.lock() {
            agents.retain(|(n, _)| *n != id);
        }
    }

    fn connected(&self) -> usize {
        self.agents.lock().map_or(0, |a| a.len())
    }
}

/// The listening port and what it has taken.
pub struct Control {
    /// Where agents find it: the port, and the token that lets them in.
    pub entry: Entry,
    asked: Receiver<Asked>,
    /// The window's context, once a frame has set it, so that a request wakes the window.
    wake: Arc<OnceLock<Context>>,
    /// The bridge file written for it, removed when the port closes.
    file: Option<PathBuf>,
    shared: Arc<Shared>,
}

impl Control {
    /// Listens on `port` of 127.0.0.1 (0: any that is free) and writes the bridge `file` with the port and a token of 128 random
    /// bits, for the user alone.
    pub fn start(port: u16, file: Option<PathBuf>) -> io::Result<Control> {
        Control::start_within(port, file, FIRST_LINE)
    }

    pub(crate) fn start_within(port: u16, file: Option<PathBuf>, first_line: Duration) -> io::Result<Control> {
        let token = lamina_assistant::seal::token().map_err(io::Error::other)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let entry = Entry { port: listener.local_addr()?.port(), token };
        if let Some(file) = &file {
            bridge::write_entry(file, &entry)?;
        }
        let (tx, asked) = channel();
        let wake = Arc::new(OnceLock::new());
        let shared = Arc::new(Shared::default());
        let (token, woken, live, held) = (entry.token.clone(), Arc::clone(&wake), Arc::new(AtomicUsize::new(0)), Arc::clone(&shared));
        std::thread::Builder::new().name("control-accept".into()).spawn(move || {
            for stream in listener.incoming() {
                if held.stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else {
                    // A failing accept (no file descriptors left, say) is waited out, not spun on.
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                };
                // One agent too many is turned away, not queued: dropping it closes it.
                if live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    live.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let (token, tx, woken, live, held) = (token.clone(), tx.clone(), Arc::clone(&woken), Arc::clone(&live), Arc::clone(&held));
                let spawned = std::thread::Builder::new().name("control-agent".into()).spawn(move || {
                    let mut id = None;
                    let _ = serve(stream, &token, &tx, &woken, first_line, &held, &mut id);
                    if let Some(id) = id {
                        held.left(id);
                        if let Some(ctx) = woken.get() {
                            ctx.request_repaint();
                        }
                    }
                    live.fetch_sub(1, Ordering::SeqCst);
                });
                if spawned.is_err() {
                    break;
                }
            }
        })?;
        Ok(Control { entry, asked, wake, file, shared })
    }

    /// How many agents are connected: those that have said a request with the token and not hung up.
    pub fn agents(&self) -> usize {
        self.shared.connected()
    }
}

impl Drop for Control {
    /// Closes the port: the agents are hung up on, and the listening thread is woken by a connection of its own to see that it is to
    /// stop, which lets the port go.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Ok(agents) = self.shared.agents.lock() {
            for (_, stream) in agents.iter() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        let _ = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, self.entry.port)), Duration::from_millis(500));
        if let Some(file) = &self.file {
            bridge::remove_entry(file, &self.entry);
        }
    }
}

/// One line of at most [`MAX_LINE`] bytes, or none at the end of what the agent sends; a longer one is an error.
fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = String::new();
    let read = reader.by_ref().take(MAX_LINE as u64 + 1).read_line(&mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if read > MAX_LINE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a request line is too long"));
    }
    Ok(Some(line))
}

/// The first line of a connection: whole within `within` of the start, and no longer than [`MAX_LINE`]. None where the connection
/// ends before it.
fn read_first_line(reader: &mut io::BufReader<TcpStream>, within: Duration) -> io::Result<Option<String>> {
    let start = Instant::now();
    let mut line = Vec::new();
    loop {
        let left = within
            .checked_sub(start.elapsed())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "the first line did not come in time"))?;
        reader.get_ref().set_read_timeout(Some(left))?;
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(None);
        }
        let end = buffer.iter().position(|&b| b == b'\n').map_or(buffer.len(), |at| at + 1);
        line.extend_from_slice(&buffer[..end]);
        let whole = buffer[end - 1] == b'\n';
        reader.consume(end);
        if line.len() > MAX_LINE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a request line is too long"));
        }
        if whole {
            return String::from_utf8(line).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
        }
    }
}

/// An agent's connection: a line in, the window's answer back, until it hangs up or says a wrong token. Until a request has come with
/// the token, the first line must come whole and soon, and what is not a request closes the connection (a web page's HTTP, say).
fn serve(
    stream: TcpStream,
    token: &str,
    tx: &Sender<Asked>,
    wake: &OnceLock<Context>,
    first_line: Duration,
    shared: &Shared,
    id: &mut Option<u64>,
) -> io::Result<()> {
    stream.set_nodelay(true)?;
    let mut reader = io::BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut known = false;
    loop {
        let line = if known { read_line(&mut reader)? } else { read_first_line(&mut reader, first_line)? };
        let Some(line) = line else { return Ok(()) };
        let reply = match bridge::parse(&line, token) {
            Err(e @ Error::Token) => {
                writeln!(writer, "{}", bridge::reply(Err(e.to_string())))?;
                return Ok(());
            }
            Err(e) if !known => {
                writeln!(writer, "{}", bridge::reply(Err(e.to_string())))?;
                return Ok(());
            }
            Err(e) => bridge::reply(Err(e.to_string())),
            Ok((request, read_only)) => {
                if !std::mem::replace(&mut known, true) {
                    // Once known, an agent may take as long as it likes between requests.
                    reader.get_ref().set_read_timeout(None)?;
                    *id = Some(shared.arrived(&writer)?);
                }
                let (reply, answer) = channel();
                let abandoned = Arc::new(AtomicBool::new(false));
                if tx.send(Asked { request, read_only, reply, abandoned: Arc::clone(&abandoned) }).is_err() {
                    return Ok(()); // the window has gone
                }
                if let Some(ctx) = wake.get() {
                    ctx.request_repaint();
                }
                answer.recv_timeout(ANSWER).unwrap_or_else(|_| {
                    abandoned.store(true, Ordering::SeqCst);
                    bridge::reply(Err("the window did not answer".into()))
                })
            }
        };
        writeln!(writer, "{reply}")?;
        writer.flush()?;
    }
}

impl App {
    /// Opens the port the Agents section says, in place of one that is open; why it could not is kept to be said.
    pub fn open_control(&mut self) {
        match Control::start(self.agents.port, self.bridge_file.clone()) {
            Ok(control) => (self.control, self.agents_failed) = (Some(control), None),
            Err(e) => {
                self.agents_failed =
                    Some(tf("Could not listen on port {} for agents: {}", &[&self.agents.port.to_string(), &e.to_string()]));
            }
        }
    }

    /// Closes the port: no agent reaches the plate, and those connected are hung up on.
    pub fn close_control(&mut self) {
        (self.control, self.agents_failed) = (None, None);
    }

    /// Once a frame: what agents asked since the last one is done, each as a button's command is, and answered. While a job that
    /// changes the plate runs, they wait.
    pub(crate) fn answer_agents(&mut self, ctx: &Context) {
        let Some(control) = &self.control else { return };
        let _ = control.wake.set(ctx.clone());
        if self.job.as_ref().is_some_and(|j| j.changes_plate) {
            return;
        }
        let asked: Vec<Asked> = std::iter::from_fn(|| control.asked.try_recv().ok()).take(PER_FRAME).collect();
        let more = asked.len() == PER_FRAME;
        for Asked { request, read_only, reply, abandoned } in asked {
            if abandoned.load(Ordering::SeqCst) {
                continue;
            }
            let result = match bridge::mentions(&self.session, &request, REFUSED) {
                Some(id) => Err(format!(
                    "`{id}` is not for an agent of the window: the user starts, opens and sends the plate from the window itself"
                )),
                None => {
                    let access = if self.agents.edit && !read_only { Access::Edit } else { Access::Read };
                    bridge::run(&mut self.session, &request, access)
                }
            };
            self.prune();
            let _ = reply.send(bridge::reply(result));
            ctx.request_repaint();
        }
        if more {
            ctx.request_repaint();
        }
    }

    /// The Agents section of the plate panel: let agents in or not, on which port, whether they may change the plate, and how to connect.
    pub(crate) fn agents_ui(&mut self, ui: &mut Ui) {
        egui::CollapsingHeader::new(t("Agents")).show(ui, |ui| {
            let mut on = self.control.is_some();
            let tip = t("Lets an AI agent (Claude Code, Codex, any MCP client) work on this plate through lamina-cli mcp --connect. Only \
                       programs on this computer that can read your settings folder get in.");
            if ui.checkbox(&mut on, t("Let agents work on this plate")).on_hover_text(tip).changed() {
                self.agents.listen = on;
                if on { self.open_control() } else { self.close_control() }
            }
            ui.horizontal(|ui| {
                ui.label(t("Port"));
                let port = ui.add(egui::DragValue::new(&mut self.agents.port).range(0..=65535)).on_hover_text(t("0: any free port"));
                if port.changed() && self.control.as_ref().is_some_and(|c| self.agents.port != 0 && c.entry.port != self.agents.port) {
                    self.open_control();
                }
            });
            ui.checkbox(&mut self.agents.edit, t("Agents may change the plate")).on_hover_text(t("Untick to let them only look"));
            match &self.control {
                Some(c) => {
                    let n = c.agents();
                    let who = if n == 0 { t("no agent yet").to_string() } else { tf("{} connected", &[&n.to_string()]) };
                    ui.label(RichText::new(tf("Listening on port {}: {}.", &[&c.entry.port.to_string(), &who])).small());
                    ui.label(RichText::new("lamina-cli mcp --connect").monospace().small());
                }
                None => {
                    ui.label(RichText::new(t("Off: agents cannot reach this plate.")).small().weak());
                }
            }
            if let Some(why) = &self.agents_failed {
                ui.label(RichText::new(why).small().color(crate::WARNING));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    /// A connection to `port`, saying lines and reading the answers.
    struct Agent {
        reader: BufReader<TcpStream>,
        writer: TcpStream,
    }

    impl Agent {
        fn connect(port: u16) -> Agent {
            let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            Agent { reader: BufReader::new(stream.try_clone().unwrap()), writer: stream }
        }

        fn say(&mut self, line: &str) -> Option<serde_json::Value> {
            writeln!(self.writer, "{line}").ok()?;
            let mut answer = String::new();
            (self.reader.read_line(&mut answer).ok()? > 0).then(|| serde_json::from_str(&answer).unwrap())
        }
    }

    #[test]
    fn a_port_turns_away_a_wrong_token_and_what_is_no_request_and_lets_go_when_dropped() {
        let control = Control::start_within(0, None, Duration::from_millis(300)).unwrap();
        let port = control.entry.port;
        // A wrong token: told so, and hung up on.
        let mut a = Agent::connect(port);
        let said = a.say(r#"{"token": "guess", "op": "hello"}"#).unwrap();
        assert!(said["error"].as_str().unwrap().contains("token"));
        assert!(a.say(r#"{"op": "hello"}"#).is_none(), "the connection is closed");
        // A web page's request: an error, and closed.
        let mut web = Agent::connect(port);
        assert!(web.say("GET / HTTP/1.1").unwrap()["error"].as_str().unwrap().contains("not JSON"));
        // Silence past the first line's time: closed.
        let mut quiet = Agent::connect(port);
        std::thread::sleep(Duration::from_millis(500));
        assert!(quiet.say(&format!(r#"{{"token": "{}", "op": "hello"}}"#, control.entry.token)).is_none());
        assert_eq!(control.agents(), 0);
        // Dropped, the port stops listening (the thread that listened lets go of it a moment later).
        drop(control);
        let until = Instant::now() + Duration::from_secs(5);
        while TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_millis(200)).is_ok() {
            assert!(Instant::now() < until, "port {port} still takes connections");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
