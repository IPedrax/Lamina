//! Duet boards (RepRapFirmware) on their own, from Duet3D's documented HTTP requests: `rr_connect` with the board's password (the key
//! field; RepRapFirmware's default is none) gives a session key for the requests after it, `rr_upload` takes the file into
//! `0:/gcodes/`, `rr_gcode` with `M32` starts it, and `rr_status` says what the board is doing. A Duet run by a single-board computer
//! (DSF) answers a different API, which is not reached yet.

use std::io::Read;

use serde_json::Value;

use crate::{MAX_REPLY, Sent, Status, agent, agent_for, finite, short};

/// `text` for a URL's query: what is not a letter, a digit or `-_.~` as `%XX`.
fn encoded(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') { (b as char).to_string() } else { format!("%{b:02X}") }
        })
        .collect()
}

/// One request's JSON reply, or what went wrong.
fn json(call: Result<ureq::http::Response<ureq::Body>, ureq::Error>, base: &str) -> Result<Value, String> {
    let response = call.map_err(|e| format!("Duet at {base}: {e}"))?;
    let status = response.status().as_u16();
    let mut body = Vec::new();
    response
        .into_body()
        .into_with_config()
        .limit(MAX_REPLY + 1)
        .reader()
        .read_to_end(&mut body)
        .map_err(|e| format!("Duet at {base}: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(format!("Duet at {base} answered {status}"));
    }
    serde_json::from_slice(&body).map_err(|_| format!("Duet at {base} sent a reply that is not JSON"))
}

/// Connects with `password` and gives the session key to send with what follows, when the board gives one.
fn connect(base: &str, password: Option<&str>, agent: &ureq::Agent) -> Result<Option<String>, String> {
    let v = json(agent.get(&format!("{base}/rr_connect?password={}", encoded(password.unwrap_or("")))).call(), base)?;
    match v["err"].as_u64() {
        Some(0) => Ok(v["sessionKey"].as_u64().map(|k| k.to_string())),
        Some(1) => Err(format!("Duet at {base}: wrong password (the key field holds the board's password)")),
        Some(2) => Err(format!("Duet at {base}: no more connections are free on the board")),
        _ => Err(format!("Duet at {base} did not let Lamina connect")),
    }
}

pub fn send(base: &str, password: Option<&str>, name: &str, gcode: &[u8], print: bool) -> Result<Sent, String> {
    let agent = agent();
    let session = connect(base, password, &agent)?;
    let path = format!("0:/gcodes/{name}");
    let mut up = agent.post(&format!("{base}/rr_upload?name={}", encoded(&path))).header("Content-Type", "application/octet-stream");
    if let Some(k) = &session {
        up = up.header("X-Session-Key", k);
    }
    let v = json(up.send(gcode), base)?;
    if v["err"].as_u64() != Some(0) {
        return Err(format!("Duet at {base} did not take the file"));
    }
    if print {
        let mut start = agent.get(&format!("{base}/rr_gcode?gcode={}", encoded(&format!("M32 \"{path}\""))));
        if let Some(k) = &session {
            start = start.header("X-Session-Key", k);
        }
        json(start.call(), base)?;
    }
    Ok(Sent { name: name.to_string(), printing: print })
}

pub fn status(base: &str, password: Option<&str>) -> Result<Status, String> {
    let agent = agent_for(std::time::Duration::from_secs(5));
    let session = connect(base, password, &agent)?;
    let mut call = agent.get(&format!("{base}/rr_status?type=3"));
    if let Some(k) = &session {
        call = call.header("X-Session-Key", k);
    }
    let v = json(call.call(), base)?;
    let state = match v["status"].as_str() {
        Some("I") => "idle",
        Some("P") => "printing",
        Some("S") | Some("A") | Some("D") => "paused",
        Some("B") => "busy",
        Some("H") => "halted",
        Some("O") => "off",
        _ => "unknown",
    };
    let t = &v["temps"];
    Ok(Status {
        state: state.into(),
        file: short(&v["printFileName"]),
        progress: finite(&v["fractionPrinted"]).map(|x| (x / 100.0).clamp(0.0, 1.0)),
        nozzle: finite(&t["current"][1]).zip(finite(&t["tools"]["active"][0][0])),
        bed: finite(&t["bed"]["current"]).zip(finite(&t["bed"]["active"])),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_is_encoded() {
        assert_eq!(encoded("M32 \"0:/gcodes/a.gcode\""), "M32%20%220%3A%2Fgcodes%2Fa.gcode%22");
    }
}
