//! The built-in tiler, run by the Lua host against the desktop module with a
//! recording backend: the whole Thing path without touching real windows.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bones_kernel::bus::{Bus, Envelope, Handler, Module, ModuleContext, Registry, ServiceRegistry};
use bones_kernel::logging::Logger;
use bones_module_lua::{GuestState, LuaControl, LuaHost};
use home_feeling_desktop::protocol::{
    Capabilities, Id, Monitor, Overlay, Point, Rect, Snapshot, WindowInfo,
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
    overlays: std::collections::HashMap<u32, (Rect, String)>,
    minimized: Vec<Id>,
    /// Sizes windows refuse to go below, as real apps do.
    minimums: std::collections::HashMap<Id, (i32, i32)>,
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
    fn move_window(&mut self, id: Id, rect: Rect, _: bool) -> Result<(), String> {
        let mut screen = self.0.lock().unwrap();
        screen.moves.push((id, rect));
        // Report the frame the window really took, as Windows does.
        let (min_w, min_h) = screen.minimums.get(&id).copied().unwrap_or((0, 0));
        let actual = Rect {
            w: rect.w.max(min_w),
            h: rect.h.max(min_h),
            ..rect
        };
        screen.pending.push(RawEvent::Moved(id, actual));
        screen.pending.push(RawEvent::Settled(id, actual, rect));
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
    fn set_overlay(&mut self, key: u32, overlay: &Overlay) {
        let mut screen = self.0.lock().unwrap();
        if overlay.color == "#2f6f5e" && overlay.width == 3 {
            screen.overlay = Some(overlay.rect);
        }
        screen
            .overlays
            .insert(key, (overlay.rect, overlay.color.clone()));
    }
    fn clear_overlay(&mut self, key: u32) {
        let mut screen = self.0.lock().unwrap();
        if let Some((rect, _)) = screen.overlays.remove(&key) {
            if screen.overlay == Some(rect) {
                screen.overlay = None;
            }
        }
    }
    fn minimize(&mut self, id: Id) -> Result<(), String> {
        let mut screen = self.0.lock().unwrap();
        screen.minimized.push(id);
        screen.pending.push(RawEvent::Minimized(id));
        Ok(())
    }
    fn restore(&mut self, id: Id) -> Result<(), String> {
        let mut screen = self.0.lock().unwrap();
        screen.minimized.retain(|m| *m != id);
        screen.pending.push(RawEvent::Opened(window(id)));
        Ok(())
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
        maximized: false,
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
        "desktop/windows/minimize",
        "desktop/windows/restore",
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
        // A Lua error quarantines the tiler, after which nothing moves and an
        // assertion about moves can pass by accident.
        let tiler = self
            .control
            .records()
            .into_iter()
            .find(|r| r.name == "tiler")
            .unwrap();
        assert_eq!(
            tiler.state,
            GuestState::Running,
            "the tiler faulted: {:?}",
            tiler.last_error
        );
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
    assert_eq!(world.screen.lock().unwrap().hotkeys.len(), 10);
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

#[test]
fn a_window_that_refuses_its_tile_gets_a_wider_one_when_there_is_room() {
    let mut world = world(None, &[1]);
    world.screen.lock().unwrap().minimums.insert(2, (550, 0));
    world.event(RawEvent::Opened(window(2)));
    // 2 refused 488 and needs 550: the split moves so it fits beside 1.
    let moves = world.last_moves();
    assert_eq!(
        moves[&2],
        Rect {
            x: 442,
            y: 8,
            w: 550,
            h: 784
        }
    );
    assert_eq!(
        moves[&1],
        Rect {
            x: 8,
            y: 8,
            w: 426,
            h: 784
        }
    );
}

#[test]
fn dragging_the_edge_between_two_tiles_moves_the_split() {
    let mut world = world(None, &[1, 2]);
    world.screen.lock().unwrap().moves.clear();
    // The user drags 1's right edge from 496 to 568.
    world.event(RawEvent::Dragged(
        1,
        Rect {
            x: 8,
            y: 8,
            w: 560,
            h: 784,
        },
        Point { x: 0, y: 0 },
    ));
    // 1 is already where the user put it; only its neighbour moves.
    let moves = world.last_moves();
    assert!(!moves.contains_key(&1), "{moves:?}");
    assert_eq!(
        moves[&2],
        Rect {
            x: 576,
            y: 8,
            w: 416,
            h: 784
        }
    );
}

#[test]
fn a_drag_never_squeezes_the_neighbour_below_its_minimum() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Dragged(
        1,
        Rect {
            x: 8,
            y: 8,
            w: 900,
            h: 784,
        },
        Point { x: 0, y: 0 },
    ));
    // 2 keeps the 400 minimum; 1 gets the rest.
    let moves = world.last_moves();
    assert_eq!(
        moves[&2],
        Rect {
            x: 592,
            y: 8,
            w: 400,
            h: 784
        }
    );
    assert_eq!(
        moves[&1],
        Rect {
            x: 8,
            y: 8,
            w: 576,
            h: 784
        }
    );
}

#[test]
fn moving_a_tiled_window_snaps_it_back() {
    let mut world = world(None, &[1, 2]);
    world.screen.lock().unwrap().moves.clear();
    world.event(RawEvent::Dragged(
        1,
        Rect {
            x: 300,
            y: 200,
            w: 488,
            h: 784,
        },
        Point { x: 0, y: 0 },
    ));
    assert_eq!(
        world.last_moves()[&1],
        Rect {
            x: 8,
            y: 8,
            w: 488,
            h: 784
        }
    );
}

#[test]
fn a_maximized_window_is_left_maximized_until_restored() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Maximized(1, true));
    world.event(RawEvent::Moved(1, WORK));
    world.screen.lock().unwrap().moves.clear();
    world.event(RawEvent::Opened(window(3)));
    assert!(
        !world.last_moves().contains_key(&1),
        "{:?}",
        world.last_moves()
    );
    world.event(RawEvent::Maximized(1, false));
    assert!(world.last_moves().contains_key(&1));
}

