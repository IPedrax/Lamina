//! A Duet run by a single-board computer (a Raspberry Pi with DuetSoftwareFramework, "SBC mode"), from the OpenAPI description of
//! DSF's web server: `GET /machine/connect` with the board's password gives a session key for the `X-Session-Key` header,
//! `PUT /machine/file/0:/gcodes/NAME` takes the file, `POST /machine/code` with `M32` starts it, and `GET /machine/model` is the object
//! model the status is read from. `duet.rs` asks here first: a board on its own does not answer DSF's connect as DSF does, and is
//! reached with its `rr_` requests instead.

use std::io::Read;

use serde_json::Value;
use ureq::{Agent, RequestBuilder};

use crate::duet::{encoded, json};
use crate::{MAX_REPLY, Sent, Status, finite, key_ok, short};

/// A session with DSF: its key, when it gave one.
pub struct Session(Option<String>);

impl Session {
    fn on<B>(&self, call: RequestBuilder<B>) -> RequestBuilder<B> {
        match &self.0 {
            Some(k) => call.header("X-Session-Key", k),
            None => call,
        }
    }
}

/// A reply's status and text, read up to [`MAX_REPLY`].
fn text(call: Result<ureq::http::Response<ureq::Body>, ureq::Error>, base: &str) -> Result<(u16, String), String> {
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
    Ok((status, String::from_utf8_lossy(&body).chars().filter(|c| !c.is_control() || *c == '\n').take(300).collect()))
}

/// Connects with `password`, or none when the board is not DSF (it answers with RepRapFirmware's `err`, a page, or 404).
pub fn connect(base: &str, password: Option<&str>, agent: &Agent) -> Result<Option<Session>, String> {
    let url = match password {
        Some(p) => format!("{base}/machine/connect?password={}", encoded(p)),
        None => format!("{base}/machine/connect"),
    };
    let (status, body) = text(agent.get(&url).call(), base)?;
    match status {
        403 => Err(format!("Duet at {base}: wrong password (the key field holds the board's password)")),
        500 | 502 | 503 => {
            Err(format!("Duet at {base}: DSF answered {status}: it is running but cannot reach the board, or is out of date"))
        }
        200..=299 => Ok(match serde_json::from_str::<Value>(&body) {
            _ if body.trim().is_empty() => Some(Session(None)),
            Ok(Value::Object(m)) if !m.contains_key("err") => {
                let key = m.get("sessionKey").and_then(|k| k.as_str().map(str::to_string).or_else(|| k.as_u64().map(|n| n.to_string())));
                Some(Session(key.filter(|k| key_ok(k))))
            }
            _ => None,
        }),
        _ => Ok(None),
    }
}

pub fn send(base: &str, s: &Session, agent: &Agent, name: &str, gcode: &[u8], print: bool) -> Result<Sent, String> {
    let path = format!("0:/gcodes/{name}");
    let up = s.on(agent.put(&format!("{base}/machine/file/{}", encoded(&path)))).header("Content-Type", "application/octet-stream");
    let (status, reply) = text(up.send(gcode), base)?;
    if !(200..300).contains(&status) {
        return Err(format!("Duet at {base} did not take the file ({status}): {reply}"));
    }
    if print {
        let start = s.on(agent.post(&format!("{base}/machine/code"))).header("Content-Type", "text/plain");
        let (status, reply) = text(start.send(format!("M32 \"{path}\"")), base)?;
        if !(200..300).contains(&status) || reply.trim_start().starts_with("Error") {
            return Err(format!("Duet at {base} took the file but did not start it ({status}): {reply}"));
        }
    }
    Ok(Sent { name: name.to_string(), printing: print })
}

pub fn status(base: &str, s: &Session, agent: &Agent) -> Result<Status, String> {
    let v = json(s.on(agent.get(&format!("{base}/machine/model"))).call(), base)?;
    let heater = |i: &Value| {
        let h = &v["heat"]["heaters"][usize::try_from(i.as_u64()?).ok()?];
        Some((finite(&h["current"])?, finite(&h["active"])?))
    };
    let size = finite(&v["job"]["file"]["size"]).filter(|s| *s > 0.0);
    let state = short(&v["state"]["status"]).unwrap_or_else(|| "unknown".into());
    Ok(Status {
        state: if state == "processing" { "printing".into() } else { state },
        file: short(&v["job"]["file"]["fileName"]),
        progress: size.zip(finite(&v["job"]["filePosition"])).map(|(size, at)| (at / size).clamp(0.0, 1.0)),
        nozzle: heater(&v["tools"][0]["heaters"][0]),
        bed: heater(&v["heat"]["bedHeaters"][0]),
    })
}

