//! The module's bus behaviour against a recording backend.
use std::sync::{Arc, Mutex};

use bones_kernel_infra::bus::{Bus, Envelope, Module, ModuleContext, Registry, ServiceRegistry};
use bones_kernel_infra::logging::Logger;
use bones_messages::lifecycle::{Event as Lifecycle, LifecycleEvent};
use bones_messages::{EncodeMessage, Message};

use crate::protocol::{Overlay, Rect, Snapshot};
use crate::{Backend, Desktop, Hotkey, RawEvent, QUERY};

#[derive(Default)]
struct Log {
    calls: Vec<String>,
    pending: Vec<RawEvent>,
}

struct Fake(Arc<Mutex<Log>>);

impl Backend for Fake {
    fn snapshot(&mut self) -> Snapshot {
        Snapshot {
            platform: "fake".into(),
            ..Snapshot::default()
        }
    }
    fn poll(&mut self) -> Vec<RawEvent> {
        std::mem::take(&mut self.0.lock().unwrap().pending)
    }
    fn move_window(&mut self, id: i64, rect: Rect, _: bool) -> Result<(), String> {
        self.0.lock().unwrap().calls.push(format!(
            "move {id} {} {} {} {}",
            rect.x, rect.y, rect.w, rect.h
        ));
        Ok(())
    }
    fn focus(&mut self, id: i64) -> Result<(), String> {
        self.0.lock().unwrap().calls.push(format!("focus {id}"));
        Ok(())
    }
    fn register_hotkey(&mut self, key: u32, _: Hotkey) -> Result<(), String> {
        self.0.lock().unwrap().calls.push(format!("register {key}"));
        Ok(())
    }
    fn unregister_hotkey(&mut self, key: u32) {
        self.0
            .lock()
            .unwrap()
            .calls
            .push(format!("unregister {key}"));
    }
    fn set_overlay(&mut self, key: u32, _: &Overlay) {
        self.0.lock().unwrap().calls.push(format!("overlay {key}"));
    }
    fn clear_overlay(&mut self, key: u32) {
        self.0.lock().unwrap().calls.push(format!("clear {key}"));
    }
}

struct World {
    bus: Bus,
    registry: Registry,
    desktop: Desktop,
    log: Arc<Mutex<Log>>,
    heard: Arc<Mutex<Vec<(String, String)>>>,
}

fn world() -> World {
    let bus = Bus::new();
    let registry = Registry::new();
    let log: Arc<Mutex<Log>> = Arc::default();
    let mut desktop = Desktop::new(Logger::default(), Box::new(Fake(log.clone())));
    let mut services = ServiceRegistry::new();
    services.provide(bus.clone()).unwrap();
    services.provide(registry.clone()).unwrap();
    let mut ctx = ModuleContext::new(&mut services);
    desktop.init(&mut ctx).unwrap();
    // Stand in for the kernel: register the module's endpoint and topics.
    let heard: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let sink = heard.clone();
    let listener = bus.register("listener", move |e: &Envelope| {
        sink.lock()
            .unwrap()
            .push((e.topic.clone(), String::from_utf8_lossy(&e.payload).into()));
    });
    listener.subscribe("desktop/windows/opened");
    listener.subscribe("desktop/windows/moved");
    listener.subscribe("desktop/hotkeys/pressed");
    std::mem::forget(listener);
    World {
        bus,
        registry,
        desktop,
        log,
        heard,
    }
}

impl World {
    fn send(&mut self, sender: &str, topic: &str, payload: &[u8]) {
        use bones_kernel_infra::bus::Handler;
        self.desktop.handle(&Envelope {
            topic: topic.into(),
            sender: sender.into(),
            correlation: None,
            payload: payload.to_vec(),
        });
    }

    fn lifecycle(&mut self, name: &str, event: Lifecycle) {
        let payload = LifecycleEvent {
            event,
            extension: name,
        }
        .encode();
        self.send("lua-host", LifecycleEvent::TOPIC, &payload);
    }

    fn frame(&mut self) {
        self.desktop.before_tick();
        self.bus.dispatch();
        self.desktop.after_tick();
    }

    fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().calls.clone()
    }
}

