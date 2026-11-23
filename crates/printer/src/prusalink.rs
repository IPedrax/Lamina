//! PrusaLink (Prusa's printers: MK4, MK3.9, XL, MINI, and an MK3 with a Raspberry Pi), from its published OpenAPI description: the
//! file goes up in one `PUT /api/v1/files/usb/NAME` with `Print-After-Upload` when it should start, and `GET /api/v1/status` says what
//! the printer is doing. The key goes in `X-Api-Key`; a printer set to ask for its user name and password (digest) instead is not
//! reached yet, and the error says so.

use std::io::Read;

use serde_json::Value;

use crate::{MAX_REPLY, Sent, Status, agent, agent_for, finite, short};

fn reply(response: ureq::http::Response<ureq::Body>) -> Result<(u16, Vec<u8>), String> {
    let status = response.status().as_u16();
    let mut body = Vec::new();
    response.into_body().into_with_config().limit(MAX_REPLY + 1).reader().read_to_end(&mut body).map_err(|e| e.to_string())?;
    Ok((status, body))
}

fn refused(base: &str, status: u16) -> String {
    format!(
        "PrusaLink at {base} refused ({status}): check the API key (Settings, Network, PrusaLink on the printer); a printer that asks for a user name and password instead is not supported yet"
    )
}

pub fn send(base: &str, key: Option<&str>, name: &str, gcode: &[u8], print: bool) -> Result<Sent, String> {
    let mut call = agent()
        .put(&format!("{base}/api/v1/files/usb/{name}"))
        .header("Content-Type", "text/x.gcode")
        .header("Overwrite", "?1")
        .header("Print-After-Upload", if print { "?1" } else { "?0" });
    if let Some(k) = key {
        call = call.header("X-Api-Key", k);
    }
    let (status, body) =
        reply(call.send(gcode).map_err(|e| format!("PrusaLink at {base}: {e}"))?).map_err(|e| format!("PrusaLink at {base}: {e}"))?;
    match status {
        200..=299 => Ok(Sent { name: name.to_string(), printing: print }),
        401 | 403 => Err(refused(base, status)),
        409 => Err(format!("PrusaLink at {base}: the printer is busy, or no USB stick is in it (409)")),
        _ => {
            Err(format!("PrusaLink at {base} answered {status}: {}", String::from_utf8_lossy(&body).chars().take(300).collect::<String>()))
        }
    }
}

pub fn status(base: &str, key: Option<&str>) -> Result<Status, String> {
    let mut call = agent_for(std::time::Duration::from_secs(5)).get(&format!("{base}/api/v1/status"));
    if let Some(k) = key {
        call = call.header("X-Api-Key", k);
    }
    let (status, body) =
        reply(call.call().map_err(|e| format!("PrusaLink at {base}: {e}"))?).map_err(|e| format!("PrusaLink at {base}: {e}"))?;
    match status {
        200..=299 => {}
        401 | 403 => return Err(refused(base, status)),
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
