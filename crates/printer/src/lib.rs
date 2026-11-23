//! Sending G-code to a printer on the user's own network: Moonraker (Klipper's API server), OctoPrint, PrusaLink (`prusalink.rs`) and
//! Duet boards (`duet.rs`), from their public API documentation. To the address the user typed and nowhere else: an upload of the
//! file, which may also start the print, and [`status`], what the printer is doing, which the window asks while the user watches.
//! With the assistant, this is the only crate that reaches the network (the `network_boundary` test in `lamina-core` lists both).
//!
//! What goes out is the user's file and, for OctoPrint (or a Moonraker or PrusaLink that asks for one), the user's API key in the
//! `X-Api-Key` header, never in an address or a message. A PrusaLink password is only hashed (HTTP digest, `digest.rs`). A redirect
//! is never followed, so no reply can send the file or the key elsewhere, and a reply is read only up to [`MAX_REPLY`].
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod commands;
pub mod digest;
mod duet;
mod prusalink;

use std::io::Read;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ureq::Agent;
use ureq::http::Uri;

pub use commands::COMMANDS;

/// The largest G-code file sent, bytes.
pub const MAX_GCODE: u64 = 1 << 30;
/// The most of a printer's reply that is read, bytes.
pub const MAX_REPLY: u64 = 1 << 20;

/// The kind of printer server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Klipper's Moonraker (Mainsail, Fluidd): `POST /server/files/upload`.
    #[default]
    Moonraker,
    /// OctoPrint: `POST /api/files/local`, with the key in `X-Api-Key`.
    Octoprint,
    /// PrusaLink (Prusa's printers): `PUT /api/v1/files/usb/NAME`, with the key in `X-Api-Key`.
    #[serde(rename = "prusalink")]
    PrusaLink,
    /// A Duet board running RepRapFirmware on its own: `rr_upload`, with the board's password in the key field.
    Duet,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Moonraker => "Moonraker",
            Kind::Octoprint => "OctoPrint",
            Kind::PrusaLink => "PrusaLink",
            Kind::Duet => "Duet",
        }
    }

    pub const ALL: [Kind; 4] = [Kind::Moonraker, Kind::Octoprint, Kind::PrusaLink, Kind::Duet];

    fn path(self) -> &'static str {
        match self {
            Kind::Moonraker => "/server/files/upload",
            _ => "/api/files/local",
        }
    }
}

/// The address a request goes to: `http://` or `https://`, a host and an optional port, and an optional path the server sits under
/// (OctoPrint behind a proxy at `/octoprint`), with no credentials, query or fragment. A bare host means plain HTTP, as printers on a
/// home network serve. Returns the address without a trailing `/`.
pub fn base(address: &str) -> Result<String, String> {
    let a = address.trim();
    if a.is_empty() {
        return Err("give the printer's address, such as 192.168.1.20 or http://printer.local:7125".into());
    }
    if a.len() > 500 || a.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the printer's address has spaces or odd characters in it".into());
    }
    let with_scheme = if a.contains("://") { a.to_string() } else { format!("http://{a}") };
    let uri: Uri = with_scheme.parse().map_err(|_| format!("`{a}` is not an address"))?;
    let scheme = uri.scheme_str().unwrap_or("");
    if scheme != "http" && scheme != "https" {
        return Err(format!("the printer's address must start with http:// or https://, not {scheme}://"));
    }
    let authority = uri.authority().ok_or_else(|| format!("`{a}` names no host"))?;
    if authority.as_str().contains('@') {
        return Err("put no user name or password in the printer's address; OctoPrint's API key has its own field".into());
    }
    if uri.query().is_some() || with_scheme.contains('#') {
        return Err("the printer's address has a query or a fragment; give only the host, port and path".into());
    }
    let path = uri.path().trim_end_matches('/');
    Ok(format!("{scheme}://{}{path}", authority.as_str()))
}

/// Whether `key` may go in a header: printable ASCII, no spaces, at most 200 characters.
pub fn key_ok(key: &str) -> bool {
    !key.is_empty() && key.len() <= 200 && key.bytes().all(|b| b.is_ascii_graphic())
}

/// A file name the printer will show and keep: letters, digits, `-`, `_` and `.`, ending in `.gcode`.
pub fn file_name(name: &str) -> String {
    let stem: String = name
        .trim_end_matches(".gcode")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .take(100)
        .collect();
    let stem = stem.trim_matches('.');
    format!("{}.gcode", if stem.is_empty() { "lamina" } else { stem })
}

