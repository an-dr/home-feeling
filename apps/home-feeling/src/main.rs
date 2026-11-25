//! The composition root: composes bones with the Lua host, consent, and the
//! desktop module, and owns the Things window (docs/architecture.md).
// A release build is a desktop app, so Windows must not open a console behind it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod background;
mod builtins;
mod capabilities;
mod color;
mod installation;
mod paths;
mod settings;
mod things_window;
mod tray;

use bones_consent::{Consent, Manifest, Status};
use bones_kernel::logging::Logger;
use bones_module_lua::{LuaHost, Permissions};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = installation::command_line(&args) {
        match result {
            Ok(message) => println!("{message}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Err(error) = run() {
        eprintln!("fatal: {error}");
        std::process::exit(1);
    }
}

fn run() -> bones_kernel::Result<()> {
    // One copy per user: a later launch hands over to this one, which comes
    // back from the tray, and ends.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let handoff = bones_kernel::instance::Handoff {
        args: args.clone(),
        cwd: std::env::current_dir().unwrap_or_default(),
    };
    let primary = match bones_kernel::instance::claim(
        "home-feeling",
        handoff,
        std::time::Duration::from_secs(2),
    ) {
        Ok(bones_kernel::instance::Claim::HandedOff) => return Ok(()),
        Ok(bones_kernel::instance::Claim::Primary(primary)) => Some(primary),
        Err(error) => {
            eprintln!("single instance is off: {error}");
            None
        }
    };
    let start_hidden = args.iter().any(|arg| arg == "--minimized");
    installation::refresh_launcher();
    let logger = Logger::default();
    let things_dir = paths::things_dir();
    let data_dir = paths::data_dir();
    let offered = builtins::install(&things_dir, &data_dir.join("builtins.json"));
    let consent = Consent::open(data_dir.join("consent.json"), capabilities::all());
    let settings = settings::Settings::open(data_dir.join("settings.json"));
    // The startup entry names a path; write it again so a moved install heals.
    let app = settings.app();
    if app.autostart {
        let _ = autostart::set(true, app.start_minimized);
    }
    let trace = home_feeling_desktop::Trace::new(data_dir.join("window-trace.jsonl"));
    if app.trace_windows {
        if let Err(err) = trace.set_enabled(true) {
            eprintln!("{err}");
        }
    }

    let mut lua = LuaHost::new(logger.clone()).guests_dir(&things_dir);
    let mut startup = Vec::new();
    let mut denied = std::collections::HashMap::new();
    for thing in thing_names(&things_dir) {
        let Ok(manifest) = Manifest::read(&things_dir.join(&thing)) else {
            continue;
        };
        let deny = consent.deny_list(&thing, &manifest);
        denied.insert(thing.clone(), deny.clone());
        lua = lua.permissions(&thing, Permissions::deny(deny));
        let values = bones_consent::values(&manifest.settings, &settings.chosen(&thing));
        lua = lua.config(&thing, bones_consent::nest(&values));
        let answered = matches!(consent.status(&thing, &manifest), Status::Answered { .. });
        if settings.enabled(&thing) && answered {
            startup.push(thing);
        }
    }
    let lua = lua.startup(startup);
    let control = lua.control();
    let (update_tx, update_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();

    let kernel = bones_kernel::Kernel::new()
        .logger(logger.clone())
        .window(installation::NAME, 900, 600)
        .renderer()
        .upgrade(installation::config())
        .module(installation::UpdateBridge::new(update_rx, result_tx))
        // Before the Lua host, so Things can query it during their own init.
        .module(
            home_feeling_desktop::Desktop::new(
                logger.clone(),
                home_feeling_desktop::native_backend(),
            )
            .trace(trace.clone()),
        )
        .module(lua)
        .ui()
        .module(bones_kernel::ui::ViewModule::new(
            things_window::NAME,
            things_window::ThingsWindow::new(
                control,
                consent,
                settings,
                things_dir,
                offered,
                denied,
                things_window::Updates {
                    requests: update_tx,
                    results: result_rx,
                },
            )
            .trace(trace)
            .start_in_tray(start_hidden),
        ));
    match primary {
        Some(primary) => kernel.single_instance(primary).run(),
        None => kernel.run(),
    }
}

/// The folders in `dir` that hold an `init.lua`.
fn thing_names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join("init.lua").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}
