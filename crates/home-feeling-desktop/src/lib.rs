//! A native bones module that gives the bus mechanisms over the desktop:
//! windows, monitors, global hotkeys, and overlays (ADR-004, ADR-005). Policy,
//! such as how to tile, lives in the Things that use it.
//!
//! Hotkeys and overlays belong to the guest that asked for them and are
//! released when it stops, faults, or reloads without asking for them again.
mod keys;
pub mod protocol;
mod trace;

#[cfg(windows)]
mod windows;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use bones_kernel_infra::bus::{Bus, Envelope, Handler, Module, ModuleContext, Registry, Respond};
use bones_kernel_infra::logging::Logger;
use bones_messages::lifecycle::{Event as Lifecycle, LifecycleEvent};
use bones_messages::{DecodeMessage, Message};
use serde::Serialize;

pub use keys::{Hotkey, Key, Modifiers};
pub use protocol::MenuItem;
use protocol::{
    Capabilities, HotkeyRequest, Id, IdOnly, MenuRequest, Monitor, MoveRequest, Overlay,
    OverlayRequest, Point, Rect, Snapshot, TraceRequest, WindowInfo,
};
pub use trace::Trace;

/// The module's endpoint.
const ENDPOINT: &str = "desktop";
/// The direct-send endpoint answering with a [`Snapshot`]; named under the
/// windows topics so the same deny-list pattern covers it.
pub const QUERY: &str = "desktop/windows/query";

/// What the operating system reported since the last poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawEvent {
    Opened(WindowInfo),
    Closed(Id),
    Focused(Id),
    Moved(Id, Rect),
    /// The frame a window settled at after the move to `requested`.
    Settled(Id, Rect, Rect),
    /// The user started dragging or resizing the window.
    DragStarted(Id, Point),
    /// The window's frame and the cursor during a drag or resize.
    Dragging(Id, Rect, Point),
    /// The user ended a drag or resize of the window.
    Dragged(Id, Rect, Point),
    Maximized(Id, bool),
    Minimized(Id),
    MonitorsChanged(Vec<Monitor>),
    Hotkey(u32),
    /// The overlay with this key, a badge, was clicked.
    OverlayClicked(u32),
    /// The popup menu with this key closed: the chosen item, or none.
    Menu(u32, Option<u32>),
}

/// One platform's mechanisms. Methods report failures as text the module logs.
pub trait Backend: Send {
    fn snapshot(&mut self) -> Snapshot;
    fn poll(&mut self) -> Vec<RawEvent>;
    /// Moves a window; `raise` also brings it in front of other windows,
    /// without focusing it.
    fn move_window(&mut self, id: Id, rect: Rect, raise: bool) -> Result<(), String>;
    fn focus(&mut self, id: Id) -> Result<(), String>;
    /// Minimizes a window without moving focus.
    fn minimize(&mut self, _id: Id) -> Result<(), String> {
        Err("minimizing is not supported on this platform".into())
    }
    /// Restores a minimized window without focusing it.
    fn restore(&mut self, _id: Id) -> Result<(), String> {
        Err("restoring is not supported on this platform".into())
    }
    fn register_hotkey(&mut self, key: u32, hotkey: Hotkey) -> Result<(), String>;
    fn unregister_hotkey(&mut self, key: u32);
    fn set_overlay(&mut self, key: u32, overlay: &Overlay);
    fn clear_overlay(&mut self, key: u32);
    /// Opens a popup menu at a screen position; its outcome comes back as
    /// [`RawEvent::Menu`] with the same key.
    fn show_menu(&mut self, _key: u32, _items: &[MenuItem], _at: Point, _accent: &str) {}
}

/// The backend for platforms without desktop control in v1.
pub struct Unsupported;

impl Backend for Unsupported {
    fn snapshot(&mut self) -> Snapshot {
        Snapshot {
            capabilities: Capabilities::default(),
            platform: std::env::consts::OS.to_string(),
            ..Snapshot::default()
        }
    }
    fn poll(&mut self) -> Vec<RawEvent> {
        Vec::new()
    }
    fn move_window(&mut self, _: Id, _: Rect, _: bool) -> Result<(), String> {
        Err("window management is not supported on this platform".into())
    }
    fn focus(&mut self, _: Id) -> Result<(), String> {
        Err("window management is not supported on this platform".into())
    }
    fn register_hotkey(&mut self, _: u32, _: Hotkey) -> Result<(), String> {
        Err("global hotkeys are not supported on this platform".into())
    }
    fn unregister_hotkey(&mut self, _: u32) {}
    fn set_overlay(&mut self, _: u32, _: &Overlay) {}
    fn clear_overlay(&mut self, _: u32) {}
}

