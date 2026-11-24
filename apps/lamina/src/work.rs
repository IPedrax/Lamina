//! The window's long work on a thread of its own: slicing for the preview, the booleans and the cut, writing and sending G-code. They
//! can take seconds, and the window keeps drawing meanwhile, with a spinner and Cancel on the status bar.
//!
//! A job runs on a sibling of the window's session (the same commands and print settings, the same scene, shared not copied), so it
//! still goes through `Session::execute`. When it is done its scene, if it changed, is taken back with `Session::adopt` as one undo
//! step, unless the plate changed meanwhile. While a job that changes the plate runs, the window's own commands wait for it. Cancel
//! sets the job's flag (`lamina_core::cancel`), which stops a slice at its next layer and ends a boolean's child, and the window
//! forgets the job at once: what it would have done is never applied.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{RichText, Ui};
use lamina_core::{Scene, Session};
use serde_json::Value;

use crate::lang::{self, t, tf};
use crate::{App, Sliced};

/// What a job hands back: a command's answer, or a slice read back for the preview.
pub enum Done {
    Value(Value),
    Sliced(Sliced),
}

/// What the window does with a job's result, on its own thread.
type Then = Box<dyn FnOnce(&mut App, Result<Done, String>)>;

pub struct Job {
    /// What the status bar says it is doing ("Slicing", "union").
    pub label: String,
    /// Whether it changes the plate, so the window's own commands wait for it.
    pub changes_plate: bool,
    started: Instant,
    flag: Arc<AtomicBool>,
    from: Arc<Scene>,
    rx: Receiver<Result<(Done, Arc<Scene>), String>>,
    then: Then,
}

impl App {
    /// Whether a job is running.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Runs `work` on a thread, on a sibling of the session, and `then` here with what it gave. One job at a time: while one runs,
    /// another is refused and said on the status bar.
    pub(crate) fn start(
        &mut self,
        label: &str,
        changes_plate: bool,
        work: impl FnOnce(&mut Session) -> Result<Done, String> + Send + 'static,
        then: impl FnOnce(&mut App, Result<Done, String>) + 'static,
    ) {
        if let Some(job) = &self.job {
            (self.failed, self.status) = (true, tf("Wait for {} to finish, or cancel it.", &[t(&job.label)]));
            return;
        }
        let mut s = self.session.sibling();
        s.scene = Arc::clone(&self.session.scene);
        s.print = self.session.print.clone();
        let flag = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let watched = Arc::clone(&flag);
        // A panic on the thread drops `tx`, which the window reads as an error; a job the window has forgotten sends to no one.
        // What the job says (a slice's summary) is in the window's language.
        let language = lang::get();
        std::thread::spawn(move || {
            lang::set(language);
            let done = lamina_core::cancel::watch(watched, || work(&mut s));
            let _ = tx.send(done.map(|d| (d, s.scene)));
        });
        let from = Arc::clone(&self.session.scene);
        self.job = Some(Job { label: label.to_string(), changes_plate, started: Instant::now(), flag, from, rx, then: Box::new(then) });
        (self.failed, self.status) = (false, String::new());
    }

    /// Takes a finished job's result, if there is one; else asks to be drawn again soon, so the spinner turns and the result is seen.
    pub(crate) fn poll_work(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        match job.rx.try_recv() {
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Ok(result) => self.finish(Ok(result)),
            Err(TryRecvError::Disconnected) => self.finish(Err(())),
        }
    }

    /// Waits for the running job and takes its result (tests, and anything that cannot go on without it).
    pub fn wait(&mut self) {
        let Some(job) = &self.job else { return };
        let result = job.rx.recv().map_err(|_| ());
        self.finish(result);
    }

    fn finish(&mut self, result: Result<Result<(Done, Arc<Scene>), String>, ()>) {
        let Some(job) = self.job.take() else { return };
        let result = match result {
            Err(()) => Err(tf("internal error: {} stopped part way", &[t(&job.label)])),
            Ok(Err(e)) => Err(e),
            Ok(Ok((done, scene))) if Arc::ptr_eq(&scene, &job.from) => Ok(done),
            Ok(Ok((done, scene))) => self.session.adopt(&job.label, &job.from, scene).map(|()| done),
        };
        self.prune();
        (self.failed, self.status) = (result.is_err(), result.as_ref().err().cloned().unwrap_or_default());
        (job.then)(self, result);
    }

    /// Stops the running job: its flag is set and the window forgets it.
    pub fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.flag.store(true, Ordering::Relaxed);
            (self.failed, self.status) = (false, tf("Cancelled {}.", &[t(&job.label)]));
        }
    }

    /// The running job on the status bar: a spinner, what it is, how long it has run, and Cancel.
    pub(crate) fn work_ui(&mut self, ui: &mut Ui) {
        let Some(job) = &self.job else { return };
        let text = format!("{}... {:.0} s", t(&job.label), job.started.elapsed().as_secs_f64());
        ui.spinner();
        ui.label(RichText::new(text).strong());
        if ui.button(t("Cancel")).on_hover_text(t("Stop it; nothing it would have done is applied")).clicked() {
            self.cancel();
        }
        ui.separator();
    }
}
