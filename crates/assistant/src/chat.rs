//! The chat: a conversation with a model that works on a [`Session`]. The window calls [`Chat::poll`] once a frame. A request to
//! the provider runs on a thread of its own (never the window's), and its reply is taken in the frame, where the tool calls in it
//! run on the window's session, as a command from a button does. Each command a call runs is an undo step of its own.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use lamina_core::bridge::Access;
use lamina_core::engine::Session;

use crate::attach::{self, Attached, Attachment};
use crate::net::Net;
use crate::tools::{self, TODO, Todo, to_text};
use crate::{
    Call, Done, MAX_CALLS, MAX_FILES_TOTAL, MAX_HISTORY, MAX_ROUNDS, MAX_TEXT, Msg, Provider, Reply, Usage, clip, complete, key_ok,
    model_ok,
};

/// How many lines of the log are kept.
const MAX_LOG: usize = 500;

/// What the palette shows, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    You(String),
    /// The names of the files that went with the message just above.
    Files(Vec<String>),
    /// What the model said of its reasoning, apart from its answer.
    Thinks(String),
    Says(String),
    /// A tool the model ran: its name, its arguments and what came back, both cut short for the eye.
    Used {
        name: String,
        args: String,
        result: String,
        error: bool,
    },
    /// Something the user should know: an error from the provider, a limit reached.
    Note(String),
}

/// A message of the user's that is being worked on.
struct Turn {
    key: String,
    provider: Provider,
    model: String,
    rounds: usize,
}

pub struct Chat {
    pub provider: Provider,
    pub model: String,
    pub log: Vec<Line>,
    pub(crate) history: Vec<Msg>,
    net: Arc<dyn Net>,
    /// What wakes the window when a reply has come: the window sets it before it sends.
    pub wake: Arc<dyn Fn() + Send + Sync>,
    flight: Option<Receiver<Result<Reply, String>>>,
    turn: Option<Turn>,
    /// The model's checklist, as it last wrote it: the palette shows it while the turn runs.
    pub todos: Vec<Todo>,
    /// Files picked for the next message, copied into `stage`.
    pub attached: Vec<Attachment>,
    /// The temporary folder those copies are in: made when the first file is picked, removed when the chat is cleared or dropped.
    stage: Option<tempfile::TempDir>,
    staged: usize,
    /// Tokens the provider has counted since the window last took them ([`Chat::take_usage`]).
    unreported: Usage,
    /// The provider the conversation the model has was made with: its reasoning and calls mean nothing to another.
    with: Option<Provider>,
}

impl Chat {
    pub fn new(net: Arc<dyn Net>) -> Chat {
        Chat {
            provider: Provider::Anthropic,
            model: Provider::Anthropic.default_model().into(),
            log: Vec::new(),
            history: Vec::new(),
            net,
            wake: Arc::new(|| {}),
            flight: None,
            turn: None,
            todos: Vec::new(),
            attached: Vec::new(),
            stage: None,
            staged: 0,
            unreported: Usage::default(),
            with: None,
        }
    }

    /// The tokens counted since the last call, which the window adds to what it keeps.
    pub fn take_usage(&mut self) -> Usage {
        std::mem::take(&mut self.unreported)
    }

    /// Copies a file the user picked into the chat's temporary folder, to go with the next message. Refused, with a note, when it is
    /// not text or a picture, is too big, or is one too many.
    pub fn attach(&mut self, from: &Path) {
        let held = self.attached.len();
        let dir = match &self.stage {
            Some(dir) => Ok(dir.path().to_path_buf()),
            None => tempfile::Builder::new()
                .prefix("lamina-assistant-")
                .tempdir()
                .map(|d| (d.path().to_path_buf(), self.stage = Some(d)).0)
                .map_err(|e| format!("could not make a temporary folder for the file: {e}")),
        };
        self.staged += 1;
        match dir.and_then(|dir| attach::stage(from, &dir, self.staged, held)) {
            Ok(a) => self.attached.push(a),
            Err(e) => self.note(e),
        }
    }

