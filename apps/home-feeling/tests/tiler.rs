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
    /// The window the focus frame was asked to sit above.
    overlay_attach: Option<Id>,
    /// The fade the focus frame was drawn with, each time.
    overlay_fades: Vec<Option<u32>>,
    /// The corner radius the focus frame was last drawn with.
    overlay_radius: i32,
    /// Every overlay drawn with a fill: its rect and fade.
    filled: Vec<(Rect, Option<u32>)>,
    overlays: std::collections::HashMap<u32, (Rect, String)>,
    minimized: Vec<Id>,
    /// Windows whose next move ends smaller than asked, once, as a window
    /// that was minimized does.
    shrink_once: std::collections::HashSet<Id>,
    /// Windows whose next move ends this much wider than asked, once, as a
    /// window still sizing itself after it opened does.
    grow_once: std::collections::HashMap<Id, i32>,
    /// The window each badge was asked to sit above.
    attached: Vec<Id>,
    /// Popup menus asked for: key, items, and where.
    menus: Vec<(u32, Vec<home_feeling_desktop::MenuItem>, Point)>,
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
                corner_radius: 0,
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
        let mut actual = Rect {
            w: rect.w.max(min_w),
            h: rect.h.max(min_h),
            ..rect
        };
        if screen.shrink_once.remove(&id) {
            actual = Rect {
                w: 300,
                h: 200,
                ..rect
            };
        }
        if let Some(w) = screen.grow_once.remove(&id) {
            actual.w = w;
        }
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
        if overlay.color == "#00bcd4" && overlay.width == 3 {
            screen.overlay = Some(overlay.rect);
            screen.overlay_attach = overlay.attach;
            screen.overlay_fades.push(overlay.fade_in);
            screen.overlay_radius = overlay.radius;
        }
        if overlay.fill.is_some() {
            screen.filled.push((overlay.rect, overlay.fade_in));
        }
        if let (Some(_), Some(id)) = (&overlay.badge, overlay.attach) {
            screen.attached.push(id);
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
    fn show_menu(
        &mut self,
        key: u32,
        items: &[home_feeling_desktop::MenuItem],
        at: Point,
        _: &str,
    ) {
        self.0.lock().unwrap().menus.push((key, items.to_vec(), at));
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

    let manifest = bones_consent::Manifest::read(&things_dir().join("tiler")).unwrap();
    let mut defaults = bones_consent::values(&manifest.settings, &Default::default());
    // Ask a too-large window again at once rather than a second later.
    defaults.insert("reask_ms".into(), serde_json::json!(0));
    let mut lua = LuaHost::new(Logger::default())
        .guests_dir(things_dir())
        .config("tiler", bones_consent::nest(&defaults))
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
    assert_eq!(world.screen.lock().unwrap().hotkeys.len(), 11);
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
    // Above its window, not above every window.
    assert_eq!(world.screen.lock().unwrap().overlay_attach, Some(1));
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
        screen.overlays.values().any(|(_, c)| c == "#ffb300"),
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
fn dropping_in_the_middle_of_a_tile_swaps_when_configured_to() {
    let mut world = world(None, &[1, 2]);
    configure(&mut world, &[("drop_middle", serde_json::json!("swap"))]);
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
    // A new preview fades in quickly; the same one again does not restart it.
    world.event(RawEvent::Dragging(2, Rect { x: 101, ..start }, at(21, 400)));
    let filled = world.screen.lock().unwrap().filled.clone();
    assert_eq!(filled.first(), Some(&(preview, Some(150))));
    assert!(filled[1..].iter().all(|(_, fade)| fade.is_none()));
    world.event(RawEvent::Dragged(2, Rect { x: 100, ..start }, at(20, 400)));
    assert!(!world
        .screen
        .lock()
        .unwrap()
        .overlays
        .values()
        .any(|(r, _)| *r == preview));
}

#[test]
fn the_dragged_window_has_no_focus_frame_until_it_is_dropped() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(2));
    assert!(world.screen.lock().unwrap().overlay.is_some());
    let start = Rect {
        x: 504,
        y: 8,
        w: 488,
        h: 784,
    };
    world.event(RawEvent::DragStarted(2, at(700, 20)));
    world.event(RawEvent::Dragging(2, Rect { x: 100, ..start }, at(20, 400)));
    assert_eq!(world.screen.lock().unwrap().overlay, None);
    world.event(RawEvent::Dragged(2, Rect { x: 100, ..start }, at(20, 400)));
    assert!(world.screen.lock().unwrap().overlay.is_some());
    // It comes back fading in, and later updates leave the fade alone.
    let fades = std::mem::take(&mut world.screen.lock().unwrap().overlay_fades);
    assert_eq!(fades.first(), Some(&Some(1500)));
    world.event(RawEvent::Focused(2));
    let fades = world.screen.lock().unwrap().overlay_fades.clone();
    assert!(!fades.is_empty() && fades.iter().all(Option::is_none));
}

