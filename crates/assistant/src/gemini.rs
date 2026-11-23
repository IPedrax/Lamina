//! Google's Gemini API (`POST /v1beta/models/{model}:streamGenerateContent?alt=sse`, read by [`Stream`]) with function calling:
//! the conversation goes in as `contents` of `user` and `model` turns, the tools as `functionDeclarations`, a `functionCall` part comes back, and the answer is
//! a `functionResponse` part in a user turn, with the call's `id` when it had one. The model's own turn goes back whole (its
//! `thoughtSignature` must come back in the part it came in). Declarations take a subset of OpenAPI, which has no free-form
//! object: [`schema`] writes one as text that is a JSON object, and the tools read it back (`tools::run`).

use serde_json::{Value, json};

use crate::net::Request;
use crate::{Call, Msg, Reply, SYSTEM, Usage, api_error, clip, count, json_body, tools, user_parts};

pub fn request(model: &str, key: &str, history: &[Msg]) -> Request {
    let contents: Vec<Value> = history
        .iter()
        .flat_map(|m| match m {
            Msg::User(text, files) => vec![json!({"role": "user", "parts": user_parts(
                text,
                files,
                |t| json!({"text": t}),
                |mime, b64| json!({"inlineData": {"mimeType": mime, "data": b64}}),
            )})],
            Msg::Assistant(reply) => vec![reply.raw.clone()],
            Msg::Results(done) => {
                let answers = json!({"role": "user", "parts": done.iter().map(|d| {
                    let mut call = json!({"name": d.name, "response": if d.error { json!({"error": d.text}) } else { json!({"result": d.text}) }});
                    if !d.id.is_empty() {
                        call["id"] = json!(d.id);
                    }
                    json!({"functionResponse": call})
                }).collect::<Vec<_>>()});
                // A function's response is words: its pictures follow in a turn of the user's.
                let pictures: Vec<Value> = done
                    .iter()
                    .filter_map(|d| d.image.as_ref())
                    .map(|png| json!({"inlineData": {"mimeType": "image/png", "data": png}}))
                    .collect();
                if pictures.is_empty() {
                    vec![answers]
                } else {
                    let mut parts = vec![json!({"text": crate::PICTURES})];
                    parts.extend(pictures);
                    vec![answers, json!({"role": "user", "parts": parts})]
                }
            }
        })
        .collect();
    let declarations: Vec<Value> = tools::all()
        .into_iter()
        .map(|t| {
            let mut declaration = json!({"name": t.name, "description": t.about});
            // A function with nothing to pass declares no parameters: an object with no properties is refused.
            if t.schema["properties"].as_object().is_some_and(|p| !p.is_empty()) {
                declaration["parameters"] = schema(&t.schema);
            }
            declaration
        })
        .collect();
    let body = json!({
        "systemInstruction": {"parts": [{"text": SYSTEM}]},
        "contents": contents,
        "tools": [{"functionDeclarations": declarations}],
        "generationConfig": {"thinkingConfig": {"includeThoughts": true}},
    });
    Request {
        url: format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse"),
        headers: vec![("x-goog-api-key".into(), key.into())],
        body: body.to_string(),
    }
}

/// A tool's JSON Schema as Gemini takes it: an object with no properties (a free-form one) is declared as a string that holds
/// a JSON object, and `additionalProperties` is left out.
fn schema(v: &Value) -> Value {
    let Some(map) = v.as_object() else { return v.clone() };
    if map.get("type").and_then(Value::as_str) == Some("object")
        && !map.get("properties").and_then(Value::as_object).is_some_and(|p| !p.is_empty())
    {
        let said = map.get("description").and_then(Value::as_str).unwrap_or("a JSON object");
        return json!({"type": "string", "description": format!("{said} (written as text: a JSON object)")});
    }
    map.iter().filter(|(k, _)| *k != "additionalProperties").map(|(k, v)| (k.clone(), schema(v))).collect::<serde_json::Map<_, _>>().into()
}

const WHO: &str = "Gemini";

/// A whole reply (a `GenerateContentResponse`), or an error's.
pub fn parse(status: u16, body: &str) -> Result<Reply, String> {
    if !(200..300).contains(&status) {
        return Err(api_error(WHO, status, body));
    }
    read(&json_body(WHO, body)?)
}

