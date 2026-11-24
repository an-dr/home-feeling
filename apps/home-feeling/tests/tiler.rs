//! The built-in tiler, run by the Lua host against the desktop module with a
//! recording backend: the whole Thing path without touching real windows.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bones_kernel::bus::{Bus, Envelope, Handler, Module, ModuleContext, Registry, ServiceRegistry};
use bones_kernel::logging::Logger;
use bones_module_lua::{GuestState, LuaControl, LuaHost};
use home_feeling_desktop::protocol::{
    Capabilities, Id, Monitor, Overlay, Rect, Snapshot, WindowInfo,
};
use home_feeling_desktop::{Backend, Desktop, Hotkey, RawEvent, Unsupported};

const WORK: Rect = Rect {
    x: 0,
    y: 0,
    w: 1000,
    h: 800,
};

#[derive(Default)]
struct Screen {
    windows: Vec<WindowInfo>,
    pending: Vec<RawEvent>,
    moves: Vec<(Id, Rect)>,
    hotkeys: Vec<(u32, Hotkey)>,
    overlay: Option<Rect>,
}

struct Fake(Arc<Mutex<Screen>>);

impl Backend for Fake {
    fn snapshot(&mut self) -> Snapshot {
        let screen = self.0.lock().unwrap();
        Snapshot {
            capabilities: Capabilities {
                windows: true,
                hotkeys: true,
                overlays: true,
            },
            platform: "fake".into(),
            windows: screen.windows.clone(),
            monitors: vec![Monitor {
                id: 1,
                rect: WORK,
                work: WORK,
            }],
            focused: screen.windows.last().map(|w| w.id),
        }
    }
    fn poll(&mut self) -> Vec<RawEvent> {
        std::mem::take(&mut self.0.lock().unwrap().pending)
    }
    fn move_window(&mut self, id: Id, rect: Rect) -> Result<(), String> {
        self.0.lock().unwrap().moves.push((id, rect));
        Ok(())
    }
    fn focus(&mut self, id: Id) -> Result<(), String> {
        self.0.lock().unwrap().pending.push(RawEvent::Focused(id));
        Ok(())
    }
    fn register_hotkey(&mut self, key: u32, hotkey: Hotkey) -> Result<(), String> {
        self.0.lock().unwrap().hotkeys.push((key, hotkey));
        Ok(())
    }
    fn unregister_hotkey(&mut self, _: u32) {}
    fn set_overlay(&mut self, _: u32, overlay: &Overlay) {
        self.0.lock().unwrap().overlay = Some(overlay.rect);
    }
    fn clear_overlay(&mut self, _: u32) {
        self.0.lock().unwrap().overlay = None;
    }
}

fn window(id: Id) -> WindowInfo {
    WindowInfo {
        id,
        title: format!("w{id}"),
        class: "test".into(),
        rect: Rect {
            x: 10,
            y: 10,
            w: 300,
            h: 200,
        },
        monitor: 1,
        minimized: false,
    }
}

struct World {
    bus: Bus,
    desktop: Arc<Mutex<Desktop>>,
    lua: LuaHost,
    control: LuaControl,
    screen: Arc<Mutex<Screen>>,
}

fn things_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../things")
}

fn world(backend: Option<Box<dyn Backend>>, initial: &[Id]) -> World {
    let screen: Arc<Mutex<Screen>> = Arc::default();
    screen.lock().unwrap().windows = initial.iter().map(|id| window(*id)).collect();
    let backend = backend.unwrap_or_else(|| Box::new(Fake(screen.clone())));
    let bus = Bus::new();
    let registry = Registry::new();
    let mut services = ServiceRegistry::new();
    services.provide(bus.clone()).unwrap();
    services.provide(registry.clone()).unwrap();

    let mut desktop = Desktop::new(Logger::default(), backend);
    desktop
        .init(&mut ModuleContext::new(&mut services))
        .unwrap();
    let desktop = Arc::new(Mutex::new(desktop));
    // Stand in for the kernel: give the module its endpoint and topics.
    let target = desktop.clone();
    let endpoint = bus.register("desktop", move |envelope: &Envelope| {
        target.lock().unwrap().handle(envelope);
    });
    for topic in [
        "desktop/windows/move",
        "desktop/windows/focus",
        "desktop/hotkeys/*",
        "desktop/overlay/*",
        "core/lifecycle",
    ] {
        endpoint.subscribe(topic);
    }
    std::mem::forget(endpoint);

    let mut lua = LuaHost::new(Logger::default())
        .guests_dir(things_dir())
        .startup(["tiler".to_string()])
        .budgets(Duration::from_secs(2), Duration::from_millis(500));
    let control = lua.control();
    lua.init(&mut ModuleContext::new(&mut services)).unwrap();
    let mut world = World {
        bus,
        desktop,
        lua,
        control,
        screen,
    };
    world.frame();
    world
}