#[test]
fn dragging_any_window_hides_the_focus_frame() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(2));
    assert!(world.screen.lock().unwrap().overlay.is_some());
    world.event(RawEvent::DragStarted(1, at(200, 20)));
    assert_eq!(world.screen.lock().unwrap().overlay, None);
}

#[test]
fn a_setting_from_the_manifest_reaches_the_tiler() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(1));
    assert!(world.screen.lock().unwrap().overlay.is_some());
    // The user turns the active window frame off in the Things window.
    let manifest = bones_consent::Manifest::read(&things_dir().join("tiler")).unwrap();
    let chosen = [("border.enabled".to_string(), serde_json::json!(false))].into();
    let values = bones_consent::values(&manifest.settings, &chosen);
    world
        .control
        .set_config("tiler", bones_consent::nest(&values));
    world.frame();
    world.event(RawEvent::Focused(2));
    assert_eq!(world.screen.lock().unwrap().overlay, None);
}

#[test]
fn a_badge_counts_a_stack_and_its_menu_picks_the_window_to_show() {
    let mut world = world(None, &[1]);
    world.screen.lock().unwrap().minimums.insert(2, (700, 0));
    world.event(RawEvent::Opened(window(2)));
    // The badge is the round overlay in the tile's corner.
    let badge = {
        let screen = world.screen.lock().unwrap();
        let found = screen
            .overlays
            .iter()
            .find(|(_, (r, _))| r.w == 28 && r.h == 28);
        *found.expect("a badge on the stacked tile").0
    };
    world.event(RawEvent::OverlayClicked(badge));
    let (menu, items) = {
        let screen = world.screen.lock().unwrap();
        let (key, items, _) = screen.menus.last().expect("a menu").clone();
        (key, items)
    };
    assert_eq!(items.len(), 2);
    // Window 2 shows; its entry is the checked one.
    assert_eq!(items.iter().filter(|i| i.checked).count(), 1);
    assert!(
        items[1].checked && items[1].label.contains("w2"),
        "{items:?}"
    );
    // Choosing the first entry shows window 1 and hides window 2.
    world.event(RawEvent::Menu(menu, Some(items[0].id)));
    assert_eq!(world.screen.lock().unwrap().minimized, vec![2]);
}

#[test]
fn dismissing_the_menu_changes_nothing() {
    let mut world = world(None, &[1]);
    world.screen.lock().unwrap().minimums.insert(2, (700, 0));
    world.event(RawEvent::Opened(window(2)));
    let badge = {
        let screen = world.screen.lock().unwrap();
        *screen
            .overlays
            .iter()
            .find(|(_, (r, _))| r.w == 28 && r.h == 28)
            .unwrap()
            .0
    };
    world.event(RawEvent::OverlayClicked(badge));
    let menu = world.screen.lock().unwrap().menus.last().unwrap().0;
    world.event(RawEvent::Menu(menu, None));
    assert_eq!(world.screen.lock().unwrap().minimized, vec![1]);
}

fn badge_count(world: &World) -> usize {
    let screen = world.screen.lock().unwrap();
    screen
        .overlays
        .values()
        .filter(|(r, _)| r.w == 28 && r.h == 28)
        .count()
}