#[test]
fn moves_and_focus_reach_the_backend() {
    let mut world = world();
    world.send(
        "t",
        "desktop/windows/move",
        br#"{"id":7,"rect":{"x":1,"y":2,"w":3,"h":4}}"#,
    );
    world.send("t", "desktop/windows/focus", br#"{"id":7}"#);
    world.frame();
    assert_eq!(world.calls(), vec!["move 7 1 2 3 4", "focus 7"]);
}

#[test]
fn a_hotkey_press_is_published_with_its_owner() {
    let mut world = world();
    world.send(
        "tiler",
        "desktop/hotkeys/register",
        br#"{"id":"left","keys":"alt+left"}"#,
    );
    world.frame();
    world.log.lock().unwrap().pending.push(RawEvent::Hotkey(1));
    world.frame();
    let heard = world.heard.lock().unwrap().clone();
    assert!(
        heard.contains(&(
            "desktop/hotkeys/pressed".into(),
            r#"{"owner":"tiler","id":"left"}"#.into()
        )),
        "{heard:?}"
    );
}

#[test]
fn stopping_a_guest_releases_its_hotkeys_and_overlays() {
    let mut world = world();
    world.send(
        "tiler",
        "desktop/hotkeys/register",
        br#"{"id":"left","keys":"alt+left"}"#,
    );
    world.send(
        "tiler",
        "desktop/overlay/set",
        br#"{"id":"focus","rect":{"x":0,"y":0,"w":9,"h":9}}"#,
    );
    world.send(
        "other",
        "desktop/hotkeys/register",
        br#"{"id":"x","keys":"alt+x"}"#,
    );
    world.lifecycle("tiler", Lifecycle::Stopped);
    world.frame();
    let calls = world.calls();
    assert!(calls.contains(&"unregister 1".to_string()));
    assert!(calls.contains(&"clear 2".to_string()));
    assert!(!calls.contains(&"unregister 3".to_string()));
}

#[test]
fn a_reload_keeps_what_the_replacement_asked_for_again() {
    let mut world = world();
    world.send(
        "tiler",
        "desktop/hotkeys/register",
        br#"{"id":"left","keys":"alt+left"}"#,
    );
    world.send(
        "tiler",
        "desktop/hotkeys/register",
        br#"{"id":"old","keys":"alt+o"}"#,
    );
    world.frame();
    world.lifecycle("tiler", Lifecycle::Reloading);
    world.send(
        "tiler",
        "desktop/hotkeys/register",
        br#"{"id":"left","keys":"alt+left"}"#,
    );
    world.lifecycle("tiler", Lifecycle::Reloaded);
    world.frame();
    let calls = world.calls();
    // "left" was re-registered under a new key; "old" was dropped.
    assert!(calls.contains(&"unregister 2".to_string()), "{calls:?}");
    assert!(calls.contains(&"register 3".to_string()));
    assert!(!calls.contains(&"unregister 3".to_string()));
}

#[test]
fn many_moves_of_one_window_in_a_frame_are_reported_once() {
    let mut world = world();
    for x in 0..5 {
        world.log.lock().unwrap().pending.push(RawEvent::Moved(
            1,
            Rect {
                x,
                y: 0,
                w: 1,
                h: 1,
            },
        ));
    }
    world.frame();
    let moved: Vec<_> = world
        .heard
        .lock()
        .unwrap()
        .iter()
        .filter(|h| h.0 == "desktop/windows/moved")
        .cloned()
        .collect();
    assert_eq!(moved.len(), 1);
    assert!(moved[0].1.contains(r#""x":4"#));
}

#[test]
fn the_query_endpoint_answers_with_a_snapshot() {
    let world = world();
    let reply = world.registry.call("t", QUERY, b"").unwrap();
    let snapshot: Snapshot = serde_json::from_slice(&reply).unwrap();
    assert_eq!(snapshot.platform, "fake");
}

#[test]
fn a_bad_payload_is_logged_not_fatal() {
    let mut world = world();
    world.send("t", "desktop/windows/move", b"not json");
    world.send("t", "desktop/windows/focus", br#"{"id":1}"#);
    world.frame();
    assert_eq!(world.calls(), vec!["focus 1"]);
}
