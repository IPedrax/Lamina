//! The tools the model is offered, run on the window's session through the same [`bridge::run`] that answers an agent connected
//! to the window, so they follow its rules (a window that lets agents only read lets the assistant only read). On top of that: no
//! command that reads or writes a file (a model steered by an object's name must not be able to write the user's disk), and every
//! answer is capped at [`MAX_RESULT`].

use lamina_core::bridge::{self, Access, Request};
use lamina_core::engine::{Files, Session};
use serde_json::{Value, json};

use crate::{Call, MAX_RESULT, MAX_TODO_TEXT, MAX_TODOS, clip};

/// The tool that writes the checklist the user watches. It changes nothing in the scene: the chat keeps what it says.
pub const TODO: &str = "todo_write";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    InProgress,
    Completed,
}

/// One step of the model's checklist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    pub content: String,
    pub status: Status,
}

/// The checklist a `todo_write` call writes, whole (each call replaces the list). `Err` is what the model is told.
pub fn todos(args: &Value) -> Result<Vec<Todo>, String> {
    let list = args.get("todos").and_then(Value::as_array).ok_or("`todo_write` needs `todos`, an array of {content, status}")?;
    if list.len() > MAX_TODOS {
        return Err(format!("a checklist has at most {MAX_TODOS} items: keep the steps that matter"));
    }
    list.iter()
        .map(|item| {
            let content =
                item["content"].as_str().map(str::trim).filter(|c| !c.is_empty()).ok_or("each item needs a `content`, some text")?;
            let status = match item["status"].as_str() {
                Some("pending") => Status::Pending,
                Some("in_progress") => Status::InProgress,
                Some("completed") => Status::Completed,
                _ => return Err("each item's `status` is pending, in_progress or completed".to_string()),
            };
            Ok(Todo { content: clip(content, MAX_TODO_TEXT), status })
        })
        .collect()
}

pub struct Tool {
    pub name: &'static str,
    pub about: &'static str,
    /// JSON Schema of its arguments: plain types only, so all three providers take it.
    pub schema: Value,
}

pub fn all() -> Vec<Tool> {
    vec![
        Tool {
            name: "execute",
            about: "Run a Lamina command with JSON params, e.g. add_primitive {kind: \"box\", size: [20,20,10]}, move {id, by: [x,y,z]}, \
                    rotate {id, axis, degrees}, scale {id, factor}, delete {id}, undo {}. list_commands shows every command with its \
                    parameters. A command that fails changes nothing; one that reads or writes a file is refused.",
            schema: json!({"type": "object", "properties": {
                "command": {"type": "string", "description": "a command id such as move"},
                "params": {"type": "object", "description": "the command's parameters, as JSON"},
            }, "required": ["command"]}),
        },
        Tool {
            name: "list_commands",
            about: "The commands with their id, parameters and whether they touch a file (`files`: execute refuses those). Pass a filter \
                    (part of an id or of the parameters text): the whole list is long.",
            schema: json!({"type": "object", "properties": {"filter": {"type": "string", "description": "part of an id or of the parameters text"}}}),
        },
        Tool {
            name: "inspect_scene",
            about: "The scene at a glance: every object on the plate with its id, name, triangles, size, position, volume and whether its mesh \
                    is watertight.",
            schema: json!({"type": "object", "properties": {}}),
        },
        Tool {
            name: "read_skill",
            about: "Skills are short notes on modelling for a printer, the slicer's settings and working from a picture. Call with no name for the \
                    list, then with a name to read one. Read the ones that fit before you model, slice or change a setting.",
            schema: json!({"type": "object", "properties": {"name": {"type": "string", "description": "a skill's name, from the list"}}}),
        },
        Tool {
            name: TODO,
            about: "Show the user your checklist for a request of three or more steps: the whole list each time, with exactly one item in_progress. \
                    Call it again as each step finishes. It changes nothing in the scene.",
            schema: json!({"type": "object", "properties": {
                "todos": {"type": "array", "description": "every step, in order", "items": {"type": "object", "properties": {
                    "content": {"type": "string", "description": "the step, short and imperative"},
                    "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                }, "required": ["content", "status"]}},
            }, "required": ["todos"]}),
        },
    ]
}

/// A tool's result as text for the model: compact JSON, cut at [`MAX_RESULT`] bytes, saying what was left out.
pub fn to_text(v: &Value) -> String {
    let text = v.to_string();
    if text.len() <= MAX_RESULT {
        return text;
    }
    let mut cut = MAX_RESULT;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}… [{} more bytes left out: ask for less, with a filter]", &text[..cut], text.len() - cut)
}

fn exec(session: &mut Session, command: &str, params: Value, access: Access) -> Result<Value, String> {
    bridge::run(session, &Request::Execute { command: command.into(), params }, access)
}

/// Runs a call on the session. `Err` is what the model is told went wrong.
pub fn run(session: &mut Session, call: &Call, access: Access) -> Result<Value, String> {
    let args = match &call.args {
        Value::Object(_) => call.args.clone(),
        Value::Null => json!({}),
        _ => return Err(format!("the arguments of `{}` must be a JSON object", call.name)),
    };
    match call.name.as_str() {
        "execute" => {
            let command = args["command"].as_str().ok_or("`execute` needs a `command`, a string")?;
            let params = match args.get("params") {
                None | Some(Value::Null) => json!({}),
                Some(p @ Value::Object(_)) => p.clone(),
                // Gemini declares the free-form object as text.
                Some(Value::String(s)) => serde_json::from_str(s).ok().filter(Value::is_object).ok_or("`params` is a JSON object")?,
                Some(_) => return Err("`params` is a JSON object".into()),
            };
            if let Some(c) = session.find(command).filter(|c| c.files != Files::None) {
                return Err(format!(
                    "`{}` reads or writes a file, which the assistant never does: the user opens, saves and exports from the app",
                    c.id
                ));
            }
            if let Some(c) = session.find(command).filter(|c| bridge::REPLACES.contains(&c.id)) {
                return Err(format!(
                    "`{}` would replace the user's whole plate and its undo history, which the assistant never does: the user starts or opens a project from the app",
                    c.id
                ));
            }
            exec(session, command, params, access)
        }
        "list_commands" => {
            let filter = args["filter"].as_str().map(str::to_ascii_lowercase).unwrap_or_default();
            let has = |v: &Value| v.as_str().is_some_and(|t| t.to_ascii_lowercase().contains(&filter));
            let all = bridge::run(session, &Request::Catalog, access)?;
            Ok(all.as_array().map(|cmds| cmds.iter().filter(|c| has(&c["id"]) || has(&c["params"])).cloned().collect()).unwrap_or_default())
        }
        "inspect_scene" => exec(session, "info", json!({}), access),
        "read_skill" => crate::skills::read(args["name"].as_str()),
        TODO => todos(&args).map(|_| json!("checklist shown")),
        other => Err(format!("unknown tool `{}`: execute, list_commands, inspect_scene, read_skill or todo_write", clip(other, 40))),
    }
}
