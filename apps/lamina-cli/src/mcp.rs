//! Lamina's MCP server: JSON-RPC 2.0 over stdio, one message per line (MCP's stdio transport). Ported from BIMCraft's.
//! Its tools run the same commands as the CLI and scripts, through `bridge::run`, so an agent started with `--read-only` can look at
//! the scene and never change it.

use std::io::{BufRead, Write};

use lamina_core::Session;
use lamina_core::bridge::{self, Access, Request};
use serde_json::{Value, json};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED: &[&str] = &[PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const INSTRUCTIONS: &str = "Lamina is a 3D slicer and modelling tool. The scene is the objects on the print plate. Lengths are millimetres; \
x east, y north, z up; the origin is the middle of the plate and z = 0 is the plate's surface. Look first: inspect_scene shows every \
object, list_commands (with a filter such as scale or slice) shows a command's parameters. Change the scene with execute, for example \
add_primitive {kind: \"box\", size: [20, 20, 10]}, move {id, by: [x, y, z]}, rotate {id, axis: \"z\", degrees: 45}, scale {id, factor: 2}; or send \
several at once with script, one `command {json}` per line. A command that fails changes nothing.";

const READ_ONLY_NOTE: &str = " The scene is read-only for you: inspect it and read the catalog, but every command that would change it, or step \
through its undo history, is refused.";

pub struct Server {
    session: Session,
    access: Access,
}

fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn text(v: &Value) -> Value {
    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default()}]})
}

fn tool_error(e: &str) -> Value {
    json!({"content": [{"type": "text", "text": e}], "isError": true})
}

