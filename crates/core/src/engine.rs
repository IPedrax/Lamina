//! The engine. Every front end (the CLI, scripts, the MCP server, the AI chat, the desktop) changes the scene only through
//! [`Session::execute`], so a new behaviour is a new command with a test, never a new path into the scene.
//! (The same design as BIMCraft's engine, from which the assistant and the MCP server are ported.)

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::commands;
use crate::scene::Scene;

pub const UNDO_LIMIT: usize = 200;

pub type Run = fn(&mut Session, &Value) -> Result<Value, String>;

/// What a command does with the disk, so a front end that must not touch it (the AI chat) can refuse by reading the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Files {
    None,
    Read,
    Write,
}

pub struct Command {
    /// Lower-case id.
    pub id: &'static str,
    /// The JSON parameters, written for agents: they read this to call it.
    pub params: &'static str,
    pub run: Run,
    /// Whether a change it makes is an undo step.
    pub undoable: bool,
    pub files: Files,
}

#[derive(Default)]
pub struct Session {
    pub scene: Arc<Scene>,
    /// The front end's print settings and preset labels (`{"profile": {...}, "printer", "filament"}`), kept in the project file. The
    /// core does not read them; the slicer lays `profile` under the settings a slice is given. Not part of the undo history.
    pub print: Value,
    undo: Vec<(String, Arc<Scene>)>,
    redo: Vec<(String, Arc<Scene>)>,
    /// Command sets other crates add (the slicer's), beside [`commands::ALL`].
    extra: Vec<&'static [Command]>,
}

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    /// Adds a crate's commands to this session.
    pub fn register(&mut self, commands: &'static [Command]) {
        self.extra.push(commands);
    }

    /// A command by id, in any case.
    pub fn find(&self, name: &str) -> Option<&'static Command> {
        commands::ALL.iter().chain(self.extra.iter().flat_map(|s| s.iter())).find(|c| c.id.eq_ignore_ascii_case(name))
    }

    /// Every command as an agent reads it.
    pub fn catalog(&self) -> Value {
        let files = |f: Files| match f {
            Files::None => Value::Null,
            Files::Read => json!("read"),
            Files::Write => json!("write"),
        };
        let all = commands::ALL.iter().chain(self.extra.iter().flat_map(|s| s.iter()));
        Value::Array(all.map(|c| json!({"id": c.id, "params": c.params, "files": files(c.files)})).collect())
    }

    /// For a command that edits: a copy of the scene it may change in place.
    pub fn scene_mut(&mut self) -> &mut Scene {
        Arc::make_mut(&mut self.scene)
    }

    /// Runs a command. One that fails, panics or leaves the scene failing [`Scene::check`] changes nothing.
    pub fn execute(&mut self, name: &str, params: &Value) -> Result<Value, String> {
        let cmd = self.find(name).ok_or_else(|| format!("unknown command `{name}`; the catalog lists them"))?;
        let before = Arc::clone(&self.scene);
        let result = catch_unwind(AssertUnwindSafe(|| (cmd.run)(self, params)))
            .unwrap_or_else(|p| Err(format!("internal error in `{}`: {}", cmd.id, panic_text(p.as_ref()))))
            .and_then(|v| if Arc::ptr_eq(&before, &self.scene) { Ok(v) } else { self.scene.check().map(|()| v) });
        match result {
            Err(e) => {
                self.scene = before;
                Err(e)
            }
            Ok(v) => {
                if cmd.undoable && !Arc::ptr_eq(&before, &self.scene) {
                    self.undo.push((cmd.id.to_string(), before));
                    if self.undo.len() > UNDO_LIMIT {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                }
                Ok(v)
            }
        }
    }

    /// Puts another project in this one's place (`open`, `new`): its scene and print settings, and a history that starts here.
    pub fn replace(&mut self, scene: Scene, print: Value) {
        self.scene = Arc::new(scene);
        self.print = print;
        self.undo.clear();
        self.redo.clear();
    }

    /// Whether there is a step to undo, and one to redo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Steps back; says what it took back.
    pub fn undo(&mut self) -> Result<String, String> {
        let (label, scene) = self.undo.pop().ok_or("nothing to undo")?;
        self.redo.push((label.clone(), std::mem::replace(&mut self.scene, scene)));
        Ok(label)
    }

    pub fn redo(&mut self) -> Result<String, String> {
        let (label, scene) = self.redo.pop().ok_or("nothing to redo")?;
        self.undo.push((label.clone(), std::mem::replace(&mut self.scene, scene)));
        Ok(label)
    }

    /// A script: one `id {json}` a line, `#` for comments. Stops at the first line that fails and says which.
    pub fn script(&mut self, text: &str) -> Result<Vec<Value>, String> {
        let mut out = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (id, rest) = line.split_once(char::is_whitespace).unwrap_or((line, "{}"));
            let params: Value = serde_json::from_str(rest.trim()).map_err(|e| format!("line {}: parameters are not JSON: {e}", n + 1))?;
            out.push(self.execute(id, &params).map_err(|e| format!("line {}: {e}", n + 1))?);
        }
        Ok(out)
    }
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "a panic".into())
}