#[test]
fn a_badge_belongs_to_the_window_the_stack_shows() {
    let mut world = world(None, &[1]);
    world.screen.lock().unwrap().minimums.insert(2, (700, 0));
    world.event(RawEvent::Opened(window(2)));
    assert_eq!(badge_count(&world), 1);
    assert_eq!(world.screen.lock().unwrap().attached.last(), Some(&2));
}

#[test]
fn a_window_moved_out_of_a_stack_onto_no_tile_floats_and_the_stack_shrinks() {
    let mut world = world(None, &[1]);
    world.screen.lock().unwrap().minimums.insert(2, (700, 0));
    world.event(RawEvent::Opened(window(2)));
    assert_eq!(badge_count(&world), 1);
    let tile = Rect {
        x: 8,
        y: 8,
        w: 984,
        h: 784,
    };
    world.event(RawEvent::DragStarted(2, at(500, 20)));
    world.event(RawEvent::Dragged(
        2,
        Rect { x: 1200, ..tile },
        at(1500, 400),
    ));
    // One window is left in the tile, so the badge goes and window 1 is back.
    assert_eq!(badge_count(&world), 0);
    assert!(world.screen.lock().unwrap().minimized.is_empty());
}

#[test]
fn a_window_moved_out_of_a_stack_onto_the_middle_of_a_tile_joins_that_tile() {
    let mut world = world(None, &[1, 2]);
    world.screen.lock().unwrap().minimums.insert(3, (0, 500));
    world.event(RawEvent::Opened(window(3)));
    let hidden = world.screen.lock().unwrap().minimized.clone();
    assert_eq!(hidden.len(), 1, "3 should have stacked onto a tile");
    let (left, right) = (252, 748);
    let (own_x, other_x, other) = if hidden[0] == 1 {
        (left, right, 2)
    } else {
        (right, left, 1)
    };
    let tile = Rect {
        x: 8,
        y: 8,
        w: 488,
        h: 784,
    };
    world.event(RawEvent::DragStarted(3, at(own_x, 20)));
    world.event(RawEvent::Dragging(
        3,
        Rect { x: 300, ..tile },
        at(other_x, 400),
    ));
    world.event(RawEvent::Dragged(
        3,
        Rect { x: 300, ..tile },
        at(other_x, 400),
    ));
    // 3 now shares the other tile; the window it left is back in its own.
    assert_eq!(world.screen.lock().unwrap().minimized, vec![other]);
    assert_eq!(badge_count(&world), 1);
}

/// Gives the tiler these settings over its defaults, as the Things window does.
fn configure(world: &mut World, chosen: &[(&str, serde_json::Value)]) {
    let manifest = bones_consent::Manifest::read(&things_dir().join("tiler")).unwrap();
    let chosen = chosen
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect();
    let values = bones_consent::values(&manifest.settings, &chosen);
    world
        .control
        .set_config("tiler", bones_consent::nest(&values));
    world.frame();
}