#[cfg(test)]
mod tests {
    use crate::testing::serve;
    use crate::{Kind, Printer, Sent, send, status};

    const MODEL: &str = r#"{"state": {"status": "processing"}, "job": {"file": {"fileName": "0:/gcodes/part.gcode", "size": 2000},
        "filePosition": 500}, "heat": {"bedHeaters": [0, -1], "heaters": [{"current": 59.6, "active": 60}, {"current": 214.8, "active": 215}]},
        "tools": [{"heaters": [1]}]}"#;

    /// A board run by DSF: a session key for the right password, the file, the code, and the object model.
    fn dsf() -> (String, std::sync::Arc<std::sync::Mutex<Vec<crate::testing::Got>>>) {
        serve(|g| match (g.method.as_str(), g.path.as_str()) {
            ("GET", "/machine/connect?password=reprap") => (200, Vec::new(), r#"{"sessionKey": "k-42"}"#.into()),
            ("GET", p) if p.starts_with("/machine/connect") => (403, Vec::new(), String::new()),
            _ if g.header("X-Session-Key") != Some("k-42") => (403, Vec::new(), String::new()),
            ("PUT", "/machine/file/0%3A%2Fgcodes%2Fpart.gcode") => (201, Vec::new(), String::new()),
            ("POST", "/machine/code") if g.body_text() == "M32 \"0:/gcodes/part.gcode\"" => (200, Vec::new(), String::new()),
            ("POST", "/machine/code") => (200, Vec::new(), "Error: M32: unknown file".into()),
            ("GET", "/machine/model") => (200, Vec::new(), MODEL.into()),
            _ => (404, Vec::new(), String::new()),
        })
    }

    #[test]
    fn a_duet_run_by_dsf_is_found_and_takes_the_file_and_starts_it() {
        let (address, got) = dsf();
        let mut p = Printer { kind: Kind::Duet, address, key: Some("reprap".into()), ..Default::default() };
        assert_eq!(send(&p, "part", b"G28\nM84\n", true).unwrap(), Sent { name: "part.gcode".into(), printing: true });
        assert_eq!(status(&p).unwrap().line(), "printing 0:/gcodes/part.gcode, 25 %; nozzle 215 / 215 C, bed 60 / 60 C");
        {
            let got = got.lock().unwrap();
            let asked: Vec<(&str, &str)> = got.iter().map(|g| (g.method.as_str(), g.path.as_str())).collect();
            assert_eq!(
                asked,
                [
                    ("GET", "/machine/connect?password=reprap"),
                    ("PUT", "/machine/file/0%3A%2Fgcodes%2Fpart.gcode"),
                    ("POST", "/machine/code"),
                    ("GET", "/machine/connect?password=reprap"),
                    ("GET", "/machine/model")
                ]
            );
            assert_eq!(got[1].body, b"G28\nM84\n");
        }
        p.key = Some("wrong".into());
        assert!(send(&p, "part", b"G28\n", true).unwrap_err().contains("wrong password"));
        p.key = Some("reprap".into());
        let e = send(&p, "other", b"G28\n", true).unwrap_err();
        assert!(e.contains("did not take the file"), "{e}");
    }

    #[test]
    fn a_board_on_its_own_is_reached_with_its_rr_requests() {
        let (address, got) = serve(|g| match g.path.split('?').next().unwrap_or("") {
            "/machine/connect" => (404, Vec::new(), "<html>Not found</html>".into()),
            "/rr_connect" => (200, Vec::new(), r#"{"err": 0}"#.into()),
            "/rr_upload" => (200, Vec::new(), r#"{"err": 0}"#.into()),
            _ => (404, Vec::new(), String::new()),
        });
        let p = Printer { kind: Kind::Duet, address, ..Default::default() };
        send(&p, "a", b"G28\n", false).unwrap();
        let paths: Vec<String> = got.lock().unwrap().iter().map(|g| g.path.clone()).collect();
        assert_eq!(paths, ["/machine/connect", "/rr_connect?password=", "/rr_upload?name=0%3A%2Fgcodes%2Fa.gcode"]);
    }
}
