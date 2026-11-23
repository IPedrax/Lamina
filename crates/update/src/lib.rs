//! The updater: asks GitHub for the latest release of Lamina, compares its tag with the running version, and downloads the
//! file a newer release carries for this kind of install (a [`Target`]), checking it against the SHA-256 digest GitHub gives for
//! it before anything runs it. Only the desktop app uses it: the core, the CLI and the MCP server never phone home.
//!
//! Lifted from BIMCraft's `crates/update` (github.com/IPedrax/BIMCraft, same author, MIT OR Apache-2.0), without its preferences
//! file (Lamina keeps the switch in its own settings file), Skip this version, the release's SHA256SUMS cross-check and the Flatpak.
//!
//! What comes from the network is hostile. A reply is read up to a cap and parsed leniently: a draft, a pre-release, a tag
//! that isn't `vMAJOR.MINOR.PATCH`, an asset that isn't exactly the file named for that version and kind of install, or one
//! with no digest or an absurd size is no installer. Where the file is downloaded from is built here from the tag and the
//! name, never read from the reply. The download is held to the size the release lists and hashed as it arrives, and a folder
//! of its own holds it only while every check passes. The network sits behind [`Net`], so the tests use a table of answers.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ring::digest::{Context, SHA256};
pub use semver::Version;
use serde_json::Value;

pub mod appimage;
pub mod detect;
mod exec;
pub mod install;
pub mod macos;
pub mod net;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
#[cfg(test)]
mod tests;
mod updater;

pub use detect::Install;
pub use updater::{Handover, Note, State, Updater};

/// The repository releases are read from and downloaded of.
const REPO: &str = "IPedrax/Lamina";

/// GitHub's latest published release: not a draft, not a pre-release.
pub const LATEST_URL: &str = "https://api.github.com/repos/IPedrax/Lamina/releases/latest";

/// The most of a reply to [`LATEST_URL`] that is read: GitHub's is about 7 KB, release notes and all.
pub const MAX_JSON: u64 = 1 << 20;

/// The largest installer taken. A release lists each file's size, and a download is held to that.
pub const MAX_DOWNLOAD: u64 = 128 << 20;

/// The network as the updater needs it: one GET. The desktop's is [`net::Ureq`]; the tests' is a table of answers.
pub trait Net: Send + Sync {
    /// Writes the body of `url` to `out`, and gives up when `out` does (it fails once more than the caller takes has come).
    /// `limit` is that cap, for a network that can refuse early, on a length it was told. `url` is HTTPS and GitHub's.
    fn get(&self, url: &str, limit: u64, out: &mut dyn Write) -> Result<(), String>;
}

/// A published release.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    /// Its page on GitHub, where Download and What's new go: built from the tag, not read from the reply.
    pub page: String,
    /// The file that updates this kind of install, when the release has a sound one: this version's, with a digest and a size
    /// that makes sense. None for a copy an update can't replace, and for a release that lacks it.
    pub installer: Option<Installer>,
}

/// The file of a release that updates an install, as far as it can be trusted before it's downloaded.
#[derive(Clone, Debug, PartialEq)]
pub struct Installer {
    /// `lamina-<version>-windows-x64.msi` for Windows: also the file's name once downloaded, so it holds no path.
    pub name: String,
    /// On github.com, in this repository's releases, built from the tag and the name.
    pub url: String,
    /// Its size in bytes, which the download must come to exactly.
    pub size: u64,
    pub sha256: [u8; 32],
}