#[test]
fn dropping_in_the_middle_of_a_tile_stacks_even_when_there_is_room() {
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
    // 2 shares 1's tile, which now takes the whole area; 1 waits behind it.
    assert_eq!(world.screen.lock().unwrap().minimized, vec![1]);
    assert_eq!(badge_count(&world), 1);
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
fn a_new_window_can_join_the_focused_tile_when_configured_to() {
    let mut world = world(None, &[1]);
    configure(&mut world, &[("new_window", serde_json::json!("stack"))]);
    world.event(RawEvent::Focused(1));
    world.event(RawEvent::Opened(window(2)));
    assert_eq!(world.screen.lock().unwrap().minimized, vec![1]);
    assert_eq!(badge_count(&world), 1);
}

#[test]
fn a_change_of_monitors_keeps_the_stacks() {
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
    assert_eq!(badge_count(&world), 1);
    world.event(RawEvent::MonitorsChanged(vec![Monitor {
        id: 1,
        rect: WORK,
        work: WORK,
        corner_radius: 0,
    }]));
    assert_eq!(badge_count(&world), 1);
    assert_eq!(world.screen.lock().unwrap().minimized, vec![1]);
}

#[test]
fn a_stacked_window_can_split_its_own_tile() {
    let mut world = world(None, &[1, 2]);
    let start = Rect {
        x: 504,
        y: 8,
        w: 488,
        h: 784,
    };
    // 2 is dropped on the middle of 1's tile: they share it, 2 showing.
    world.event(RawEvent::DragStarted(2, at(700, 20)));
    world.event(RawEvent::Dragging(
        2,
        Rect { x: 100, ..start },
        at(250, 400),
    ));
    world.event(RawEvent::Dragged(2, Rect { x: 100, ..start }, at(250, 400)));
    assert_eq!(badge_count(&world), 1);
    // Dragged to the left edge of the same tile, 2 splits it and takes the left.
    let whole = Rect {
        x: 8,
        y: 8,
        w: 984,
        h: 784,
    };
    world.event(RawEvent::DragStarted(2, at(500, 20)));
    world.event(RawEvent::Dragging(2, Rect { x: 200, ..whole }, at(20, 400)));
    world.event(RawEvent::Dragged(2, Rect { x: 200, ..whole }, at(20, 400)));
    assert_eq!(badge_count(&world), 0);
    assert!(world.screen.lock().unwrap().minimized.is_empty());
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
fn caps_lock_floats_and_tiles_like_the_toggle_key() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(1));
    world.press("capslock");
    // 1 floats, so 2 takes the whole area.
    assert_eq!(
        world.last_moves()[&2],
        Rect {
            x: 8,
            y: 8,
            w: 984,
            h: 784
        }
    );
    world.press("capslock");
    // Pressed again, 1 is back in the layout beside 2.
    assert_eq!(world.last_moves()[&2].w, 488);
}

#[test]
fn a_floating_window_is_moved_freely_and_never_tiled_by_a_drag() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(2));
    world.press("alt+shift+space");
    let moves_before = world.screen.lock().unwrap().moves.len();
    let rect = Rect {
        x: 100,
        y: 100,
        w: 300,
        h: 200,
    };
    world.event(RawEvent::DragStarted(2, at(200, 110)));
    // Over the middle of tile 1: a tiled window would preview a drop here.
    world.event(RawEvent::Dragging(2, rect, at(500, 400)));
    assert!(!world
        .screen
        .lock()
        .unwrap()
        .overlays
        .values()
        .any(|(r, c)| c == "#00bcd4" && r.w > 28 && r.h > 28 && r.h != 0 && r.w == 984));
    world.event(RawEvent::Dragged(2, rect, at(500, 400)));
    // Nothing tiled it: no move was asked of any window.
    assert_eq!(world.screen.lock().unwrap().moves.len(), moves_before);
}

#[test]
fn an_enlarged_window_leaves_its_stack_and_returns_when_restored() {
    let mut world = world(None, &[1, 2]);
    let start = Rect {
        x: 504,
        y: 8,
        w: 488,
        h: 784,
    };
    // 2 is dropped on 1's middle: they share a tile, 2 showing.
    world.event(RawEvent::DragStarted(2, at(700, 20)));
    world.event(RawEvent::Dragging(
        2,
        Rect { x: 100, ..start },
        at(250, 400),
    ));
    world.event(RawEvent::Dragged(2, Rect { x: 100, ..start }, at(250, 400)));
    assert_eq!(badge_count(&world), 1);
    assert_eq!(world.screen.lock().unwrap().minimized, vec![1]);
    // Enlarged, 2 leaves: 1 is back in its tile alone, and the badge goes.
    world.event(RawEvent::Maximized(2, true));
    assert_eq!(badge_count(&world), 0);
    assert!(world.screen.lock().unwrap().minimized.is_empty());
    // Restored, 2 joins 1's stack again.
    world.event(RawEvent::Maximized(2, false));
    assert_eq!(badge_count(&world), 1);
    assert_eq!(world.screen.lock().unwrap().minimized, vec![1]);
}

#[test]
fn a_window_that_ends_smaller_than_its_tile_is_moved_again() {
    let mut world = world(None, &[1]);
    world.screen.lock().unwrap().shrink_once.insert(2);
    world.event(RawEvent::Opened(window(2)));
    let moves_of_2 = world
        .screen
        .lock()
        .unwrap()
        .moves
        .iter()
        .filter(|(id, _)| *id == 2)
        .count();
    assert!(moves_of_2 >= 2, "asked {moves_of_2} time(s)");
    assert_eq!(
        world.last_moves()[&2],
        Rect {
            x: 504,
            y: 8,
            w: 488,
            h: 784
        }
    );
}

