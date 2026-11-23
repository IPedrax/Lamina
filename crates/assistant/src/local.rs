//! A model on the user's own computer, behind a server that speaks OpenAI's Chat Completions (`POST /v1/chat/completions`): Ollama,
//! LM Studio, llama.cpp's server, vLLM. Chat Completions and not the Responses API, because it is what these servers all serve. The
//! conversation goes in as `messages`, the tools as `function` tools, a tool's answer is a `tool` message naming the call's id, and
//! the model's own message goes back as it came. A picture goes in a user message (a tool message takes words only).
//!
//! The server is on this machine and nowhere else: [`endpoint`] takes the address the user typed, refuses any host but this
//! computer's, and writes `localhost` as 127.0.0.1, so the request goes to an address and no name lookup can send it elsewhere.
//! `net::allowed` lets plain HTTP through to 127.0.0.1 and ::1 only.

use serde_json::{Value, json};

use crate::net::Request;
use crate::{Call, Msg, PICTURES, Reply, SYSTEM, Usage, api_error, clip, count, json_body, tools, user_parts};

/// Ollama's address, the most common local server; LM Studio's is `http://localhost:1234/v1`.
pub const DEFAULT_URL: &str = "http://localhost:11434/v1";
/// The longest address accepted.
const MAX_URL: usize = 300;

/// The chat completions address of the server the user typed (`localhost:11434`, `http://127.0.0.1:1234/v1`), or why it is not one:
/// only this computer, plain HTTP, no user name, query or fragment. `localhost` becomes 127.0.0.1.
pub fn endpoint(typed: &str) -> Result<String, String> {
    const ONLY: &str = "a local model's server must be on this computer: http://localhost:PORT or http://127.0.0.1:PORT";
    let t = typed.trim();
    if t.is_empty() || t.len() > MAX_URL || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ONLY.into());
    }
    let rest = match t.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
        Some(_) => return Err(format!("{ONLY} (plain http: a local server has no certificate)")),
        None => t,
    };
    if rest.contains(['@', '?', '#', '\\']) {
        return Err(ONLY.into());
    }
    let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
    // The host, and the port after it: [::1]:1234, 127.0.0.1:1234, localhost:1234.
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6.split_once(']').ok_or(ONLY)?;
        (format!("[{h}]"), after.strip_prefix(':'))
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (authority.to_string(), None),
        }
    };
    let host = match host.to_ascii_lowercase().as_str() {
        "localhost" | "127.0.0.1" => "127.0.0.1".to_string(),
        "[::1]" => "[::1]".to_string(),
        _ => return Err(ONLY.into()),
    };
    let port = match port {
        None => String::new(),
        Some(p) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|n| n > 0) => {
            format!(":{p}")
        }
        Some(_) => return Err(ONLY.into()),
    };
    if !path.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.')) || path.contains("..") {
        return Err(ONLY.into());
    }
    let path = path.trim_matches('/');
    let path = if path.is_empty() { "v1".to_string() } else { path.to_string() };
    let path = path.strip_suffix("/chat/completions").unwrap_or(&path).to_string();
    let url = format!("http://{host}{port}/{path}/chat/completions");
    if crate::net::allowed(&url) { Ok(url) } else { Err(ONLY.into()) }
}

/// The request to the server at `url` (from [`endpoint`]); the key goes only when there is one (Ollama and LM Studio want none).
pub fn request(url: &str, model: &str, key: &str, history: &[Msg]) -> Request {
    let mut messages = vec![json!({"role": "system", "content": SYSTEM})];
    for m in history {
        match m {
            Msg::User(text, files) if files.is_empty() => messages.push(json!({"role": "user", "content": text})),
            Msg::User(text, files) => messages.push(json!({"role": "user", "content": user_parts(
                text,
                files,
                |t| json!({"type": "text", "text": t}),
                |mime, b64| json!({"type": "image_url", "image_url": {"url": format!("data:{mime};base64,{b64}")}}),
            )})),
            Msg::Assistant(reply) => messages.push(reply.raw.clone()),
            Msg::Results(done) => {
                messages.extend(done.iter().map(|d| json!({"role": "tool", "tool_call_id": d.id, "content": d.text})));
                let pictures: Vec<Value> = done
                    .iter()
                    .filter_map(|d| d.image.as_ref())
                    .map(|png| json!({"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{png}")}}))
                    .collect();
                if !pictures.is_empty() {
                    let mut content = vec![json!({"type": "text", "text": PICTURES})];
                    content.extend(pictures);
                    messages.push(json!({"role": "user", "content": content}));
                }
            }
        }
    }
    let tools: Vec<Value> = tools::all()
        .into_iter()
        .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.about, "parameters": t.schema}}))
        .collect();
    let body = json!({"model": model, "messages": messages, "tools": tools, "stream": false});
    let headers = if key.is_empty() { Vec::new() } else { vec![("Authorization".into(), format!("Bearer {key}"))] };
    Request { url: url.into(), headers, body: body.to_string() }
}

pub fn parse(status: u16, body: &str) -> Result<Reply, String> {
    const WHO: &str = "The local model's server";
    if !(200..300).contains(&status) {
        return Err(api_error(WHO, status, body));
    }
    let v = json_body(WHO, body)?;
    let choice =
        v["choices"].as_array().and_then(|c| c.first()).ok_or_else(|| format!("{WHO} sent no choices ({})", clip(body.trim(), 80)))?;
    let message = &choice["message"];
    if !message.is_object() {
        return Err(format!("{WHO} sent a choice with no message ({})", clip(body.trim(), 80)));
    }
    let text = message["content"].as_str().unwrap_or_default().to_string();
    // Servers that pass a reasoning model's thinking on name it one of these.
    let thinking = message["reasoning_content"].as_str().or_else(|| message["reasoning"].as_str()).unwrap_or_default().to_string();
    let mut raw_calls = Vec::new();
    let mut calls = Vec::new();
    for (i, c) in message["tool_calls"].as_array().into_iter().flatten().enumerate() {
        let Some(name) = c["function"]["name"].as_str() else {
            return Err(format!("{WHO} sent a tool call with no name"));
        };
        // A server that gives no id gets one, kept in the message that goes back, so the answer names its call.
        let id = c["id"].as_str().filter(|s| !s.is_empty()).map_or_else(|| format!("call_{i}"), str::to_string);
        let arguments = &c["function"]["arguments"];
        let args = match arguments {
            Value::String(s) if s.trim().is_empty() => json!({}),
            Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(clip(s, 200))),
            Value::Object(_) => arguments.clone(),
            _ => json!({}),
        };
        raw_calls.push(json!({"id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}));
        calls.push(Call { id, name: name.into(), args });
    }
    if text.is_empty() && calls.is_empty() {
        return match choice["finish_reason"].as_str() {
            Some("length") => Err(format!("{WHO}'s model ran out of room before it answered: try a model with a longer context")),
            _ => Err(format!("{WHO} sent an empty answer")),
        };
    }
    let mut raw = json!({"role": "assistant", "content": text});
    if !raw_calls.is_empty() {
        raw["tool_calls"] = json!(raw_calls);
    }
    let usage = Usage { input: count(&v["usage"]["prompt_tokens"]), output: count(&v["usage"]["completion_tokens"]) };
    Ok(Reply { thinking, text, calls, raw, usage })
}
