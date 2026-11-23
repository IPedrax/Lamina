//! Anthropic's Messages API (`POST /v1/messages`, streamed and read by [`Stream`]) with tool use, written from the public
//! reference for raw HTTP: the conversation goes in as `messages`, the tools as `tools` with an `input_schema`, a tool's answer
//! comes back as a `tool_result` block in a user message naming the `tool_use` block's id, and the assistant's `content` array
//! goes back whole, its thinking blocks included (and their summaries shown to the user apart from the answer), because the API
//! reads them only in the conversation that made them.

use serde_json::{Value, json};

use crate::net::Request;
use crate::{Call, Msg, Reply, SYSTEM, Usage, api_error, clip, count, json_body, tools, user_parts};

const URL: &str = "https://api.anthropic.com/v1/messages";
/// The API's dated version of itself, as the current reference sends it. Nothing here sends a sampling parameter, which that version refuses.
const VERSION: &str = "2026-10-21";
/// Room for a model that thinks before it calls: an answer is a few lines, but a reply that runs out here is cut off.
const MAX_TOKENS: u32 = 16_000;

pub fn request(model: &str, key: &str, history: &[Msg]) -> Request {
    let messages: Vec<Value> = history
        .iter()
        .map(|m| match m {
            Msg::User(text, files) if files.is_empty() => json!({"role": "user", "content": text}),
            Msg::User(text, files) => json!({"role": "user", "content": user_parts(
                text,
                files,
                |t| json!({"type": "text", "text": t}),
                |mime, b64| json!({"type": "image", "source": {"type": "base64", "media_type": mime, "data": b64}}),
            )}),
            Msg::Assistant(reply) => json!({"role": "assistant", "content": reply.raw}),
            // A picture goes in the tool's answer itself, beside its words.
            Msg::Results(done) => json!({"role": "user", "content": done.iter().map(|d| {
                let content = match &d.image {
                    Some(png) => json!([
                        {"type": "text", "text": d.text},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": png}},
                    ]),
                    None => json!(d.text),
                };
                json!({"type": "tool_result", "tool_use_id": d.id, "content": content, "is_error": d.error})
            }).collect::<Vec<_>>()}),
        })
        .collect();
    let tools: Vec<Value> =
        tools::all().into_iter().map(|t| json!({"name": t.name, "description": t.about, "input_schema": t.schema})).collect();
    // Summarized, because the default hides the reasoning (an empty block) and the palette shows it apart from the answer.
    let thinking = json!({"type": "adaptive", "display": "summarized"});
    let body = json!({"model": model, "max_tokens": MAX_TOKENS, "thinking": thinking, "system": SYSTEM, "tools": tools,
        "messages": messages, "stream": true});
    Request {
        url: URL.into(),
        headers: vec![("x-api-key".into(), key.into()), ("anthropic-version".into(), VERSION.into())],
        body: body.to_string(),
    }
}

const WHO: &str = "Anthropic";

/// A whole reply (a `Message`), or an error's.
pub fn parse(status: u16, body: &str) -> Result<Reply, String> {
    if !(200..300).contains(&status) {
        return Err(api_error(WHO, status, body));
    }
    read(&json_body(WHO, body)?)
}

/// The stream's events, put back together as the `Message` they make: `message_start` gives the message, each content block is
/// started, grown by its deltas (a tool's input as JSON text, read when the stream ends) and stopped, `message_delta` gives the stop
/// reason and the counts so far (they are cumulative), and `message_stop` ends it. Pings and events of kinds added later are passed by.
#[derive(Default)]
pub struct Stream {
    message: Value,
    ended: bool,
}

impl crate::Stream for Stream {
    const WHO: &'static str = WHO;