/// The stream: each event is a `GenerateContentResponse` with the next parts of the candidate. Every part is kept as it came, in
/// order (a thought signature must go back in the part it came in, and may come in a part with empty text), the last
/// `finishReason`, `usageMetadata` and `promptFeedback` win, and the model's turn is the parts together. A stream with a candidate
/// but no `finishReason` was cut off.
#[derive(Default)]
pub struct Stream {
    parts: Vec<Value>,
    candidate: bool,
    finish: Option<Value>,
    usage: Value,
    feedback: Value,
}

impl crate::Stream for Stream {
    const WHO: &'static str = WHO;

    fn event(&mut self, _name: &str, data: &str) -> Result<String, String> {
        let chunk = json_body(WHO, data)?;
        if let Some(e) = chunk.get("error") {
            let said = e["message"].as_str().or_else(|| e.as_str()).unwrap_or("no reason given");
            return Err(format!("{WHO} failed while answering: {}", clip(said, 300)));
        }
        if chunk["usageMetadata"].is_object() {
            self.usage = chunk["usageMetadata"].clone();
        }
        if chunk["promptFeedback"].is_object() {
            self.feedback = chunk["promptFeedback"].clone();
        }
        let mut text = String::new();
        if let Some(candidate) = chunk["candidates"].as_array().and_then(|c| c.first()) {
            self.candidate = true;
            for part in candidate["content"]["parts"].as_array().into_iter().flatten() {
                if part["thought"] != true && part.get("functionCall").is_none() {
                    text.push_str(part["text"].as_str().unwrap_or_default());
                }
                self.parts.push(part.clone());
            }
            if candidate["finishReason"].is_string() {
                self.finish = Some(candidate["finishReason"].clone());
            }
        }
        Ok(text)
    }

    fn finish(self) -> Result<Reply, String> {
        let candidates = match self.finish {
            Some(finish) => json!([{"content": {"role": "model", "parts": self.parts}, "finishReason": finish}]),
            None if self.candidate => return Err(format!("{WHO}'s reply stopped before its end: ask again")),
            // No answer at all: a blocked prompt says why in its feedback.
            None => json!([]),
        };
        read(&json!({"candidates": candidates, "usageMetadata": self.usage, "promptFeedback": self.feedback}))
    }
}

/// A whole `GenerateContentResponse`, as `parse` and [`Stream`] have it.
fn read(v: &Value) -> Result<Reply, String> {
    let Some(candidate) = v["candidates"].as_array().and_then(|c| c.first()) else {
        let why = v["promptFeedback"]["blockReason"].as_str().map(|r| format!(" ({r})")).unwrap_or_default();
        return Err(format!("{WHO} blocked the request or sent no answer{why}"));
    };
    let finish = candidate["finishReason"].as_str().unwrap_or("STOP");
    let (mut text, mut thinking, mut calls) = (String::new(), String::new(), Vec::new());
    for part in candidate["content"]["parts"].as_array().into_iter().flatten() {
        if let Some(call) = part.get("functionCall") {
            let Some(name) = call["name"].as_str() else { return Err(format!("{WHO} sent a tool call with no name")) };
            let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
            calls.push(Call { id: call["id"].as_str().unwrap_or_default().into(), name: name.into(), args });
        } else if let Some(t) = part["text"].as_str() {
            if part["thought"] == true { &mut thinking } else { &mut text }.push_str(t);
        }
    }
    match finish {
        "SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "RECITATION" | "IMAGE_SAFETY" => {
            return Err(format!("{WHO} declined this request ({})", clip(finish, 40)));
        }
        "MALFORMED_FUNCTION_CALL" | "UNEXPECTED_TOOL_CALL" => {
            return Err(format!("{WHO} wrote a tool call it could not send ({finish}): try again"));
        }
        "MAX_TOKENS" if !calls.is_empty() => {
            return Err(format!("{WHO}'s reply was cut off at its length limit, in the middle of a tool call: ask for less at once"));
        }
        "MAX_TOKENS" => text.push_str("\n\n(cut off at the length limit)"),
        _ => {}
    }
    let mut raw = candidate["content"].clone();
    if !raw.is_object() {
        raw = json!({"role": "model", "parts": []});
    }
    raw["role"] = json!("model");
    let u = &v["usageMetadata"];
    // Gemini counts the thinking apart from the candidates, and bills it as output.
    let output = count(&u["candidatesTokenCount"]).saturating_add(count(&u["thoughtsTokenCount"]));
    Ok(Reply { thinking, text, calls, raw, usage: Usage { input: count(&u["promptTokenCount"]), output } })
}
