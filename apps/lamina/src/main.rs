//! `lamina`: the Lamina desktop app. `lamina [FILE.stl]` opens the window, with the file on the plate when one is given.

// A release build on Windows is a window, not a console program: no console opens behind it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::process::ExitCode;
use std::sync::Arc;

use lamina_desktop::{App, session, startup};

fn main() -> ExitCode {
    // Started as the child that runs one boolean: do that and nothing else, with no window.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = lamina_csg::child_main(&args) {
        return code;
    }
    // What the packaging scripts run to check the program starts: no window.
    if args.first().map(String::as_str) == Some("--version") {
        println!("lamina {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    // Every boolean runs in a copy of this program, so a crash in the engine is an error in the window and not the end of it.
    if let Ok(exe) = std::env::current_exe() {
        lamina_csg::run_in_child(exe);
    }
    // The assistant's only way out: HTTPS to the three provider hosts, and only once the user sends a message with a key.
    let net = Arc::new(lamina_assistant::net::Ureq::new());
    // The settings (print settings, presets, a remembered key) live in the user's settings folder, or where LAMINA_SETTINGS says;
    // the log, and the mark that makes a start after one that died try other graphics, beside them.
    let settings = lamina_desktop::settings::default_path();
    let files = settings.as_deref().map(startup::Files::beside);
    let graphics = startup::begin(files.as_ref());
    let mut app = match settings {
        Some(path) => App::with_settings(session(), net, path),
        None => App::new(session(), net),
    };
    if let Some(file) = std::env::args().nth(1) {
        app.open(&file);
    }
    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Lamina")
            .with_app_id("io.github.ipedrax.lamina")
            .with_inner_size([1440.0, 900.0])
            .with_icon(eframe::egui::IconData {
                rgba: lamina_desktop::icon::pixels(256).into_iter().flatten().collect(),
                width: 256,
                height: 256,
            }),
        ..Default::default()
    };
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    if let Some(backends) = graphics.backends() {
        setup.instance_descriptor.backends = backends;
    }
    options.wgpu_options.wgpu_setup = eframe::egui_wgpu::WgpuSetup::CreateNew(setup);
    let up_files = files.clone();
    let start = move |cc: &eframe::CreationContext| -> Result<Box<dyn eframe::App>, _> {
        let adapter = cc.wgpu_render_state.as_ref().map(|state| {
            let info = state.adapter.get_info();
            format!("{} ({:?}, driver {} {})", info.name, info.backend, info.driver, info.driver_info)
        });
        startup::up(up_files.as_ref(), graphics, adapter.as_deref().unwrap_or("no wgpu adapter"));
        Ok(Box::new(app))
    };
    match eframe::run_native("Lamina", options, Box::new(start)) {
        Ok(()) => {
            startup::closed(files.as_ref(), graphics);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("lamina: {e}");
            if let Some(files) = &files {
                startup::note(&files.log, &format!("start failed on {} graphics: {e}", graphics.name()));
            }
            startup::tell(&e.to_string(), files.as_ref());
            ExitCode::FAILURE
        }
    }
}
