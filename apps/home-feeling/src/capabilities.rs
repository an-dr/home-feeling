//! The powers the app guards, as the user reads them in a consent prompt.
//! Each covers the desktop module's topics and query endpoints for it.
use bones_consent::Capability;

pub fn all() -> Vec<Capability> {
    vec![
        Capability::new(
            "windows",
            "See, move, resize, and focus your windows",
            &["desktop/windows/*", "desktop/monitors/*"],
        ),
        Capability::new(
            "hotkeys",
            "Use global keyboard shortcuts",
            &["desktop/hotkeys/*"],
        ),
        Capability::new(
            "overlays",
            "Draw on top of the desktop",
            &["desktop/overlay/*"],
        ),
        Capability::new("os", "Open links, files, and dialogs", &["os/*", "files"]),
    ]
}