#[test]
fn a_window_too_wide_once_gets_its_own_tile_after_all() {
    // As Firefox in a real trace: just opened, it ignored the first move.
    let mut world = world(None, &[1]);
    world.screen.lock().unwrap().grow_once.insert(2, 700);
    world.event(RawEvent::Opened(window(2)));
    assert!(world.screen.lock().unwrap().minimized.is_empty());
    assert_eq!(world.last_moves()[&2].w, 488);
}

#[test]
fn a_window_minimized_and_restored_comes_back_to_its_place() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::Focused(2));
    world.event(RawEvent::Opened(window(3)));
    let before = world.last_moves();
    world.event(RawEvent::Minimized(3));
    // Focus elsewhere: a new window would split the focused tile instead.
    world.event(RawEvent::Focused(1));
    world.event(RawEvent::Opened(window(3)));
    let after = world.last_moves();
    for id in [1, 2, 3] {
        assert_eq!(after[&id], before[&id], "window {id}");
    }
}

#[test]
fn a_stacked_window_minimized_and_restored_rejoins_its_stack() {
    let mut world = world(None, &[1, 2]);
    world.screen.lock().unwrap().minimums.insert(3, (0, 500));
    world.event(RawEvent::Focused(2));
    world.event(RawEvent::Opened(window(3)));
    let shared = world.screen.lock().unwrap().minimized.clone();
    assert_eq!(shared.len(), 1);
    world.event(RawEvent::Minimized(3));
    world.event(RawEvent::Focused(if shared[0] == 1 { 2 } else { 1 }));
    world.event(RawEvent::Opened(window(3)));
    // The same partner is hidden again, not the other tile's window.
    assert_eq!(world.screen.lock().unwrap().minimized, shared);
}

#[test]
fn a_drag_wins_over_a_learned_minimum_the_window_has_outgrown() {
    let mut world = world(None, &[1]);
    world.screen.lock().unwrap().minimums.insert(2, (550, 0));
    world.event(RawEvent::Opened(window(2)));
    assert_eq!(world.last_moves()[&2].w, 550);
    // It could shrink after all, as a window refusing only while restoring.
    world.screen.lock().unwrap().minimums.clear();
    world.event(RawEvent::Dragged(
        1,
        Rect {
            x: 8,
            y: 8,
            w: 560,
            h: 784,
        },
        at(0, 0),
    ));
    assert_eq!(
        world.last_moves()[&2],
        Rect {
            x: 576,
            y: 8,
            w: 416,
            h: 784
        }
    );
}

#[test]
fn a_window_dragged_back_over_its_own_tile_previews_it() {
    let mut world = world(None, &[1, 2]);
    let tile = Rect {
        x: 504,
        y: 8,
        w: 488,
        h: 784,
    };
    world.event(RawEvent::DragStarted(2, at(700, 20)));
    world.event(RawEvent::Dragging(2, Rect { x: 100, ..tile }, at(20, 400)));
    world.screen.lock().unwrap().filled.clear();
    world.event(RawEvent::Dragging(2, Rect { x: 520, ..tile }, at(700, 400)));
    let filled = world.screen.lock().unwrap().filled.clone();
    assert_eq!(filled.last().map(|f| f.0), Some(tile));
    // Dropping there puts it back; nothing is split or stacked.
    world.event(RawEvent::Dragged(2, Rect { x: 520, ..tile }, at(700, 400)));
    assert_eq!(world.last_moves()[&2], tile);
    assert!(world.screen.lock().unwrap().minimized.is_empty());
}

#[test]
fn the_frame_follows_the_system_corner_radius_by_default() {
    let mut world = world(None, &[1, 2]);
    world.event(RawEvent::MonitorsChanged(vec![Monitor {
        id: 1,
        rect: WORK,
        work: WORK,
        corner_radius: 12,
    }]));
    world.event(RawEvent::Focused(1));
    world.event(RawEvent::Focused(2));
    assert_eq!(world.screen.lock().unwrap().overlay_radius, 12);
}
