//! The assistant's network: HTTPS POSTs to the three providers' API hosts, through ureq with rustls (Mozilla's root certificates
//! built in), or plain HTTP to a model's server on this computer at 127.0.0.1 or ::1 (by address: no name is looked up), and
//! nowhere else. A request goes only to an address [`allowed`] says yes to, a redirect is never followed (so no
//! reply can send the key anywhere else), and the reply's body is handed over to be read as it comes (`crate::complete` reads it, up
//! to [`crate::MAX_REPLY`]). The key travels in a header, never in an address, and no message made here has it in it.

use std::fmt;
use std::io::Read;
use std::time::Duration;

use ureq::Agent;
use ureq::http::Uri;

/// A request as the providers take it: where, with which headers (the key is one), and the JSON body.
#[derive(Clone, PartialEq)]
pub struct Request {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Not the headers: the key is in one.
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Request {{ url: {:?}, body: {} bytes }}", self.url, self.body.len())
    }
}

/// A reply's body, read as it arrives.
pub type Body = Box<dyn Read + Send>;

/// The network as the assistant needs it: one POST, answered with the status and the body whatever the status is, the body to be
/// read as it comes (a provider streams its answer). The desktop's is [`Ureq`]; the tests' is `testing::Fake`.
pub trait Net: Send + Sync {
    fn stream(&self, request: &Request) -> Result<(u16, Body), String>;
}

/// The hosts the assistant talks to, and no others.
pub const HOSTS: [&str; 3] = ["api.anthropic.com", "api.openai.com", "generativelanguage.googleapis.com"];

/// Whether a request to `url` is one the assistant makes: HTTPS, to one of [`HOSTS`] as named (no port, no credentials in front of
/// it), or [`loopback`].
pub fn allowed(url: &str) -> bool {
    let Ok(uri) = url.parse::<Uri>() else { return false };
    let Some(host) = uri.host() else { return false };
    let remote = uri.scheme_str() == Some("https")
        && uri.authority().is_some_and(|a| a.as_str().eq_ignore_ascii_case(host))
        && HOSTS.iter().any(|h| h.eq_ignore_ascii_case(host));
    remote || loopback(url)
}

/// Whether `url` is plain HTTP to this computer by address: 127.0.0.1 or [::1], any port, no credentials. Not `localhost`, a name
/// a hosts file could point anywhere (`local::endpoint` writes it as 127.0.0.1).
pub fn loopback(url: &str) -> bool {
    let Ok(uri) = url.parse::<Uri>() else { return false };
    let (Some(host), Some(authority)) = (uri.host(), uri.authority()) else { return false };
    let port = authority.port().map_or(String::new(), |p| format!(":{}", p.as_str()));
    uri.scheme_str() == Some("http") && (host == "127.0.0.1" || host == "[::1]") && authority.as_str() == format!("{host}{port}")
}

/// `url` for a message: its host, never the path or the query.
fn host_of(url: &str) -> String {
    url.parse::<Uri>().ok().and_then(|u| u.host().map(str::to_string)).unwrap_or_else(|| "the address".into())
}

/// A reply to a message of a few kilobytes comes in seconds; one that thinks first can take minutes, and a long answer streams for
/// minutes more. A stream that never ends is given up on here.
const WAIT: Duration = Duration::from_secs(10 * 60);
/// The longest wait for the next bytes of a reply: a model may think quietly before it answers, but a stream gone silent is dead.
const SILENCE: Duration = Duration::from_secs(2 * 60);

pub struct Ureq {
    agent: Agent,
    /// The same, for plain HTTP to a model's server on this computer: used only for an address [`loopback`] says yes to.
    local: Agent,
    /// In a test, the one address a request may go to (a server on this machine); none: what [`allowed`] allows.
    only: Option<String>,
}

impl Default for Ureq {
    fn default() -> Self {
        Ureq::new()
    }
}

/// An agent that follows no redirect, gives up on a stalled connection or stream and hands a 4xx or 5xx back as a reply to be read:
/// the provider's error says what is wrong with the key or the request.
fn agent(https_only: bool) -> Agent {
    Agent::config_builder()
        .https_only(https_only)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(WAIT))
        .timeout_per_read(Some(SILENCE))
        .user_agent(format!("Lamina/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

impl Ureq {
    pub fn new() -> Ureq {
        Ureq { agent: agent(true), local: agent(false), only: None }
    }

    /// For tests: talks plain HTTP to the server at `base` on this machine, and to nothing else.
    #[cfg(test)]
    pub(crate) fn loopback(base: &str) -> Ureq {
        Ureq { agent: agent(false), local: agent(false), only: Some(format!("{base}/")) }
    }

    fn allowed(&self, url: &str) -> bool {
        self.only.as_ref().map_or_else(|| allowed(url), |only| url.starts_with(only))
    }
}

impl Net for Ureq {
    fn stream(&self, request: &Request) -> Result<(u16, Body), String> {
        if !self.allowed(&request.url) {
            return Err(format!("won't send to {}", host_of(&request.url)));
        }
        let host = host_of(&request.url);
        let agent = if loopback(&request.url) { &self.local } else { &self.agent };
        let mut call = agent.post(&request.url).header("Content-Type", "application/json");
        for (name, value) in &request.headers {
            call = call.header(name, value);
        }
        let response = call.send(request.body.as_bytes()).map_err(|e| format!("{host}: {e}"))?;
        Ok((response.status().as_u16(), Box::new(response.into_body().into_reader())))
    }
}
