//! The provider clients against recorded replies (a text answer, a tool call, two tool calls, a malformed reply, a refusal), the
//! tools on a session, and the loop of a chat against a fake network. Nothing here reaches a real one.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lamina_core::bridge::Access;
use lamina_core::engine::Session;
use serde_json::{Value, json};

use crate::attach::base64;
use crate::net::{Net, Request, Ureq, allowed};
use crate::testing::Fake;
use crate::{
    Attached, Body, Call, Chat, Done, Line, MAX_FILES, MAX_FILES_TOTAL, Msg, Provider, Reply, Status, Usage, anthropic, clip, complete,
    gemini, key_ok, model_ok, openai, tools,
};

const KEY: &str = "sk-test-0123456789abcdef";

fn run(s: &mut Session, name: &str, args: Value) -> Result<Value, String> {
    tools::run(s, &Call { id: "c".into(), name: name.into(), args }, Access::Edit)
}

/// How many objects are on the plate, as the model would see them.
fn objects(s: &mut Session) -> usize {
    run(s, "inspect_scene", json!({})).unwrap()["objects"].as_array().unwrap().len()
}

// ---------- the clients, against recorded replies ----------

const BOX: &str = r#"{"command":"add_primitive","params":{"kind":"box","size":[20,20,10]}}"#;

fn anthropic_reply(content: Value, stop: &str) -> String {
    json!({"id": "msg_01", "type": "message", "role": "assistant", "model": "claude-sonnet-5-5", "content": content,
        "stop_reason": stop, "usage": {"input_tokens": 10, "output_tokens": 5}})
    .to_string()
}

#[test]
fn anthropic_reads_a_text_answer_and_keeps_the_thinking_for_the_next_request() {
    let content = json!([{"type": "thinking", "thinking": "", "signature": "sig"}, {"type": "text", "text": "Hello "}, {"type": "text", "text": "there"}]);
    let r = anthropic::parse(200, &anthropic_reply(content.clone(), "end_turn")).unwrap();
    assert_eq!((r.text.as_str(), r.calls.len()), ("Hello there", 0));
    assert_eq!(r.raw, content);
}

#[test]
fn anthropic_reads_one_tool_call_and_sends_its_answer_back_by_id() {
    let content = json!([{"type": "text", "text": "Adding."}, {"type": "tool_use", "id": "toolu_01", "name": "execute", "input": serde_json::from_str::<Value>(BOX).unwrap()}]);
    let r = anthropic::parse(200, &anthropic_reply(content.clone(), "tool_use")).unwrap();
    assert_eq!(r.text, "Adding.");
    assert_eq!(r.calls, vec![Call { id: "toolu_01".into(), name: "execute".into(), args: serde_json::from_str(BOX).unwrap() }]);
    let done = Done { id: "toolu_01".into(), name: "execute".into(), text: "{\"id\":1}".into(), error: false, image: None };
    let sent = anthropic::request("claude-sonnet-5-5", KEY, &[Msg::user("a box"), Msg::Assistant(r), Msg::Results(vec![done])]);
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(body["model"], "claude-sonnet-5-5");
    assert_eq!(body["tools"].as_array().unwrap().len(), tools::all().len());
    assert!(body["tools"][0]["input_schema"]["properties"].is_object());
    assert_eq!(body["messages"][0], json!({"role": "user", "content": "a box"}));
    assert_eq!(body["messages"][1], json!({"role": "assistant", "content": content}), "the assistant's turn goes back as it came");
    assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "toolu_01");
    assert!(body.get("temperature").is_none(), "the current API version refuses sampling parameters");
    assert_eq!(body["stream"], true);
    assert_eq!(sent.url, "https://api.anthropic.com/v1/messages");
    assert!(sent.headers.contains(&("x-api-key".into(), KEY.into())));
    assert!(!sent.body.contains(KEY) && !sent.url.contains(KEY), "the key is in a header and nowhere else");
}

#[test]
fn anthropic_reads_two_tool_calls_in_one_reply() {
    let content = json!([
        {"type": "tool_use", "id": "toolu_a", "name": "inspect_scene", "input": {}},
        {"type": "tool_use", "id": "toolu_b", "name": "list_commands", "input": {"filter": "move"}},
    ]);
    let r = anthropic::parse(200, &anthropic_reply(content, "tool_use")).unwrap();
    assert_eq!(r.calls.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["toolu_a", "toolu_b"]);
    assert_eq!(r.calls[1].args["filter"], "move");
}

#[test]
fn anthropic_malformed_replies_are_errors_not_panics() {
    for body in
        ["", "not json", "[]", "{}", r#"{"content": "text"}"#, r#"{"content": [{"type": "tool_use", "name": "execute", "input": {}}]}"#]
    {
        assert!(anthropic::parse(200, body).is_err(), "{body:?}");
    }
    let e = anthropic::parse(401, r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#).unwrap_err();
    assert!(e.contains("401") && e.contains("invalid x-api-key") && e.contains("check the key"), "{e}");
    assert!(anthropic::parse(529, "<html>overloaded</html>").unwrap_err().contains("529"));
    // A reply cut off in a tool call runs nothing; cut off in words it is shown, with a note.
    let call = json!([{"type": "tool_use", "id": "t", "name": "execute", "input": {"command": "add_primit"}}]);
    assert!(anthropic::parse(200, &anthropic_reply(call, "max_tokens")).unwrap_err().contains("cut off"));
    let words = anthropic::parse(200, &anthropic_reply(json!([{"type": "text", "text": "Half"}]), "max_tokens")).unwrap();
    assert!(words.text.starts_with("Half") && words.text.contains("cut off"));
}

#[test]
fn anthropic_refusal_is_said_with_its_category() {
    let body = json!({"content": [], "stop_reason": "refusal", "stop_details": {"type": "refusal", "category": "cyber"}}).to_string();
    let e = anthropic::parse(200, &body).unwrap_err();
    assert!(e.contains("declined") && e.contains("cyber"), "{e}");
}

fn openai_reply(output: Value, status: &str) -> String {
    json!({"id": "resp_1", "object": "response", "status": status, "output": output}).to_string()
}

#[test]
fn openai_reads_a_text_answer() {
    let output = json!([{"id": "rs_1", "type": "reasoning", "summary": []},
        {"id": "msg_1", "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Hello there", "annotations": []}]}]);
    let r = openai::parse(200, &openai_reply(output, "completed")).unwrap();
    assert_eq!((r.text.as_str(), r.calls.len()), ("Hello there", 0));
}

#[test]
fn openai_reads_a_tool_call_and_replays_only_what_can_be_replayed() {
    let output = json!([
        {"id": "rs_1", "type": "reasoning", "summary": []},
        {"id": "rs_2", "type": "reasoning", "summary": [], "encrypted_content": "blob"},
        {"id": "fc_1", "type": "function_call", "call_id": "call_1", "name": "execute", "arguments": BOX, "status": "completed"},
    ]);
    let r = openai::parse(200, &openai_reply(output, "completed")).unwrap();
    assert_eq!(r.calls, vec![Call { id: "call_1".into(), name: "execute".into(), args: serde_json::from_str(BOX).unwrap() }]);
    let done = Done { id: "call_1".into(), name: "execute".into(), text: "ok".into(), error: false, image: None };
    let sent = openai::request("gpt-6-astra", KEY, &[Msg::user("a box"), Msg::Assistant(r), Msg::Results(vec![done])]);
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!((&body["store"], &body["model"]), (&json!(false), &json!("gpt-6-astra")));
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["stream"], true);
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["strict"], false);
    let types: Vec<&str> = body["input"].as_array().unwrap().iter().map(|i| i["type"].as_str().unwrap_or("message")).collect();
    assert_eq!(types, ["message", "reasoning", "function_call", "function_call_output"], "the reasoning without its content is left out");
    assert_eq!(body["input"][3]["call_id"], "call_1");
    assert_eq!(sent.url, "https://api.openai.com/v1/responses");
    assert!(sent.headers.contains(&("Authorization".into(), format!("Bearer {KEY}"))));
    assert!(!sent.body.contains(KEY) && !sent.url.contains(KEY));
}

#[test]
fn openai_reads_two_tool_calls_and_keeps_bad_arguments_for_the_tool_to_refuse() {
    let output = json!([
        {"type": "function_call", "call_id": "call_a", "name": "inspect_scene", "arguments": "{}"},
        {"type": "function_call", "call_id": "call_b", "name": "execute", "arguments": "{not json"},
    ]);
    let r = openai::parse(200, &openai_reply(output, "completed")).unwrap();
    assert_eq!(r.calls.len(), 2);
    assert_eq!(r.calls[1].args, json!("{not json"));
    let mut s = Session::new();
    assert!(tools::run(&mut s, &r.calls[1], Access::Edit).unwrap_err().contains("JSON object"));
}