/// `boundary`, made longer until `file` does not hold it.
fn free(file: &[u8], mut boundary: String) -> String {
    while file.windows(boundary.len()).any(|w| w == boundary.as_bytes()) {
        boundary.push('x');
    }
    boundary
}

/// A multipart form: the fields, then the file. The boundary is checked not to occur in what it separates.
fn multipart(fields: &[(&str, &str)], name: &str, file: &[u8]) -> (String, Vec<u8>) {
    let mut n: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in file.iter().take(1 << 20) {
        n = (n ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    let boundary = free(file, format!("----LaminaBoundary{n:016x}{:x}", file.len()));
    let mut body = Vec::with_capacity(file.len() + 512);
    for (k, v) in fields {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

/// A printer and how to reach it.
#[derive(Clone, Default, PartialEq)]
pub struct Printer {
    pub kind: Kind,
    pub address: String,
    pub key: Option<String>,
    /// PrusaLink's user name, when it asks for a user name and password (digest) rather than an API key: `key` is the password then.
    pub user: Option<String>,
}

/// Not the key.
impl std::fmt::Debug for Printer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Printer {{ kind: {:?}, address: {:?}, key: {}, user: {:?} }}",
            self.kind,
            self.address,
            if self.key.is_some() { "set" } else { "none" },
            self.user
        )
    }
}

/// What a printer said to an upload.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    /// The name the file has on the printer.
    pub name: String,
    pub printing: bool,
}

/// Sends `gcode` to `printer` as `name`, and starts printing it when `print` is set.
pub fn send(printer: &Printer, name: &str, gcode: &[u8], print: bool) -> Result<Sent, String> {
    if gcode.len() as u64 > MAX_GCODE {
        return Err(format!("the G-code is {} bytes, more than the {MAX_GCODE} sent", gcode.len()));
    }
    let base = base(&printer.address)?;
    let user = user(printer)?;
    let key = match printer.key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        // A digest password is hashed, never sent, so any text will do.
        Some(k) if user.is_some() && k.len() <= 200 && !k.chars().any(char::is_control) => Some(k),
        Some(k) if key_ok(k) => Some(k),
        Some(_) => return Err("the API key has spaces or odd characters in it".into()),
        None if printer.kind == Kind::Octoprint => return Err("OctoPrint needs an API key (OctoPrint's settings, Application Keys)".into()),
        None => None,
    };
    let name = file_name(name);
    match printer.kind {
        Kind::PrusaLink => return prusalink::send(&base, key, user, &name, gcode, print),
        Kind::Duet => return duet::send(&base, key, &name, gcode, print),
        Kind::Moonraker | Kind::Octoprint => {}
    }
    let flag = if print { "true" } else { "false" };
    let fields: Vec<(&str, &str)> = match printer.kind {
        Kind::Moonraker => vec![("root", "gcodes"), ("print", flag)],
        _ => vec![("select", flag), ("print", flag)],
    };
    let (boundary, body) = multipart(&fields, &name, gcode);
    let url = format!("{base}{}", printer.kind.path());
    let mut call = agent().post(&url).header("Content-Type", format!("multipart/form-data; boundary={boundary}"));
    if let Some(k) = key {
        call = call.header("X-Api-Key", k);
    }
    let what = printer.kind.name();
    let response = call.send(&body[..]).map_err(|e| format!("{what} at {base}: {e}"))?;
    let status = response.status().as_u16();
    let mut reply = Vec::new();
    response
        .into_body()
        .into_with_config()
        .limit(MAX_REPLY + 1)
        .reader()
        .read_to_end(&mut reply)
        .map_err(|e| format!("{what} at {base}: {e}"))?;
    let text = String::from_utf8_lossy(&reply[..reply.len().min(MAX_REPLY as usize)]).into_owned();
    match status {
        200..=299 => Ok(Sent { name, printing: print }),
        401 | 403 => Err(format!("{what} at {base} refused the upload ({status}): check the API key")),
        _ => Err(format!("{what} at {base} answered {status}: {}", text.chars().take(300).collect::<String>())),
    }
}

/// PrusaLink's user name for digest, when one is given; it goes in a header, so printable ASCII without quotes.
fn user(printer: &Printer) -> Result<Option<&str>, String> {
    match printer.user.as_deref().map(str::trim).filter(|u| !u.is_empty() && printer.kind == Kind::PrusaLink) {
        Some(u) if !key_ok(u) || u.contains(['"', '\\']) => Err("the user name has spaces or odd characters in it".into()),
        u => Ok(u),
    }
}

