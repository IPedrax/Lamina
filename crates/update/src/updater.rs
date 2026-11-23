//! The updater as the desktop drives it: a check at startup or by hand, an offer, a download, the install. The work runs on
//! threads of its own and comes back through a channel the window polls each frame, so nothing waits on the network or the
//! disk, and a panic in what it runs is an error, not a check that never finishes.

use std::ffi::OsString;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use semver::Version;

use crate::net::Ureq;
use crate::{Install, Net, Release, Target, check, download, install};

/// Set (to anything but nothing), this keeps the app from phoning home at all: no check, no menu items.
const NO_CHECK: &str = "LAMINA_NO_UPDATE_CHECK";

type Job = Box<dyn FnOnce() + Send>;

/// What an install is given: the downloaded update, which has passed its checks, and what it is for.
#[derive(Clone, Debug, PartialEq)]
pub struct Handover {
    pub version: Version,
    /// The file, in a folder an update made.
    pub file: PathBuf,
    pub sha256: [u8; 32],
    /// The project open now, which the new version opens again: an absolute path, since the new version may start somewhere else.
    pub project: Option<PathBuf>,
}

/// What puts an update in place: [`Install::hand_over`], or in a test a stand-in.
type HandOver = Arc<dyn Fn(&Handover) -> Result<(), String> + Send + Sync>;

/// Where the updater stands, which the window draws.
#[derive(Clone, Debug)]
pub enum State {
    /// Nothing to say.
    Idle,
    /// A newer release exists, and the user is asked.
    Available(Release),
    /// Its installer is coming down: how many bytes are in.
    Downloading { release: Release, done: Arc<AtomicU64> },
    /// Its installer is here and has passed its checks.
    Ready { release: Release, file: PathBuf },
    /// It is being put in place, which takes a moment, and nothing may change until it has gone through or failed.
    Installing { release: Release, file: PathBuf },
}

/// What the window says once, in its status box.
#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    /// A check by hand found nothing newer.
    UpToDate,
    /// A check by hand failed, and why (one at the start says nothing).
    CheckFailed(String),
    /// A download failed or didn't pass its checks, and why.
    DownloadFailed(String),
    /// The update is in, and the new version has been started (on Windows, will be once this one has exited): the window closes.
    Installed,
    /// The update could not be put in place, and why. It is ready to try again.
    InstallFailed(String),
}

enum Msg {
    Checked { manual: bool, result: Result<Option<Release>, String> },
    Downloaded(Result<PathBuf, String>),
    Installed(Result<(), String>),
}

pub struct Updater {
    net: Arc<dyn Net>,
    current: Version,
    state: State,
    checking: bool,
    target: Option<Target>,
    /// What an update replaces, when this copy can be updated in place.
    install: Option<Arc<Install>>,
    /// Where downloads get their folders.
    temp: PathBuf,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Asks the window for a frame, when work comes back.
    wake: Arc<dyn Fn() + Send + Sync>,
    spawn: fn(Job),
    hand_over: HandOver,
}

/// `work`'s result, or an error if it panicked.
fn guarded<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| Err("the updater failed unexpectedly".into()))
}

impl Updater {
    /// An updater of `current` on `net` that calls `wake` when work comes back for the window to take.
    pub fn new(net: Arc<dyn Net>, current: Version, wake: impl Fn() + Send + Sync + 'static) -> Updater {
        let (tx, rx) = channel();
        let mut updater = Updater {
            net,
            current,
            state: State::Idle,
            checking: false,
            target: None,
            install: None,
            temp: std::env::temp_dir(),
            tx,
            rx,
            wake: Arc::new(wake),
            spawn: |job| {
                std::thread::spawn(job);
            },
            hand_over: Arc::new(|_| Err("this copy can't be updated in place".into())),
        };
        if let Some(install) = Install::detect() {
            updater = updater.found(install);
        }
        updater
    }

    /// The desktop app's: this version, GitHub over HTTPS. None when the environment variable `LAMINA_NO_UPDATE_CHECK` is set, for a
    /// machine that must not phone home. Whether to look at the start is the window's to say (its settings file keeps the switch).
    pub fn desktop(wake: impl Fn() + Send + Sync + 'static) -> Option<Updater> {
        if !phones_home(std::env::var_os(NO_CHECK)) {
            return None;
        }
        let current = Version::parse(env!("CARGO_PKG_VERSION")).ok()?;
        Some(Updater::new(Arc::new(Ureq::new()), current, wake))
    }

    /// For tests: does its work on the thread that asks, so a result is there to poll at once.
    pub fn inline(mut self) -> Updater {
        self.spawn = |job| job();
        self
    }

    /// This copy is `install`: its target decides the file an update takes, and it is what an install replaces. `new` does this with
    /// what it detects; a test does it with any.
    pub fn found(mut self, install: Install) -> Updater {
        self.target = Some(install.target);
        let install = Arc::new(install);
        self.install = Some(Arc::clone(&install));
        self.hand_over = Arc::new(move |job| install.hand_over(job));
        self
    }

    /// For tests: the kind of install this copy is, which decides the file an update takes (none: it takes none).
    pub fn target(mut self, target: Option<Target>) -> Updater {
        self.target = target;
        self
    }

    /// For tests: `install` calls this, with the download, its digest and the project to reopen, instead of replacing this program.
    pub fn handing_over(mut self, f: impl Fn(&Handover) -> Result<(), String> + Send + Sync + 'static) -> Updater {
        self.hand_over = Arc::new(f);
        self
    }

    /// For tests: downloads go in folders under `temp`.
    pub fn in_folder(mut self, temp: PathBuf) -> Updater {
        self.temp = temp;
        self
    }