#[test]
fn openai_malformed_replies_failures_and_refusals_are_errors() {
    for body in ["", "<html>", "[]", "{}", r#"{"output": {}}"#, r#"{"output": [{"type": "function_call", "name": "x"}]}"#] {
        assert!(openai::parse(200, body).is_err(), "{body:?}");
    }
    let e = openai::parse(429, r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota"}}"#).unwrap_err();
    assert!(e.contains("429") && e.contains("quota"), "{e}");
    let failed = json!({"status": "failed", "output": [], "error": {"message": "the server had an error"}}).to_string();
    assert!(openai::parse(200, &failed).unwrap_err().contains("the server had an error"));
    let refusal =
        openai_reply(json!([{"type": "message", "content": [{"type": "refusal", "refusal": "I can't help with that."}]}]), "completed");
    let e = openai::parse(200, &refusal).unwrap_err();
    assert!(e.contains("declined") && e.contains("can't help"), "{e}");
    let cut = openai_reply(json!([{"type": "function_call", "call_id": "c", "name": "execute", "arguments": "{"}]), "incomplete");
    assert!(openai::parse(200, &cut).unwrap_err().contains("cut off"));
}

fn gemini_reply(parts: Value, finish: &str) -> String {
    json!({"candidates": [{"content": {"role": "model", "parts": parts}, "finishReason": finish}]}).to_string()
}

#[test]
fn gemini_reads_a_text_answer_and_skips_the_thoughts() {
    let parts = json!([{"text": "pondering", "thought": true}, {"text": "Hello there"}]);
    let r = gemini::parse(200, &gemini_reply(parts, "STOP")).unwrap();
    assert_eq!((r.text.as_str(), r.calls.len()), ("Hello there", 0));
}

#[test]
fn gemini_reads_a_tool_call_and_sends_back_its_signature_and_an_answer_by_id() {
    let parts = json!([{"functionCall": {"name": "execute", "args": serde_json::from_str::<Value>(BOX).unwrap(), "id": "c1"}, "thoughtSignature": "sig"}]);
    let r = gemini::parse(200, &gemini_reply(parts, "STOP")).unwrap();
    assert_eq!(r.calls, vec![Call { id: "c1".into(), name: "execute".into(), args: serde_json::from_str(BOX).unwrap() }]);
    let done = vec![Done { id: "c1".into(), name: "execute".into(), text: "ok".into(), error: false, image: None }];
    let nameless = vec![Done { id: String::new(), name: "inspect_scene".into(), text: "boom".into(), error: true, image: None }];
    let sent =
        gemini::request("gemini-3.8-flash", KEY, &[Msg::user("a box"), Msg::Assistant(r), Msg::Results(done), Msg::Results(nameless)]);
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(body["contents"][1]["role"], "model");
    assert_eq!(body["contents"][1]["parts"][0]["thoughtSignature"], "sig", "the signature goes back in the part it came in");
    assert_eq!(body["contents"][2]["parts"][0]["functionResponse"], json!({"name": "execute", "id": "c1", "response": {"result": "ok"}}));
    assert_eq!(body["contents"][3]["parts"][0]["functionResponse"], json!({"name": "inspect_scene", "response": {"error": "boom"}}));
    assert_eq!(sent.url, "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:streamGenerateContent?alt=sse");
    assert!(allowed(&sent.url));
    assert!(sent.headers.contains(&("x-goog-api-key".into(), KEY.into())));
    assert!(!sent.body.contains(KEY) && !sent.url.contains(KEY), "the key is in a header, not the address");
    // The declarations are Gemini's subset: no free-form object, none with nothing to pass.
    let declared = body["tools"][0]["functionDeclarations"].as_array().unwrap();
    assert_eq!(declared.len(), tools::all().len());
    assert_eq!(declared[0]["parameters"]["properties"]["params"]["type"], "string");
    assert!(declared.iter().find(|d| d["name"] == "inspect_scene").unwrap().get("parameters").is_none());
}

#[test]
fn gemini_reads_two_tool_calls_and_the_free_form_object_written_as_text() {
    let parts = json!([
        {"functionCall": {"name": "inspect_scene", "args": {}}},
        {"functionCall": {"name": "execute", "args": {"command": "add_primitive", "params": "{\"kind\": \"box\", \"size\": [20, 20, 10]}"}}},
    ]);
    let r = gemini::parse(200, &gemini_reply(parts, "STOP")).unwrap();
    assert_eq!(r.calls.len(), 2);
    let mut s = Session::new();
    run(&mut s, "execute", r.calls[1].args.clone()).unwrap();
    assert_eq!(objects(&mut s), 1, "the object came back out of its text");
}

#[test]
fn gemini_malformed_blocked_and_refused_replies_are_errors() {
    for body in ["", "nope", "[]", "{}", r#"{"candidates": []}"#, r#"{"candidates": [{"content": {"parts": [{"functionCall": {}}]}}]}"#] {
        assert!(gemini::parse(200, body).is_err(), "{body:?}");
    }
    let blocked = r#"{"promptFeedback": {"blockReason": "SAFETY"}}"#;
    assert!(gemini::parse(200, blocked).unwrap_err().contains("SAFETY"));
    let e = gemini::parse(400, r#"{"error":{"code":400,"message":"API key not valid.","status":"INVALID_ARGUMENT"}}"#).unwrap_err();
    assert!(e.contains("400") && e.contains("API key not valid"), "{e}");
    assert!(gemini::parse(200, &gemini_reply(json!([]), "SAFETY")).unwrap_err().contains("declined"));
    assert!(gemini::parse(200, &gemini_reply(json!([]), "MALFORMED_FUNCTION_CALL")).unwrap_err().contains("tool call"));
    // A model turn with no content at all still goes back as a model turn.
    let r = gemini::parse(200, r#"{"candidates": [{"finishReason": "STOP"}]}"#).unwrap();
    assert_eq!(r.raw["role"], "model");
}

#[test]
fn a_key_or_model_that_is_no_key_or_model_never_reaches_a_request() {
    let net = Fake::default();
    let history = [Msg::user("hi")];
    for model in ["", "a b", "x/../y", "m?key=1", "m:generate"] {
        assert!(!model_ok(model), "{model:?}");
        assert!(complete(Provider::Gemini, &net, "", model, KEY, &history, &mut |_| true).is_err());
    }
    for key in ["", "short", "has space in it ok", "line\nbreak-in-it-0123", "ключ-0123456789"] {
        assert!(!key_ok(key), "{key:?}");
        assert!(complete(Provider::OpenAi, &net, "", "gpt-6-astra", key, &history, &mut |_| true).is_err());
    }
    assert!(net.seen().is_empty());
}

#[test]
fn every_provider_lists_models_the_ids_of_which_pass() {
    for p in Provider::ALL {
        assert!(p.models().iter().all(|m| crate::model_ok_for(p, m)), "{p:?}");
        assert_eq!(Provider::from_key(p.key()), Some(p));
    }
    assert_eq!(Provider::Anthropic.default_model(), "claude-sonnet-5-5");
}

// ---------- the clients, against recorded streams ----------

/// Events as a server sends them: `event:` with the event's `type` when it has one, then its data.
fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| match e["type"].as_str() {
            Some(name) => format!("event: {name}\ndata: {e}\n\n"),
            None => format!("data: {e}\n\n"),
        })
        .collect()
}

/// The Messages API stream that makes `content`: each block started empty, grown in two pieces (a tool's input as JSON text), and
/// stopped, then the stop reason and the counts.
fn anthropic_stream(content: &Value, stop: &str) -> String {
    let start = json!({"id": "msg_01", "type": "message", "role": "assistant", "model": "claude-sonnet-5-5", "content": [],
        "stop_reason": null, "usage": {"input_tokens": 10, "output_tokens": 1}});
    let mut events = vec![json!({"type": "message_start", "message": start})];
    for (i, block) in content.as_array().unwrap().iter().enumerate() {
        let (empty, kind, key, whole) = match block["type"].as_str().unwrap() {
            "text" => (json!({"type": "text", "text": ""}), "text_delta", "text", block["text"].as_str().unwrap().to_string()),
            "thinking" => {
                (json!({"type": "thinking", "thinking": ""}), "thinking_delta", "thinking", block["thinking"].as_str().unwrap().into())
            }
            _ => (
                json!({"type": "tool_use", "id": block["id"], "name": block["name"], "input": {}}),
                "input_json_delta",
                "partial_json",
                block["input"].to_string(),
            ),
        };
        events.push(json!({"type": "content_block_start", "index": i, "content_block": empty}));
        let half = whole.char_indices().nth(whole.chars().count() / 2).map_or(whole.len(), |(at, _)| at);
        for piece in [&whole[..half], &whole[half..]] {
            let mut delta = json!({"type": kind});
            delta[key] = json!(piece);
            events.push(json!({"type": "content_block_delta", "index": i, "delta": delta}));
        }
        if let Some(signature) = block["signature"].as_str() {
            events.push(json!({"type": "content_block_delta", "index": i, "delta": {"type": "signature_delta", "signature": signature}}));
        }
        events.push(json!({"type": "content_block_stop", "index": i}));
    }
    events.push(json!({"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": null}, "usage": {"output_tokens": 5}}));
    events.push(json!({"type": "message_stop"}));
    sse(&events)
}

/// A recorded reply as `complete` reads it off the network: the reply, and the pieces of text handed over as they came.
fn streamed(provider: Provider, status: u16, body: &str) -> (Result<Reply, String>, Vec<String>) {
    let net = Fake::default().reply(status, body);
    let mut pieces = Vec::new();
    let mut each = |text: &str| {
        if !text.is_empty() {
            pieces.push(text.to_string());
        }
        true
    };
    let reply = complete(provider, &net, crate::local::DEFAULT_URL, provider.default_model(), KEY, &[Msg::user("hi")], &mut each);
    (reply, pieces)
}

/// The basic stream of Anthropic's streaming guide, as it prints it.
const ANTHROPIC_HELLO: &str = r#"event: message_start
data: {"type": "message_start", "message": {"id": "msg_1nZdL29xx5MUA1yADyHTEsnR8uuvGzszyY", "type": "message", "role": "assistant", "content": [], "model": "claude-opus-5-5", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 25, "output_tokens": 1}}}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}

event: ping
data: {"type": "ping"}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "!"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence":null}, "usage": {"output_tokens": 15}}

event: message_stop
data: {"type": "message_stop"}

"#;

#[test]
fn anthropic_streams_text_and_puts_the_message_back_together() {
    let (r, pieces) = streamed(Provider::Anthropic, 200, ANTHROPIC_HELLO);
    let r = r.unwrap();
    assert_eq!(pieces, ["Hello", "!"], "the text as it came");
    assert_eq!((r.text.as_str(), r.usage), ("Hello!", Usage { input: 25, output: 15 }), "the counts of message_delta are the whole");
    assert_eq!(r.raw, json!([{"type": "text", "text": "Hello!"}]));
    // The same stream with Windows line ends, a comment and an event of a kind added later reads the same.
    let odd = ANTHROPIC_HELLO.replace('\n', "\r\n").replacen(
        "event: ping",
        ": a comment\r\nevent: future_kind\r\ndata: {\"type\": \"future_kind\"}\r\n\r\nevent: ping",
        1,
    );
    assert_eq!(streamed(Provider::Anthropic, 200, &odd).0.unwrap(), r);
    // Thinking is put together whole, its signature with it, and goes back as it came.
    let content = json!([{"type": "thinking", "thinking": "A box first.", "signature": "EqQB"}, {"type": "text", "text": "Hello there"}]);
    let (r, pieces) = streamed(Provider::Anthropic, 200, &anthropic_stream(&content, "end_turn"));
    let r = r.unwrap();
    assert_eq!(pieces.concat(), "Hello there", "thinking is not part of the answer's text");
    assert_eq!((r.thinking.as_str(), &r.raw), ("A box first.", &content));
    let sent: Value = serde_json::from_str(&anthropic::request("claude-sonnet-5-5", KEY, &[Msg::Assistant(r)]).body).unwrap();
    assert_eq!(sent["messages"][0]["content"], content);
}

#[test]
fn anthropic_streams_one_and_two_tool_calls_with_their_input_whole() {
    let one = json!([{"type": "text", "text": "Adding."}, {"type": "tool_use", "id": "toolu_01", "name": "execute", "input": serde_json::from_str::<Value>(BOX).unwrap()}]);
    let r = streamed(Provider::Anthropic, 200, &anthropic_stream(&one, "tool_use")).0.unwrap();
    assert_eq!(r.calls, vec![Call { id: "toolu_01".into(), name: "execute".into(), args: serde_json::from_str(BOX).unwrap() }]);
    assert_eq!(r.raw, one, "the input goes back as the object it is");
    // A call with nothing to pass has no input deltas at all, or an empty one.
    let two = json!([
        {"type": "tool_use", "id": "toolu_a", "name": "inspect_scene", "input": {}},
        {"type": "tool_use", "id": "toolu_b", "name": "list_commands", "input": {"filter": "move"}},
    ]);
    let r = streamed(Provider::Anthropic, 200, &anthropic_stream(&two, "tool_use")).0.unwrap();
    assert_eq!(
        r.calls.iter().map(|c| (c.id.as_str(), &c.args)).collect::<Vec<_>>(),
        [("toolu_a", &json!({})), ("toolu_b", &json!({"filter": "move"}))]
    );
    let bare = sse(&[
        json!({"type": "message_start", "message": {"content": [], "usage": {}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "t", "name": "inspect_scene", "input": {}}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 3}}),
        json!({"type": "message_stop"}),
    ]);
    assert_eq!(streamed(Provider::Anthropic, 200, &bare).0.unwrap().calls[0].args, json!({}));
}