#[test]
fn the_border_follows_the_real_frame() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(1));
    let real = Rect {
        x: 8,
        y: 8,
        w: 600,
        h: 784,
    };
    world.event(RawEvent::Moved(1, real));
    assert_eq!(world.screen.lock().unwrap().overlay, Some(real));
}

fn at(x: i32, y: i32) -> Point {
    Point { x, y }
}

#[test]
fn a_new_window_splits_another_tile_when_the_focused_one_is_too_small() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(2));
    world.event(RawEvent::Opened(window(3)));
    world.event(RawEvent::Focused(3));
    // 3's tile is 488x388; a split would leave 240 wide, under 400. 1's tile
    // is the largest and splits instead; nothing stacks.
    world.event(RawEvent::Opened(window(4)));
    let moves = world.last_moves();
    assert_eq!(
        moves[&4],
        Rect {
            x: 8,
            y: 404,
            w: 488,
            h: 388
        }
    );
    assert!(world.screen.lock().unwrap().minimized.is_empty());
}

#[test]
fn a_window_that_refuses_its_tile_joins_a_stack_where_it_fits() {
    let mut world = world(None, &[1]);
    world.screen.lock().unwrap().minimums.insert(2, (700, 0));
    world.event(RawEvent::Opened(window(2)));
    // 2 needs 700, which would leave 1 only 276: 2 shares 1's whole tile.
    assert_eq!(
        world.last_moves()[&2],
        Rect {
            x: 8,
            y: 8,
            w: 984,
            h: 784
        }
    );
    assert_eq!(world.screen.lock().unwrap().minimized, vec![1]);
}

#[test]
fn stack_next_shows_the_other_member_and_closing_one_shows_the_rest() {
    let mut world = world(None, &[1]);
    world.screen.lock().unwrap().minimums.insert(2, (700, 0));
    world.event(RawEvent::Opened(window(2)));
    world.event(RawEvent::Focused(2));
    world.press("alt+shift+n");
    assert_eq!(world.screen.lock().unwrap().minimized, vec![2]);
    world.event(RawEvent::Closed(1));
    // 2 comes back to the whole tile.
    assert!(world.screen.lock().unwrap().minimized.is_empty());
}

#[test]
fn a_stack_shows_a_tab_strip_over_its_tile() {
    let mut world = world(None, &[1]);
    world.screen.lock().unwrap().minimums.insert(2, (700, 0));
    world.event(RawEvent::Opened(window(2)));
    let screen = world.screen.lock().unwrap();
    let tabs: Vec<&Rect> = screen
        .overlays
        .values()
        .filter(|(r, _)| r.h == 0)
        .map(|(r, _)| r)
        .collect();
    assert_eq!(tabs.len(), 2, "{:?}", screen.overlays);
}