    /// The version running.
    pub fn current(&self) -> &Version {
        &self.current
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Whether a check is on its way.
    pub fn checking(&self) -> bool {
        self.checking
    }

    /// Looks for a newer version without asking the window to wait, and says nothing when there is none or the check fails.
    pub fn check_at_startup(&mut self) {
        self.start_check(false);
    }

    /// A check the user asked for: it says when there is nothing newer or it failed.
    pub fn check_now(&mut self) {
        self.start_check(true);
    }

    fn start_check(&mut self, manual: bool) {
        if self.checking {
            return;
        }
        self.checking = true;
        let (net, current, temp, tx, wake) =
            (self.net.clone(), self.current.clone(), self.temp.clone(), self.tx.clone(), self.wake.clone());
        let (target, install) = (self.target, self.install.clone());
        // An update ready to install keeps its folder, however long the banner has been up.
        let keep = match &self.state {
            State::Ready { file, .. } | State::Installing { file, .. } => file.parent().map(Path::to_path_buf),
            _ => None,
        };
        (self.spawn)(Box::new(move || {
            install::sweep(&temp, keep.as_deref());
            let result = guarded(|| Ok(check(&*net, &current, target.as_ref())?.map(|release| fit(release, install.as_deref()))));
            let _ = tx.send(Msg::Checked { manual, result });
            wake();
        }));
    }

    /// Takes what the threads have sent since the last call; a note for the status box when one is due.
    pub fn poll(&mut self) -> Option<Note> {
        let mut note = None;
        while let Ok(msg) = self.rx.try_recv() {
            note = self.apply(msg).or(note);
        }
        note
    }

    fn apply(&mut self, msg: Msg) -> Option<Note> {
        match msg {
            Msg::Checked { manual, result } => {
                self.checking = false;
                // A check that comes back after the download began has nothing to add.
                if matches!(self.state, State::Downloading { .. } | State::Ready { .. } | State::Installing { .. }) {
                    return None;
                }
                match result {
                    Err(e) => manual.then_some(Note::CheckFailed(e)),
                    Ok(None) => manual.then_some(Note::UpToDate),
                    Ok(Some(release)) => {
                        self.state = State::Available(release);
                        None
                    }
                }
            }
            Msg::Downloaded(result) => match std::mem::replace(&mut self.state, State::Idle) {
                State::Downloading { release, .. } => match result {
                    Ok(file) => {
                        self.state = State::Ready { release, file };
                        None
                    }
                    Err(e) => {
                        self.state = State::Available(release);
                        Some(Note::DownloadFailed(e))
                    }
                },
                other => {
                    self.state = other;
                    None
                }
            },
            Msg::Installed(result) => match std::mem::replace(&mut self.state, State::Idle) {
                State::Installing { release, file } => match result {
                    Ok(()) => Some(Note::Installed),
                    Err(e) => {
                        self.state = State::Ready { release, file };
                        Some(Note::InstallFailed(e))
                    }
                },
                other => {
                    self.state = other;
                    None
                }
            },
        }
    }

    /// Starts downloading the offered release's installer, which is checked as it comes. Nothing happens where the release has no
    /// file for this kind of install (or this copy can't be updated in place, which comes to the same).
    pub fn update_now(&mut self) {
        let State::Available(release) = &self.state else { return };
        let Some(installer) = release.installer.clone() else { return };
        let done = Arc::new(AtomicU64::new(0));
        self.state = State::Downloading { release: release.clone(), done: done.clone() };
        let (net, temp, tx, wake) = (self.net.clone(), self.temp.clone(), self.tx.clone(), self.wake.clone());
        (self.spawn)(Box::new(move || {
            let result = guarded(|| download(&*net, &installer, &temp, &mut |n| done.store(n, Ordering::Relaxed)));
            let _ = tx.send(Msg::Downloaded(result));
            wake();
        }));
    }

    /// Hides the offer for this run: Lamina asks again the next time it starts (or is asked to check).
    pub fn later(&mut self) {
        if matches!(self.state, State::Available(_) | State::Ready { .. }) {
            self.state = State::Idle;
        }
    }

    /// Puts the downloaded update in place, on a thread of its own: `poll` says how it went. On `Note::Installed` the caller closes its
    /// window, which an installer needs (it can't replace a program that's running). `project`, the file open now, is opened again
    /// once the new version starts. Nothing happens where no update is ready.
    pub fn install(&mut self, project: Option<&Path>) {
        let State::Ready { release, file } = &self.state else { return };
        let Some(installer) = &release.installer else { return };
        let job = Handover {
            version: release.version.clone(),
            file: file.clone(),
            sha256: installer.sha256,
            project: project.map(|p| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())),
        };
        self.state = State::Installing { release: release.clone(), file: file.clone() };
        let (hand_over, tx, wake) = (self.hand_over.clone(), self.tx.clone(), self.wake.clone());
        (self.spawn)(Box::new(move || {
            let _ = tx.send(Msg::Installed(guarded(|| hand_over(&job))));
            wake();
        }));
    }
}

/// `release` as this copy can use it: where this user can't write beside the program, the update has no file to install, and the release
/// is only told of. It is looked at here, once there is something to offer, because it writes a file to find out.
fn fit(release: Release, install: Option<&Install>) -> Release {
    if install.is_some_and(|install| !install.replaceable()) {
        return Release { installer: None, ..release };
    }
    release
}

/// Whether the desktop app may make requests: not with `no_check` (what `LAMINA_NO_UPDATE_CHECK` holds) set to anything but nothing.
pub(crate) fn phones_home(no_check: Option<OsString>) -> bool {
    !no_check.is_some_and(|v| !v.is_empty())
}