/// The processors a Linux build is made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    /// From the name Rust gives the processor a program runs on (`std::env::consts::ARCH`).
    pub fn from_name(name: &str) -> Option<Arch> {
        match name {
            "x86_64" => Some(Arch::X86_64),
            "aarch64" => Some(Arch::Aarch64),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

/// A kind of install an update can replace, and so which file of a release is its update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The copy the Windows installer put in Program Files: the MSI.
    Windows,
    /// A Linux AppImage, which is one file.
    AppImage(Arch),
    /// Lamina.app from the macOS disk image: one build for Intel and Apple silicon alike.
    MacApp,
}

impl Target {
    /// The update's file in release `version`, named as the release workflow names it.
    pub fn asset_name(self, version: &str) -> String {
        match self {
            Target::Windows => format!("lamina-{version}-windows-x64.msi"),
            Target::AppImage(arch) => format!("lamina-{version}-linux-{}.AppImage", arch.name()),
            Target::MacApp => format!("lamina-{version}-macos-universal.dmg"),
        }
    }
}

/// What `lamina --version` prints for `version`. The updater runs the new build to see it, and the desktop prints it with this, so that
/// rewording it can't make every installed copy refuse the next update.
pub fn version_line(version: &str) -> String {
    format!("lamina {version}")
}

/// The version a release tag names: `v0.5.0` is 0.5.0. Anything else (no `v`, a pre-release, build metadata, leading zeros,
/// a number past 64 bits) is none, so a release tagged so is ignored.
pub fn tag_version(tag: &str) -> Option<Version> {
    let version = Version::parse(tag.strip_prefix('v')?).ok()?;
    (version.pre.is_empty() && version.build.is_empty()).then_some(version)
}

/// Whether `release` is a later version than `current`, as semver orders them (0.10.0 after 0.9.0; 0.5.0 after 0.5.0-rc.1).
pub fn is_newer(release: &Version, current: &Version) -> bool {
    release > current
}

/// Release `json` (a reply to [`LATEST_URL`]) as a release, with the file that updates `target` (none: with no file). Errors for
/// what isn't one: not JSON, a draft or a pre-release (or no word either way), or a tag that isn't a stable version.
pub fn parse_release(json: &[u8], target: Option<&Target>) -> Result<Release, String> {
    let j: Value = serde_json::from_slice(json).map_err(|e| format!("the reply isn't a release: {e}"))?;
    let flag = |name: &str| j.get(name).and_then(Value::as_bool);
    if flag("draft") != Some(false) || flag("prerelease") != Some(false) {
        return Err("the reply isn't a published release".into());
    }
    let tag = j.get("tag_name").and_then(Value::as_str).unwrap_or_default();
    let version = tag_version(tag).ok_or_else(|| format!("`{}` isn't the tag of a release", tag.chars().take(40).collect::<String>()))?;
    let assets = j.get("assets").and_then(Value::as_array);
    let installer = target.and_then(|t| {
        let name = t.asset_name(&version.to_string());
        assets.into_iter().flatten().find_map(|a| installer_of(a, &name, tag))
    });
    Ok(Release { page: format!("https://github.com/{REPO}/releases/tag/{tag}"), version, installer })
}

/// `asset` as the installer called `name` of release `tag`: whole, uploaded, of a size we take, and with a SHA-256 digest.
fn installer_of(asset: &Value, name: &str, tag: &str) -> Option<Installer> {
    if asset.get("name")?.as_str()? != name || asset.get("state")?.as_str()? != "uploaded" {
        return None;
    }
    let size = asset.get("size")?.as_u64().filter(|s| (1..=MAX_DOWNLOAD).contains(s))?;
    let sha256 = parse_digest(asset.get("digest")?.as_str()?)?;
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/{name}");
    Some(Installer { name: name.into(), url, size, sha256 })
}

/// The bytes of a digest as GitHub writes it, `sha256:` and 64 hex digits.
pub(crate) fn parse_digest(field: &str) -> Option<[u8; 32]> {
    let digits: Vec<u8> = field.strip_prefix("sha256:")?.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect::<Option<_>>()?;
    if digits.len() != 64 {
        return None;
    }
    let mut digest = [0; 32];
    for (byte, pair) in digest.iter_mut().zip(digits.chunks(2)) {
        if let [high, low] = pair {
            *byte = high << 4 | low;
        }
    }
    Some(digest)
}

/// Writes through to `inner` up to `left` bytes, then fails and says so: a cap on a reply.
struct Capped<W> {
    inner: W,
    left: u64,
    over: bool,
}

impl<W: Write> Write for Capped<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() as u64 > self.left {
            self.over = true;
            return Err(io::Error::other("more than the size limit"));
        }
        self.left -= buf.len() as u64;
        self.inner.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// The release the latest one is, when it's newer than `current`; none when it isn't, with the file that updates `target`. An
/// error for anything that goes wrong, for the caller to say or to keep quiet about.
pub fn check(net: &dyn Net, current: &Version, target: Option<&Target>) -> Result<Option<Release>, String> {
    let mut body = Capped { inner: Vec::new(), left: MAX_JSON, over: false };
    let got = net.get(LATEST_URL, MAX_JSON, &mut body);
    if body.over {
        return Err(format!("the reply is too large (over {MAX_JSON} bytes)"));
    }
    got?;
    let release = parse_release(&body.inner, target)?;
    Ok(is_newer(&release.version, current).then_some(release))
}

/// Takes in what is downloaded: to the file, through the hash, and held to the size, saying how much is in.
struct Sink<'a> {
    file: File,
    hash: Context,
    left: u64,
    done: u64,
    over: bool,
    progress: &'a mut dyn FnMut(u64),
}