impl World {
    /// A few frames, so requests, replies, and their effects all settle.
    fn frame(&mut self) {
        for _ in 0..6 {
            self.bus.begin_frame();
            self.lua.before_tick();
            self.desktop.lock().unwrap().before_tick();
            self.bus.dispatch();
            self.lua.after_tick();
            self.desktop.lock().unwrap().after_tick();
            self.bus.dispatch();
        }
    }

    fn last_moves(&self) -> std::collections::HashMap<Id, Rect> {
        self.screen.lock().unwrap().moves.iter().cloned().collect()
    }

    fn event(&mut self, event: RawEvent) {
        self.screen.lock().unwrap().pending.push(event);
        self.frame();
    }

    fn press(&mut self, keys: &str) {
        let wanted = Hotkey::parse(keys).unwrap();
        let key = self
            .screen
            .lock()
            .unwrap()
            .hotkeys
            .iter()
            .find(|(_, h)| *h == wanted)
            .map(|(k, _)| *k);
        self.event(RawEvent::Hotkey(
            key.unwrap_or_else(|| panic!("{keys} was not registered")),
        ));
    }

    fn tiler_log(&self) -> Vec<String> {
        let record = self
            .control
            .records()
            .into_iter()
            .find(|r| r.name == "tiler")
            .unwrap();
        record.log.iter().map(|l| l.text.clone()).collect()
    }
}

#[test]
fn existing_windows_are_tiled_side_by_side_with_gaps() {
    let world = world(None, &[1, 2]);
    let moves = world.last_moves();
    // Area 984x784 inside an 8px gap; two halves with 8px between them.
    assert_eq!(
        moves[&1],
        Rect {
            x: 8,
            y: 8,
            w: 488,
            h: 784
        }
    );
    assert_eq!(
        moves[&2],
        Rect {
            x: 504,
            y: 8,
            w: 488,
            h: 784
        }
    );
    assert_eq!(world.screen.lock().unwrap().hotkeys.len(), 9);
}

#[test]
fn a_new_window_splits_the_focused_tile_along_its_longer_side() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(2));
    world.event(RawEvent::Opened(window(3)));
    let moves = world.last_moves();
    assert_eq!(
        moves[&2],
        Rect {
            x: 504,
            y: 8,
            w: 488,
            h: 388
        }
    );
    assert_eq!(
        moves[&3],
        Rect {
            x: 504,
            y: 404,
            w: 488,
            h: 388
        }
    );
}

#[test]
fn a_window_whose_tile_would_be_too_small_floats() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(2));
    world.event(RawEvent::Opened(window(3)));
    world.event(RawEvent::Focused(3));
    // 3's tile is 488x388; split side by side it would be 240 wide, under 400.
    world.event(RawEvent::Opened(window(4)));
    let moves = world.last_moves();
    assert!(!moves.contains_key(&4), "{moves:?}");
    assert_eq!(
        moves[&3],
        Rect {
            x: 504,
            y: 404,
            w: 488,
            h: 388
        }
    );
}

#[test]
fn closing_a_window_gives_its_space_back() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Closed(2));
    assert_eq!(
        world.last_moves()[&1],
        Rect {
            x: 8,
            y: 8,
            w: 984,
            h: 784
        }
    );
}

#[test]
fn the_focus_border_follows_the_focused_tile() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(1));
    assert_eq!(
        world.screen.lock().unwrap().overlay,
        Some(Rect {
            x: 8,
            y: 8,
            w: 488,
            h: 784
        })
    );
}

#[test]
fn hotkeys_swap_and_toggle_float() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(1));
    world.press("alt+shift+right");
    assert_eq!(
        world.last_moves()[&1],
        Rect {
            x: 504,
            y: 8,
            w: 488,
            h: 784
        }
    );
    world.press("alt+shift+space");
    // 1 floats; 2 takes the whole area.
    assert_eq!(
        world.last_moves()[&2],
        Rect {
            x: 8,
            y: 8,
            w: 984,
            h: 784
        }
    );
}

#[test]
fn focus_moves_to_the_neighbour_in_the_pressed_direction() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(1));
    world.press("alt+right");
    assert_eq!(
        world.screen.lock().unwrap().overlay,
        Some(Rect {
            x: 504,
            y: 8,
            w: 488,
            h: 784
        })
    );
}

#[test]
fn without_window_management_the_tiler_says_so_and_does_nothing() {
    let world = world(Some(Box::new(Unsupported)), &[]);
    let log = world.tiler_log();
    assert!(
        log.iter().any(|line| line.contains("not supported")),
        "{log:?}"
    );
    let record = world
        .control
        .records()
        .into_iter()
        .find(|r| r.name == "tiler")
        .unwrap();
    assert_eq!(record.state, GuestState::Running);
}
