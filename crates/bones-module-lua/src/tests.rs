//! Headless: a bus, a registry, and the host, driven frame by frame.
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bones_kernel_infra::bus::{Bus, Envelope, Module, ModuleContext, Registry, ServiceRegistry};
use bones_kernel_infra::logging::Logger;

use crate::{GuestState, LuaControl, LuaHost, Permissions};

/// What the recorder heard: topic, sender, payload.
type Heard = Arc<Mutex<Vec<(String, String, Vec<u8>)>>>;

struct World {
    bus: Bus,
    registry: Registry,
    host: LuaHost,
    control: LuaControl,
    heard: Heard,
    _dir: tempfile::TempDir,
}

fn write(dir: &Path, name: &str, file: &str, source: &str) {
    std::fs::create_dir_all(dir.join(name)).unwrap();
    std::fs::write(dir.join(name).join(file), source).unwrap();
}

fn world(guests: &[(&str, &str)], configure: impl FnOnce(LuaHost) -> LuaHost) -> World {
    let dir = tempfile::tempdir().unwrap();
    for (name, source) in guests {
        write(dir.path(), name, "init.lua", source);
    }
    let bus = Bus::new();
    let registry = Registry::new();
    let heard: Heard = Arc::default();
    let sink = heard.clone();
    let recorder = bus.register("recorder", move |envelope: &Envelope| {
        sink.lock().unwrap().push((
            envelope.topic.clone(),
            envelope.sender.clone(),
            envelope.payload.clone(),
        ));
    });
    recorder.subscribe("test/*");
    std::mem::forget(recorder);
    let mut host = configure(
        LuaHost::new(Logger::default())
            .guests_dir(dir.path())
            .budgets(Duration::from_millis(200), Duration::from_millis(50)),
    );
    let control = host.control();
    let mut services = ServiceRegistry::new();
    services.provide(bus.clone()).unwrap();
    services.provide(registry.clone()).unwrap();
    host.init(&mut ModuleContext::new(&mut services)).unwrap();
    World {
        bus,
        registry,
        host,
        control,
        heard,
        _dir: dir,
    }
}

impl World {
    fn frame(&mut self) {
        self.bus.begin_frame();
        self.host.before_tick();
        self.bus.dispatch();
        self.host.after_tick();
        self.bus.dispatch();
    }

    fn state(&self, name: &str) -> (GuestState, Option<String>) {
        let record = self
            .control
            .records()
            .into_iter()
            .find(|record| record.name == name)
            .unwrap();
        (record.state, record.last_error)
    }

    fn heard_topics(&self) -> Vec<String> {
        self.heard
            .lock()
            .unwrap()
            .iter()
            .map(|h| h.0.clone())
            .collect()
    }

    fn dir(&self) -> &Path {
        self._dir.path()
    }
}

#[test]
fn init_publishes_and_config_is_read() {
    let mut world = world(&[], |host| host);
    write(world.dir(), "a", "config.lua", "return { gap = 8 }");
    write(
        world.dir(),
        "a",
        "init.lua",
        "function init() bones.publish('test/gap', tostring(config.gap)) end",
    );
    world.control.reload_all();
    world.control.start("a");
    world.frame();
    world.frame();
    let heard = world.heard.lock().unwrap().clone();
    assert!(
        heard.contains(&("test/gap".into(), "a".into(), b"8".to_vec())),
        "{heard:?}"
    );
    assert_eq!(world.state("a").0, GuestState::Running);
}

#[test]
fn settings_overlay_config_and_keep_what_they_do_not_name() {
    let mut world = world(&[], |host| {
        host.config("a", serde_json::json!({ "border": { "width": 5 } }))
    });
    write(
        world.dir(),
        "a",
        "config.lua",
        "return { gap = 8, border = { width = 1, color = 'x' } }",
    );
    write(
        world.dir(),
        "a",
        "init.lua",
        "function init() bones.publish('test/c', config.gap .. config.border.width .. config.border.color) end",
    );
    world.control.reload_all();
    world.control.start("a");
    world.frame();
    world.frame();
    let heard = world.heard.lock().unwrap().clone();
    assert!(
        heard.contains(&("test/c".into(), "a".into(), b"85x".to_vec())),
        "{heard:?}"
    );
}

#[test]
fn on_message_reply_answers_a_direct_send() {
    let world = world(
        &[(
            "echo",
            "function on_message(t, s, p) return 'got ' .. p end",
        )],
        |h| h,
    );
    let reply = world.registry.call("tester", "echo", b"hi").unwrap();
    assert_eq!(reply, b"got hi");
}