/// An agent that follows no redirect, gives up on a printer that does not answer, and reads an error status as a reply.
fn agent() -> Agent {
    agent_for(Duration::from_secs(15 * 60))
}

fn agent_for(time: Duration) -> Agent {
    Agent::config_builder()
        .https_only(false)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10).min(time)))
        .timeout_global(Some(time))
        .user_agent(format!("Lamina/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// What a printer is doing, as far as it said: its state in its own words, the file, how far along it is (0 to 1), and the nozzle's
/// and the bed's temperatures (now, and what they are heating to).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub state: String,
    pub file: Option<String>,
    pub progress: Option<f64>,
    pub nozzle: Option<(f64, f64)>,
    pub bed: Option<(f64, f64)>,
}

impl Status {
    /// One line for the window: "printing part.gcode, 42 %; nozzle 210 / 210 C, bed 60 / 60 C".
    pub fn line(&self) -> String {
        let mut out = self.state.clone();
        if let Some(f) = &self.file {
            out.push_str(&format!(" {f}"));
        }
        if let Some(p) = self.progress {
            out.push_str(&format!(", {:.0} %", p * 100.0));
        }
        let temp = |name: &str, t: Option<(f64, f64)>| t.map(|(now, to)| format!("{name} {now:.0} / {to:.0} C"));
        let temps: Vec<String> = [temp("nozzle", self.nozzle), temp("bed", self.bed)].into_iter().flatten().collect();
        if !temps.is_empty() {
            out.push_str(&format!("; {}", temps.join(", ")));
        }
        out
    }
}

/// A number a printer sent, when it is one and finite.
fn finite(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite())
}

/// Text a printer sent, cut short.
fn short(v: &serde_json::Value) -> Option<String> {
    v.as_str().map(|s| s.chars().filter(|c| !c.is_control()).take(120).collect())
}

/// One GET to the printer, its reply read up to [`MAX_REPLY`] as JSON.
fn get(printer: &Printer, path: &str) -> Result<serde_json::Value, String> {
    let base = base(&printer.address)?;
    let what = printer.kind.name();
    let mut call = agent_for(Duration::from_secs(5)).get(&format!("{base}{path}"));
    if let Some(k) = printer.key.as_deref().map(str::trim).filter(|k| !k.is_empty() && key_ok(k)) {
        call = call.header("X-Api-Key", k);
    }
    let response = call.call().map_err(|e| format!("{what} at {base}: {e}"))?;
    let status = response.status().as_u16();
    let mut reply = Vec::new();
    response
        .into_body()
        .into_with_config()
        .limit(MAX_REPLY + 1)
        .reader()
        .read_to_end(&mut reply)
        .map_err(|e| format!("{what} at {base}: {e}"))?;
    match status {
        200..=299 => serde_json::from_slice(&reply).map_err(|_| format!("{what} at {base} sent a status that is not JSON")),
        401 | 403 => Err(format!("{what} at {base} refused ({status}): check the API key")),
        _ => Err(format!("{what} at {base} answered {status}")),
    }
}

/// Asks `printer` what it is doing (Moonraker's printer objects; OctoPrint's job and printer), by the address the user typed only.
/// What it says is read leniently: a field it does not send, or sends oddly, is none.
pub fn status(printer: &Printer) -> Result<Status, String> {
    let key = printer.key.as_deref().map(str::trim).filter(|k| !k.is_empty() && key_ok(k));
    match printer.kind {
        Kind::PrusaLink => {
            let user = user(printer)?;
            let password = printer.key.as_deref().map(str::trim).filter(|_| user.is_some());
            prusalink::status(&base(&printer.address)?, password.or(key), user)
        }
        Kind::Duet => duet::status(&base(&printer.address)?, key),
        Kind::Moonraker => {
            let v = get(printer, "/printer/objects/query?print_stats&virtual_sdcard&extruder&heater_bed")?;
            let s = &v["result"]["status"];
            let temps = |o: &serde_json::Value| Some((finite(&o["temperature"])?, finite(&o["target"])?));
            Ok(Status {
                state: short(&s["print_stats"]["state"]).unwrap_or_else(|| "unknown".into()),
                file: short(&s["print_stats"]["filename"]).filter(|f| !f.is_empty()),
                progress: finite(&s["virtual_sdcard"]["progress"]).map(|p| p.clamp(0.0, 1.0)),
                nozzle: temps(&s["extruder"]),
                bed: temps(&s["heater_bed"]),
            })
        }
        Kind::Octoprint => {
            let job = get(printer, "/api/job")?;
            // The printer's temperatures: OctoPrint answers 409 while no printer is connected, which leaves them out.
            let state = get(printer, "/api/printer").unwrap_or_default();
            let temps = |o: &serde_json::Value| Some((finite(&o["actual"])?, finite(&o["target"])?));
            Ok(Status {
                state: short(&job["state"]).unwrap_or_else(|| "unknown".into()).to_lowercase(),
                file: short(&job["job"]["file"]["name"]),
                progress: finite(&job["progress"]["completion"]).map(|p| (p / 100.0).clamp(0.0, 1.0)),
                nozzle: temps(&state["temperature"]["tool0"]),
                bed: temps(&state["temperature"]["bed"]),
            })
        }
    }
}