    /// Takes the picked file `i` off the next message and deletes its copy.
    pub fn detach(&mut self, i: usize) {
        if i < self.attached.len() {
            let _ = fs::remove_file(self.attached.remove(i).path);
        }
    }

    /// Where the copies of the files picked are, while there are any.
    pub fn stage_dir(&self) -> Option<&Path> {
        self.stage.as_ref().map(tempfile::TempDir::path)
    }

    /// Forgets the files: the folder and every copy in it go.
    fn drop_files(&mut self) {
        (self.attached, self.stage, self.staged) = (Vec::new(), None, 0);
    }

    /// Whether a message is being worked on.
    pub fn busy(&self) -> bool {
        self.turn.is_some()
    }

    /// Starts a new conversation. What the model was told is forgotten; the scene is as it is.
    pub fn clear(&mut self) {
        if !self.busy() {
            self.log.clear();
            self.history.clear();
            self.todos.clear();
            self.drop_files();
            self.with = None;
        }
    }

    fn note(&mut self, text: impl Into<String>) {
        self.log.push(Line::Note(text.into()));
        if self.log.len() > MAX_LOG {
            self.log.remove(0);
        }
    }

    /// Sends the user's message with `key`, and begins the turn. Refused, with a note in the log, when a message is being worked on,
    /// the message or the key is no good, the conversation is with another provider, or it has outgrown [`MAX_HISTORY`].
    pub fn send(&mut self, text: &str, key: &str) {
        let text = text.trim();
        if self.busy() || text.is_empty() {
            return;
        }
        if !key_ok(key) || !model_ok(&self.model) {
            return self.note("Give a key and a model id first.");
        }
        if text.chars().count() > MAX_TEXT {
            return self.note(format!("That message is longer than {MAX_TEXT} characters: shorten it."));
        }
        // The model's own turns go back as they came, and another provider can't take them.
        if let Some(was) = self.with.filter(|p| *p != self.provider && self.history.iter().any(|m| matches!(m, Msg::Assistant(_)))) {
            return self.note(format!("This chat is with {}: clear it to ask {}.", was.name(), self.provider.name()));
        }
        let Some(files) = self.load_attached() else { return };
        if self.history.iter().map(Msg::size).sum::<usize>() + text.len() > MAX_HISTORY {
            return self.note("This conversation is too long to send again: clear the chat and start a new one.");
        }
        let sent: usize = self.history.iter().map(Msg::file_bytes).sum::<usize>() + files.iter().map(|f| f.len()).sum::<usize>();
        if sent > MAX_FILES_TOTAL {
            return self.note("The files in this conversation are more than can be sent again: clear the chat, or send fewer.");
        }
        self.with = Some(self.provider);
        self.log.push(Line::You(text.into()));
        if !files.is_empty() {
            self.log.push(Line::Files(files.iter().map(|f| f.name.clone()).collect()));
        }
        self.todos.clear();
        self.history.push(Msg::User(text.into(), files));
        self.turn = Some(Turn { key: key.into(), provider: self.provider, model: self.model.clone(), rounds: 0 });
        if let Err(e) = self.launch() {
            self.note(e);
            self.turn = None;
        }
        if self.busy() {
            self.attached.clear();
        }
    }

    /// Reads the files picked for the next message, which stay picked; `None`, with a note, when one can no longer be read.
    pub fn load_attached(&mut self) -> Option<Vec<Attached>> {
        let mut files = Vec::with_capacity(self.attached.len());
        for a in &self.attached {
            match a.load() {
                Ok(f) => files.push(f),
                Err(e) => {
                    self.note(e);
                    return None;
                }
            }
        }
        Some(files)
    }