#[test]
fn anthropic_streams_that_are_cut_off_malformed_refused_or_failed_are_errors() {
    let words = json!([{"type": "text", "text": "Hello there"}]);
    let whole = anthropic_stream(&words, "end_turn");
    // Cut off before message_stop, or in the middle of an event.
    let cut = &whole[..whole.find("event: message_stop").unwrap()];
    assert!(streamed(Provider::Anthropic, 200, cut).0.unwrap_err().contains("stopped before its end"));
    let mid = &whole[..whole.find("Hello").unwrap()];
    assert!(streamed(Provider::Anthropic, 200, mid).0.is_err());
    assert!(streamed(Provider::Anthropic, 200, "").0.is_err(), "nothing at all");
    // Events that are not what the API documents.
    for bad in [
        json!({"type": "content_block_delta", "index": 7, "delta": {"type": "text_delta", "text": "x"}}),
        json!({"type": "content_block_start", "index": 3, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_start", "index": 1, "content_block": "text"}),
        json!({"type": "message_start", "message": "hi"}),
    ] {
        let stream = whole.replacen("event: message_delta", &format!("{}event: message_delta", sse(std::slice::from_ref(&bad))), 1);
        assert!(streamed(Provider::Anthropic, 200, &stream).0.is_err(), "{bad}");
    }
    let not_json = whole.replacen(r#""text":"Hello"#, r#""text":"Hello"}}}"#, 1);
    assert!(streamed(Provider::Anthropic, 200, &not_json).0.is_err());
    let bad_input = sse(&[
        json!({"type": "message_start", "message": {"content": [], "usage": {}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "t", "name": "execute", "input": {}}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"command\": \"add_"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 3}}),
        json!({"type": "message_stop"}),
    ]);
    assert!(streamed(Provider::Anthropic, 200, &bad_input).0.unwrap_err().contains("not JSON"));
    // An error in the stream, as the guide shows one, after some text was shown.
    let overloaded = format!(
        "{}event: error\ndata: {{\"type\": \"error\", \"error\": {{\"type\": \"overloaded_error\", \"message\": \"Overloaded\"}}}}\n\n",
        &whole[..whole.find("event: content_block_stop").unwrap()]
    );
    let (r, pieces) = streamed(Provider::Anthropic, 200, &overloaded);
    assert!(r.unwrap_err().contains("Overloaded") && !pieces.is_empty());
    // A refusal is a stop reason, with its category, in message_delta; an error status is read whole.
    let refused =
        whole.replacen(r#""stop_reason":"end_turn""#, r#""stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber"}"#, 1);
    let e = streamed(Provider::Anthropic, 200, &refused).0.unwrap_err();
    assert!(e.contains("declined") && e.contains("cyber"), "{e}");
    let e = streamed(Provider::Anthropic, 401, r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#)
        .0
        .unwrap_err();
    assert!(e.contains("401") && e.contains("invalid x-api-key"), "{e}");
    let tool_cut =
        anthropic_stream(&json!([{"type": "tool_use", "id": "t", "name": "execute", "input": {"command": "add"}}]), "max_tokens");
    assert!(streamed(Provider::Anthropic, 200, &tool_cut).0.unwrap_err().contains("cut off"));
}

/// A Responses API stream: the events around a reasoning item and a message saying `text` in two pieces, or around `calls`, then
/// `response.completed` with the whole response.
fn openai_stream(text: &str, calls: &[(&str, &str, &str)], status: &str) -> String {
    let reasoning =
        json!({"id": "rs_1", "type": "reasoning", "summary": [{"type": "summary_text", "text": "Look."}], "encrypted_content": "blob"});
    let mut events = vec![
        json!({"type": "response.created", "sequence_number": 0, "response": {"id": "resp_1", "object": "response", "status": "in_progress", "output": []}}),
        json!({"type": "response.output_item.added", "output_index": 0, "item": {"id": "rs_1", "type": "reasoning", "summary": []}}),
        json!({"type": "response.reasoning_summary_text.delta", "item_id": "rs_1", "output_index": 0, "summary_index": 0, "delta": "Look."}),
        json!({"type": "response.output_item.done", "output_index": 0, "item": reasoning}),
    ];
    let mut output = vec![reasoning];
    if !text.is_empty() {
        let message = json!({"id": "msg_1", "type": "message", "role": "assistant", "status": "completed",
            "content": [{"type": "output_text", "text": text, "annotations": []}]});
        events.push(json!({"type": "response.output_item.added", "output_index": 1, "item": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}}));
        let half = text.len() / 2;
        for piece in [&text[..half], &text[half..]] {
            events.push(json!({"type": "response.output_text.delta", "item_id": "msg_1", "output_index": 1, "content_index": 0, "delta": piece, "logprobs": []}));
        }
        events.push(json!({"type": "response.output_text.done", "item_id": "msg_1", "output_index": 1, "content_index": 0, "text": text}));
        events.push(json!({"type": "response.output_item.done", "output_index": 1, "item": message}));
        output.push(message);
    }
    for (i, (id, name, arguments)) in calls.iter().enumerate() {
        let call = json!({"id": format!("fc_{i}"), "type": "function_call", "call_id": id, "name": name, "arguments": arguments, "status": "completed"});
        events.push(json!({"type": "response.function_call_arguments.delta", "item_id": format!("fc_{i}"), "output_index": i + 1, "delta": arguments}));
        events.push(json!({"type": "response.function_call_arguments.done", "item_id": format!("fc_{i}"), "output_index": i + 1, "arguments": arguments}));
        events.push(json!({"type": "response.output_item.done", "output_index": i + 1, "item": call}));
        output.push(call);
    }
    let response = json!({"id": "resp_1", "object": "response", "status": status, "output": output,
        "incomplete_details": if status == "incomplete" { json!({"reason": "max_output_tokens"}) } else { Value::Null },
        "usage": {"input_tokens": 40, "output_tokens": 9}});
    events.push(json!({"type": format!("response.{status}"), "sequence_number": 9, "response": response}));
    sse(&events)
}

#[test]
fn openai_streams_text_and_takes_the_whole_response_from_its_last_event() {
    let (r, pieces) = streamed(Provider::OpenAi, 200, &openai_stream("Hello there", &[], "completed"));
    let r = r.unwrap();
    assert_eq!(pieces, ["Hello", " there"]);
    assert_eq!((r.text.as_str(), r.thinking.as_str(), r.usage), ("Hello there", "Look.", Usage { input: 40, output: 9 }));
    assert_eq!(r.raw[0]["encrypted_content"], "blob", "the reasoning goes back with its encrypted content");
    let sent: Value = serde_json::from_str(&openai::request("gpt-6-astra", KEY, &[Msg::user("hi"), Msg::Assistant(r)]).body).unwrap();
    assert_eq!(
        sent["input"].as_array().unwrap().iter().map(|i| i["type"].as_str().unwrap_or("message")).collect::<Vec<_>>(),
        ["message", "reasoning", "message"]
    );
}

#[test]
fn openai_streams_one_and_two_tool_calls() {
    let r = streamed(Provider::OpenAi, 200, &openai_stream("", &[("call_1", "execute", BOX)], "completed")).0.unwrap();
    assert_eq!(r.calls, vec![Call { id: "call_1".into(), name: "execute".into(), args: serde_json::from_str(BOX).unwrap() }]);
    let two = openai_stream("", &[("call_a", "inspect_scene", "{}"), ("call_b", "list_commands", r#"{"filter":"move"}"#)], "completed");
    let r = streamed(Provider::OpenAi, 200, &two).0.unwrap();
    assert_eq!(r.calls.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["call_a", "call_b"]);
    assert_eq!(r.calls[1].args["filter"], "move");
}

#[test]
fn openai_streams_that_are_cut_off_failed_refused_or_errors_are_errors() {
    let whole = openai_stream("Hello there", &[], "completed");
    let cut = &whole[..whole.find("event: response.completed").unwrap()];
    let (r, pieces) = streamed(Provider::OpenAi, 200, cut);
    assert!(r.unwrap_err().contains("stopped before its end") && pieces.concat() == "Hello there");
    assert!(streamed(Provider::OpenAi, 200, &whole.replacen("\"Hello\"", "\"Hel", 1)).0.is_err(), "an event that is not JSON");
    // The error event, as the reference prints it.
    let error = format!(
        "{cut}event: error\ndata: {{\"type\": \"error\", \"code\": \"ERR_SOMETHING\", \"message\": \"Something went wrong\", \"param\": null, \"sequence_number\": 1}}\n\n"
    );
    assert!(streamed(Provider::OpenAi, 200, &error).0.unwrap_err().contains("Something went wrong"));
    let failed = openai_stream("", &[], "failed").replacen(
        "\"status\":\"failed\"",
        "\"status\":\"failed\",\"error\":{\"message\":\"the server had an error\"}",
        1,
    );
    assert!(streamed(Provider::OpenAi, 200, &failed).0.unwrap_err().contains("the server had an error"));
    let refusal = whole.replacen(
        r#"{"annotations":[],"text":"Hello there","type":"output_text"}"#,
        r#"{"refusal":"I can't help with that.","type":"refusal"}"#,
        2,
    );
    let e = streamed(Provider::OpenAi, 200, &refusal).0.unwrap_err();
    assert!(e.contains("declined") && e.contains("can't help"), "{e}");
    let cut_call = openai_stream("", &[("c", "execute", "{")], "incomplete");
    assert!(streamed(Provider::OpenAi, 200, &cut_call).0.unwrap_err().contains("cut off"));
    let e = streamed(Provider::OpenAi, 429, r#"{"error":{"message":"You exceeded your current quota"}}"#).0.unwrap_err();
    assert!(e.contains("429") && e.contains("quota"), "{e}");
}

/// A `streamGenerateContent?alt=sse` stream: each chunk the next parts, the last with the finish reason and the counts.
fn gemini_stream(chunks: &[Value], finish: &str) -> String {
    let last = chunks.len() - 1;
    let events: Vec<Value> = chunks
        .iter()
        .enumerate()
        .map(|(i, parts)| {
            let mut candidate = json!({"content": {"role": "model", "parts": parts}, "index": 0});
            let mut chunk = json!({"modelVersion": "gemini-3.8-flash"});
            if i == last {
                candidate["finishReason"] = json!(finish);
                chunk["usageMetadata"] = json!({"promptTokenCount": 30, "candidatesTokenCount": 4, "thoughtsTokenCount": 6});
            }
            chunk["candidates"] = json!([candidate]);
            chunk
        })
        .collect();
    sse(&events)
}

#[test]
fn gemini_streams_text_and_keeps_every_part_as_it_came() {
    let chunks = [
        json!([{"text": "Checking.", "thought": true}]),
        json!([{"text": "Hello"}]),
        json!([{"text": " there"}]),
        json!([{"text": "", "thoughtSignature": "sig"}]),
    ];
    let (r, pieces) = streamed(Provider::Gemini, 200, &gemini_stream(&chunks, "STOP"));
    let r = r.unwrap();
    assert_eq!(pieces, ["Hello", " there"]);
    assert_eq!((r.text.as_str(), r.thinking.as_str(), r.usage), ("Hello there", "Checking.", Usage { input: 30, output: 10 }));
    // The signature, which came in a part of its own with no text, goes back in that part.
    assert_eq!(r.raw, json!({"role": "model", "parts": chunks.iter().flat_map(|c| c.as_array().unwrap().clone()).collect::<Vec<_>>()}));
}

#[test]
fn gemini_streams_one_and_two_tool_calls() {
    let one = [
        json!([{"functionCall": {"name": "execute", "args": serde_json::from_str::<Value>(BOX).unwrap(), "id": "c1"}, "thoughtSignature": "sig"}]),
    ];
    let r = streamed(Provider::Gemini, 200, &gemini_stream(&one, "STOP")).0.unwrap();
    assert_eq!(r.calls, vec![Call { id: "c1".into(), name: "execute".into(), args: serde_json::from_str(BOX).unwrap() }]);
    assert_eq!(r.raw["parts"][0]["thoughtSignature"], "sig");
    let two = [
        json!([{"text": "Looking."}]),
        json!([{"functionCall": {"name": "inspect_scene", "args": {}}, "thoughtSignature": "sig"}, {"functionCall": {"name": "list_commands", "args": {"filter": "move"}}}]),
    ];
    let (r, pieces) = streamed(Provider::Gemini, 200, &gemini_stream(&two, "STOP"));
    let r = r.unwrap();
    assert_eq!((r.calls.len(), r.calls[1].args["filter"].as_str()), (2, Some("move")));
    assert_eq!(pieces, ["Looking."], "a call is not text");
}

#[test]
fn gemini_streams_that_are_cut_off_blocked_refused_or_failed_are_errors() {
    let words = [json!([{"text": "Hello"}]), json!([{"text": " there"}])];
    let whole = gemini_stream(&words, "STOP");
    let cut = &whole[..whole.rfind("data:").unwrap()];
    assert!(streamed(Provider::Gemini, 200, cut).0.unwrap_err().contains("stopped before its end"));
    assert!(streamed(Provider::Gemini, 200, &whole[..whole.len() - 20]).0.is_err(), "cut in the middle of a chunk");
    let blocked = "data: {\"promptFeedback\": {\"blockReason\": \"SAFETY\"}}\n\n";
    assert!(streamed(Provider::Gemini, 200, blocked).0.unwrap_err().contains("SAFETY"));
    let refused = gemini_stream(&words, "SAFETY");
    assert!(streamed(Provider::Gemini, 200, &refused).0.unwrap_err().contains("declined"));
    let malformed = gemini_stream(&words, "MALFORMED_FUNCTION_CALL");
    assert!(streamed(Provider::Gemini, 200, &malformed).0.unwrap_err().contains("tool call"));
    let failed =
        format!("{cut}data: {{\"error\": {{\"code\": 503, \"message\": \"The model is overloaded.\", \"status\": \"UNAVAILABLE\"}}}}\n\n");
    assert!(streamed(Provider::Gemini, 200, &failed).0.unwrap_err().contains("overloaded"));
    let e = streamed(Provider::Gemini, 400, r#"{"error":{"code":400,"message":"API key not valid.","status":"INVALID_ARGUMENT"}}"#)
        .0
        .unwrap_err();
    assert!(e.contains("400") && e.contains("API key not valid"), "{e}");
}

/// A Chat Completions stream: `chunks` (each a choice's delta, or a whole chunk when it has `choices`), the counts and `[DONE]`.
fn local_stream(deltas: &[Value], finish: &str) -> String {
    let mut events: Vec<Value> = deltas
        .iter()
        .map(|d| json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": d, "finish_reason": null}]}))
        .collect();
    events.push(
        json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": {}, "finish_reason": finish}]}),
    );
    events.push(json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": [], "usage": {"prompt_tokens": 900, "completion_tokens": 40}}));
    format!("{}data: [DONE]\n\n", sse(&events))
}

#[test]
fn the_local_server_streams_text_and_tool_calls_in_pieces() {
    let deltas = [
        json!({"role": "assistant", "content": ""}),
        json!({"reasoning_content": "a box"}),
        json!({"content": "Hel"}),
        json!({"content": "lo"}),
    ];
    let (r, pieces) = streamed(Provider::Local, 200, &local_stream(&deltas, "stop"));
    let r = r.unwrap();
    assert_eq!(pieces, ["Hel", "lo"]);
    assert_eq!((r.text.as_str(), r.thinking.as_str(), r.usage), ("Hello", "a box", Usage { input: 900, output: 40 }));
    assert_eq!(r.raw, json!({"role": "assistant", "content": "Hello"}));
    // Two calls, each by its index: the id and name once, the arguments in pieces.
    let deltas = [
        json!({"tool_calls": [{"index": 0, "id": "call_a", "type": "function", "function": {"name": "execute", "arguments": ""}}]}),
        json!({"tool_calls": [{"index": 0, "function": {"arguments": &BOX[..10]}}]}),
        json!({"tool_calls": [{"index": 0, "function": {"arguments": &BOX[10..]}}]}),
        json!({"tool_calls": [{"index": 1, "id": "call_b", "type": "function", "function": {"name": "inspect_scene", "arguments": "{}"}}]}),
    ];
    let r = streamed(Provider::Local, 200, &local_stream(&deltas, "tool_calls")).0.unwrap();
    assert_eq!(r.calls[0], Call { id: "call_a".into(), name: "execute".into(), args: serde_json::from_str(BOX).unwrap() });
    assert_eq!((r.calls[1].id.as_str(), &r.calls[1].args), ("call_b", &json!({})));
    assert_eq!(r.raw["tool_calls"][0]["function"]["arguments"], serde_json::from_str::<Value>(BOX).unwrap().to_string());
    // A server that sends each call whole, unnumbered, with no id: one call each, given an id.
    let whole_calls = [json!({"tool_calls": [{"function": {"name": "inspect_scene", "arguments": {}}}]})];
    let r = streamed(Provider::Local, 200, &local_stream(&whole_calls, "tool_calls")).0.unwrap();
    assert_eq!((r.calls.len(), r.calls[0].id.as_str()), (1, "call_0"));
    // A server that does not stream answers whole, and that is read too.
    let answer = json!({"choices": [{"message": {"role": "assistant", "content": "Whole."}, "finish_reason": "stop"}]});
    assert_eq!(streamed(Provider::Local, 200, &answer.to_string()).0.unwrap().text, "Whole.");
}

#[test]
fn local_streams_that_are_cut_off_out_of_order_or_failed_are_errors() {
    let whole = local_stream(&[json!({"content": "Hello"})], "stop");
    let cut = &whole[..whole.find("\"finish_reason\":\"stop\"").unwrap()];
    assert!(streamed(Provider::Local, 200, cut).0.is_err(), "cut in the middle of a chunk");
    let no_end = &whole[..whole.find("data: {\"choices\":[{\"delta\":{},").unwrap()];
    assert!(streamed(Provider::Local, 200, no_end).0.unwrap_err().contains("stopped before its end"), "{no_end}");
    let jump =
        local_stream(&[json!({"tool_calls": [{"index": 5, "id": "c", "function": {"name": "x", "arguments": "{}"}}]})], "tool_calls");
    assert!(streamed(Provider::Local, 200, &jump).0.unwrap_err().contains("out of order"));
    let failed = "data: {\"error\": {\"message\": \"model not found\"}}\n\n";
    assert!(streamed(Provider::Local, 200, failed).0.unwrap_err().contains("model not found"));
    let empty = local_stream(&[json!({"content": ""})], "length");
    assert!(streamed(Provider::Local, 200, &empty).0.unwrap_err().contains("ran out of room"));
    assert!(streamed(Provider::Local, 500, r#"{"error":{"message":"model not found"}}"#).0.unwrap_err().contains("500"));
}

#[test]
fn the_cap_holds_over_the_whole_stream_and_a_stopped_reader_stops() {
    let ping = "event: ping\ndata: {\"type\": \"ping\"}\n\n";
    let endless = ping.repeat(crate::MAX_REPLY as usize / ping.len() + 2);
    let e = streamed(Provider::Anthropic, 200, &endless).0.unwrap_err();
    assert!(e.contains("larger than"), "{e}");
    // A line with no end is capped the same.
    assert!(streamed(Provider::Anthropic, 200, &"a".repeat(crate::MAX_REPLY as usize + 10)).0.unwrap_err().contains("larger than"));
    // The reader asks after each event whether it is still wanted.
    let net = Fake::default().reply(200, ANTHROPIC_HELLO);
    let mut events = 0;
    let r = complete(Provider::Anthropic, &net, "", "claude-sonnet-5-5", KEY, &[Msg::user("hi")], &mut |_| {
        events += 1;
        events < 3
    });
    assert!(r.unwrap_err().contains("stopped") && events == 3);
}

// ---------- the network ----------

#[test]
fn the_assistant_talks_to_its_three_hosts_over_https_and_to_nothing_else() {
    for yes in [
        "https://api.anthropic.com/v1/messages",
        "https://api.openai.com/v1/responses",
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent",
        "https://API.OPENAI.COM/v1/responses",
    ] {
        assert!(allowed(yes), "{yes}");
    }
    for no in [
        "http://api.anthropic.com/v1/messages",
        "https://api.anthropic.com.evil.example/v1/messages",
        "https://evil.example/api.anthropic.com",
        "https://api.anthropic.com:8443/v1/messages",
        "https://user:pass@api.anthropic.com/v1/messages",
        "https://api.anthropic.com@evil.example/",
        "https://github.com/",
        "ftp://api.openai.com/",
        "api.openai.com",
        "",
    ] {
        assert!(!allowed(no), "{no}");
    }
}

/// Serves one reply to each of `replies` on this machine, and gives back what the requests said.
fn serve(replies: Vec<(u16, Vec<u8>)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let mut said = Vec::new();
        for (status, body) in replies {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let length = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|n| n.trim().parse().ok()))
                .unwrap_or(0);
            let mut sent = vec![0u8; length];
            stream.read_exact(&mut sent).unwrap();
            said.push(format!("{head}{}", String::from_utf8_lossy(&sent)));
            let _ =
                stream.write_all(format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
            let _ = stream.write_all(&body);
        }
        said
    });
    (base, server)
}

#[test]
fn a_reply_is_read_with_its_status_and_capped_and_a_redirect_goes_nowhere() {
    let big = vec![b'a'; crate::MAX_REPLY as usize + 10];
    let (base, server) = serve(vec![
        (200, br#"{"ok":1}"#.to_vec()),
        (401, br#"{"error":{"message":"bad key"}}"#.to_vec()),
        (200, big),
        (302, b"moved".to_vec()),
    ]);
    let net = Ureq::loopback(&base);
    let ask = |path: &str| {
        let (status, mut body) = net
            .stream(&Request { url: format!("{base}/{path}"), headers: vec![("x-api-key".into(), KEY.into())], body: "{\"q\":1}".into() })
            .unwrap();
        let mut text = String::new();
        body.read_to_string(&mut text).unwrap();
        (status, text)
    };
    assert_eq!(ask("a"), (200, r#"{"ok":1}"#.to_string()));
    assert_eq!(ask("b"), (401, r#"{"error":{"message":"bad key"}}"#.to_string()), "an error status is a reply to read");
    // The cap is the reader's, over the whole body, whatever the network: here the window's own, to a server on this computer.
    let e = complete(Provider::Local, &net, &base, "qwen3", "", &[Msg::user("x")], &mut |_| true).unwrap_err();
    assert!(e.contains("larger than"), "a reply past the cap is not read: {e}");
    assert_eq!(ask("d").0, 302, "a redirect is handed back, not followed");
    let said = server.join().unwrap();
    assert!(said[0].starts_with("POST /a "), "{}", said[0]);
    assert!(said[0].to_ascii_lowercase().contains("x-api-key: sk-test"), "the key went in a header");
    assert!(said[0].ends_with("{\"q\":1}"));
    assert!(said[2].starts_with("POST /v1/chat/completions "), "{}", said[2]);
    let e = net.stream(&Request { url: "https://api.openai.com/v1/responses".into(), headers: vec![], body: String::new() }).err().unwrap();
    assert!(e.contains("won't send") && !e.contains("/v1/responses"), "{e}");
}

// ---------- the tools ----------

#[test]
fn execute_adds_a_box_and_a_failed_command_changes_nothing() {
    let mut s = Session::new();
    let made = run(&mut s, "execute", json!({"command": "add_primitive", "params": {"kind": "box", "size": [20, 20, 10]}})).unwrap();
    assert_eq!(made["id"], 1, "{made}");
    assert_eq!(objects(&mut s), 1);
    let e = run(&mut s, "execute", json!({"command": "add_primitive", "params": {"kind": "box", "size": [0, 1, 1]}})).unwrap_err();
    assert!(!e.is_empty());
    assert_eq!(objects(&mut s), 1);
    assert!(run(&mut s, "execute", json!({"command": "nonsense"})).unwrap_err().contains("unknown command"));
    assert!(run(&mut s, "execute", json!({})).unwrap_err().contains("command"));
    assert!(run(&mut s, "execute", json!({"command": "move", "params": [1]})).unwrap_err().contains("JSON object"));
    assert!(run(&mut s, "nope", json!({})).unwrap_err().contains("unknown tool"));
    assert!(tools::run(&mut s, &Call { id: "c".into(), name: "execute".into(), args: json!([1]) }, Access::Edit).is_err());
}

#[test]
fn the_model_can_add_move_look_delete_and_undo() {
    let mut s = Session::new();
    run(&mut s, "execute", json!({"command": "add_primitive", "params": {"kind": "box", "size": [20, 20, 10]}})).unwrap();
    run(&mut s, "execute", json!({"command": "move", "params": {"id": 1, "by": [5, 0, 0]}})).unwrap();
    let seen = run(&mut s, "inspect_scene", json!({})).unwrap();
    assert_eq!((seen["objects"][0]["min"][0].as_f64().unwrap(), seen["objects"][0]["size"][2].as_f64().unwrap()), (-5.0, 10.0));
    run(&mut s, "execute", json!({"command": "delete", "params": {"id": 1}})).unwrap();
    assert_eq!(objects(&mut s), 0);
    run(&mut s, "execute", json!({"command": "undo"})).unwrap();
    assert_eq!(objects(&mut s), 1, "undo is a command like the others");
}

#[test]
fn the_tools_inspect_the_scene_and_list_commands_by_filter() {
    let mut s = Session::new();
    run(&mut s, "execute", json!({"command": "add_primitive", "params": {"kind": "sphere", "radius": 5}})).unwrap();
    let seen = run(&mut s, "inspect_scene", json!({})).unwrap();
    assert_eq!((&seen["objects"][0]["name"], &seen["objects"][0]["watertight"]), (&json!("sphere"), &json!(true)));
    let mut ids = |filter: Value| -> Vec<String> {
        let list = run(&mut s, "list_commands", filter).unwrap();
        list.as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap().to_string()).collect()
    };
    let all = ids(json!({}));
    assert!(all.len() > 5 && all.contains(&"move".to_string()), "{all:?}");
    assert_eq!(ids(json!({"filter": "ROTATE"})), ["rotate"], "an id, in any case");
    assert_eq!(ids(json!({"filter": "geodesic"})), ["add_primitive"], "the text of the parameters");
    assert!(ids(json!({"filter": "no such command"})).is_empty());
}

#[test]
fn what_would_touch_a_file_is_refused_by_what_the_catalog_says_not_by_a_list_of_names() {
    let mut s = Session::new();
    run(&mut s, "execute", json!({"command": "add_primitive", "params": {"kind": "box", "size": [20, 20, 10]}})).unwrap();
    let path = std::env::temp_dir().join(format!("lamina-assistant-test-{}.stl", std::process::id()));
    let path = path.to_string_lossy().into_owned();
    for command in ["export_mesh", "EXPORT_MESH", "import_mesh", "Import_Mesh"] {
        let e = run(&mut s, "execute", json!({"command": command, "params": {"path": path}})).unwrap_err();
        assert!(e.contains("reads or writes a file") && e.contains("opens, saves and exports"), "{command}: {e}");
    }
    assert!(!std::path::Path::new(&path).exists(), "nothing was written");
    // Every command the catalog marks as touching the disk is refused, whatever its parameters, so a command another crate adds is too.
    let catalog = s.catalog();
    let touching: Vec<&str> =
        catalog.as_array().unwrap().iter().filter(|c| !c["files"].is_null()).map(|c| c["id"].as_str().unwrap()).collect();
    assert!(touching.len() >= 2, "{touching:?}");
    for id in touching {
        let e = run(&mut s, "execute", json!({"command": id})).unwrap_err();
        assert!(e.contains("reads or writes a file"), "{id}: {e}");
    }
    // The model is told which ones those are, and a command that touches nothing still runs.
    let listed = run(&mut s, "list_commands", json!({"filter": "export_mesh"})).unwrap();
    assert_eq!(listed[0]["files"], "write");
    assert!(run(&mut s, "execute", json!({"command": "info"})).is_ok());
    assert_eq!(objects(&mut s), 1, "the scene is still the user's");
    // `new` touches no file, but it would empty the plate and its history: refused too.
    let e = run(&mut s, "execute", json!({"command": "NEW"})).unwrap_err();
    assert!(e.contains("replace the user's whole plate"), "{e}");
    assert_eq!(objects(&mut s), 1);
}

#[test]
fn a_session_that_may_only_read_is_read_through_the_tools_too() {
    let mut s = Session::new();
    let call = |name: &str, args: Value| Call { id: "c".into(), name: name.into(), args };
    let add = call("execute", json!({"command": "add_primitive", "params": {"kind": "box", "size": [1, 1, 1]}}));
    assert!(tools::run(&mut s, &add, Access::Read).unwrap_err().contains("only read"));
    assert_eq!(objects(&mut s), 0);
    assert!(tools::run(&mut s, &call("inspect_scene", json!({})), Access::Read).is_ok());
    assert!(tools::run(&mut s, &call("list_commands", json!({})), Access::Read).is_ok());
}

/// A schema all three providers take: each `type` one name, and no `$ref`, `oneOf`, `anyOf` or `allOf`.
fn plain(schema: &Value) -> bool {
    match schema {
        Value::Object(map) => map.iter().all(|(k, v)| match k.as_str() {
            "type" => v.is_string(),
            "$ref" | "oneOf" | "anyOf" | "allOf" => false,
            "properties" => v.as_object().is_some_and(|p| p.values().all(plain)),
            _ => plain(v),
        }),
        Value::Array(items) => items.iter().all(plain),
        _ => true,
    }
}

#[test]
fn the_tools_offered_are_these_and_their_schemas_are_plain_json_types() {
    let all = tools::all();
    assert_eq!(
        all.iter().map(|t| t.name).collect::<Vec<_>>(),
        ["execute", "list_commands", "inspect_scene", "read_skill", "preview", "todo_write"]
    );
    for t in &all {
        assert!(plain(&t.schema), "{}: {}", t.name, t.schema);
    }
    assert!(!plain(&json!({"type": ["string", "number"]})) && !plain(&json!({"anyOf": []})), "the check itself looks");
}

#[test]
fn a_long_result_is_cut_where_a_character_ends_and_says_so() {
    let long = json!({"text": "é".repeat(crate::MAX_RESULT)});
    let text = tools::to_text(&long);
    assert!(text.len() < crate::MAX_RESULT + 200, "{}", text.len());
    assert!(text.contains("more bytes left out"));
    assert_eq!(tools::to_text(&json!({"a": 1})), r#"{"a":1}"#);
    assert_eq!(clip("héllo", 2), "hé…");
    assert_eq!(clip("hi", 5), "hi");
}

// ---------- the loop ----------

fn tool_reply(calls: &[(&str, &str, &str)]) -> String {
    let blocks: Vec<Value> = calls
        .iter()
        .map(
            |(id, name, input)| json!({"type": "tool_use", "id": id, "name": name, "input": serde_json::from_str::<Value>(input).unwrap()}),
        )
        .collect();
    anthropic_stream(&Value::Array(blocks), "tool_use")
}

fn text_reply(text: &str) -> String {
    anthropic_stream(&json!([{"type": "text", "text": text}]), "end_turn")
}

/// Polls until the turn is over, the way frames do.
fn drive(chat: &mut Chat, s: &mut Session) {
    let until = Instant::now() + Duration::from_secs(20);
    while chat.busy() {
        assert!(Instant::now() < until, "the turn did not end: {:?}", chat.log);
        if !chat.poll(s, Access::Edit) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn chat_on(net: &Arc<Fake>) -> Chat {
    Chat::new(net.clone())
}

#[test]
fn a_message_that_adds_a_box_adds_it_and_undo_takes_it_back() {
    let net = Arc::new(Fake::default().reply(200, tool_reply(&[("t1", "execute", BOX)])).reply(200, text_reply("Added the box.")));
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("add a box twenty millimetres wide", KEY);
    assert!(chat.busy());
    drive(&mut chat, &mut s);
    assert_eq!(objects(&mut s), 1);
    assert_eq!(chat.log.iter().filter(|l| matches!(l, Line::Used { error: false, .. })).count(), 1);
    assert_eq!(chat.log.last(), Some(&Line::Says("Added the box.".into())));
    // The second request carried the first one's tool call and its answer.
    let second: Value = serde_json::from_str(&net.seen()[1].body).unwrap();
    assert_eq!(second["messages"][2]["content"][0]["tool_use_id"], "t1");
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(objects(&mut s), 0);
}

#[test]
fn a_whole_turn_is_one_undo_step_across_replies() {
    let net = Arc::new(
        Fake::default()
            .reply(200, tool_reply(&[("a", "execute", BOX), ("b", "inspect_scene", "{}"), ("c", "execute", BOX)]))
            .reply(200, tool_reply(&[("d", "execute", BOX)]))
            .reply(200, text_reply("Three boxes."))
            .reply(200, tool_reply(&[("e", "execute", BOX)]))
            .reply(200, text_reply("A fourth.")),
    );
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("three boxes", KEY);
    drive(&mut chat, &mut s);
    assert_eq!(objects(&mut s), 3);
    assert!(s.undo_label().is_some_and(|l| l.starts_with("assistant: three boxes")), "{:?}", s.undo_label());
    // A second turn is a step of its own.
    chat.send("one more", KEY);
    drive(&mut chat, &mut s);
    assert_eq!(objects(&mut s), 4);
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(objects(&mut s), 3, "undo takes back the second turn");
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(objects(&mut s), 0, "and then the whole first turn, over both its replies");
    assert!(s.execute("undo", &json!({})).is_err());
}

#[test]
fn a_failing_call_is_told_to_the_model_and_the_others_still_run() {
    let bad = r#"{"command":"add_primitive","params":{"kind":"box","size":[0,1,1]}}"#;
    let net = Arc::new(
        Fake::default()
            .reply(200, tool_reply(&[("a", "execute", bad), ("b", "execute", BOX)]))
            .reply(200, text_reply("One of them failed.")),
    );
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("two", KEY);
    drive(&mut chat, &mut s);
    assert_eq!(objects(&mut s), 1);
    let results: Value = serde_json::from_str(&net.seen()[1].body).unwrap();
    let results = &results["messages"][2]["content"];
    assert_eq!((&results[0]["is_error"], &results[1]["is_error"]), (&json!(true), &json!(false)));
}

#[test]
fn a_provider_error_is_a_note_in_the_chat_and_the_scene_is_untouched() {
    let net = Arc::new(Fake::default().reply(401, r#"{"error":{"message":"invalid x-api-key"}}"#).fail("connection reset"));
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("hello", KEY);
    drive(&mut chat, &mut s);
    assert!(matches!(chat.log.last(), Some(Line::Note(n)) if n.contains("invalid x-api-key")), "{:?}", chat.log);
    assert_eq!(objects(&mut s), 0);
    chat.send("again", KEY);
    drive(&mut chat, &mut s);
    assert!(matches!(chat.log.last(), Some(Line::Note(n)) if n.contains("connection reset")), "{:?}", chat.log);
    // Nothing left to send: the fake has run dry, and that is an error in the chat too.
    chat.send("once more", KEY);
    drive(&mut chat, &mut s);
    assert!(matches!(chat.log.last(), Some(Line::Note(n)) if n.contains("no more replies")));
}

#[test]
fn a_model_that_keeps_calling_tools_is_stopped_and_what_it_did_stays() {
    let mut fake = Fake::default();
    for _ in 0..=crate::MAX_ROUNDS {
        fake = fake.reply(200, tool_reply(&[("t", "execute", BOX)]));
    }
    let net = Arc::new(fake);
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("loop", KEY);
    drive(&mut chat, &mut s);
    assert_eq!(net.seen().len(), crate::MAX_ROUNDS);
    assert!(matches!(chat.log.last(), Some(Line::Note(n)) if n.contains("Stopped after")));
    assert_eq!(objects(&mut s), crate::MAX_ROUNDS);
}

#[test]
fn more_calls_than_the_cap_are_answered_with_errors_not_run() {
    let calls: Vec<(String, &str, &str)> = (0..crate::MAX_CALLS + 2).map(|i| (format!("t{i}"), "execute", BOX)).collect();
    let calls: Vec<(&str, &str, &str)> = calls.iter().map(|(a, b, c)| (a.as_str(), *b, *c)).collect();
    let net = Arc::new(Fake::default().reply(200, tool_reply(&calls)).reply(200, text_reply("ok")));
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("many", KEY);
    drive(&mut chat, &mut s);
    assert_eq!(objects(&mut s), crate::MAX_CALLS);
    let errors = chat.log.iter().filter(|l| matches!(l, Line::Used { error: true, .. })).count();
    assert_eq!(errors, 2);
}

#[test]
fn the_key_is_in_one_header_and_never_in_the_log_the_history_or_a_debug_print() {
    let net = Arc::new(Fake::default().reply(200, tool_reply(&[("t", "execute", BOX)])).reply(200, text_reply("done")));
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("a box", KEY);
    drive(&mut chat, &mut s);
    for sent in net.seen() {
        assert!(!sent.body.contains(KEY) && !sent.url.contains(KEY) && !format!("{sent:?}").contains(KEY));
        assert_eq!(sent.headers.iter().filter(|(_, v)| v.contains(KEY)).count(), 1);
    }
    assert!(!format!("{:?}", chat.log).contains(KEY));
}

#[test]
fn a_message_is_refused_while_one_runs_and_when_it_is_too_long_or_the_key_is_bad() {
    let net = Arc::new(Fake::default().reply(200, text_reply("hi")));
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("   ", KEY);
    assert!(!chat.busy() && chat.log.is_empty());
    chat.send("hello", "bad key");
    assert!(!chat.busy() && matches!(chat.log.last(), Some(Line::Note(_))));
    chat.send(&"x".repeat(crate::MAX_TEXT + 1), KEY);
    assert!(!chat.busy() && net.seen().is_empty());
    chat.send("hello", KEY);
    chat.send("second while the first runs", KEY);
    drive(&mut chat, &mut s);
    assert_eq!(net.seen().len(), 1);
    // A conversation past the cap is not sent again.
    chat.history.push(Msg::user("x".repeat(crate::MAX_HISTORY)));
    chat.send("one more", KEY);
    assert!(!chat.busy() && matches!(chat.log.last(), Some(Line::Note(n)) if n.contains("too long")));
    chat.clear();
    assert!(chat.log.is_empty());
}

#[test]
fn stopping_ends_the_turn_and_keeps_what_was_done() {
    let net = Arc::new(Fake::default().reply(200, tool_reply(&[("t", "execute", BOX)])).reply(200, text_reply("never read")));
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("a box", KEY);
    // Wait for the first reply, take it, and stop before the second is read.
    let until = Instant::now() + Duration::from_secs(20);
    while !chat.poll(&mut s, Access::Edit) {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
    chat.stop();
    assert!(!chat.busy());
    assert!(!chat.poll(&mut s, Access::Edit));
    assert_eq!(objects(&mut s), 1);
}

/// Polls, as frames do, until `seen` says yes.
fn poll_until(chat: &mut Chat, s: &mut Session, seen: impl Fn(&Chat) -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !seen(chat) {
        assert!(Instant::now() < until, "not seen: {:?}", chat.log);
        chat.poll(s, Access::Edit);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn the_answer_grows_in_the_log_as_it_streams_and_the_whole_reply_takes_its_place() {
    let (fake, body) = Fake::default().live(200);
    let net = Arc::new(fake);
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    let stream = anthropic_stream(
        &json!([{"type": "thinking", "thinking": "Short.", "signature": "s"}, {"type": "text", "text": "Hello there"}]),
        "end_turn",
    );
    let at = stream.find("event: content_block_stop\ndata: {\"index\":1").unwrap();
    chat.send("hi", KEY);
    body.send(stream[..at].to_string()).unwrap();
    poll_until(&mut chat, &mut s, |c| c.log.last() == Some(&Line::Says("Hello there".into())));
    assert!(chat.busy(), "the turn is still on: the reply has not ended");
    assert_eq!(chat.log.len(), 2, "one line grows: {:?}", chat.log);
    body.send(stream[at..].to_string()).unwrap();
    drop(body);
    drive(&mut chat, &mut s);
    assert_eq!(chat.log, [Line::You("hi".into()), Line::Thinks("Short.".into()), Line::Says("Hello there".into())]);
    assert_eq!(chat.history.len(), 2);
}

#[test]
fn a_reply_refused_in_the_middle_takes_back_what_it_showed() {
    let refused = text_reply("Here is how").replacen(
        r#""stop_reason":"end_turn""#,
        r#""stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber"}"#,
        1,
    );
    let net = Arc::new(Fake::default().reply(200, refused));
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("hi", KEY);
    drive(&mut chat, &mut s);
    assert!(matches!(&chat.log[..], [Line::You(_), Line::Note(n)] if n.contains("declined")), "{:?}", chat.log);
}

#[test]
fn stopping_a_stream_closes_it_and_keeps_what_was_shown() {
    let (fake, body) = Fake::default().live(200);
    let net = Arc::new(fake);
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    let stream = text_reply("Half an answer");
    chat.send("hi", KEY);
    body.send(stream[..stream.find("event: content_block_stop").unwrap()].to_string()).unwrap();
    poll_until(&mut chat, &mut s, |c| matches!(c.log.last(), Some(Line::Says(t)) if t == "Half an answer"));
    chat.stop();
    assert!(!chat.busy());
    assert!(
        matches!(&chat.log[..], [Line::You(_), Line::Says(t), Line::Note(n)] if t == "Half an answer" && n == "Stopped."),
        "{:?}",
        chat.log
    );
    // The thread stops reading at the next event, which drops the body: the server's next send finds no one.
    let until = Instant::now() + Duration::from_secs(20);
    while body.send("event: ping\ndata: {\"type\": \"ping\"}\n\n".into()).is_ok() {
        assert!(Instant::now() < until, "the stream is still read after Stop");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!chat.poll(&mut s, Access::Edit) && chat.history.len() == 1);
}

#[test]
fn a_conversation_is_not_continued_on_another_provider_until_the_chat_is_cleared() {
    let said = openai_reply(json!([{"type": "message", "content": [{"type": "output_text", "text": "Hello."}]}]), "completed");
    let net = Arc::new(Fake::default().reply(401, r#"{"error":{"message":"bad key"}}"#).reply(200, said).reply(200, text_reply("Hi.")));
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("hi", KEY);
    drive(&mut chat, &mut s);
    // Nothing the model said is in the conversation yet, so another provider may take it.
    (chat.provider, chat.model) = (Provider::OpenAi, "gpt-6-astra".into());
    chat.send("hi again", KEY);
    drive(&mut chat, &mut s);
    assert_eq!(chat.log.last(), Some(&Line::Says("Hello.".into())));
    // Now it holds OpenAI's own turn, which Anthropic would read as nonsense.
    (chat.provider, chat.model) = (Provider::Anthropic, "claude-sonnet-5-5".into());
    chat.send("and again", KEY);
    assert!(!chat.busy() && matches!(chat.log.last(), Some(Line::Note(n)) if n.contains("OpenAI") && n.contains("Anthropic Claude")));
    assert_eq!(net.seen().len(), 2);
    chat.clear();
    chat.send("and again", KEY);
    drive(&mut chat, &mut s);
    assert_eq!(net.seen().len(), 3);
}

// ---------- the saved key, scrambled ----------

use crate::seal;

/// A secret for the test of the format: the build's own is not the test's to know.
const SECRET: &str = "correct horse battery";
const ADDED: &str = "sk-ant-test-0123456789";

fn hex_of(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn a_sealed_key_is_the_standard_pbkdf2_and_aes_gcm_that_another_program_reads() {
    // Made by Python's hashlib.pbkdf2_hmac and cryptography's AESGCM, from the same secret, salt, nonce and iterations: the format is
    // the standards', not just something this code can read back.
    let known = "lm1:1000:000102030405060708090a0b0c0d0e0f:a0a1a2a3a4a5a6a7a8a9aaab:1aa41a32de7c6c9b027a51f888b267496b05424b7bcb161bfe678282977623d3b9dfad56389a";
    assert_eq!(seal::open_with(SECRET, known).unwrap(), ADDED);
    let salt: Vec<u8> = (0..16).collect();
    let nonce: Vec<u8> = (0xa0..0xac).collect();
    assert_eq!(seal::seal_with(SECRET, ADDED, &salt, &nonce, 1000).unwrap(), known);
}

#[test]
fn a_key_sealed_opens_under_the_builds_secret_and_under_no_other() {
    let sealed = seal::seal(ADDED).unwrap();
    assert!(sealed.starts_with("lm1:") && seal::well_formed(&sealed));
    assert!(!sealed.contains(ADDED) && !sealed.contains(&hex_of(ADDED)), "the key is not in it, in any form a reader would see");
    assert_eq!(seal::open(&sealed).unwrap(), ADDED);
    for other in ["", "another secret", SECRET, seal::DEV_SECRET.to_uppercase().as_str()] {
        if other != if seal::built_with_secret() { "" } else { seal::DEV_SECRET } {
            assert!(seal::open_with(other, &sealed).is_err(), "{other:?}");
        }
    }
    // The same key seals differently each time: a salt and a nonce of its own. A key pasted with its line break is the key.
    assert_ne!(sealed, seal::seal(ADDED).unwrap());
    assert_eq!(seal::open(&seal::seal(&format!("  {ADDED}\n")).unwrap()).unwrap(), ADDED);
    let long = "k".repeat(crate::KEY_MAX);
    assert_eq!(seal::open(&seal::seal(&long).unwrap()).unwrap(), long);
    for no_key in ["", "short", "has a space in it", "line\nbreak-in-it-0123"] {
        assert!(seal::seal(no_key).is_err(), "{no_key:?}");
    }
    assert!(seal::seal(&"k".repeat(crate::KEY_MAX + 1)).is_err());
}

#[test]
fn a_sealed_key_that_was_changed_is_refused() {
    let sealed = seal::seal(ADDED).unwrap();
    let parts: Vec<&str> = sealed.split(':').collect();
    // Any hex digit of the salt, the nonce or the text, changed, is a key that does not open: the tag says so.
    for part in 2..5 {
        for at in [0, parts[part].len() / 2, parts[part].len() - 1] {
            let mut p: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
            let other = if p[part].as_bytes()[at] == b'0' { "1" } else { "0" };
            p[part].replace_range(at..=at, other);
            assert!(seal::open(&p.join(":")).is_err(), "part {part} at {at}");
        }
    }
    // The iterations are not covered by the tag: they only decide the key derived, so a changed number is the wrong key.
    let slower = sealed.replacen(&format!(":{}:", seal::ITERATIONS), &format!(":{}:", seal::ITERATIONS + 1), 1);
    assert_ne!(slower, sealed);
    assert!(seal::open(&slower).is_err());
}

#[test]
fn what_is_not_a_sealed_key_is_refused_quickly_and_never_panics() {
    let sealed = seal::seal(ADDED).unwrap();
    let parts: Vec<&str> = sealed.split(':').collect();
    let (salt, nonce, text) = (parts[2], parts[3], parts[4]);
    let huge = format!("lm1:4294967295:{salt}:{nonce}:{text}");
    let zero = format!("lm1:0:{salt}:{nonce}:{text}");
    let started = std::time::Instant::now();
    for bad in [
        "",
        "lm1",
        "lm1:::::",
        "lm2:10000:00:00:00",
        "x:10000:00:00:00",
        &sealed[..sealed.len() - 3],
        &format!("{sealed}0"),
        &format!("{sealed}:extra"),
        &sealed.to_uppercase(),
        &format!("lm1:10000:{}:{nonce}:{text}", "zz".repeat(16)),
        &format!("lm1:10000:{}:{nonce}:{text}", &salt[..30]),
        &format!("lm1:10000:{salt}:{}:{text}", &nonce[..22]),
        &format!("lm1:10000:{salt}:{nonce}:"),
        &format!("lm1:10000:{salt}:{nonce}:00"),
        "\u{0}\u{0}",
        huge.as_str(),
        zero.as_str(),
        &format!("lm1:{}:{salt}:{nonce}:{text}", "9".repeat(40)),
    ] {
        assert!(!seal::well_formed(bad), "{bad:?}");
        assert!(seal::open(bad).is_err(), "{bad:?}");
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a hostile file does not make the window derive a key for minutes: {:?}",
        started.elapsed()
    );
    assert!(seal::seal_with("", ADDED, &[0; 16], &[0; 12], 1000).is_err(), "a secret to seal with");
}

#[test]
fn a_release_is_built_with_its_own_secret() {
    // `built_with_secret` is what the build was given. The release workflow says a release must have one (`LAMINA_REQUIRE_KEY_SECRET`),
    // and this is the test that fails it when the CI secret is missing or empty: it would otherwise seal with the public one.
    assert_eq!(seal::built_with_secret(), option_env!("LAMINA_KEY_SECRET").is_some_and(|s| !s.trim().is_empty()));
    if std::env::var_os("LAMINA_REQUIRE_KEY_SECRET").is_some() {
        assert!(seal::built_with_secret(), "this build has no LAMINA_KEY_SECRET: set the repository secret of that name before a release");
    }
}

// ---------- the saved key, under a passphrase ----------

const PASSPHRASE: &str = "correct horse battery";

#[test]
fn a_key_sealed_with_a_passphrase_is_the_standard_pbkdf2_and_aes_gcm_too() {
    // Made by Python's hashlib.pbkdf2_hmac and cryptography's AESGCM, from the same passphrase, salt, nonce, iterations and associated data.
    let known = "lm2:1000:000102030405060708090a0b0c0d0e0f:a0a1a2a3a4a5a6a7a8a9aaab:1aa41a32de7c6c9b027a51f888b267496b05424b7bcb369a227610b38e496fe58aae3f73c6a3";
    assert_eq!(seal::open_passphrase(PASSPHRASE, known).unwrap(), ADDED);
    let salt: Vec<u8> = (0..16).collect();
    let nonce: Vec<u8> = (0xa0..0xac).collect();
    assert_eq!(seal::seal_passphrase_with(PASSPHRASE, ADDED, &salt, &nonce, 1000).unwrap(), known);
    assert!(seal::needs_passphrase(known) && seal::well_formed(known));
}

#[test]
fn a_key_under_a_passphrase_opens_with_it_alone_and_only_with_it() {
    let sealed = seal::seal_passphrase(PASSPHRASE, ADDED).unwrap();
    assert!(sealed.starts_with("lm2:600000:") && seal::needs_passphrase(&sealed) && seal::well_formed(&sealed));
    assert!(!sealed.contains(ADDED) && !sealed.contains(&hex_of(ADDED)) && !sealed.contains(PASSPHRASE));
    assert_eq!(seal::open_passphrase(PASSPHRASE, &sealed).unwrap(), ADDED);
    for wrong in ["", "correct horse batter", "Correct horse battery", " correct horse battery", seal::DEV_SECRET] {
        assert!(seal::open_passphrase(wrong, &sealed).is_err(), "{wrong:?}");
    }
    // It does not open without the passphrase, whatever the build's secret is, and a key sealed under the build's is not a passphrase's.
    assert!(seal::open(&sealed).unwrap_err().contains("passphrase"));
    let built_in = seal::seal(ADDED).unwrap();
    assert!(!seal::needs_passphrase(&built_in) && seal::open_passphrase(PASSPHRASE, &built_in).is_err());
    assert_ne!(sealed, seal::seal_passphrase(PASSPHRASE, ADDED).unwrap(), "a salt and a nonce of its own");
    // A passphrase in any script, a key as long as keys come, and a key pasted with its line break.
    let long = "k".repeat(crate::KEY_MAX);
    for (p, k) in [("пароль-у-мене-є", ADDED), ("pass phrase with spaces", long.as_str())] {
        assert_eq!(seal::open_passphrase(p, &seal::seal_passphrase(p, k).unwrap()).unwrap(), k);
    }
    assert_eq!(seal::open_passphrase(PASSPHRASE, &seal::seal_passphrase(PASSPHRASE, &format!(" {ADDED}\n")).unwrap()).unwrap(), ADDED);
    // Nothing to keep, or nothing to keep it with.
    for (p, k) in [("short", ADDED), ("", ADDED), (PASSPHRASE, ""), (PASSPHRASE, "short")] {
        assert!(seal::seal_passphrase(p, k).is_err(), "{p:?} {k:?}");
    }
    assert!(seal::seal_passphrase(&"p".repeat(seal::PASSPHRASE_MAX + 1), ADDED).is_err());
    assert!(seal::seal_passphrase(PASSPHRASE, &"k".repeat(crate::KEY_MAX + 1)).is_err());
}

#[test]
fn a_key_under_a_passphrase_that_was_changed_is_refused_and_a_bad_text_never_panics() {
    let salt: Vec<u8> = (0..16).collect();
    let nonce: Vec<u8> = (0xa0..0xac).collect();
    let sealed = seal::seal_passphrase_with(PASSPHRASE, ADDED, &salt, &nonce, 1000).unwrap();
    let parts: Vec<&str> = sealed.split(':').collect();
    for part in 2..5 {
        for at in [0, parts[part].len() / 2, parts[part].len() - 1] {
            let mut p: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
            let other = if p[part].as_bytes()[at] == b'0' { "1" } else { "0" };
            p[part].replace_range(at..=at, other);
            assert!(seal::open_passphrase(PASSPHRASE, &p.join(":")).is_err(), "part {part} at {at}");
        }
    }
    assert!(seal::open_passphrase(PASSPHRASE, &sealed.replacen(":1000:", ":1001:", 1)).is_err());
    // A text of the format's name that is not that format's, or any other, is refused before anything is derived.
    let (salt, nonce, text) = (parts[2], parts[3], parts[4]);
    let huge = format!("lm2:4294967295:{salt}:{nonce}:{text}");
    let started = std::time::Instant::now();
    for bad in [
        "lm2",
        "lm2:::::",
        "bc3:1000:00:00:00",
        &format!("lm2:1000:{}:{nonce}:{text}", &salt[..30]),
        &format!("lm2:1000:{salt}:{}:{text}", &nonce[..22]),
        &format!("lm2:1000:{salt}:{nonce}:00"),
        &format!("lm2:1000:{salt}:{nonce}:{text}:extra"),
        &sealed.to_uppercase(),
        &format!("lm2:0:{salt}:{nonce}:{text}"),
        &format!("lm2:99999999999:{salt}:{nonce}:{text}"),
        huge.as_str(),
    ] {
        assert!(!seal::well_formed(bad), "{bad:?}");
        assert!(seal::open_passphrase(PASSPHRASE, bad).is_err(), "{bad:?}");
    }
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    assert!(!seal::needs_passphrase("") && !seal::needs_passphrase("lm1:1000:00:00:00") && !seal::needs_passphrase("garbage"));
}

// ---------- thinking, the checklist, files and tokens ----------

#[test]
fn every_provider_gives_its_thinking_apart_from_the_answer_and_asks_for_it() {
    let a = anthropic_reply(
        json!([{"type": "thinking", "thinking": "Boxes first.", "signature": "s"}, {"type": "text", "text": "Hi"}]),
        "end_turn",
    );
    let r = anthropic::parse(200, &a).unwrap();
    assert_eq!((r.thinking.as_str(), r.text.as_str()), ("Boxes first.", "Hi"));
    let body: Value = serde_json::from_str(&anthropic::request("claude-sonnet-5-5", KEY, &[Msg::user("x")]).body).unwrap();
    assert_eq!(body["thinking"], json!({"type": "adaptive", "display": "summarized"}), "the default hides the summary");

    let o = openai_reply(
        json!([{"type": "reasoning", "summary": [{"type": "summary_text", "text": "One."}, {"type": "summary_text", "text": "Two."}]},
               {"type": "message", "content": [{"type": "output_text", "text": "Hi"}]}]),
        "completed",
    );
    let r = openai::parse(200, &o).unwrap();
    assert_eq!((r.thinking.as_str(), r.text.as_str()), ("One.\n\nTwo.", "Hi"));
    let body: Value = serde_json::from_str(&openai::request("gpt-6-astra", KEY, &[Msg::user("x")]).body).unwrap();
    assert_eq!(body["reasoning"]["summary"], "auto");

    let g = gemini_reply(json!([{"text": "Checking.", "thought": true}, {"text": "Hi"}]), "STOP");
    let r = gemini::parse(200, &g).unwrap();
    assert_eq!((r.thinking.as_str(), r.text.as_str()), ("Checking.", "Hi"));
    let body: Value = serde_json::from_str(&gemini::request("gemini-3.8-flash", KEY, &[Msg::user("x")]).body).unwrap();
    assert_eq!(body["generationConfig"]["thinkingConfig"]["includeThoughts"], true);
}

#[test]
fn every_provider_counts_the_tokens_of_a_reply_in_its_own_fields() {
    let a = json!({"content": [], "stop_reason": "end_turn", "usage": {"input_tokens": 10, "cache_read_input_tokens": 90, "cache_creation_input_tokens": 5, "output_tokens": 7}});
    assert_eq!(anthropic::parse(200, &a.to_string()).unwrap().usage, Usage { input: 105, output: 7 });
    let o = json!({"output": [], "status": "completed", "usage": {"input_tokens": 40, "output_tokens": 9}});
    assert_eq!(openai::parse(200, &o.to_string()).unwrap().usage, Usage { input: 40, output: 9 });
    let g = json!({"candidates": [{"finishReason": "STOP", "content": {"parts": []}}], "usageMetadata": {"promptTokenCount": 30, "candidatesTokenCount": 4, "thoughtsTokenCount": 6}});
    assert_eq!(gemini::parse(200, &g.to_string()).unwrap().usage, Usage { input: 30, output: 10 });
    // A reply with no count, or a hostile one, counts as none and never panics.
    let hostile = json!({"content": [], "usage": {"input_tokens": "lots", "output_tokens": -1}});
    assert_eq!(anthropic::parse(200, &hostile.to_string()).unwrap().usage, Usage::default());
    assert_eq!(Usage { input: u64::MAX, output: 1 }.total(), u64::MAX);
}

#[test]
fn files_go_before_the_words_in_each_providers_own_form() {
    let files = vec![
        Attached { name: "notes.txt".into(), body: Body::Text("two boxes".into()) },
        Attached { name: "plan.png".into(), body: Body::Image { mime: "image/png", b64: "AAAA".into() } },
    ];
    let history = [Msg::User("what is this?".into(), files)];
    let body = |r: Request| serde_json::from_str::<Value>(&r.body).unwrap();
    let a = body(anthropic::request("claude-sonnet-5-5", KEY, &history));
    assert_eq!(a["messages"][0]["content"][0], json!({"type": "text", "text": "<file name=\"notes.txt\">\ntwo boxes\n</file>"}));
    assert_eq!(
        a["messages"][0]["content"][1],
        json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}})
    );
    assert_eq!(a["messages"][0]["content"][2], json!({"type": "text", "text": "what is this?"}));
    let o = body(openai::request("gpt-6-astra", KEY, &history));
    assert_eq!(o["input"][0]["content"][1], json!({"type": "input_image", "image_url": "data:image/png;base64,AAAA"}));
    assert_eq!(o["input"][0]["content"][2], json!({"type": "input_text", "text": "what is this?"}));
    let g = body(gemini::request("gemini-3.8-flash", KEY, &history));
    assert_eq!(g["contents"][0]["parts"][1], json!({"inlineData": {"mimeType": "image/png", "data": "AAAA"}}));
    assert_eq!(g["contents"][0]["parts"][2], json!({"text": "what is this?"}));
    assert!(!a.to_string().contains(KEY));
}

#[test]
fn base64_is_the_standard_one_with_padding() {
    for (plain, coded) in
        [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")]
    {
        assert_eq!(base64(plain.as_bytes()), coded);
    }
    assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
}

#[test]
fn a_checklist_is_read_whole_and_a_bad_one_is_told_what_is_wrong() {
    let ok = tools::todos(&json!({"todos": [{"content": " Add boxes ", "status": "in_progress"}, {"content": "Move them", "status": "pending"}, {"content": "x".repeat(500), "status": "completed"}]})).unwrap();
    assert_eq!(
        (ok[0].content.as_str(), ok[0].status, ok[1].status, ok[2].status),
        ("Add boxes", Status::InProgress, Status::Pending, Status::Completed)
    );
    assert_eq!(ok[2].content.chars().count(), crate::MAX_TODO_TEXT + 1, "cut, with its ellipsis");
    for bad in [
        json!({}),
        json!({"todos": "all of it"}),
        json!({"todos": [{"content": "", "status": "pending"}]}),
        json!({"todos": [{"content": "a", "status": "done"}]}),
        json!({"todos": (0..=crate::MAX_TODOS).map(|i| json!({"content": format!("step {i}"), "status": "pending"})).collect::<Vec<_>>()}),
    ] {
        assert!(tools::todos(&bad).is_err(), "{bad}");
    }
    // The model is offered it, and it changes nothing in the project.
    assert!(tools::all().iter().any(|t| t.name == tools::TODO));
    let mut s = Session::new();
    run(&mut s, "todo_write", json!({"todos": [{"content": "a", "status": "pending"}]})).unwrap();
    assert_eq!(objects(&mut s), 0);
}

#[test]
fn the_checklist_is_kept_for_the_palette_not_logged_as_a_call_and_cleared_by_the_next_message() {
    let list = r#"{"todos":[{"content":"Add boxes","status":"in_progress"},{"content":"Move them","status":"pending"}]}"#;
    let net = Arc::new(
        Fake::default()
            .reply(200, tool_reply(&[("t1", "todo_write", list), ("t2", "execute", BOX)]))
            .reply(200, text_reply("Done."))
            .reply(200, text_reply("Again.")),
    );
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("two boxes and doors", KEY);
    drive(&mut chat, &mut s);
    assert_eq!(chat.todos.len(), 2);
    assert_eq!((chat.todos[0].content.as_str(), chat.todos[0].status), ("Add boxes", Status::InProgress));
    let used: Vec<_> = chat.log.iter().filter_map(|l| if let Line::Used { name, .. } = l { Some(name.as_str()) } else { None }).collect();
    assert_eq!(used, ["execute"], "the checklist is not a line of the log");
    assert_eq!(objects(&mut s), 1);
    chat.send("and again", KEY);
    assert!(chat.todos.is_empty(), "a new message starts a new list");
    drive(&mut chat, &mut s);
    // A list that is refused is shown as the failed call it is.
    let net = Arc::new(
        Fake::default()
            .reply(200, tool_reply(&[("t1", "todo_write", r#"{"todos":[{"content":"a","status":"nope"}]}"#)]))
            .reply(200, text_reply("ok")),
    );
    let mut chat = chat_on(&net);
    chat.send("plan", KEY);
    drive(&mut chat, &mut s);
    assert!(chat.todos.is_empty() && chat.log.iter().any(|l| matches!(l, Line::Used { name, error: true, .. } if name == "todo_write")));
}

#[test]
fn thinking_comes_before_the_answer_in_the_log_and_the_tokens_are_handed_over_once() {
    let first = anthropic_stream(
        &json!([{"type": "thinking", "thinking": "Look first.", "signature": "s"}, {"type": "tool_use", "id": "t1", "name": "inspect_scene", "input": {}}]),
        "tool_use",
    );
    let net = Arc::new(Fake::default().reply(200, first).reply(200, text_reply("One box.")));
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("what is in it?", KEY);
    drive(&mut chat, &mut s);
    let kinds: Vec<&str> = chat
        .log
        .iter()
        .map(|l| match l {
            Line::You(_) => "you",
            Line::Thinks(_) => "thinks",
            Line::Says(_) => "says",
            Line::Used { .. } => "used",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["you", "thinks", "used", "says"]);
    assert_eq!(chat.take_usage(), Usage { input: 20, output: 10 }, "two replies of 10 and 5 tokens each");
    assert_eq!(chat.take_usage(), Usage::default(), "handed over once");
}

/// A folder with one file of each kind a user might pick.
fn picked() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "two boxes").unwrap();
    std::fs::write(dir.path().join("plan.png"), [0x89, b'P', b'N', b'G']).unwrap();
    std::fs::write(dir.path().join("data.bin"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
    std::fs::write(dir.path().join("big.txt"), "x".repeat(crate::MAX_FILE_TEXT as usize + 1)).unwrap();
    std::fs::create_dir(dir.path().join("folder")).unwrap();
    dir
}

#[test]
fn a_picked_file_is_copied_to_a_folder_of_the_chats_and_what_is_not_sendable_is_refused_with_a_note() {
    let (from, net) = (picked(), Arc::new(Fake::default()));
    let mut chat = chat_on(&net);
    assert!(chat.stage_dir().is_none(), "no folder until a file is picked");
    chat.attach(&from.path().join("notes.txt"));
    chat.attach(&from.path().join("plan.png"));
    let stage = chat.stage_dir().unwrap().to_path_buf();
    assert_eq!(chat.attached.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["notes.txt", "plan.png"]);
    assert!(chat.attached.iter().all(|a| std::fs::metadata(&stage).is_ok() && a.size > 0));
    assert_eq!(std::fs::read_dir(&stage).unwrap().count(), 2);
    // The copy is the chat's: the original may go.
    std::fs::remove_file(from.path().join("notes.txt")).unwrap();
    for (bad, why) in [("data.bin", "not text"), ("big.txt", "at most"), ("folder", "not a file"), ("missing.txt", "could not read")] {
        let before = chat.attached.len();
        chat.attach(&from.path().join(bad));
        assert_eq!(chat.attached.len(), before, "{bad}");
        assert!(matches!(chat.log.last(), Some(Line::Note(n)) if n.contains(why)), "{bad}: {:?}", chat.log.last());
    }
    for _ in 0..MAX_FILES {
        chat.attach(&from.path().join("plan.png"));
    }
    assert_eq!(chat.attached.len(), MAX_FILES);
    assert!(matches!(chat.log.last(), Some(Line::Note(n)) if n.contains("at most go with one message")));
    chat.detach(0);
    assert_eq!(chat.attached.len(), MAX_FILES - 1);
    assert_eq!(std::fs::read_dir(&stage).unwrap().count(), MAX_FILES - 1, "the removed one's copy is deleted");
}

#[test]
fn a_file_name_that_could_end_the_block_early_is_cleaned() {
    assert_eq!(crate::attach::clean("a\"><b.txt"), "a___b.txt");
    // Windows has no file by that name to pick: the cleaning is checked on it above, and on a file where one can be made.
    if cfg!(unix) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a\"><b.txt"), "x").unwrap();
        let mut chat = chat_on(&Arc::new(Fake::default()));
        chat.attach(&dir.path().join("a\"><b.txt"));
        assert_eq!(chat.attached[0].name, "a___b.txt");
    }
}

#[test]
fn files_go_with_the_message_and_their_folder_goes_with_the_chat() {
    let (from, net) = (picked(), Arc::new(Fake::default().reply(200, text_reply("Two boxes and a picture."))));
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.attach(&from.path().join("notes.txt"));
    chat.attach(&from.path().join("plan.png"));
    let stage = chat.stage_dir().unwrap().to_path_buf();
    chat.send("what is in these?", KEY);
    drive(&mut chat, &mut s);
    assert!(chat.attached.is_empty(), "sent files are not sent again");
    assert!(chat.log.contains(&Line::Files(vec!["notes.txt".into(), "plan.png".into()])));
    let body: Value = serde_json::from_str(&net.seen()[0].body).unwrap();
    let content = &body["messages"][0]["content"];
    assert!(content[0]["text"].as_str().unwrap().contains("<file name=\"notes.txt\">\ntwo boxes\n</file>"));
    assert_eq!(content[1]["source"]["data"], base64(&[0x89, b'P', b'N', b'G']));
    assert_eq!(content[2]["text"], "what is in these?");
    assert!(stage.exists(), "the copies stay until the chat goes");
    chat.clear();
    assert!(!stage.exists() && chat.stage_dir().is_none(), "a new chat removes the folder");
    // Dropping the chat, as the window closing does, removes it too.
    chat.attach(&from.path().join("plan.png"));
    let stage = chat.stage_dir().unwrap().to_path_buf();
    assert!(stage.exists());
    drop(chat);
    assert!(!stage.exists(), "closing removes the folder");
}

#[test]
fn more_file_bytes_than_a_conversation_may_carry_are_refused() {
    let (from, net) = (picked(), Arc::new(Fake::default()));
    let mut chat = chat_on(&net);
    chat.history.push(Msg::User("x".into(), vec![Attached { name: "old.txt".into(), body: Body::Text("y".repeat(MAX_FILES_TOTAL)) }]));
    chat.attach(&from.path().join("notes.txt"));
    chat.send("again", KEY);
    assert!(!chat.busy() && matches!(chat.log.last(), Some(Line::Note(n)) if n.contains("more than can be sent")));
    assert_eq!(chat.attached.len(), 1, "what was picked is still there to send after a new chat");
}

// ---------- fuzz ----------

#[test]
fn provider_replies_and_sealed_keys_never_panic() {
    use lamina_core::fuzz::Rng;
    let replies = [
        anthropic_reply(
            json!([{"type": "text", "text": "hi"}, {"type": "tool_use", "id": "t", "name": "execute", "input": {"command": "info"}}]),
            "tool_use",
        ),
        openai_reply(json!([{"type": "message", "content": [{"type": "output_text", "text": "hi"}]}]), "completed"),
        gemini_reply(json!([{"text": "hi"}, {"functionCall": {"name": "execute", "args": {"command": "info"}}}]), "STOP"),
    ];
    let good: Vec<&[u8]> = replies.iter().map(|r| r.as_bytes()).collect();
    let mut r = Rng::new(0xa551);
    for _ in 0..3000 {
        let body = String::from_utf8_lossy(&r.input(&good)).into_owned();
        let status = [200, 400, 401, 429, 500][r.below(5)];
        let _ = crate::anthropic::parse(status, &body);
        let _ = crate::openai::parse(status, &body);
        let _ = crate::gemini::parse(status, &body);
    }
    // Sealed keys: the cheap format only (a passphrase key costs 600,000 rounds of PBKDF2 to open).
    let sealed = crate::seal::seal("sk-test-0123456789abcdef").unwrap();
    for _ in 0..3000 {
        let text = String::from_utf8_lossy(&r.input(&[sealed.as_bytes()])).into_owned();
        if crate::seal::well_formed(&text) && !crate::seal::needs_passphrase(&text) {
            let _ = crate::seal::open(&text);
        }
    }
}

// ---------- pictures ----------

#[test]
fn preview_draws_the_plate_as_a_png_and_refuses_odd_arguments() {
    let mut s = Session::new();
    run(&mut s, "execute", serde_json::from_str(BOX).unwrap()).unwrap();
    let (text, png) = tools::picture(&s, &json!({"azimuth": 30, "elevation": 60})).unwrap();
    assert!(text.contains("1 object") && text.contains("640x448"), "{text}");
    let bytes = lamina_core::base64::decode(&png).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert!(tools::picture(&Session::new(), &json!({})).unwrap().0.contains("empty"));
    for bad in [json!({"elevation": 95}), json!({"azimuth": "east"}), json!({"zoom": 2}), json!([1])] {
        assert!(tools::picture(&s, &bad).is_err(), "{bad}");
    }
    // Asked as an ordinary tool (an agent, say), it says what it drew; a session that may only read may look.
    let said = tools::run(&mut s, &Call { id: "p".into(), name: "preview".into(), args: json!({}) }, Access::Read).unwrap();
    assert!(said.as_str().unwrap().contains("A picture of the plate"));
}

fn with_picture() -> Vec<Msg> {
    let call = Call { id: "p1".into(), name: "preview".into(), args: json!({}) };
    let reply =
        Reply { thinking: String::new(), text: String::new(), calls: vec![call.clone()], raw: json!([]), usage: Default::default() };
    let done = Done { image: Some("iVBORw0KGgo=".into()), ..Done::text(&call, "A picture of the plate.".into(), false) };
    vec![Msg::user("does it look like my photo?"), Msg::Assistant(reply), Msg::Results(vec![done])]
}

#[test]
fn a_picture_goes_to_each_provider_in_its_own_form() {
    let history = with_picture();
    // Anthropic: in the tool's answer, beside its words.
    let body: Value = serde_json::from_str(&anthropic::request("claude-sonnet-5-5", KEY, &history).body).unwrap();
    let result = &body["messages"][2]["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["content"][0]["text"], "A picture of the plate.");
    assert_eq!(result["content"][1]["source"], json!({"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}));
    // OpenAI: the output in words, then a user message with the picture.
    let body: Value = serde_json::from_str(&openai::request("gpt-6-astra", KEY, &history).body).unwrap();
    let input = body["input"].as_array().unwrap();
    let at = input.iter().position(|i| i["type"] == "function_call_output").unwrap();
    assert_eq!(input[at]["output"], "A picture of the plate.");
    assert_eq!(input[at + 1]["role"], "user");
    assert_eq!(input[at + 1]["content"][1], json!({"type": "input_image", "image_url": "data:image/png;base64,iVBORw0KGgo="}));
    // Gemini: the response in words, then a user turn with the picture.
    let body: Value = serde_json::from_str(&gemini::request("gemini-3.8-flash", KEY, &history).body).unwrap();
    let contents = body["contents"].as_array().unwrap();
    assert!(contents[contents.len() - 2]["parts"][0]["functionResponse"].is_object());
    assert_eq!(contents[contents.len() - 1]["parts"][1], json!({"inlineData": {"mimeType": "image/png", "data": "iVBORw0KGgo="}}));
    // The picture counts with the files a conversation may carry.
    assert_eq!(history[2].file_bytes(), "iVBORw0KGgo=".len());
}

#[test]
fn a_preview_call_in_a_turn_sends_the_model_the_picture() {
    let net = Arc::new(
        Fake::default()
            .reply(200, tool_reply(&[("a", "execute", BOX), ("b", "preview", r#"{"elevation": 45}"#)]))
            .reply(200, text_reply("It is a box.")),
    );
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.send("make a box and look at it", KEY);
    drive(&mut chat, &mut s);
    let second: Value = serde_json::from_str(&net.seen()[1].body).unwrap();
    let results = &second["messages"][2]["content"];
    assert_eq!(results[1]["tool_use_id"], "b");
    let png = results[1]["content"][1]["source"]["data"].as_str().unwrap();
    assert_eq!(&lamina_core::base64::decode(png).unwrap()[..4], b"\x89PNG");
}

// ---------- a model on this computer ----------

#[test]
fn a_local_server_is_this_computer_by_address_and_nothing_else() {
    use crate::local::endpoint;
    assert_eq!(endpoint("localhost:11434").unwrap(), "http://127.0.0.1:11434/v1/chat/completions");
    assert_eq!(endpoint("http://localhost:11434/v1/").unwrap(), "http://127.0.0.1:11434/v1/chat/completions");
    assert_eq!(endpoint("http://127.0.0.1:1234/v1").unwrap(), "http://127.0.0.1:1234/v1/chat/completions");
    assert_eq!(endpoint("http://[::1]:8080/v1/chat/completions").unwrap(), "http://[::1]:8080/v1/chat/completions");
    assert_eq!(endpoint("http://LOCALHOST").unwrap(), "http://127.0.0.1/v1/chat/completions");
    for no in [
        "http://192.168.1.20:11434/v1",
        "http://example.com/v1",
        "http://localhost.evil.example:11434",
        "http://127.0.0.1.nip.io:11434",
        "http://127.0.0.2:11434",
        "http://0.0.0.0:11434",
        "http://user@127.0.0.1:11434",
        "http://evil.example@127.0.0.1",
        "http://127.0.0.1:11434@evil.example",
        "https://127.0.0.1:11434/v1",
        "ftp://127.0.0.1",
        "http://127.0.0.1:99999",
        "http://127.0.0.1:11434/v1?x=1",
        "http://127.0.0.1:11434/../v1",
        "",
        "http://127.0.0.1 /v1",
    ] {
        assert!(endpoint(no).is_err(), "{no}");
    }
    // The network allows plain HTTP to the loopback addresses only, never by name, and the providers stay HTTPS.
    assert!(allowed("http://127.0.0.1:11434/v1/chat/completions") && allowed("http://[::1]:1234/v1/chat/completions"));
    for no in ["http://localhost:11434/v1", "http://127.0.0.1.nip.io/", "http://u:p@127.0.0.1:1/", "http://api.openai.com/v1/responses"] {
        assert!(!allowed(no), "{no}");
    }
}

#[test]
fn the_local_provider_writes_chat_completions_and_reads_tool_calls() {
    let url = crate::local::endpoint("localhost:11434").unwrap();
    // No key for a local server: no Authorization header at all.
    let sent = crate::local::request(&url, "llama3.2:3b", "", &[Msg::user("a box")]);
    assert!(sent.headers.is_empty());
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!((body["model"].as_str(), body["stream"].as_bool()), (Some("llama3.2:3b"), Some(true)));
    assert_eq!(body["stream_options"]["include_usage"], true, "the counts come in a last chunk");
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1], json!({"role": "user", "content": "a box"}));
    assert_eq!(body["tools"][0]["function"]["name"], "execute");
    // A reply with a call whose arguments are text, and one with no id.
    let reply = json!({"choices": [{"message": {"role": "assistant", "content": "", "reasoning_content": "a box first", "tool_calls": [
        {"id": "call_a", "type": "function", "function": {"name": "execute", "arguments": BOX}},
        {"type": "function", "function": {"name": "inspect_scene", "arguments": ""}},
    ]}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 900, "completion_tokens": 40}});
    let r = crate::local::parse(200, &reply.to_string()).unwrap();
    assert_eq!(r.calls.len(), 2);
    assert_eq!(r.calls[0].args, serde_json::from_str::<Value>(BOX).unwrap());
    assert_eq!((r.calls[1].id.as_str(), &r.calls[1].args), ("call_1", &json!({})));
    assert_eq!((r.thinking.as_str(), r.usage.input), ("a box first", 900));
    // What goes back: the model's message with the ids it was given, then each answer as a tool message, then any picture.
    let done = vec![
        Done::text(&r.calls[0], "{\"id\":1}".into(), false),
        Done { image: Some("iVBORw0KGgo=".into()), ..Done::text(&r.calls[1], "seen".into(), false) },
    ];
    let next = crate::local::request(&url, "llama3.2:3b", KEY, &[Msg::user("a box"), Msg::Assistant(r), Msg::Results(done)]);
    assert_eq!(next.headers, vec![("Authorization".to_string(), format!("Bearer {KEY}"))]);
    let body: Value = serde_json::from_str(&next.body).unwrap();
    let m = body["messages"].as_array().unwrap();
    assert_eq!(m[2]["tool_calls"][1]["id"], "call_1");
    assert_eq!(m[3], json!({"role": "tool", "tool_call_id": "call_a", "content": "{\"id\":1}"}));
    assert_eq!(m[4]["tool_call_id"], "call_1");
    assert_eq!(m[5]["content"][1]["image_url"]["url"], "data:image/png;base64,iVBORw0KGgo=");
    // Errors are said, never a panic.
    for (status, bad) in [
        (500, r#"{"error":{"message":"model not found"}}"#),
        (200, "{}"),
        (200, "nope"),
        (200, r#"{"choices":[{"message":{"content":""},"finish_reason":"length"}]}"#),
    ] {
        assert!(crate::local::parse(status, bad).is_err(), "{bad}");
    }
}

#[test]
fn a_turn_with_a_local_model_needs_no_key_and_goes_to_this_computer() {
    let answer = json!({"choices": [{"message": {"role": "assistant", "content": "Hello from here."}, "finish_reason": "stop"}]});
    let (base, server) = serve(vec![(200, answer.to_string().into_bytes())]);
    // The window's own network, which keeps to the loopback addresses for a local model.
    let mut chat = Chat::new(Arc::new(Ureq::new()));
    chat.provider = Provider::Local;
    chat.model = "qwen3".into();
    chat.local_url = base.replace("127.0.0.1", "localhost");
    let mut s = Session::new();
    chat.send("hi", "");
    drive(&mut chat, &mut s);
    assert!(chat.log.iter().any(|l| matches!(l, Line::Says(t) if t == "Hello from here.")), "{:?}", chat.log);
    let said = server.join().unwrap();
    assert!(said[0].starts_with("POST /v1/chat/completions "), "{}", said[0]);
    assert!(!said[0].to_ascii_lowercase().contains("authorization"), "no key, no header");
    // An address that is not this computer is refused before anything is sent.
    chat.local_url = "http://192.168.1.20:11434".into();
    chat.send("again", "");
    assert!(!chat.busy());
    assert!(matches!(chat.log.last(), Some(Line::Note(n)) if n.contains("this computer")), "{:?}", chat.log.last());
}

#[test]
fn a_control_port_token_is_128_random_bits_in_hex() {
    let (a, b) = (seal::token().unwrap(), seal::token().unwrap());
    assert_eq!(a.len(), 32);
    assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, b);
}

// ---------- saved chats ----------

#[test]
fn a_projects_chat_is_kept_beside_the_settings_and_comes_back() {
    use crate::saved;
    let dir = tempfile::tempdir().unwrap();
    let project = std::path::Path::new("/home/me/parts/bracket.lamina");
    let net =
        Arc::new(Fake::default().reply(200, tool_reply(&[("a", "execute", BOX), ("b", "preview", "{}")])).reply(200, text_reply("A box.")));
    let (mut chat, mut s) = (chat_on(&net), Session::new());
    chat.provider = Provider::Anthropic;
    chat.send("a box, and look at it", KEY);
    drive(&mut chat, &mut s);
    saved::write(dir.path(), project, &chat).unwrap();
    let file = dir.path().join(saved::file_name(project));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(!text.contains(KEY), "the key is never kept");
    assert!(!text.contains("iVBOR"), "nor the picture preview drew");
    // The next window: the log, the conversation and the checklist come back, and the model is sent it again.
    let mut back = chat_on(&Arc::new(Fake::default()));
    assert!(back.restore(&saved::read(dir.path(), project).unwrap(), Provider::Anthropic));
    assert_eq!(back.log, chat.log);
    assert_eq!(back.history.len(), chat.history.len());
    // With another provider chosen, it is shown but the model starts afresh.
    let mut other = chat_on(&Arc::new(Fake::default()));
    assert!(other.restore(&saved::read(dir.path(), project).unwrap(), Provider::Gemini));
    assert!(other.history.is_empty() && matches!(other.log.last(), Some(Line::Note(n)) if n.contains("afresh")));
    // A file that is not one, or of a newer version, is left alone, and the chat stays as it was.
    let mut kept = chat_on(&Arc::new(Fake::default()));
    for bad in [
        json!({}),
        json!({"v": 99, "log": [], "history": [], "todos": []}),
        json!({"v": 1, "log": [{"t": "?"}], "history": [], "todos": []}),
    ] {
        assert!(!kept.restore(&bad, Provider::Anthropic), "{bad}");
    }
    assert!(kept.log.is_empty());
    saved::remove(dir.path(), project);
    assert!(saved::read(dir.path(), project).is_none());
    assert_ne!(saved::file_name(project), saved::file_name(std::path::Path::new("/home/me/parts/other.lamina")));
}
