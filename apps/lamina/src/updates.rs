//! Updates: the banner under the toolbar that says a newer Lamina is out (or coming down, or ready), the box that holds the window
//! while one is put in place, and the File menu's two items. Checking, downloading and installing are `lamina-update`'s; this draws
//! what it says and passes the user's clicks on. A window starts with no updater, so a test never phones home: the desktop's `main`
//! attaches one. Lifted from BIMCraft's `crates/ui-egui/src/updates.rs` (same author, MIT OR Apache-2.0), without Skip this version.

use std::sync::atomic::Ordering;
use std::time::Duration;

use egui::{Context, OpenUrl, ProgressBar, RichText, Ui, ViewportCommand};
use lamina_update::{Note, State};

use crate::App;
use crate::files::Pending;
use crate::lang::{t, tf};

/// What a click on the banner asks for, done once it is drawn.
enum Click {
    Update,
    Later,
    Restart,
    Page(String),
}

impl App {
    /// Takes what the updater's threads have finished and says on the status bar what a check, a download or an install came to;
    /// an install that went through closes the window. While one is being put in place it draws the box that holds the window,
    /// keeps the window open, and says so: the caller draws nothing else, so nothing can change under it.
    pub(crate) fn poll_updates(&mut self, ctx: &Context) -> bool {
        let Some(updates) = self.updates.as_mut() else { return false };
        let note = updates.poll();
        if matches!(updates.state(), State::Downloading { .. }) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let current = updates.current().to_string();
        match note {
            None => {}
            Some(Note::UpToDate) => (self.failed, self.status) = (false, tf("Lamina {} is up to date.", &[&current])),
            Some(Note::CheckFailed(e)) => (self.failed, self.status) = (true, tf("Could not check for updates: {}", &[&e])),
            Some(Note::DownloadFailed(e)) => (self.failed, self.status) = (true, tf("Could not download the update: {}", &[&e])),
            Some(Note::InstallFailed(e)) => (self.failed, self.status) = (true, tf("Could not install the update: {}", &[&e])),
            // It is in, and this window has to go for the new version to take its place: the plate was saved or given up before.
            Some(Note::Installed) => {
                self.may_close = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
        let Some(State::Installing { release, .. }) = self.updates.as_ref().map(|u| u.state()) else { return false };
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }
        let text = tf("Installing Lamina {}...", &[&release.version.to_string()]);
        egui::Modal::new(egui::Id::new("installing")).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(text).strong());
            });
        });
        true
    }

    /// Whether the banner has something to say.
    pub(crate) fn has_update_banner(&self) -> bool {
        self.updates.as_ref().is_some_and(|u| !matches!(u.state(), State::Idle | State::Installing { .. }))
    }

    /// The banner: the offer with its answers, the download's progress, or the update ready to install.
    pub(crate) fn update_banner(&mut self, ui: &mut Ui) {
        let Some(updates) = self.updates.as_ref() else { return };
        let current = updates.current().to_string();
        let mut click = None;
        ui.horizontal(|ui| match updates.state() {
            State::Idle | State::Installing { .. } => {}
            State::Available(release) => {
                ui.label(RichText::new(tf("Lamina {} is available. You have {}.", &[&release.version.to_string(), &current])).strong());
                if release.installer.is_some() {
                    let tip = t("Downloads the update and checks it; Lamina then closes, updates and starts again.");
                    if ui.button(t("Update now")).on_hover_text(tip).clicked() {
                        click = Some(Click::Update);
                    }
                } else if ui
                    .button(t("Download"))
                    .on_hover_text(t("Opens the release page on GitHub. This copy cannot update itself."))
                    .clicked()
                {
                    click = Some(Click::Page(release.page.clone()));
                }
                if ui.button(t("Later")).clicked() {
                    click = Some(Click::Later);
                }
                if ui.link(t("What's new")).clicked() {
                    click = Some(Click::Page(release.page.clone()));
                }
            }
            State::Downloading { release, done } => {
                ui.label(tf("Downloading Lamina {}...", &[&release.version.to_string()]));
                let size = release.installer.as_ref().map_or(1, |i| i.size).max(1);
                let fraction = (done.load(Ordering::Relaxed) as f32 / size as f32).clamp(0.0, 1.0);
                ui.add(ProgressBar::new(fraction).desired_width(220.0).show_percentage());
            }
            State::Ready { release, .. } => {
                ui.label(
                    RichText::new(tf("Lamina {} is ready. It will close, update and start again.", &[&release.version.to_string()]))
                        .strong(),
                );
                let tip =
                    t("Asks to save first if there are unsaved changes, then closes Lamina, installs the update and starts it again.");
                if ui.button(t("Restart and install")).on_hover_text(tip).clicked() {
                    click = Some(Click::Restart);
                }
                if ui.button(t("Later")).clicked() {
                    click = Some(Click::Later);
                }
            }
        });
        let Some(click) = click else { return };
        let Some(updates) = self.updates.as_mut() else { return };
        match click {
            Click::Update => updates.update_now(),
            Click::Later => updates.later(),
            Click::Restart => self.request(Pending::Update),
            Click::Page(url) => ui.ctx().open_url(OpenUrl::new_tab(url)),
        }
    }

    /// The File menu's updates items: the switch for the check at start (kept in the settings file), and a check now.
    pub(crate) fn update_menu(&mut self, ui: &mut Ui) {
        let Some(updates) = self.updates.as_mut() else { return };
        ui.separator();
        ui.checkbox(&mut self.update_check, t("Check for updates at start")).on_hover_text(t(
            "Asks GitHub for the latest release when Lamina starts. LAMINA_NO_UPDATE_CHECK=1 in the environment turns updates off altogether.",
        ));
        if ui.button(t("Check for updates now")).clicked() {
            updates.check_now();
        }
    }

    /// Starts putting the update in place, with this project to open again. The window closes when that has gone through
    /// ([`poll_updates`](Self::poll_updates)): it has to, for the new version to take its place.
    pub(crate) fn install_update(&mut self) {
        // The changes were saved or given up a moment ago, as a clean exit has them: no autosave for the new version to offer back.
        if self.recovery.is_none()
            && let Some(a) = self.autosave.as_mut()
        {
            a.remove();
        }
        let project = self.project.clone();
        if let Some(updates) = self.updates.as_mut() {
            updates.install(project.as_deref());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use egui::{Key, Modifiers};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use lamina_update::testing::{Fake, release_json};
    use lamina_update::{Handover, LATEST_URL, State, Target, Updater, Version};

    use crate::settings::Settings;
    use crate::{App, session};

    const INSTALLER: &[u8] = b"pretend this is an installer";
    const OFFER: &str = "Lamina 0.5.0 is available. You have 0.4.0.";
    const READY: &str = "Lamina 0.5.0 is ready. It will close, update and start again.";

    /// What the stand-in for the installer's hand-over was given: the file and the project to open again.
    type Handed = Arc<Mutex<Vec<(PathBuf, Option<PathBuf>)>>>;

    struct Rig {
        h: Harness<'static, App>,
        net: Arc<Fake>,
        dir: tempfile::TempDir,
        handed: Handed,
        /// While set, the stand-in for the install fails.
        failing: Arc<AtomicBool>,
    }

    /// A network where `tag` is the latest release, with its Windows installer.
    fn latest(tag: &str) -> Arc<Fake> {
        let url = format!("https://github.com/IPedrax/Lamina/releases/download/{tag}/{}", Target::Windows.asset_name(&tag[1..]));
        Arc::new(Fake::default().answer(LATEST_URL, release_json(tag, Some(INSTALLER))).answer(&url, INSTALLER))
    }

    /// The window, running 0.4.0 with its settings in a folder of its own, and an updater on `net` that works as it is asked, inline;
    /// `installing` is whether this copy can be updated in place.
    fn rig_on(net: Arc<Fake>, installing: bool) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_settings(session(), Arc::new(lamina_assistant::testing::Fake::default()), dir.path().join("settings.json"));
        let handed: Handed = Arc::default();
        let (log, failing) = (Arc::clone(&handed), Arc::new(AtomicBool::new(false)));
        let fails = Arc::clone(&failing);
        let updater = Updater::new(net.clone(), Version::new(0, 4, 0), || {})
            .inline()
            .target(installing.then_some(Target::Windows))
            .in_folder(dir.path().to_path_buf())
            .handing_over(move |job: &Handover| {
                log.lock().unwrap().push((job.file.clone(), job.project.clone()));
                if fails.load(Ordering::SeqCst) { Err("no room".to_string()) } else { Ok(()) }
            });
        app.updates = Some(updater);
        let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        h.run();
        Rig { h, net, dir, handed, failing }
    }

    fn rig(tag: &str, installing: bool) -> Rig {
        rig_on(latest(tag), installing)
    }

    impl Rig {
        fn updater(&mut self) -> &mut Updater {
            self.h.state_mut().updates.as_mut().unwrap()
        }

        /// The check at the start, and the frames that take its answer.
        fn start(&mut self) {
            self.updater().check_at_startup();
            self.h.run();
        }

        fn press(&mut self, label: &str) {
            self.h.get_by_label(label).click();
            self.h.run();
        }

        fn menu(&mut self, item: &str) {
            self.press("File");
            self.press(item);
        }

        fn status(&self) -> (bool, String) {
            (self.h.state().failed, self.h.state().status.clone())
        }
    }

    #[test]
    fn a_window_without_an_updater_shows_none_of_it() {
        let app = App::new(session(), Arc::new(lamina_assistant::testing::Fake::default()));
        let mut h = Harness::builder().with_size([1400.0, 860.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
        h.run();
        assert!(h.state().updates.is_none(), "a test never phones home");
        h.get_by_label("File").click();
        h.run();
        assert!(h.query_by_label("Check for updates now").is_none() && h.query_by_label("Check for updates at start").is_none());
    }

    #[test]
    fn a_newer_release_is_offered_in_a_banner_and_nothing_else_changes() {
        let mut r = rig("v0.5.0", true);
        assert!(r.h.query_by_label(OFFER).is_none(), "nothing before the check comes back");
        let scene = Arc::clone(&r.h.state().session.scene);
        r.start();
        assert!(r.h.query_by_label(OFFER).is_some());
        for label in ["Update now", "Later", "What's new"] {
            assert!(r.h.query_by_label(label).is_some(), "{label}");
        }
        assert!(r.h.query_by_label("Download").is_none(), "this copy can install it");
        assert!(Arc::ptr_eq(&scene, &r.h.state().session.scene) && !r.h.state().dirty(), "the plate is as it was");
        assert_eq!(r.status(), (false, String::new()), "and the status bar has nothing to say");
        assert_eq!(r.net.asked(), [LATEST_URL], "only the release was asked for");
        assert!(r.handed.lock().unwrap().is_empty(), "nothing is installed without a click");
    }

    #[test]
    fn later_hides_the_banner_until_the_next_check() {
        let mut r = rig("v0.5.0", true);
        r.start();
        r.press("Later");
        assert!(r.h.query_by_label(OFFER).is_none());
        r.start();
        assert!(r.h.query_by_label(OFFER).is_some(), "asked again at the next start");
    }

    #[test]
    fn update_now_downloads_then_restart_hands_the_installer_over_and_closes() {
        let mut r = rig("v0.5.0", true);
        r.start();
        r.press("Update now");
        assert!(r.h.query_by_label(READY).is_some());
        assert!(r.h.query_by_label("Update now").is_none() && r.handed.lock().unwrap().is_empty(), "nothing is installed yet");
        assert!(!r.h.state().may_close);
        r.press("Restart and install");
        let handed = r.handed.lock().unwrap().clone();
        assert_eq!(handed.len(), 1);
        assert_eq!(std::fs::read(&handed[0].0).unwrap(), INSTALLER, "what was downloaded and checked");
        assert_eq!(handed[0].1, None, "a plate never saved has no file to open again");
        assert!(r.h.state().may_close, "the window closes so the installer can replace the program");
        assert!(r.h.query_by_label("Restart and install").is_none(), "the banner goes once the installer is handed over");
    }

    #[test]
    fn restart_asks_about_unsaved_changes_first_and_opens_the_project_again() {
        let mut r = rig("v0.5.0", true);
        let project = r.dir.path().join("plate.lamina");
        assert!(r.h.state_mut().save_project(&project));
        r.press("Add box");
        assert!(r.h.state().dirty());
        r.start();
        r.press("Update now");
        r.press("Restart and install");
        assert!(r.h.query_by_label("Save the changes?").is_some(), "unsaved work is asked about, not dropped");
        assert!(r.handed.lock().unwrap().is_empty() && !r.h.state().may_close);
        r.press("Cancel");
        assert!(r.handed.lock().unwrap().is_empty() && !r.h.state().may_close, "Cancel leaves everything as it was");
        assert!(r.h.query_by_label("Restart and install").is_some(), "and the update still ready");
        r.press("Restart and install");
        r.press("Save");
        assert!(!r.h.state().dirty(), "saved");
        let handed = r.handed.lock().unwrap().clone();
        assert_eq!(handed[0].1.as_deref(), Some(project.as_path()), "the project opens again when Lamina starts");
        assert!(r.h.state().may_close);
    }

    #[test]
    fn an_install_that_fails_says_why_and_the_update_is_ready_to_try_again() {
        let mut r = rig("v0.5.0", true);
        r.failing.store(true, Ordering::SeqCst);
        r.start();
        r.press("Update now");
        r.press("Restart and install");
        assert_eq!(r.status(), (true, "Could not install the update: no room".to_string()));
        assert!(!r.h.state().may_close, "the window stays, with the plate in it");
        assert!(r.h.query_by_label("Restart and install").is_some());
        r.failing.store(false, Ordering::SeqCst);
        r.press("Restart and install");
        assert!(r.h.state().may_close && r.handed.lock().unwrap().len() == 2, "the second try went through");
    }

    #[test]
    fn a_copy_that_cannot_update_itself_is_given_the_release_page() {
        let mut r = rig("v0.5.0", false);
        r.start();
        assert!(r.h.query_by_label(OFFER).is_some());
        assert!(r.h.query_by_label("Update now").is_none(), "no promise it cannot keep");
        let page = "https://github.com/IPedrax/Lamina/releases/tag/v0.5.0";
        for button in ["Download", "What's new"] {
            r.h.get_by_label(button).click();
            r.h.step();
            let opened: Vec<String> =
                r.h.output()
                    .platform_output
                    .commands
                    .iter()
                    .filter_map(|c| if let egui::OutputCommand::OpenUrl(u) = c { Some(u.url.clone()) } else { None })
                    .collect();
            assert_eq!(opened, [page], "{button} opens the release's page, built from its tag");
            r.h.run();
        }
        assert_eq!(r.net.asked(), [LATEST_URL], "and downloads nothing");
    }

    #[test]
    fn the_file_menu_checks_by_hand_and_the_switch_is_kept_in_the_settings_file() {
        let mut r = rig("v0.4.0", true);
        r.menu("Check for updates now");
        assert_eq!(r.status(), (false, "Lamina 0.4.0 is up to date.".to_string()));
        assert!(r.h.query_by_label(OFFER).is_none());
        let path = r.dir.path().join("settings.json");
        assert!(!Settings::read(&path).0.no_update_check, "on by default");
        r.menu("Check for updates at start");
        assert!(!r.h.state().update_check);
        assert!(Settings::read(&path).0.no_update_check, "kept in the settings file");
        let again = App::with_settings(session(), Arc::new(lamina_assistant::testing::Fake::default()), path);
        assert!(!again.update_check, "and read back at the next start, which then does not check");
    }

    #[test]
    fn a_failed_check_by_hand_says_so_and_one_at_the_start_does_not() {
        let mut r = rig_on(Arc::new(Fake::default().fail(LATEST_URL, "offline")), true);
        r.start();
        assert_eq!(r.status(), (false, String::new()));
        assert!(r.h.query_by_label(OFFER).is_none());
        r.menu("Check for updates now");
        let (failed, text) = r.status();
        assert!(failed && text.starts_with("Could not check for updates:") && text.contains("offline"), "{text}");
    }

    #[test]
    fn a_download_that_fails_its_check_says_why_and_the_offer_stays() {
        let url = "https://github.com/IPedrax/Lamina/releases/download/v0.5.0/lamina-0.5.0-windows-x64.msi";
        let net = Fake::default().answer(LATEST_URL, release_json("v0.5.0", Some(INSTALLER))).answer(url, "another file");
        let mut r = rig_on(Arc::new(net), true);
        r.start();
        r.press("Update now");
        let (failed, text) = r.status();
        assert!(failed && text.starts_with("Could not download the update:"), "{text}");
        assert!(r.h.query_by_label(OFFER).is_some() && r.h.query_by_label("Update now").is_some(), "it can be tried again");
        assert!(r.handed.lock().unwrap().is_empty(), "a file that failed its check is never handed over");
    }

    /// Frames a few milliseconds apart until `done`: for an updater on threads of its own, as in the app.
    fn wait(h: &mut Harness<'static, App>, done: impl Fn(&App) -> bool) {
        for _ in 0..2000 {
            h.step();
            if done(h.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("gave up waiting: {:?}", h.state().updates.as_ref().map(|u| u.state().clone()));
    }

    #[test]
    fn while_an_update_installs_the_window_says_so_and_holds_still() {
        let mut r = rig("v0.5.0", true);
        // On threads of its own, with an install that waits for the test, as a slow one does, then fails.
        let (open, gate) = std::sync::mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let updater = Updater::new(r.net.clone(), Version::new(0, 4, 0), || {})
            .target(Some(Target::Windows))
            .in_folder(r.dir.path().to_path_buf())
            .handing_over(move |_| {
                gate.lock().map_err(|e| e.to_string())?.recv_timeout(Duration::from_secs(20)).map_err(|e| e.to_string())?;
                Err("no room".to_string())
            });
        r.h.state_mut().updates = Some(updater);
        r.updater().check_at_startup();
        wait(&mut r.h, |app| app.has_update_banner());
        r.h.get_by_label("Update now").click();
        wait(&mut r.h, |app| app.updates.as_ref().is_some_and(|u| matches!(u.state(), State::Ready { .. })));
        r.h.get_by_label("Restart and install").click();
        // The box has a spinner, which asks for frame after frame: `run` would never settle.
        r.h.step();
        r.h.step();
        assert!(r.h.query_by_label("Installing Lamina 0.5.0...").is_some(), "the box says what is going on");
        assert!(r.h.query_by_label("Restart and install").is_none(), "and the window gives way to it");
        // Nothing can change under it: a new plate, or the window's close button, which would end the install half way.
        let scene = Arc::clone(&r.h.state().session.scene);
        r.h.key_press_modifiers(Modifiers::COMMAND, Key::N);
        r.h.step();
        assert!(Arc::ptr_eq(&scene, &r.h.state().session.scene), "Ctrl+N started a new plate under the box");
        r.h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        r.h.step();
        let cancelled =
            r.h.output()
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|v| v.commands.contains(&egui::ViewportCommand::CancelClose));
        assert!(cancelled && !r.h.state().may_close, "closing the window waits for the install");
        open.send(()).unwrap();
        wait(&mut r.h, |app| app.failed);
        assert_eq!(r.status(), (true, "Could not install the update: no room".to_string()));
        assert!(r.h.query_by_label("Restart and install").is_some(), "the window is back, the update ready to try again");
    }
}