/// What the keyboard says while the user is asked to press a hotkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capture {
    /// Nothing usable is held yet.
    Waiting,
    /// The user pressed Escape.
    Cancelled,
    /// The user pressed Delete or Backspace: no hotkey.
    Cleared,
    /// The hotkey held, in the text form `Hotkey::parse` reads.
    Combo(String),
    /// This platform cannot read the keyboard this way.
    Unsupported,
}

/// Reads the keys held right now; call it each frame while asking for a hotkey.
pub fn capture_hotkey() -> Capture {
    #[cfg(windows)]
    {
        windows::capture()
    }
    #[cfg(not(windows))]
    {
        Capture::Unsupported
    }
}

/// The backend for the platform this build runs on.
pub fn native_backend() -> Box<dyn Backend> {
    #[cfg(windows)]
    {
        Box::new(windows::Windows::start())
    }
    #[cfg(not(windows))]
    {
        Box::new(Unsupported)
    }
}

/// A hotkey or overlay a guest owns: `(owner, its own id)`.
type Owned = (String, String);

struct State {
    backend: Box<dyn Backend>,
    hotkeys: HashMap<Owned, u32>,
    overlays: HashMap<Owned, u32>,
    /// The popup menus opened, by key.
    menus: HashMap<u32, Owned>,
    next_key: u32,
    /// For a guest that is reloading: what its replacement asked for so far.
    renewed: HashMap<String, HashSet<Owned>>,
    inbox: Vec<Envelope>,
    /// The guests that asked for the window trace.
    tracers: HashSet<String>,
    /// Moves asked for with a delay, and when each is due.
    delayed: Vec<(std::time::Instant, MoveRequest)>,
}

impl State {
    fn key(&mut self) -> u32 {
        self.next_key += 1;
        self.next_key
    }

    fn renew(&mut self, owned: &Owned) {
        if let Some(renewed) = self.renewed.get_mut(&owned.0) {
            renewed.insert(owned.clone());
        }
    }

    fn release(&mut self, owner: &str, keep: Option<&HashSet<Owned>>) {
        if keep.is_none() {
            self.tracers.remove(owner);
        }
        let gone =
            |owned: &Owned| owned.0 == owner && !keep.is_some_and(|keep| keep.contains(owned));
        let hotkeys: Vec<(Owned, u32)> = self
            .hotkeys
            .iter()
            .filter(|(o, _)| gone(o))
            .map(|(o, k)| (o.clone(), *k))
            .collect();
        for (owned, key) in hotkeys {
            self.backend.unregister_hotkey(key);
            self.hotkeys.remove(&owned);
        }
        let overlays: Vec<(Owned, u32)> = self
            .overlays
            .iter()
            .filter(|(o, _)| gone(o))
            .map(|(o, k)| (o.clone(), *k))
            .collect();
        for (owned, key) in overlays {
            self.backend.clear_overlay(key);
            self.overlays.remove(&owned);
        }
    }
}

#[derive(Clone)]
struct Shared(Arc<Mutex<State>>);

impl Respond for Shared {
    fn respond(&self, _sender: &str, _payload: &[u8]) -> Option<Vec<u8>> {
        let snapshot = self.0.lock().unwrap().backend.snapshot();
        serde_json::to_vec(&snapshot).ok()
    }
}

/// The desktop module. Register it before the guest host, so guests can
/// query it during their own `init`.
pub struct Desktop {
    logger: Logger,
    shared: Shared,
    bus: Option<Bus>,
    trace: Trace,
}

impl Desktop {
    pub fn new(logger: Logger, backend: Box<dyn Backend>) -> Self {
        Self {
            logger,
            shared: Shared(Arc::new(Mutex::new(State {
                backend,
                hotkeys: HashMap::new(),
                overlays: HashMap::new(),
                menus: HashMap::new(),
                next_key: 0,
                renewed: HashMap::new(),
                inbox: Vec::new(),
                tracers: HashSet::new(),
                delayed: Vec::new(),
            }))),
            bus: None,
            trace: Trace::off(),
        }
    }

