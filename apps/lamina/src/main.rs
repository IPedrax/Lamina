//! `lamina`: the Lamina desktop app. `lamina [FILE.stl]` opens the window, with the file on the plate when one is given.

// A release build on Windows is a window, not a console program: no console opens behind it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::process::ExitCode;
use std::sync::Arc;

use lamina_desktop::{App, session, startup};

const USAGE: &str = "USAGE: lamina [FILE]... [--control PORT]
  FILE       a .lamina project to open, or a mesh to put on the plate: .stl, .3mf, .obj, .gltf or .glb
  --control  let agents work on this window's plate (`lamina-cli mcp --connect`); 0 takes any free port. Only this computer can
             connect, with the token the window writes beside its settings
  --version  the release
  --help     this text";

/// What the arguments ask for before any window opens: the usage (`--help`, or an option the program does not know, which is an
/// error), or nothing, and the window starts.
fn before_window(args: &[String]) -> Option<(String, bool)> {
    let mut rest = args.iter();
    while let Some(a) = rest.next() {
        match a.as_str() {
            "--help" | "-h" => return Some((USAGE.to_string(), true)),
            "--control" => {
                rest.next();
            }
            other if other.starts_with('-') => return Some((format!("lamina: unknown option `{other}`\n\n{USAGE}"), false)),
            _ => {}
        }
    }
    None
}

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
    if let Some((text, ok)) = before_window(&args) {
        return if ok {
            println!("{text}");
            ExitCode::SUCCESS
        } else {
            eprintln!("{text}");
            ExitCode::FAILURE
        };
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
    // `--control PORT` lets agents work on this window's plate (0: any free port), as the Agents section does; any other argument is
    // a file to open.
    let mut rest = args.iter();
    while let Some(a) = rest.next() {
        if a == "--control" {
            match rest.next().and_then(|p| p.parse::<u16>().ok()) {
                Some(port) => {
                    app.agents.port = port;
                    app.open_control();
                }
                None => eprintln!("lamina: --control needs a port number (0 for any free one)"),
            }
        } else {
            app.open(a);
        }
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
    // The 3D view puts a scene's triangles in one buffer on the card: as big a buffer as the adapter allows (OpenGL keeps its own).
    let asked = setup.device_descriptor.clone();
    setup.device_descriptor = Arc::new(move |adapter| {
        let mut d = asked(adapter);
        if adapter.get_info().backend != eframe::egui_wgpu::wgpu::Backend::Gl {
            d.required_limits.max_buffer_size = adapter.limits().max_buffer_size;
        }
        d
    });
    options.wgpu_options.wgpu_setup = eframe::egui_wgpu::WgpuSetup::CreateNew(setup);
    let up_files = files.clone();
    let start = move |cc: &eframe::CreationContext| -> Result<Box<dyn eframe::App>, _> {
        let mut app = app;
        app.view.card.state = cc.wgpu_render_state.clone();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn help_and_unknown_options_print_the_usage_instead_of_opening_a_window() {
        assert!(before_window(&args(&["--help"])).is_some_and(|(t, ok)| ok && t.starts_with("USAGE")));
        assert!(before_window(&args(&["part.stl", "-h"])).is_some_and(|(_, ok)| ok));
        assert!(before_window(&args(&["--frobnicate"])).is_some_and(|(t, ok)| !ok && t.contains("`--frobnicate`")));
        assert_eq!(before_window(&args(&["part.stl", "--control", "0"])), None);
        assert_eq!(before_window(&args(&[])), None);
    }
}