#[test]
fn swap_and_toggle_float_by_hotkey_with_a_cyan_frame() {
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
    // 1 floats: 2 takes the whole area, 1 is framed in cyan.
    assert_eq!(
        world.last_moves()[&2],
        Rect {
            x: 8,
            y: 8,
            w: 984,
            h: 784
        }
    );
    let screen = world.screen.lock().unwrap();
    assert!(
        screen.overlays.values().any(|(_, c)| c == "#00bcd4"),
        "{:?}",
        screen.overlays
    );
}

#[test]
fn dropping_near_a_tile_edge_splits_that_tile_on_that_side() {
    let mut world = world(None, &[1, 2]);
    let start = Rect {
        x: 504,
        y: 8,
        w: 488,
        h: 784,
    };
    world.event(RawEvent::DragStarted(2, at(700, 20)));
    world.event(RawEvent::Dragging(2, Rect { x: 100, ..start }, at(20, 400)));
    world.event(RawEvent::Dragged(2, Rect { x: 100, ..start }, at(20, 400)));
    // Dropped near 1's left edge: 2 goes left of 1.
    let moves = world.last_moves();
    assert_eq!(
        moves[&2],
        Rect {
            x: 8,
            y: 8,
            w: 488,
            h: 784
        }
    );
    assert_eq!(
        moves[&1],
        Rect {
            x: 504,
            y: 8,
            w: 488,
            h: 784
        }
    );
}

#[test]
fn dropping_near_the_bottom_stacks_the_tiles_vertically() {
    let mut world = world(None, &[1, 2]);
    let start = Rect {
        x: 504,
        y: 8,
        w: 488,
        h: 784,
    };
    world.event(RawEvent::DragStarted(2, at(700, 20)));
    world.event(RawEvent::Dragging(
        2,
        Rect { x: 100, ..start },
        at(250, 780),
    ));
    world.event(RawEvent::Dragged(2, Rect { x: 100, ..start }, at(250, 780)));
    let moves = world.last_moves();
    assert_eq!(
        moves[&1],
        Rect {
            x: 8,
            y: 8,
            w: 984,
            h: 388
        }
    );
    assert_eq!(
        moves[&2],
        Rect {
            x: 8,
            y: 404,
            w: 984,
            h: 388
        }
    );
}

#[test]
fn dropping_in_the_middle_of_a_tile_swaps() {
    let mut world = world(None, &[1, 2]);
    let start = Rect {
        x: 504,
        y: 8,
        w: 488,
        h: 784,
    };
    world.event(RawEvent::DragStarted(2, at(700, 20)));
    world.event(RawEvent::Dragging(
        2,
        Rect { x: 100, ..start },
        at(250, 400),
    ));
    world.event(RawEvent::Dragged(2, Rect { x: 100, ..start }, at(250, 400)));
    let moves = world.last_moves();
    assert_eq!(
        moves[&2],
        Rect {
            x: 8,
            y: 8,
            w: 488,
            h: 784
        }
    );
    assert_eq!(
        moves[&1],
        Rect {
            x: 504,
            y: 8,
            w: 488,
            h: 784
        }
    );
}

#[test]
fn a_drag_previews_the_drop_and_clears_it_after() {
    let mut world = world(None, &[1, 2]);
    let start = Rect {
        x: 504,
        y: 8,
        w: 488,
        h: 784,
    };
    world.event(RawEvent::DragStarted(2, at(700, 20)));
    world.event(RawEvent::Dragging(2, Rect { x: 100, ..start }, at(20, 400)));
    let preview = Rect {
        x: 8,
        y: 8,
        w: 240,
        h: 784,
    };
    assert!(world
        .screen
        .lock()
        .unwrap()
        .overlays
        .values()
        .any(|(r, _)| *r == preview));
    world.event(RawEvent::Dragged(2, Rect { x: 100, ..start }, at(20, 400)));
    assert!(!world
        .screen
        .lock()
        .unwrap()
        .overlays
        .values()
        .any(|(r, _)| *r == preview));
}
