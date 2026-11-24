//! The composition root: composes bones with the Lua host, consent, and the
//! desktop module, and owns the Things window (docs/architecture.md).
// A release build is a desktop app, so Windows must not open a console behind it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod builtins;
mod capabilities;
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
    installation::refresh_launcher();
    let logger = Logger::default();
    let things_dir = paths::things_dir();
    let data_dir = paths::data_dir();
    let offered = builtins::install(&things_dir, &data_dir.join("builtins.json"));
    let consent = Consent::open(data_dir.join("consent.json"), capabilities::all());
    let settings = settings::Settings::open(data_dir.join("settings.json"));

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
        let answered = matches!(consent.status(&thing, &manifest), Status::Answered { .. });
        if settings.enabled(&thing) && answered {
            startup.push(thing);
        }
    }
    let lua = lua.startup(startup);
    let control = lua.control();
    let (update_tx, update_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();

    bones_kernel::Kernel::new()
        .logger(logger.clone())
        .window(installation::NAME, 900, 600)
        .renderer()
        .upgrade(installation::config())
        .module(installation::UpdateBridge::new(update_rx, result_tx))
        // Before the Lua host, so Things can query it during their own init.
        .module(home_feeling_desktop::Desktop::new(
            logger.clone(),
            home_feeling_desktop::native_backend(),
        ))
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
            ),
        ))
        .run()
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