    /// Records what the module sees and does to `trace` while a guest asks
    /// for it on `desktop/windows/trace`.
    pub fn trace(mut self, trace: Trace) -> Self {
        self.trace = trace;
        self
    }

    fn publish(&self, topic: &str, value: &impl Serialize) {
        if let (Some(bus), Ok(payload)) = (&self.bus, serde_json::to_vec(value)) {
            self.trace.message("out", topic, None, &payload);
            bus.publish(Envelope {
                topic: topic.to_string(),
                sender: ENDPOINT.to_string(),
                correlation: None,
                payload,
            });
        }
    }

    fn apply(&self, state: &mut State, envelope: &Envelope) -> Result<(), String> {
        let owner = envelope.sender.clone();
        let payload = &envelope.payload;
        let parse_error =
            |err: serde_json::Error| format!("bad payload on {}: {err}", envelope.topic);
        match envelope.topic.as_str() {
            LifecycleEvent::TOPIC => {
                let event = LifecycleEvent::decode(payload).map_err(|err| err.to_string())?;
                let name = event.extension.to_string();
                match event.event {
                    Lifecycle::Reloading => {
                        state.renewed.insert(name, HashSet::new());
                    }
                    Lifecycle::Reloaded => {
                        let keep = state.renewed.remove(&name).unwrap_or_default();
                        state.release(&name, Some(&keep));
                    }
                    Lifecycle::Stopped | Lifecycle::Faulted => {
                        state.renewed.remove(&name);
                        state.release(&name, None);
                    }
                    Lifecycle::Loaded => {}
                }
            }
            "desktop/windows/move" => {
                let request: MoveRequest = serde_json::from_slice(payload).map_err(parse_error)?;
                // A newer move of the window replaces one still waiting.
                state
                    .delayed
                    .retain(|(_, waiting)| waiting.id != request.id);
                if request.delay > 0 {
                    let due = std::time::Instant::now()
                        + std::time::Duration::from_millis(request.delay.into());
                    state.delayed.push((due, request));
                } else {
                    state
                        .backend
                        .move_window(request.id, request.rect, request.raise)?;
                }
            }
            "desktop/windows/trace" => {
                let request: TraceRequest = serde_json::from_slice(payload).map_err(parse_error)?;
                if request.on {
                    state.tracers.insert(owner);
                } else {
                    state.tracers.remove(&owner);
                }
            }
            "desktop/windows/focus" => {
                let request: IdOnly = serde_json::from_slice(payload).map_err(parse_error)?;
                state.backend.focus(request.id)?;
            }
            "desktop/windows/minimize" => {
                let request: IdOnly = serde_json::from_slice(payload).map_err(parse_error)?;
                state.backend.minimize(request.id)?;
            }
            "desktop/windows/restore" => {
                let request: IdOnly = serde_json::from_slice(payload).map_err(parse_error)?;
                state.backend.restore(request.id)?;
            }
            "desktop/hotkeys/register" => {
                let request: HotkeyRequest =
                    serde_json::from_slice(payload).map_err(parse_error)?;
                let hotkey = Hotkey::parse(&request.keys)?;
                let owned = (owner, request.id);
                if let Some(old) = state.hotkeys.remove(&owned) {
                    state.backend.unregister_hotkey(old);
                }
                let key = state.key();
                state
                    .backend
                    .register_hotkey(key, hotkey)
                    .map_err(|err| format!("{}: {err}", request.keys))?;
                state.renew(&owned);
                state.hotkeys.insert(owned, key);
            }
            "desktop/hotkeys/unregister" => {
                let request: HotkeyRequest =
                    serde_json::from_slice(payload).map_err(parse_error)?;
                if let Some(key) = state.hotkeys.remove(&(owner, request.id)) {
                    state.backend.unregister_hotkey(key);
                }
            }
            "desktop/overlay/set" => {
                let request: OverlayRequest =
                    serde_json::from_slice(payload).map_err(parse_error)?;
                let overlay = request.overlay.ok_or("overlay/set needs a rect")?;
                let owned = (owner, request.id);
                let key = match state.overlays.get(&owned) {
                    Some(key) => *key,
                    None => state.key(),
                };
                state.backend.set_overlay(key, &overlay);
                state.renew(&owned);
                state.overlays.insert(owned, key);
            }
            "desktop/overlay/menu" => {
                let request: MenuRequest = serde_json::from_slice(payload).map_err(parse_error)?;
                let key = state.key();
                state.menus.insert(key, (owner, request.id));
                let accent = request.accent.as_deref().unwrap_or("#00bcd4");
                state
                    .backend
                    .show_menu(key, &request.items, request.at, accent);
            }
            "desktop/overlay/clear" => {
                let request: OverlayRequest =
                    serde_json::from_slice(payload).map_err(parse_error)?;
                if let Some(key) = state.overlays.remove(&(owner, request.id)) {
                    state.backend.clear_overlay(key);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn translate(&self, state: &State, event: RawEvent) {
        #[derive(Serialize)]
        struct IdEvent {
            id: Id,
        }
        #[derive(Serialize)]
        struct MovedEvent {
            id: Id,
            rect: Rect,
        }
        #[derive(Serialize)]
        struct DragEvent {
            id: Id,
            #[serde(skip_serializing_if = "Option::is_none")]
            rect: Option<Rect>,
            cursor: Point,
        }
        #[derive(Serialize)]
        struct SettledEvent {
            id: Id,
            rect: Rect,
            requested: Rect,
        }
        #[derive(Serialize)]
        struct MaximizedEvent {
            id: Id,
            maximized: bool,
        }
        #[derive(Serialize)]
        struct MonitorsEvent {
            monitors: Vec<Monitor>,
        }
        #[derive(Serialize)]
        struct HotkeyEvent<'a> {
            owner: &'a str,
            id: &'a str,
        }
        #[derive(Serialize)]
        struct ChosenEvent<'a> {
            owner: &'a str,
            id: &'a str,
            item: Option<u32>,
        }
        match event {
            RawEvent::Opened(info) => self.publish("desktop/windows/opened", &info),
            RawEvent::Closed(id) => self.publish("desktop/windows/closed", &IdEvent { id }),
            RawEvent::Focused(id) => self.publish("desktop/windows/focused", &IdEvent { id }),
            RawEvent::Minimized(id) => self.publish("desktop/windows/minimized", &IdEvent { id }),
            RawEvent::Settled(id, rect, requested) => self.publish(
                "desktop/windows/settled",
                &SettledEvent {
                    id,
                    rect,
                    requested,
                },
            ),
            RawEvent::DragStarted(id, cursor) => self.publish(
                "desktop/windows/drag-started",
                &DragEvent {
                    id,
                    rect: None,
                    cursor,
                },
            ),
            RawEvent::Dragging(id, rect, cursor) => self.publish(
                "desktop/windows/dragging",
                &DragEvent {
                    id,
                    rect: Some(rect),
                    cursor,
                },
            ),
            RawEvent::Dragged(id, rect, cursor) => self.publish(
                "desktop/windows/dragged",
                &DragEvent {
                    id,
                    rect: Some(rect),
                    cursor,
                },
            ),
            RawEvent::Maximized(id, maximized) => self.publish(
                "desktop/windows/maximized",
                &MaximizedEvent { id, maximized },
            ),
            RawEvent::Moved(id, rect) => {
                self.publish("desktop/windows/moved", &MovedEvent { id, rect })
            }
            RawEvent::MonitorsChanged(monitors) => {
                self.publish("desktop/monitors/changed", &MonitorsEvent { monitors })
            }
            RawEvent::Hotkey(key) => {
                if let Some(((owner, id), _)) = state.hotkeys.iter().find(|(_, k)| **k == key) {
                    self.publish("desktop/hotkeys/pressed", &HotkeyEvent { owner, id });
                }
            }
            RawEvent::OverlayClicked(key) => {
                if let Some(((owner, id), _)) = state.overlays.iter().find(|(_, k)| **k == key) {
                    self.publish("desktop/overlay/clicked", &HotkeyEvent { owner, id });
                }
            }
            RawEvent::Menu(key, item) => {
                if let Some((owner, id)) = state.menus.get(&key) {
                    self.publish("desktop/overlay/chosen", &ChosenEvent { owner, id, item });
                }
            }
        }
    }
}

impl Handler for Desktop {
    fn handle(&mut self, envelope: &Envelope) {
        // Applied after the tick, outside any guest's call.
        self.shared.0.lock().unwrap().inbox.push(envelope.clone());
    }
}

impl Module for Desktop {
    fn name(&self) -> &str {
        ENDPOINT
    }

    fn init(&mut self, ctx: &mut ModuleContext) -> Result<(), String> {
        self.bus = Some(
            ctx.get_service::<Bus>()
                .cloned()
                .ok_or("desktop needs the bus service")?,
        );
        let registry = ctx
            .get_service::<Registry>()
            .cloned()
            .ok_or("desktop needs the registry service")?;
        registry.insert(QUERY, Arc::new(self.shared.clone()));
        for topic in [
            "desktop/windows/move",
            "desktop/windows/focus",
            "desktop/windows/trace",
            "desktop/windows/minimize",
            "desktop/windows/restore",
            "desktop/hotkeys/*",
            "desktop/overlay/*",
            LifecycleEvent::TOPIC,
        ] {
            ctx.subscribe(topic);
        }
        Ok(())
    }

    fn before_tick(&mut self) {
        let shared = self.shared.clone();
        let mut state = shared.0.lock().unwrap();
        if self.trace.snapshot_due() {
            let snapshot = state.backend.snapshot();
            self.trace
                .record(serde_json::json!({ "kind": "snapshot", "snapshot": snapshot }));
        }
        let now = std::time::Instant::now();
        let (due, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut state.delayed)
            .into_iter()
            .partition(|(at, _)| *at <= now);
        state.delayed = waiting;
        for (_, request) in due {
            if let Err(err) = state
                .backend
                .move_window(request.id, request.rect, request.raise)
            {
                self.logger.warn(ENDPOINT, &format!("delayed move: {err}"));
            }
        }
        let events = state.backend.poll();
        // A window that moves many times in a frame is reported once.
        let mut moved: HashMap<Id, Rect> = HashMap::new();
        let mut dragging: HashMap<Id, (Rect, Point)> = HashMap::new();
        for event in events {
            match event {
                RawEvent::Moved(id, rect) => {
                    moved.insert(id, rect);
                }
                RawEvent::Dragging(id, rect, cursor) => {
                    dragging.insert(id, (rect, cursor));
                }
                // The end of a drag comes after its last position.
                RawEvent::Dragged(id, rect, cursor) => {
                    if let Some((rect, cursor)) = dragging.remove(&id) {
                        self.translate(&state, RawEvent::Dragging(id, rect, cursor));
                    }
                    self.translate(&state, RawEvent::Dragged(id, rect, cursor));
                }
                other => self.translate(&state, other),
            }
        }
        for (id, rect) in moved {
            self.translate(&state, RawEvent::Moved(id, rect));
        }
        for (id, (rect, cursor)) in dragging {
            self.translate(&state, RawEvent::Dragging(id, rect, cursor));
        }
    }

    fn after_tick(&mut self) {
        let shared = self.shared.clone();
        let mut state = shared.0.lock().unwrap();
        let inbox = std::mem::take(&mut state.inbox);
        for envelope in inbox {
            self.trace.message(
                "in",
                &envelope.topic,
                Some(&envelope.sender),
                &envelope.payload,
            );
            if let Err(err) = self.apply(&mut state, &envelope) {
                self.trace.record(serde_json::json!({
                    "kind": "error",
                    "topic": envelope.topic,
                    "sender": envelope.sender,
                    "error": err,
                }));
                self.logger.warn(
                    ENDPOINT,
                    &format!("{} from '{}': {err}", envelope.topic, envelope.sender),
                );
            }
            // Started or stopped by a request, or by its guest stopping.
            let wanted = !state.tracers.is_empty();
            if wanted != self.trace.enabled() {
                if let Err(err) = self.trace.set_enabled(wanted) {
                    self.logger.warn(ENDPOINT, &err);
                    state.tracers.clear();
                }
            }
        }
    }

    fn shutdown(&mut self) {
        let mut state = self.shared.0.lock().unwrap();
        let owners: HashSet<String> = state
            .hotkeys
            .keys()
            .chain(state.overlays.keys())
            .map(|o| o.0.clone())
            .collect();
        for owner in owners {
            state.release(&owner, None);
        }
    }
}

#[cfg(test)]
mod tests;