#[test]
fn tables_are_published_as_json() {
    let mut world = world(
        &[(
            "j",
            "function init() bones.publish('test/j', { x = 1 }) end",
        )],
        |h| h,
    );
    world.frame();
    let heard = world.heard.lock().unwrap().clone();
    assert_eq!(heard[0].2, br#"{"x":1}"#.to_vec());
}

#[test]
fn a_runaway_loop_is_quarantined_and_others_keep_running() {
    let mut world = world(
        &[
            ("loop", "function init() bones.subscribe('test/go') end function on_message() while true do end end"),
            ("calm", "function on_message(t, s, p) return 'ok' end"),
        ],
        |h| h,
    );
    world.bus.publish(Envelope {
        topic: "test/go".into(),
        sender: "tester".into(),
        correlation: None,
        payload: Vec::new(),
    });
    world.frame();
    let (state, error) = world.state("loop");
    assert_eq!(state, GuestState::Faulted);
    assert!(error.unwrap().contains("time budget exceeded"));
    assert_eq!(world.registry.call("tester", "calm", b"").unwrap(), b"ok");
}

#[test]
fn a_failed_reload_keeps_the_running_instance_and_names_the_line() {
    let mut world = world(&[("t", "function on_message() return 'v1' end")], |h| h);
    write(
        world.dir(),
        "t",
        "init.lua",
        "function on_message()\n  return 'v2' +\nend",
    );
    world.control.reload_all();
    world.frame();
    let (state, error) = world.state("t");
    assert_eq!(state, GuestState::Running);
    assert!(
        error.as_deref().unwrap_or("").contains("t/init.lua:3"),
        "{error:?}"
    );
    assert_eq!(world.registry.call("tester", "t", b"").unwrap(), b"v1");

    write(
        world.dir(),
        "t",
        "init.lua",
        "function on_message() return 'v2' end",
    );
    world.control.reload_all();
    world.frame();
    assert_eq!(world.state("t"), (GuestState::Running, None));
    assert_eq!(world.registry.call("tester", "t", b"").unwrap(), b"v2");
}

#[test]
fn a_runtime_error_names_the_file_and_line() {
    let mut world = world(&[("e", "function init() bones.subscribe('test/go') end\nfunction on_message()\n  error('boom')\nend")], |h| h);
    world.bus.publish(Envelope {
        topic: "test/go".into(),
        sender: "tester".into(),
        correlation: None,
        payload: Vec::new(),
    });
    world.frame();
    let (state, error) = world.state("e");
    assert_eq!(state, GuestState::Faulted);
    assert!(error.unwrap().contains("e/init.lua:3: boom"));
}

#[test]
fn denied_topics_and_endpoints_are_refused() {
    let world_source = "function init()
        bones.publish('test/secret', 'x')
        bones.publish('test/open', 'y')
        local reply, err = bones.send('echo', 'z')
        bones.publish('test/send', err or 'sent')
    end";
    let mut world = world(
        &[
            ("echo", "function on_message() return 'e' end"),
            ("z", world_source),
        ],
        |host| host.permissions("z", Permissions::deny(["test/secret", "echo"])),
    );
    world.frame();
    let heard = world.heard.lock().unwrap().clone();
    let topics: Vec<&str> = heard.iter().map(|h| h.0.as_str()).collect();
    assert!(!topics.contains(&"test/secret"));
    assert!(topics.contains(&"test/open"));
    let send = heard.iter().find(|h| h.0 == "test/send").unwrap();
    assert_eq!(send.2, b"unknown-endpoint");
}

#[test]
fn stop_runs_shutdown_and_releases_the_endpoint() {
    let mut world = world(
        &[("s", "function shutdown() bones.publish('test/bye', '') end")],
        |h| h,
    );
    world.control.stop("s");
    world.frame();
    assert!(world.heard_topics().contains(&"test/bye".to_string()));
    assert!(!world.registry.contains("s"));
    assert_eq!(world.state("s").0, GuestState::Stopped);
}

#[test]
fn startup_list_holds_other_guests_until_started() {
    let mut world = world(
        &[
            ("a", "function on_message() return 'a' end"),
            ("b", "function on_message() return 'b' end"),
        ],
        |host| host.startup(["a".to_string()]),
    );
    assert!(!world.registry.contains("b"));
    world.control.start("b");
    world.frame();
    assert_eq!(world.registry.call("tester", "b", b"").unwrap(), b"b");
}

#[test]
fn json_helpers_round_trip() {
    let source = "function on_message(t, s, p)
        local v = bones.json.decode(p)
        return bones.json.encode({ doubled = v.n * 2 })
    end";
    let world = world(&[("j", source)], |h| h);
    let reply = world.registry.call("tester", "j", br#"{"n":21}"#).unwrap();
    assert_eq!(reply, br#"{"doubled":42}"#);
}

#[test]
fn whole_numbers_are_written_as_integers() {
    let source =
        "function on_message() return bones.json.encode({ w = 400, h = 2.5, list = { 1, 2 } }) end";
    let world = world(&[("n", source)], |h| h);
    let reply = String::from_utf8(world.registry.call("tester", "n", b"").unwrap()).unwrap();
    assert!(
        reply.contains(r#""w":400"#) && reply.contains(r#""h":2.5"#) && reply.contains("[1,2]"),
        "{reply}"
    );
}