impl Write for Sink<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() as u64 > self.left {
            self.over = true;
            return Err(io::Error::other("more than the size the release lists"));
        }
        self.file.write_all(buf)?;
        self.hash.update(buf);
        self.left -= buf.len() as u64;
        self.done += buf.len() as u64;
        (self.progress)(self.done);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// The digest `ctx` has come to, as bytes.
fn finish(ctx: Context) -> [u8; 32] {
    let mut digest = [0; 32];
    digest.iter_mut().zip(ctx.finish().as_ref()).for_each(|(d, s)| *d = *s);
    digest
}

/// Downloads `installer` into a folder of its own under `parent` and checks it: the size the release lists to the byte, and its
/// SHA-256. The file's path when every check passes; when one fails, an error and no folder, so nothing is left to run.
/// `progress` is told how many bytes are in after each piece.
pub fn download(net: &dyn Net, installer: &Installer, parent: &Path, progress: &mut dyn FnMut(u64)) -> Result<PathBuf, String> {
    let folder = fresh_folder(parent)?;
    let got = fetch(net, installer, &folder, progress);
    if got.is_err() {
        let _ = fs::remove_dir_all(&folder);
    }
    got
}

fn fetch(net: &dyn Net, installer: &Installer, folder: &Path, progress: &mut dyn FnMut(u64)) -> Result<PathBuf, String> {
    let path = folder.join(&installer.name);
    let file = OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut sink = Sink { file, hash: Context::new(&SHA256), left: installer.size, done: 0, over: false, progress };
    let got = net.get(&installer.url, installer.size, &mut sink);
    if sink.over {
        return Err(format!("the download is larger than the {} bytes GitHub lists", installer.size));
    }
    got?;
    sink.flush().map_err(|e| format!("{}: {e}", path.display()))?;
    if sink.done != installer.size {
        return Err(format!("the download is {} bytes, not the {} GitHub lists", sink.done, installer.size));
    }
    if finish(sink.hash) != installer.sha256 {
        return Err("the download's SHA-256 isn't the one GitHub lists for it".into());
    }
    Ok(path)
}

/// A folder made new under `parent`, never one that was there: its name can be guessed, and then it would be somebody else's.
fn fresh_folder(parent: &Path) -> Result<PathBuf, String> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    for attempt in 0..16 {
        let folder = parent.join(format!("{}{:x}-{nanos:x}-{attempt}", install::FOLDER_PREFIX, std::process::id()));
        match fs::create_dir(&folder) {
            Ok(()) => return Ok(folder),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {e}", folder.display())),
        }
    }
    Err("no fresh folder to download into".into())
}

/// The SHA-256 of the file at `path`.
pub fn sha256_file(path: &Path) -> io::Result<[u8; 32]> {
    sha256_of(&mut File::open(path)?)
}

/// The SHA-256 of what `reader` has left to give.
pub fn sha256_of(reader: &mut impl Read) -> io::Result<[u8; 32]> {
    let mut hash = Context::new(&SHA256);
    let mut buf = vec![0; 64 << 10];
    loop {
        match reader.read(&mut buf)? {
            0 => return Ok(finish(hash)),
            n => hash.update(buf.get(..n).unwrap_or_default()),
        }
    }
}
