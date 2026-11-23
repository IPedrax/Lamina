//! `lamina`: the Lamina desktop app. `lamina [FILE.stl]` opens the window, with the file on the plate when one is given.

// A release build on Windows is a window, not a console program: no console opens behind it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::process::ExitCode;
use std::sync::Arc;

use lamina_desktop::{App, session};

fn main() -> ExitCode {
    // Started as the child that runs one boolean: do that and nothing else, with no window.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = lamina_csg::child_main(&args) {
        return code;
    }
    // Every boolean runs in a copy of this program, so a crash in the engine is an error in the window and not the end of it.
    if let Ok(exe) = std::env::current_exe() {
        lamina_csg::run_in_child(exe);
    }
    // The assistant's only way out: HTTPS to the three provider hosts, and only once the user sends a message with a key.
    let net = Arc::new(lamina_assistant::net::Ureq::new());
    // The settings (print settings, presets, a remembered key) live in the user's settings folder, or where LAMINA_SETTINGS says.
    let mut app = match lamina_desktop::settings::default_path() {
        Some(path) => App::with_settings(session(), net, path),
        None => App::new(session(), net),
    };
    if let Some(file) = std::env::args().nth(1) {
        app.open(&file);
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Lamina")
            .with_app_id("io.github.ipedrax.lamina")
            .with_inner_size([1440.0, 900.0]),
        ..Default::default()
    };
    match eframe::run_native("Lamina", options, Box::new(|_| Ok(Box::new(app)))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lamina: {e}");
            ExitCode::FAILURE
        }
    }
}
