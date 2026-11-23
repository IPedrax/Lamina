//! `lamina`: the Lamina desktop app. `lamina [FILE.stl]` opens the window, with the file on the plate when one is given.

// A release build on Windows is a window, not a console program: no console opens behind it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::process::ExitCode;
use std::sync::Arc;

use lamina_desktop::{App, session};

fn main() -> ExitCode {
    // The assistant's only way out: HTTPS to the three provider hosts, and only once the user sends a message with a key.
    let net = Arc::new(lamina_assistant::net::Ureq::new());
    let mut app = App::new(session(), net);
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
