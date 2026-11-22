//! What a connected agent (the MCP server, the AI chat) may ask of a session, and the rules that go with it. A request is a
//! command, a script or the catalog; an agent that may only read can look at the scene and export, and never change it.

use serde_json::{Value, json};

use crate::engine::Session;

/// What the front end lets an agent do to the scene: change it as the user does, or only look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Edit,
    Read,
}

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

/// Refuses a command that would change the scene or its history when the agent may only read.
fn may_run(session: &Session, command: &str, access: Access) -> Result<(), String> {
    let Some(c) = session.find(command) else { return Ok(()) }; // `execute` says it is unknown
    if access == Access::Read && (c.undoable || matches!(c.id, "undo" | "redo")) {
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
        for (c, p) in [("add_primitive", json!({"kind": "box", "size": [1, 1, 1]})), ("delete", json!({"id": 1})), ("undo", json!({}))] {
            assert!(run(&mut s, &execute(c, p), Access::Read).unwrap_err().contains("only read"), "{c}");
        }
        let script = Request::Script { text: "info\ndelete {\"id\":1}\n".into() };
        assert!(run(&mut s, &script, Access::Read).unwrap_err().starts_with("line 2:"));
        assert_eq!(s.scene.objects.len(), 1);
    }

    #[test]
    fn the_catalog_lists_every_command_with_its_parameters() {
        let mut s = Session::new();
        let all = run(&mut s, &Request::Catalog, Access::Read).unwrap();
        assert!(all.as_array().unwrap().iter().any(|c| c["id"] == "import_mesh" && c["files"] == "read"));
    }
}
