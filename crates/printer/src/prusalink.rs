//! PrusaLink (Prusa's printers: MK4, MK3.9, XL, MINI, and an MK3 with a Raspberry Pi), from its published OpenAPI description: the
//! file goes up in one `PUT /api/v1/files/usb/NAME` with `Print-After-Upload` when it should start, and `GET /api/v1/status` says what
//! the printer is doing. The printer checks either an API key, which goes in `X-Api-Key`, or a user name and password (HTTP digest,
//! `digest.rs`): then an unsigned `GET /api/v1/status` first fetches the printer's challenge, and the request carries the answer.

use std::io::Read;

use serde_json::Value;
use ureq::RequestBuilder;
use ureq::http::Uri;

use crate::{MAX_REPLY, Sent, Status, agent, agent_for, digest, finite, short};

fn reply(response: ureq::http::Response<ureq::Body>) -> Result<(u16, Vec<u8>), String> {
    let status = response.status().as_u16();
    let mut body = Vec::new();
    response.into_body().into_with_config().limit(MAX_REPLY + 1).reader().read_to_end(&mut body).map_err(|e| e.to_string())?;
    Ok((status, body))
}

fn refused(base: &str, status: u16, user: Option<&str>) -> String {
    let what = if user.is_some() { "the user name and password" } else { "the API key" };
    format!("PrusaLink at {base} refused ({status}): check {what} (the printer's Settings, Network, PrusaLink)")
}

/// `call` to `url` with its credentials: the API key in `X-Api-Key`; or, with a user name, the answer to the challenge an unsigned
/// status request gets. A printer that asks for nothing gets nothing.
fn signed<B>(
    call: RequestBuilder<B>,
    base: &str,
    key: Option<&str>,
    user: Option<&str>,
    method: &str,
    url: &str,
) -> Result<RequestBuilder<B>, String> {
    let Some(user) = user else {
        return Ok(match key {
            Some(k) => call.header("X-Api-Key", k),
            None => call,
        });
    };
    let asked = agent_for(std::time::Duration::from_secs(10))
        .get(&format!("{base}/api/v1/status"))
        .call()
        .map_err(|e| format!("PrusaLink at {base}: {e}"))?;
    if asked.status().as_u16() != 401 {
        return Ok(call);
    }
    let headers: Vec<String> =
        asked.headers().get_all("www-authenticate").iter().filter_map(|v| v.to_str().ok()).map(str::to_string).collect();
    let c = digest::challenge(headers.iter().map(String::as_str))
        .ok_or_else(|| format!("PrusaLink at {base} asks for a login Lamina does not know (not HTTP digest with MD5 or SHA-256)"))?;
    let uri = url.parse::<Uri>().ok().and_then(|u| u.path_and_query().map(|p| p.as_str().to_string())).unwrap_or_else(|| "/".into());
    Ok(call.header("Authorization", digest::authorization(&c, user, key.unwrap_or(""), method, &uri, &digest::cnonce(), 1)))
}

pub fn send(base: &str, key: Option<&str>, user: Option<&str>, name: &str, gcode: &[u8], print: bool) -> Result<Sent, String> {
    let url = format!("{base}/api/v1/files/usb/{name}");
    let call = agent()
        .put(&url)
        .header("Content-Type", "text/x.gcode")
        .header("Overwrite", "?1")
        .header("Print-After-Upload", if print { "?1" } else { "?0" });
    let call = signed(call, base, key, user, "PUT", &url)?;
    let (status, body) =
        reply(call.send(gcode).map_err(|e| format!("PrusaLink at {base}: {e}"))?).map_err(|e| format!("PrusaLink at {base}: {e}"))?;
    match status {
        200..=299 => Ok(Sent { name: name.to_string(), printing: print }),
        401 | 403 => Err(refused(base, status, user)),
        409 => Err(format!("PrusaLink at {base}: the printer is busy, or no USB stick is in it (409)")),
        _ => {
            Err(format!("PrusaLink at {base} answered {status}: {}", String::from_utf8_lossy(&body).chars().take(300).collect::<String>()))
        }
    }
}

