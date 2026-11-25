//! The tray icon and its menu. bones designs a tray but does not implement
//! one yet, so the app owns it for v1 (docs/architecture.md); Windows only,
//! because on Linux the tray library needs a GTK loop the engine does not run.

// Only the Windows tray makes these; elsewhere the menu never opens.
#[cfg_attr(not(windows), allow(dead_code))]
pub enum TrayAction {
    Show,
    ReloadAll,
    Quit,
}

#[cfg(windows)]
mod imp {
    use super::TrayAction;
    use std::cell::RefCell;
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
    use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};

    thread_local! {
        // The icon is not Send, and the view must be; it lives on the thread
        // that made it, which is the one pumping its messages.
        static ICON: RefCell<Option<tray_icon::TrayIcon>> = const { RefCell::new(None) };
    }

    pub struct Tray {
        show: MenuId,
        reload: MenuId,
        quit: MenuId,
    }

    impl Tray {
        pub fn new() -> Option<Self> {
            let show = MenuItem::new("Show Things", true, None);
            let reload = MenuItem::new("Reload all Things", true, None);
            let quit = MenuItem::new("Quit", true, None);
            let menu = Menu::new();
            menu.append_items(&[&show, &reload, &quit]).ok()?;
            let icon = TrayIconBuilder::new()
                .with_tooltip("home-feeling")
                .with_menu(Box::new(menu))
                .with_icon(icon()?)
                .build()
                .ok()?;
            ICON.with(|slot| *slot.borrow_mut() = Some(icon));
            Some(Self {
                show: show.id().clone(),
                reload: reload.id().clone(),
                quit: quit.id().clone(),
            })
        }

        pub fn actions(&self) -> Vec<TrayAction> {
            let mut actions = Vec::new();
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id == self.show {
                    actions.push(TrayAction::Show);
                } else if event.id == self.reload {
                    actions.push(TrayAction::ReloadAll);
                } else if event.id == self.quit {
                    actions.push(TrayAction::Quit);
                }
            }
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::DoubleClick { .. } = event {
                    actions.push(TrayAction::Show);
                }
            }
            actions
        }
    }

    /// A plain rounded square in the app's colour, until the app has an icon.
    fn icon() -> Option<Icon> {
        const SIZE: u32 = 32;
        let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let edge = x.min(y).min(SIZE - 1 - x).min(SIZE - 1 - y);
                let alpha = if edge < 2 { 0 } else { 255 };
                rgba.extend_from_slice(&[0x2f, 0x6f, 0x5e, alpha]);
            }
        }
        Icon::from_rgba(rgba, SIZE, SIZE).ok()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::TrayAction;

    pub struct Tray;

    impl Tray {
        pub fn new() -> Option<Self> {
            None
        }

        pub fn actions(&self) -> Vec<TrayAction> {
            Vec::new()
        }
    }
}

pub use imp::Tray;