fn tools() -> Value {
    json!([
        {"name": "execute", "description": "Run any Lamina command with JSON params, e.g. add_primitive {kind: \"box\", size: [20, 20, 10]}, \
            move {id, by: [x, y, z]}, rotate {id, axis, degrees}, scale {id, factor}, delete {id}, undo. list_commands shows them all with their parameters.",
         "inputSchema": {"type": "object", "properties": {"command": {"type": "string"}, "params": {"type": "object"}}, "required": ["command"]}},
        {"name": "script", "description": "Run several commands, one per line: `add_primitive {\"kind\":\"box\",\"size\":[5,5,5]}`. `#` starts a comment. \
            Stops at the first failing line and names it; the lines before it stay done, each its own undo step.",
         "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}},
        {"name": "list_commands", "description": "Every command with its id, parameters and whether it touches files. `filter` matches part of an id or of the parameters' text.",
         "inputSchema": {"type": "object", "properties": {"filter": {"type": "string"}}}},
        {"name": "inspect_scene", "description": "The scene at a glance: every object with its id, name, size, volume and whether its mesh is closed (a slicer can only fill a closed one).",
         "inputSchema": {"type": "object", "properties": {}}}
    ])
}

impl Server {
    pub fn new(session: Session, access: Access) -> Server {
        Server { session, access }
    }

    /// Serve until `input` closes. Logs go to stderr: stdout carries only protocol messages.
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            if let Some(reply) = self.handle_line(&line?) {
                writeln!(output, "{reply}")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(Value::Array(batch)) if batch.is_empty() => Some(error(Value::Null, INVALID_REQUEST, "empty batch")),
            Ok(Value::Array(batch)) => {
                let replies: Vec<Value> = batch.into_iter().filter_map(|m| self.handle(m)).collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(msg) => self.handle(msg),
            Err(e) => Some(error(Value::Null, PARSE_ERROR, format!("parse error: {e}"))),
        };
        reply.map(|r| r.to_string())
    }

    fn handle(&mut self, msg: Value) -> Option<Value> {
        let Some(o) = msg.as_object() else { return Some(error(Value::Null, INVALID_REQUEST, "a message must be an object")) };
        let id = o.get("id").cloned();
        let Some(method) = o.get("method").and_then(Value::as_str) else {
            // A response to us: there is nothing to answer.
            if o.contains_key("result") || o.contains_key("error") {
                return None;
            }
            return Some(error(id.unwrap_or(Value::Null), INVALID_REQUEST, "missing `method`"));
        };
        // Notifications (no id) get no reply.
        let id = id?;
        let params = o.get("params").cloned().unwrap_or(Value::Null);
        Some(match self.request(method, &params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => error(id, code, message),
        })
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
                let version = if SUPPORTED.contains(&asked) { asked } else { PROTOCOL_VERSION };
                let mut instructions = INSTRUCTIONS.to_string();
                if self.access == Access::Read {
                    instructions.push_str(READ_ONLY_NOTE);
                }
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "lamina", "title": "Lamina", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": instructions,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "missing tool `name`".to_string()))?;
                self.call(name, params.get("arguments").unwrap_or(&Value::Null))
            }
            _ => Err((METHOD_NOT_FOUND, format!("method `{method}` not found"))),
        }
    }

    /// A tool call. `Err` is a protocol error (unknown tool, arguments that aren't an object or lack a required one); a tool that
    /// runs and fails answers with `isError`.
    fn call(&mut self, name: &str, args: &Value) -> Result<Value, (i64, String)> {
        let tools = tools();
        let tool = tools
            .as_array()
            .and_then(|a| a.iter().find(|t| t["name"] == name))
            .ok_or_else(|| (INVALID_PARAMS, format!("unknown tool `{name}`")))?;
        let args = match args {
            Value::Null => json!({}),
            Value::Object(_) => args.clone(),
            _ => return Err((INVALID_PARAMS, format!("the arguments of `{name}` must be an object"))),
        };
        for key in tool["inputSchema"]["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            if args.get(key).is_none_or(Value::is_null) {
                return Err((INVALID_PARAMS, format!("`{name}` needs `{key}`")));
            }
        }
        let (session, access) = (&mut self.session, self.access);
        let result = match name {
            "execute" => {
                let params = args.get("params").cloned().unwrap_or_else(|| json!({}));
                let request = Request::Execute { command: args["command"].as_str().unwrap_or_default().into(), params };
                bridge::run(session, &request, access)
            }
            "script" => bridge::run(session, &Request::Script { text: args["text"].as_str().unwrap_or_default().into() }, access),
            "list_commands" => {
                let filter = args["filter"].as_str().map(str::to_ascii_lowercase).unwrap_or_default();
                let has = |v: &Value| v.as_str().is_some_and(|t| t.to_ascii_lowercase().contains(&filter));
                bridge::run(session, &Request::Catalog, access).map(|all| {
                    all.as_array()
                        .map(|cmds| cmds.iter().filter(|c| has(&c["id"]) || has(&c["params"])).cloned().collect())
                        .unwrap_or_default()
                })
            }
            _ => bridge::run(session, &Request::Execute { command: "info".into(), params: json!({}) }, access),
        };
        Ok(match result {
            Ok(v) => text(&v),
            Err(e) => tool_error(&e),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(s: &mut Server, msg: Value) -> Value {
        serde_json::from_str(&s.handle_line(&msg.to_string()).unwrap()).unwrap()
    }

    fn call(s: &mut Server, name: &str, args: Value) -> Value {
        ask(s, json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}}))
    }

    #[test]
    fn it_introduces_itself_and_lists_its_tools() {
        let mut s = Server::new(Session::new(), Access::Edit);
        let hello = ask(&mut s, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}}));
        assert_eq!(hello["result"]["serverInfo"]["name"], "lamina");
        assert_eq!(hello["result"]["protocolVersion"], "2024-11-05");
        let list = ask(&mut s, json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 4);
        assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    }

    #[test]
    fn an_agent_models_and_looks() {
        let mut s = Server::new(Session::new(), Access::Edit);
        let made = call(&mut s, "execute", json!({"command": "add_primitive", "params": {"kind": "box", "size": [10, 10, 10]}}));
        assert!(made["result"]["isError"].is_null(), "{made}");
        let seen = call(&mut s, "inspect_scene", json!({}));
        assert!(seen["result"]["content"][0]["text"].as_str().unwrap().contains("\"watertight\": true"));
        let found = call(&mut s, "list_commands", json!({"filter": "scale"}));
        assert!(found["result"]["content"][0]["text"].as_str().unwrap().contains("\"id\": \"scale\""));
    }

    #[test]
    fn failures_are_tool_errors_and_protocol_mistakes_are_protocol_errors() {
        let mut s = Server::new(Session::new(), Access::Edit);
        let bad = call(&mut s, "execute", json!({"command": "move", "params": {"id": 9, "by": [0, 0, 0]}}));
        assert_eq!(bad["result"]["isError"], true);
        assert_eq!(call(&mut s, "execute", json!({}))["error"]["code"], INVALID_PARAMS);
        assert_eq!(call(&mut s, "nope", json!({}))["error"]["code"], INVALID_PARAMS);
        assert_eq!(ask(&mut s, json!({"jsonrpc": "2.0", "id": 3, "method": "x"}))["error"]["code"], METHOD_NOT_FOUND);
        let garbage: Value = serde_json::from_str(&s.handle_line("{not json").unwrap()).unwrap();
        assert_eq!(garbage["error"]["code"], PARSE_ERROR);
    }

    #[test]
    fn a_read_only_server_refuses_every_change() {
        let mut s = Server::new(Session::new(), Access::Read);
        let refused = call(&mut s, "execute", json!({"command": "add_primitive", "params": {"kind": "box", "size": [1, 1, 1]}}));
        assert_eq!(refused["result"]["isError"], true);
        assert!(call(&mut s, "script", json!({"text": "add_primitive {\"kind\":\"box\",\"size\":[1,1,1]}"}))["result"]["isError"] == true);
        assert!(call(&mut s, "inspect_scene", json!({}))["result"]["isError"].is_null());
    }
}