    /// Asks the model again, on a thread.
    fn launch(&mut self) -> Result<(), String> {
        let Some(turn) = &mut self.turn else { return Ok(()) };
        turn.rounds += 1;
        if turn.rounds > MAX_ROUNDS {
            return Err(format!("Stopped after {MAX_ROUNDS} replies to one message. What was done stays; ask again to go on."));
        }
        if self.history.iter().map(Msg::size).sum::<usize>() > MAX_HISTORY {
            return Err("Stopped: the conversation has grown past what is sent at once. Clear the chat to start a new one.".into());
        }
        let (provider, model, key) = (turn.provider, turn.model.clone(), turn.key.clone());
        let (net, history, wake) = (Arc::clone(&self.net), self.history.clone(), Arc::clone(&self.wake));
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("assistant".into())
            .spawn(move || {
                let _ = tx.send(complete(provider, net.as_ref(), &model, &key, &history));
                wake();
            })
            .map_err(|e| format!("could not start the request: {e}"))?;
        self.flight = Some(rx);
        Ok(())
    }

    /// Once a frame: takes the reply if it has come, runs the tools it asks for on `session`, and asks again, or ends the turn.
    /// Whether anything changed (the window repaints then).
    pub fn poll(&mut self, session: &mut Session, access: Access) -> bool {
        let got = match self.flight.as_ref().map(Receiver::try_recv) {
            None | Some(Err(TryRecvError::Empty)) => return false,
            Some(Ok(reply)) => reply,
            Some(Err(TryRecvError::Disconnected)) => Err("the request stopped without an answer".into()),
        };
        self.flight = None;
        let reply = match got {
            Ok(reply) => reply,
            Err(e) => {
                self.note(e);
                self.turn = None;
                return true;
            }
        };
        self.unreported.add(reply.usage);
        if !reply.thinking.trim().is_empty() {
            self.log.push(Line::Thinks(clip(reply.thinking.trim(), 20_000)));
        }
        if !reply.text.trim().is_empty() {
            self.log.push(Line::Says(reply.text.trim().into()));
        }
        let calls = reply.calls.clone();
        self.history.push(Msg::Assistant(reply));
        if calls.is_empty() {
            self.turn = None;
            return true;
        }
        let done = batch(session, &calls, access);
        for (call, d) in calls.iter().zip(&done) {
            // The checklist is drawn as a list, not as a call in the log; one that was refused is shown, so the user sees why it is not.
            if call.name == TODO && !d.error {
                self.todos = tools::todos(&call.args).unwrap_or_default();
                continue;
            }
            self.log.push(Line::Used {
                name: call.name.clone(),
                args: clip(&call.args.to_string(), 300),
                result: clip(&d.text, 600),
                error: d.error,
            });
        }
        self.history.push(Msg::Results(done));
        if let Err(e) = self.launch() {
            self.note(e);
            self.turn = None;
        }
        true
    }

    /// Gives up on the request in flight; what the turn already did stays.
    pub fn stop(&mut self) {
        self.flight = None;
        if self.busy() {
            self.note("Stopped.");
            self.turn = None;
        }
    }

    /// Forgets the conversation and whatever is in flight, for a window whose scene was replaced: the reply to a request made about
    /// the old one must not run its tools on the new one. The provider, the model and the network stay.
    pub fn reset(&mut self) {
        (self.flight, self.turn) = (None, None);
        self.log.clear();
        self.history.clear();
        self.todos.clear();
        self.drop_files();
        self.unreported = Usage::default();
        self.with = None;
    }
}

/// Runs a reply's calls: a call that fails is told so and the others go on (a failed command has changed nothing).
fn batch(session: &mut Session, calls: &[Call], access: Access) -> Vec<Done> {
    calls
        .iter()
        .enumerate()
        .map(|(i, call)| {
            let result = if i < MAX_CALLS {
                tools::run(session, call, access)
            } else {
                Err(format!("only {MAX_CALLS} tool calls are run in one reply: ask again for the rest"))
            };
            let (text, error) = match result {
                Ok(v) => (to_text(&v), false),
                Err(e) => (e, true),
            };
            Done { id: call.id.clone(), name: call.name.clone(), text, error }
        })
        .collect()
}