/// A fake printer for tests: an HTTP server on this machine that answers each request with the status given and keeps what it got.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A request as the fake printer read it.
    #[derive(Clone, Debug, Default)]
    pub struct Got {
        pub method: String,
        pub path: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Got {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        }

        pub fn body_text(&self) -> String {
            String::from_utf8_lossy(&self.body).into_owned()
        }
    }

    /// What the fake printer answers a request with: a status, headers, and a body.
    pub type Answer = (u16, Vec<(String, String)>, String);

    /// Starts the server, answering every request with `status` and `reply`; returns its address (`127.0.0.1:port`) and what it gets.
    pub fn printer(status: u16, reply: &'static str) -> (String, Arc<Mutex<Vec<Got>>>) {
        serve(move |_| (status, Vec::new(), reply.to_string()))
    }

    /// Starts a server that answers each request with what `answer` makes of it.
    pub fn serve(answer: impl Fn(&Got) -> Answer + Send + 'static) -> (String, Arc<Mutex<Vec<Got>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let got = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&got);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut parts = line.split_whitespace();
                let mut g = Got { method: parts.next().unwrap_or("").into(), path: parts.next().unwrap_or("").into(), ..Got::default() };
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        g.headers.push((k.trim().into(), v.trim().into()));
                    }
                }
                let len: usize = g.header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                g.body = body;
                let (status, headers, reply) = answer(&g);
                kept.lock().unwrap().push(g);
                let headers: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
                let text = format!(
                    "HTTP/1.1 {status} Fake\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                stream.write_all(text.as_bytes()).unwrap();
            }
        });
        (address, got)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GCODE: &[u8] = b"; generated by Lamina\nG28\nG1 X10 Y10 E1\nM84\n";

    #[test]
    fn prusalink_takes_the_file_in_one_put_and_says_its_state() {
        let (address, got) = testing::printer(201, "");
        let p = Printer { kind: Kind::PrusaLink, address: address.clone(), key: Some("ABCDEF0123456789".into()), ..Default::default() };
        assert_eq!(send(&p, "my part", GCODE, true).unwrap(), Sent { name: "my_part.gcode".into(), printing: true });
        {
            let got = got.lock().unwrap();
            let g = &got[0];
            assert_eq!((g.method.as_str(), g.path.as_str()), ("PUT", "/api/v1/files/usb/my_part.gcode"));
            assert_eq!((g.header("Print-After-Upload"), g.header("X-Api-Key")), (Some("?1"), Some("ABCDEF0123456789")));
            assert_eq!(g.body, GCODE);
        }
        let (address, _) = testing::printer(
            200,
            r#"{"job": {"progress": 30.0}, "printer": {"state": "PRINTING", "temp_nozzle": 215.2, "target_nozzle": 215, "temp_bed": 60, "target_bed": 60}}"#,
        );
        let s = status(&Printer { kind: Kind::PrusaLink, address, key: None, ..Default::default() }).unwrap();
        assert_eq!(s.line(), "printing, 30 %; nozzle 215 / 215 C, bed 60 / 60 C");
        let (address, _) = testing::printer(401, "");
        assert!(
            send(&Printer { kind: Kind::PrusaLink, address, key: None, ..Default::default() }, "a", GCODE, false)
                .unwrap_err()
                .contains("API key")
        );
    }

    #[test]
    fn a_duet_connects_takes_the_file_and_starts_it() {
        let (address, got) = testing::printer(200, r#"{"err": 0, "sessionKey": 77}"#);
        let p = Printer { kind: Kind::Duet, address, key: Some("secret".into()), ..Default::default() };
        assert_eq!(send(&p, "part", GCODE, true).unwrap(), Sent { name: "part.gcode".into(), printing: true });
        let got = got.lock().unwrap();
        let paths: Vec<&str> = got.iter().map(|g| g.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/rr_connect?password=secret",
                "/rr_upload?name=0%3A%2Fgcodes%2Fpart.gcode",
                "/rr_gcode?gcode=M32%20%220%3A%2Fgcodes%2Fpart.gcode%22"
            ]
        );
        assert_eq!((got[1].method.as_str(), got[1].header("X-Session-Key")), ("POST", Some("77")));
        assert_eq!(got[1].body, GCODE);
        drop(got);
        let (address, _) = testing::printer(
            200,
            r#"{"err": 0, "status": "P", "fractionPrinted": 55.5, "temps": {"bed": {"current": 60, "active": 60}, "current": [60, 209], "tools": {"active": [[210]]}}}"#,
        );
        assert_eq!(
            status(&Printer { kind: Kind::Duet, address, key: None, ..Default::default() }).unwrap().line(),
            "printing, 56 %; nozzle 209 / 210 C, bed 60 / 60 C"
        );
        let (address, _) = testing::printer(200, r#"{"err": 1}"#);
        assert!(
            send(&Printer { kind: Kind::Duet, address, key: None, ..Default::default() }, "a", GCODE, false)
                .unwrap_err()
                .contains("wrong password")
        );
    }

    #[test]
    fn a_printer_says_what_it_is_doing() {
        let moon = r#"{"result": {"status": {"print_stats": {"state": "printing", "filename": "part.gcode"},
            "virtual_sdcard": {"progress": 0.4217}, "extruder": {"temperature": 209.6, "target": 210.0},
            "heater_bed": {"temperature": 60.1, "target": 60.0}}}}"#;
        let (address, got) = testing::printer(200, moon);
        let s = status(&Printer { kind: Kind::Moonraker, address, key: None, ..Default::default() }).unwrap();
        assert_eq!((s.state.as_str(), s.file.as_deref()), ("printing", Some("part.gcode")));
        assert_eq!(s.line(), "printing part.gcode, 42 %; nozzle 210 / 210 C, bed 60 / 60 C");
        assert!(got.lock().unwrap()[0].path.starts_with("/printer/objects/query?print_stats"));
        // OctoPrint: its job and its printer, one reply carrying both shapes here.
        let octo = r#"{"state": "Printing", "job": {"file": {"name": "a.gcode"}}, "progress": {"completion": 12.5},
            "temperature": {"tool0": {"actual": 200, "target": 205}, "bed": {"actual": 55, "target": 60}}}"#;
        let (address, got) = testing::printer(200, octo);
        let s = status(&Printer { kind: Kind::Octoprint, address, key: Some("ABCDEF0123456789".into()), ..Default::default() }).unwrap();
        assert_eq!(s.line(), "printing a.gcode, 12 %; nozzle 200 / 205 C, bed 55 / 60 C");
        let got = got.lock().unwrap();
        assert_eq!((got[0].path.as_str(), got[1].path.as_str()), ("/api/job", "/api/printer"));
        assert_eq!(got[0].header("X-Api-Key"), Some("ABCDEF0123456789"));
    }

    #[test]
    fn an_odd_status_reply_is_read_leniently_or_said() {
        let (address, _) = testing::printer(200, r#"{"result": {"status": {"virtual_sdcard": {"progress": "lots"}, "extruder": 5}}}"#);
        let s = status(&Printer { kind: Kind::Moonraker, address, key: None, ..Default::default() }).unwrap();
        assert_eq!(s, Status { state: "unknown".into(), ..Status::default() });
        let (address, _) = testing::printer(401, "{}");
        assert!(status(&Printer { kind: Kind::Moonraker, address, key: None, ..Default::default() }).unwrap_err().contains("API key"));
        let (address, _) = testing::printer(200, "<html>");
        assert!(status(&Printer { kind: Kind::Moonraker, address, key: None, ..Default::default() }).unwrap_err().contains("not JSON"));
    }

    #[test]
    fn moonraker_gets_a_multipart_upload_that_starts_the_print() {
        let (address, got) = testing::printer(201, r#"{"result": {"print_started": true}}"#);
        let p = Printer { kind: Kind::Moonraker, address: address.clone(), key: None, ..Default::default() };
        let sent = send(&p, "my part", GCODE, true).unwrap();
        assert_eq!(sent, Sent { name: "my_part.gcode".into(), printing: true });
        let got = got.lock().unwrap();
        let g = &got[0];
        assert_eq!((g.method.as_str(), g.path.as_str()), ("POST", "/server/files/upload"));
        assert_eq!(g.header("x-api-key"), None);
        let body = g.body_text();
        assert!(body.contains("name=\"print\"\r\n\r\ntrue\r\n") && body.contains("name=\"root\"\r\n\r\ngcodes\r\n"), "{body}");
        assert!(body.contains("filename=\"my_part.gcode\"") && body.contains("G1 X10 Y10 E1"));
        assert!(g.header("content-type").unwrap().starts_with("multipart/form-data; boundary=----LaminaBoundary"));
    }

    #[test]
    fn octoprint_gets_the_key_in_a_header_and_refuses_without_one() {
        let (address, got) = testing::printer(201, r#"{"done": true}"#);
        let mut p = Printer {
            kind: Kind::Octoprint,
            address: format!("http://{address}/"),
            key: Some("ABCDEF0123456789".into()),
            ..Default::default()
        };
        send(&p, "cube.gcode", GCODE, false).unwrap();
        {
            let got = got.lock().unwrap();
            assert_eq!(got[0].path, "/api/files/local");
            assert_eq!(got[0].header("X-Api-Key"), Some("ABCDEF0123456789"));
            assert!(got[0].body_text().contains("name=\"print\"\r\n\r\nfalse\r\n"));
        }
        p.key = None;
        assert!(send(&p, "cube", GCODE, false).unwrap_err().contains("API key"));
        p.key = Some("bad key\r\nX-Evil: 1".into());
        assert!(send(&p, "cube", GCODE, false).unwrap_err().contains("odd characters"));
        assert_eq!(got.lock().unwrap().len(), 1, "nothing more was sent");
    }

    #[test]
    fn a_refusal_and_an_unreachable_printer_are_errors_that_say_so() {
        let (address, _) = testing::printer(401, r#"{"error": "Unauthorized"}"#);
        let p = Printer { kind: Kind::Octoprint, address, key: Some("wrong".into()), ..Default::default() };
        assert!(send(&p, "x", GCODE, true).unwrap_err().contains("check the API key"));
        let (address, _) = testing::printer(500, r#"{"error": "disk full"}"#);
        let p = Printer { kind: Kind::Moonraker, address, key: None, ..Default::default() };
        assert!(send(&p, "x", GCODE, true).unwrap_err().contains("disk full"));
        // A port nothing listens on: bind one, learn it, and let it go.
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let p = Printer { kind: Kind::Moonraker, address: free.to_string(), key: None, ..Default::default() };
        assert!(send(&p, "x", GCODE, true).is_err());
    }

    #[test]
    fn addresses_are_checked_before_anything_is_sent() {
        assert_eq!(base("192.168.1.20").unwrap(), "http://192.168.1.20");
        assert_eq!(base(" printer.local:7125/ ").unwrap(), "http://printer.local:7125");
        assert_eq!(base("https://pi.example/octoprint/").unwrap(), "https://pi.example/octoprint");
        for bad in ["", "ftp://x", "http://user:pass@x", "http://x/?a=b", "http://x/#f", "two words", "file:///etc/passwd", "http://"] {
            assert!(base(bad).is_err(), "{bad}");
        }
        assert_eq!(file_name("../../etc/passwd"), "_.._etc_passwd.gcode");
        assert_eq!(file_name(""), "lamina.gcode");
        assert_eq!(file_name("Bracket v2.gcode"), "Bracket_v2.gcode");
        let p = Printer { kind: Kind::Octoprint, address: "x".into(), key: Some("secret-key".into()), ..Default::default() };
        assert!(!format!("{p:?}").contains("secret-key"));
    }

    #[test]
    fn the_guide_has_a_row_for_every_kind_of_printer() {
        let guide = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/USER-GUIDE.md")).unwrap();
        for k in Kind::ALL {
            assert!(guide.replace("\r\n", "\n").contains(&format!("\n| {} |", k.name())), "docs/USER-GUIDE.md has no row for {}", k.name());
        }
    }

    #[test]
    fn the_boundary_never_occurs_in_the_file() {
        assert_eq!(free(b"a --Lamina and --Laminax here", "--Lamina".into()), "--Laminaxx");
        let (boundary, body) = multipart(&[("print", "true")], "a.gcode", b"plain");
        assert!(body.ends_with(format!("--{boundary}--\r\n").as_bytes()));
    }
}
