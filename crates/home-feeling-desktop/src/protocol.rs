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
//!
//! It listens on:
//! - `desktop/windows/move` `{ "id", "rect", "raise"? }`; `raise` also brings it to the front
//! - `desktop/windows/minimize` `{ "id" }`, `desktop/windows/restore` `{ "id" }`, neither
//!   taking focus, `desktop/windows/focus` `{ "id" }`
//! - `desktop/hotkeys/register` `{ "id", "keys" }`, `desktop/hotkeys/unregister` `{ "id" }`
//! - `desktop/overlay/set` `{ "id", "rect", "color", "width" }`, `desktop/overlay/clear` `{ "id" }`
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

/// A border drawn above every window, taking no input.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Overlay {
    pub rect: Rect,
    /// `#rrggbb`.
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default = "default_width")]
    pub width: i32,
}

fn default_color() -> String {
    "#2f6f5e".into()
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
            _ => (0x2f, 0x6f, 0x5e),
        }
    }
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