pub fn status(base: &str, key: Option<&str>, user: Option<&str>) -> Result<Status, String> {
    let url = format!("{base}/api/v1/status");
    let call = signed(agent_for(std::time::Duration::from_secs(5)).get(&url), base, key, user, "GET", &url)?;
    let (status, body) =
        reply(call.call().map_err(|e| format!("PrusaLink at {base}: {e}"))?).map_err(|e| format!("PrusaLink at {base}: {e}"))?;
    match status {
        200..=299 => {}
        401 | 403 => return Err(refused(base, status, user)),
        _ => return Err(format!("PrusaLink at {base} answered {status}")),
    }
    let v: Value = serde_json::from_slice(&body).map_err(|_| format!("PrusaLink at {base} sent a status that is not JSON"))?;
    let p = &v["printer"];
    let pair = |now: &str, to: &str| Some((finite(&p[now])?, finite(&p[to])?));
    Ok(Status {
        state: short(&p["state"]).unwrap_or_else(|| "unknown".into()).to_lowercase(),
        file: None,
        progress: finite(&v["job"]["progress"]).map(|x| (x / 100.0).clamp(0.0, 1.0)),
        nozzle: pair("temp_nozzle", "target_nozzle"),
        bed: pair("temp_bed", "target_bed"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Got, serve};
    use crate::{Kind, Printer};

    /// A PrusaLink that asks for `maker` and `secret` by digest (SHA-256, or MD5 without `qop` as the printers' own firmware may),
    /// checking the answer against what it worked out itself.
    fn printer(sha256: bool) -> (String, std::sync::Arc<std::sync::Mutex<Vec<Got>>>) {
        serve(move |g| {
            let challenge = if sha256 {
                r#"Digest realm="Printer API", nonce="abc123", qop="auth", algorithm=SHA-256, opaque="op""#
            } else {
                r#"Digest realm="Printer API", nonce="abc123", stale=false"#
            };
            let c = digest::challenge([challenge]).unwrap();
            let Some(auth) = g.header("Authorization") else {
                return (401, vec![("WWW-Authenticate".into(), challenge.into())], String::new());
            };
            let cnonce = auth.split("cnonce=\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or("");
            let want = digest::authorization(&c, "maker", "secret pass", &g.method, &g.path, cnonce, 1);
            if auth == want {
                (200, Vec::new(), r#"{"printer": {"state": "IDLE"}}"#.into())
            } else {
                (401, vec![("WWW-Authenticate".into(), challenge.into())], String::new())
            }
        })
    }

    #[test]
    fn a_printer_that_asks_for_a_user_name_and_password_gets_a_digest_answer() {
        const GCODE: &[u8] = b"G28\nM84\n";
        for sha256 in [true, false] {
            let (address, got) = printer(sha256);
            let mut p = Printer { kind: Kind::PrusaLink, address, key: Some("secret pass".into()), user: Some("maker".into()) };
            assert_eq!(crate::send(&p, "part", GCODE, true).unwrap().name, "part.gcode");
            assert_eq!(crate::status(&p).unwrap().state, "idle");
            {
                let got = got.lock().unwrap();
                let asked: Vec<(&str, &str, bool)> =
                    got.iter().map(|g| (g.method.as_str(), g.path.as_str(), g.header("Authorization").is_some())).collect();
                assert_eq!(
                    asked,
                    [
                        ("GET", "/api/v1/status", false),
                        ("PUT", "/api/v1/files/usb/part.gcode", true),
                        ("GET", "/api/v1/status", false),
                        ("GET", "/api/v1/status", true)
                    ]
                );
                let put = &got[1];
                assert_eq!(put.body, GCODE);
                assert!(put.header("X-Api-Key").is_none(), "the password is never sent as a key");
                let auth = put.header("Authorization").unwrap();
                assert!(!auth.contains("secret") && auth.contains(r#"username="maker""#), "{auth}");
                assert_eq!(auth.contains("algorithm=SHA-256"), sha256);
            }
            p.key = Some("wrong".into());
            let e = crate::send(&p, "part", GCODE, false).unwrap_err();
            assert!(e.contains("check the user name and password"), "{e}");
            p.user = Some("ma\"ker".into());
            assert!(crate::send(&p, "part", GCODE, false).unwrap_err().contains("user name"));
        }
    }

    #[test]
    fn a_challenge_lamina_cannot_answer_is_said() {
        let (address, got) = serve(|_| (401, vec![("WWW-Authenticate".into(), r#"Basic realm="x""#.into())], String::new()));
        let p = Printer { kind: Kind::PrusaLink, address, key: Some("pw".into()), user: Some("maker".into()) };
        assert!(crate::send(&p, "a", b"G28\n", false).unwrap_err().contains("does not know"));
        assert_eq!(got.lock().unwrap().len(), 1, "the file was never sent");
    }
}
