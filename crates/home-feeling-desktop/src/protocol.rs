//! What crosses the bus, as JSON so Lua Things read and write it directly.
//!
//! The module publishes:
//! - `desktop/windows/opened` [`WindowInfo`]
//! - `desktop/windows/closed`, `focused`, `minimized` `{ "id" }`
//! - `desktop/windows/moved` `{ "id", "rect" }`, after any change of position or size
//! - `desktop/windows/settled` `{ "id", "rect", "requested" }`, the frame a window took
//!   after a requested move, sent whether or not it changed
//! - `desktop/windows/drag-started` `{ "id", "cursor" }`, when the user starts a drag or resize
//! - `desktop/windows/dragging` `{ "id", "rect", "cursor" }`, during it, at most once a frame
//! - `desktop/windows/dragged` `{ "id", "rect", "cursor" }`, when the user ends it
//! - `desktop/windows/maximized` `{ "id", "maximized" }`, on maximize and restore
//! - `desktop/monitors/changed` `{ "monitors": [Monitor] }`
//! - `desktop/hotkeys/pressed` `{ "owner", "id" }`
//! - `desktop/overlay/clicked` `{ "owner", "id" }`, when a badge is clicked
//! - `desktop/overlay/chosen` `{ "owner", "id", "item" }`, when a popup menu closes; `item` is the
//!   chosen entry's id, or null when the user dismissed it
//!
//! It listens on:
//! - `desktop/windows/move` `{ "id", "rect", "raise"?, "delay"? }`; `raise` also brings it to the
//!   front, `delay` waits that many milliseconds first
//! - `desktop/windows/minimize` `{ "id" }`, `desktop/windows/restore` `{ "id" }`, neither
//!   taking focus, `desktop/windows/focus` `{ "id" }`
//! - `desktop/windows/trace` `{ "on" }`; the module records what it sees and does to its trace
//!   file while any guest has it on
//! - `desktop/hotkeys/register` `{ "id", "keys" }`, `desktop/hotkeys/unregister` `{ "id" }`
//! - `desktop/overlay/set` `{ "id", "rect", "color", "width", "fill"?, "radius"?, "badge"?, "attach"?, "fade_in"? }`; `fill` is the opacity
//!   0 to 1 of the colour inside the border, `desktop/overlay/clear` `{ "id" }`,
//!   `desktop/overlay/menu` `{ "id", "items": [{ "id", "label", "checked"?, "window"? }], "at": Point,
//!   "accent"? }`, which
//!   opens a popup menu there
//!
//! And answers a direct send to `desktop/windows/query` with a [`Snapshot`].
use serde::{Deserialize, Serialize};

/// A window or monitor id. Stable while the window exists.
pub type Id = i64;

/// Screen coordinates of the visible frame, without invisible borders.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A screen position in real pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: Id,
    pub title: String,
    pub class: String,
    pub rect: Rect,
    pub monitor: Id,
    pub minimized: bool,
    #[serde(default)]
    pub maximized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Monitor {
    pub id: Id,
    /// The whole monitor.
    pub rect: Rect,
    /// The part not covered by taskbars.
    pub work: Rect,
    /// How round the system draws window corners here, in pixels: 8 scaled
    /// to the monitor on Windows 11, 0 where corners are square.
    #[serde(default)]
    pub corner_radius: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub windows: bool,
    pub hotkeys: bool,
    pub overlays: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub capabilities: Capabilities,
    pub platform: String,
    pub windows: Vec<WindowInfo>,
    pub monitors: Vec<Monitor>,
    pub focused: Option<Id>,
}

/// A border, optionally with a translucent fill, drawn above every window,
/// taking no input.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Overlay {
    pub rect: Rect,
    /// `#rrggbb`.
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default = "default_width")]
    pub width: i32,
    /// Opacity of a fill inside the border, 0 to 1; none or 0 draws no fill.
    #[serde(default)]
    pub fill: Option<f32>,
    /// Corner radius in pixels of the border's inner edge; 0 is square.
    #[serde(default)]
    pub radius: i32,
    /// Draws a filled circle in the rect with this text in it, which takes
    /// clicks and reports them as `desktop/overlay/clicked`; the border
    /// fields are ignored.
    #[serde(default)]
    pub badge: Option<String>,
    /// The window the overlay belongs to. The overlay sits just above that
    /// window in the stacking order, so windows in front of it cover the
    /// overlay too, instead of it floating over everything.
    #[serde(default)]
    pub attach: Option<Id>,
    /// A border only: fades it in from transparent over this many
    /// milliseconds. Later updates without it leave a running fade alone.
    #[serde(default)]
    pub fade_in: Option<u32>,
}

fn default_color() -> String {
    "#00bcd4".into()
}

fn default_width() -> i32 {
    3
}

impl Overlay {
    /// The colour as red, green, blue; unreadable colours fall back to the default.
    pub fn rgb(&self) -> (u8, u8, u8) {
        let hex = self.color.trim_start_matches('#');
        let channel = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
        match (channel(0), channel(2), channel(4)) {
            (Some(r), Some(g), Some(b)) if hex.len() == 6 => (r, g, b),
            _ => (0x00, 0xbc, 0xd4),
        }
    }
}

/// One entry of a popup menu.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MenuItem {
    pub id: u32,
    pub label: String,
    #[serde(default)]
    pub checked: bool,
    /// The window the entry stands for, whose icon and program it shows.
    #[serde(default)]
    pub window: Option<Id>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MenuRequest {
    pub id: String,
    pub items: Vec<MenuItem>,
    pub at: Point,
    /// `#rrggbb` marking the checked entry and the one under the pointer.
    #[serde(default)]
    pub accent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IdOnly {
    pub id: Id,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MoveRequest {
    pub id: Id,
    pub rect: Rect,
    #[serde(default)]
    pub raise: bool,
    /// Milliseconds to wait before moving it.
    #[serde(default)]
    pub delay: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HotkeyRequest {
    pub id: String,
    #[serde(default)]
    pub keys: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OverlayRequest {
    pub id: String,
    #[serde(flatten)]
    pub overlay: Option<Overlay>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TraceRequest {
    pub on: bool,
}
