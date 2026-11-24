//! The composition root: composes bones and, as they arrive, the Lua host,
//! consent, and desktop modules (docs/architecture.md).
// A release build is a desktop app, so Windows must not open a console behind it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod things_window;

fn main() {
    if let Err(error) = run() {
        eprintln!("fatal: {error}");
        std::process::exit(1);
    }
}

fn run() -> bones_kernel::Result<()> {
    bones_kernel::Kernel::new()
        .window("home-feeling", 900, 600)
        .renderer()
        .ui()
        .module(bones_kernel::ui::ViewModule::new(
            things_window::NAME,
            things_window::ThingsWindow,
        ))
        .run()
}
