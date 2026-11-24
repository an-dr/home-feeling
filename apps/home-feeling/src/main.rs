//! The composition root: composes bones with the Lua host, consent, and the
//! desktop module, and owns the Things window (docs/architecture.md).
// A release build is a desktop app, so Windows must not open a console behind it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod builtins;
mod capabilities;
mod paths;
mod settings;
mod things_window;
mod tray;

use bones_consent::{Consent, Manifest, Status};
use bones_kernel::logging::Logger;
use bones_module_lua::{LuaHost, Permissions};

fn main() {
    if let Err(error) = run() {
        eprintln!("fatal: {error}");
        std::process::exit(1);
    }
}

fn run() -> bones_kernel::Result<()> {
    let logger = Logger::default();
    let things_dir = paths::things_dir();
    let data_dir = paths::data_dir();
    let offered = builtins::install(&things_dir, &data_dir.join("builtins.json"));
    let consent = Consent::open(data_dir.join("consent.json"), capabilities::all());
    let settings = settings::Settings::open(data_dir.join("settings.json"));

    let mut lua = LuaHost::new(logger.clone()).guests_dir(&things_dir);
    let mut startup = Vec::new();
    for thing in thing_names(&things_dir) {
        let Ok(manifest) = Manifest::read(&things_dir.join(&thing)) else {
            continue;
        };
        lua = lua.permissions(
            &thing,
            Permissions::deny(consent.deny_list(&thing, &manifest)),
        );
        let answered = matches!(consent.status(&thing, &manifest), Status::Answered { .. });
        if settings.enabled(&thing) && answered {
            startup.push(thing);
        }
    }
    let lua = lua.startup(startup);
    let control = lua.control();

    bones_kernel::Kernel::new()
        .logger(logger)
        .window("home-feeling", 900, 600)
        .renderer()
        .module(lua)
        .ui()
        .module(bones_kernel::ui::ViewModule::new(
            things_window::NAME,
            things_window::ThingsWindow::new(control, consent, settings, things_dir, offered),
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