    fn event(&mut self, _name: &str, data: &str) -> Result<String, String> {
        let e = json_body(WHO, data)?;
        let bad = || format!("{WHO} sent a stream that is not what it documents ({})", clip(data, 80));
        match e["type"].as_str() {
            Some("message_start") if e["message"]["content"].is_array() => self.message = e["message"].clone(),
            Some("content_block_start") => {
                let content = self.message["content"].as_array_mut().ok_or_else(bad)?;
                if e["index"].as_u64() != Some(content.len() as u64) || !e["content_block"].is_object() {
                    return Err(bad());
                }
                content.push(e["content_block"].clone());
            }
            Some("content_block_delta") => {
                let at = e["index"].as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(bad)?;
                let block = self.message["content"].get_mut(at).and_then(Value::as_object_mut).ok_or_else(bad)?;
                let d = &e["delta"];
                let (key, piece) = match d["type"].as_str() {
                    Some("text_delta") => ("text", &d["text"]),
                    Some("thinking_delta") => ("thinking", &d["thinking"]),
                    Some("input_json_delta") => ("input", &d["partial_json"]),
                    Some("signature_delta") => {
                        block.insert("signature".into(), d["signature"].clone());
                        return Ok(String::new());
                    }
                    _ => return Ok(String::new()),
                };
                let piece = piece.as_str().unwrap_or_default();
                // The input starts as an object and grows as text: the first piece replaces the object.
                match block.get_mut(key) {
                    Some(Value::String(grown)) => grown.push_str(piece),
                    _ => {
                        block.insert(key.into(), piece.into());
                    }
                }
                return Ok(if key == "text" { piece.to_string() } else { String::new() });
            }
            Some("message_delta") => {
                let message = self.message.as_object_mut().ok_or_else(bad)?;
                for (k, v) in e["delta"].as_object().into_iter().flatten() {
                    message.insert(k.clone(), v.clone());
                }
                if let Some(usage) = message.entry("usage").or_insert_with(|| json!({})).as_object_mut() {
                    usage.extend(e["usage"].as_object().into_iter().flatten().map(|(k, v)| (k.clone(), v.clone())));
                }
            }
            Some("message_stop") => self.ended = true,
            Some("error") => {
                let said = e["error"]["message"].as_str().or_else(|| e["error"]["type"].as_str()).unwrap_or("no reason given");
                return Err(format!("{WHO} failed while answering: {}", clip(said, 300)));
            }
            Some("message_start") => return Err(bad()),
            _ => {}
        }
        Ok(String::new())
    }

    fn finish(mut self) -> Result<Reply, String> {
        if !self.ended {
            return Err(format!("{WHO}'s reply stopped before its end: ask again"));
        }
        // A tool's input came as JSON text, in pieces: whole now, it is the object it always is in a `Message`.
        for block in self.message["content"].as_array_mut().into_iter().flatten().filter_map(Value::as_object_mut) {
            if let Some(Value::String(text)) = block.get("input") {
                let input = match text.trim() {
                    "" => json!({}),
                    text => serde_json::from_str(text).map_err(|_| format!("{WHO} sent a tool call whose input is not JSON"))?,
                };
                block.insert("input".into(), input);
            }
        }
        read(&self.message)
    }
}

/// A whole `Message`, as `parse` and [`Stream`] have it.
fn read(v: &Value) -> Result<Reply, String> {
    let blocks = v["content"].as_array().ok_or_else(|| format!("{WHO} sent a reply with no content ({})", clip(&v.to_string(), 80)))?;
    let (mut text, mut thinking, mut calls) = (String::new(), String::new(), Vec::new());
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => text.push_str(block["text"].as_str().unwrap_or_default()),
            Some("thinking") => thinking.push_str(block["thinking"].as_str().unwrap_or_default()),
            Some("tool_use") => {
                let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) else {
                    return Err(format!("{WHO} sent a tool call with no id or name"));
                };
                calls.push(Call { id: id.into(), name: name.into(), args: block["input"].clone() });
            }
            _ => {}
        }
    }
    match v["stop_reason"].as_str() {
        Some("refusal") => {
            let why = v["stop_details"]["category"].as_str().map(|c| format!(" ({c})")).unwrap_or_default();
            return Err(format!("{WHO} declined this request{why}"));
        }
        // A cut-off call's input is a partial object that parses as a whole one: nothing of it runs.
        Some("max_tokens") if !calls.is_empty() => {
            return Err(format!("{WHO}'s reply was cut off at its length limit, in the middle of a tool call: ask for less at once"));
        }
        Some("max_tokens") => text.push_str("\n\n(cut off at the length limit)"),
        _ => {}
    }
    let u = &v["usage"];
    // The cached part of the prompt is counted apart by the API and is still what the request carried.
    let input = count(&u["input_tokens"])
        .saturating_add(count(&u["cache_read_input_tokens"]))
        .saturating_add(count(&u["cache_creation_input_tokens"]));
    Ok(Reply { thinking, text, calls, raw: v["content"].clone(), usage: Usage { input, output: count(&u["output_tokens"]) } })
}
