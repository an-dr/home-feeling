//! The Things window (UC-THINGS-01 to 05): every Thing with its state, an
//! enable switch, the consent prompt when one waits, and the selected Thing's
//! last error and log. Each frame it describes itself as a bones widget tree.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bones_consent::{Consent, Kind, Manifest, PromptAnswer, Setting, Status};
use bones_kernel::exit::ExitRequest;
use bones_kernel::messages::ui::kinds::{
    Button, Checkbox, Fit, Heading, Image, Label, Select, Selectable, Separator, Slider, Space,
    Strong, TextEdit, Weak,
};
use bones_kernel::messages::ui::{Event, Layout, Node, PanelSide, Root, Spec, WidgetProps};
use bones_kernel::messages::upgrader::Action;
use bones_kernel::messages::Message;
use bones_kernel::ui::{Frame, UiView};
use bones_kernel::window_control::WindowControl;
use bones_module_lua::{GuestState, LuaControl, Permissions};
use serde_json::Value;

use crate::autostart;
use crate::background::Background;
use crate::color;
use crate::installation::{self, UpdateState};
use crate::settings::{AppSettings, Settings};
use crate::tray::{Tray, TrayAction};

/// The module name, under which bones shows this window's trees.
pub const NAME: &str = "home-feeling";

const RELOAD: u32 = 1;
const OPEN_FOLDER: u32 = 2;
const CHECK_UPDATE: u32 = 3;
const STAGE_UPDATE: u32 = 4;
const INSTALL: u32 = 5;
const UNREGISTER: u32 = 6;
const AUTOSTART: u32 = 7;
const START_MINIMIZED: u32 = 8;
/// Each Thing's row owns ten ids from here: select, allow, refuse, enable.
const ROWS: u32 = 1000;
const SELECT: u32 = 0;
const PROMPT: u32 = 0; // the prompt's buttons are +1 (allow) and +2 (refuse)
const ENABLE: u32 = 3;
/// Each Thing's settings own `THING_IDS` ids from here. Each setting takes
/// `SLOT_IDS` of them for its controls (a colour needs a field, three
/// sliders, and its preset swatches); the section tabs and the reset button
/// sit after the last slot.
const SETTINGS: u32 = 100_000;
const THING_IDS: u32 = 8000;
const SLOT_IDS: u32 = 16;
const MAX_SLOTS: usize = 490;
const SECTION_TAB: u32 = 7900;
const RESET: u32 = 7999;
/// The Overview and Settings tabs are in each Thing row's own ids.
const TAB_OVERVIEW: u32 = 4;
const TAB_SETTINGS: u32 = 5;
const SWATCH: f32 = 22.0;
/// The section a setting without one belongs to.
const GENERAL: &str = "General";
/// A changed setting reaches its Thing after this long without another
/// change, so dragging a slider reloads the Thing once, not on every frame.
const SETTLE: Duration = Duration::from_millis(400);

/// The channels to the update bridge, which talks to the bones upgrade module.
pub struct Updates {
    pub requests: std::sync::mpsc::Sender<Action>,
    pub results: std::sync::mpsc::Receiver<UpdateState>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Settings,
}

pub struct ThingsWindow {
    updates: Updates,
    update: Option<UpdateState>,
    control: LuaControl,
    consent: Consent,
    settings: Settings,
    things_dir: PathBuf,
    /// Each Thing's manifest as last read, or why it could not be read.
    manifests: HashMap<String, Result<Manifest, String>>,
    /// The deny-list last given to the host per Thing.
    denied: HashMap<String, Vec<String>>,
    /// The Thing names in row order at the last frame.
    rows: Vec<String>,
    selected: Option<String>,
    /// Keeps the window alive in the tray when it is closed.
    background: Background,
    /// Which tab each Thing shows.
    tabs: HashMap<String, Tab>,
    /// Which section of settings each Thing shows.
    sections: HashMap<String, String>,
    /// The last hue chosen per colour, which grey colours cannot tell.
    hues: HashMap<String, f32>,
    /// Colour swatches uploaded as images, by handle; the wanted ones are
    /// those this frame's tree shows.
    swatches: HashSet<u32>,
    swatches_wanted: HashSet<u32>,
    /// The hotkey setting waiting for the user to press its keys: Thing and key.
    recording: Option<(String, String)>,
    /// Text the user is typing into a setting, by `thing/key`, until it is reset.
    drafts: HashMap<String, String>,
    /// Why the text in a setting was refused, by `thing/key`.
    refused: HashMap<String, String>,
    /// The Thing whose settings changed and when, until they are applied.
    unapplied: Option<(String, Instant)>,
    /// Built-in files the user edited that have a newer version beside them.
    offered: Vec<String>,
    notice: Option<String>,
    tray: Option<Tray>,
    window: Option<WindowControl>,
    exit: Option<ExitRequest>,
}

impl ThingsWindow {
    pub fn new(
        control: LuaControl,
        consent: Consent,
        settings: Settings,
        things_dir: PathBuf,
        offered: Vec<String>,
        denied: HashMap<String, Vec<String>>,
        updates: Updates,
    ) -> Self {
        Self {
            updates,
            update: None,
            control,
            consent,
            settings,
            things_dir,
            manifests: HashMap::new(),
            denied,
            rows: Vec::new(),
            selected: None,
            recording: None,
            background: Background::new(false),
            tabs: HashMap::new(),
            sections: HashMap::new(),
            hues: HashMap::new(),
            swatches: HashSet::new(),
            swatches_wanted: HashSet::new(),
            drafts: HashMap::new(),
            refused: HashMap::new(),
            unapplied: None,
            offered,
            notice: None,
            tray: None,
            window: None,
            exit: None,
        }
    }

    /// Hides the window as soon as it exists: the app was started for the tray.
    pub fn start_in_tray(mut self, hidden: bool) -> Self {
        self.background = Background::new(hidden);
        self
    }

    fn manifest(&mut self, thing: &str) -> Result<Manifest, String> {
        let dir = self.things_dir.join(thing);
        self.manifests
            .entry(thing.to_string())
            .or_insert_with(|| Manifest::read(&dir))
            .clone()
    }

    fn pending(&mut self, thing: &str) -> bool {
        match self.manifest(thing) {
            Ok(manifest) => self.consent.status(thing, &manifest) == Status::Pending,
            Err(_) => false,
        }
    }

    /// Re-reads every manifest, holds back Things whose request changed, and
    /// reloads the rest.
    fn reload_all(&mut self) {
        self.manifests.clear();
        self.unapplied = None;
        for record in self.control.records() {
            let name = record.name;
            if let Ok(manifest) = self.manifest(&name) {
                // A manifest edit may have changed what the defaults are.
                self.send_config(&name, &manifest);
                let deny = self.consent.deny_list(&name, &manifest);
                if self.denied.get(&name) != Some(&deny) {
                    self.denied.insert(name.clone(), deny.clone());
                    self.control.set_permissions(&name, Permissions::deny(deny));
                }
                if self.consent.status(&name, &manifest) == Status::Pending {
                    self.control.stop(&name);
                }
            }
        }
        self.control.reload_all();
        self.notice = Some("Reloaded all Things.".into());
    }

    /// Gives the Thing its settings: the declared defaults under what the user chose.
    fn send_config(&self, thing: &str, manifest: &Manifest) {
        let values = bones_consent::values(&manifest.settings, &self.settings.chosen(thing));
        self.control.set_config(thing, bones_consent::nest(&values));
    }

    /// Applies settings that changed and then stayed still.
    fn apply_settled(&mut self) {
        let Some((thing, since)) = &self.unapplied else {
            return;
        };
        if since.elapsed() < SETTLE {
            return;
        }
        let thing = thing.clone();
        self.unapplied = None;
        if let Ok(manifest) = self.manifest(&thing) {
            self.send_config(&thing, &manifest);
        }
    }

    /// The Thing an id of its settings belongs to, its declarations, and the
    /// id's offset within the Thing's range.
    fn setting_of(&mut self, id: u32) -> Option<(String, Vec<Setting>, u32)> {
        let relative = id.checked_sub(SETTINGS)?;
        let thing = self.rows.get((relative / THING_IDS) as usize)?.clone();
        let specs = self.manifest(&thing).ok()?.settings;
        Some((thing, specs, relative % THING_IDS))
    }

    fn setting_event(&mut self, event: &Event) {
        let id = match event {
            Event::Toggled { id, .. }
            | Event::ValueChanged { id, .. }
            | Event::Selected { id, .. }
            | Event::Changed { id, .. }
            | Event::Clicked { id } => *id,
            _ => return,
        };
        let Some((thing, specs, offset)) = self.setting_of(id) else {
            return;
        };
        if offset == RESET {
            if let Err(err) = self.settings.reset(&thing) {
                self.notice = Some(format!("Could not save settings: {err}"));
            }
            let prefix = format!("{thing}/");
            self.drafts.retain(|key, _| !key.starts_with(&prefix));
            self.refused.retain(|key, _| !key.starts_with(&prefix));
            self.unapplied = Some((thing, Instant::now()));
            return;
        }
        if offset >= SECTION_TAB {
            if let Some(name) = section_names(&specs).get((offset - SECTION_TAB) as usize) {
                self.sections.insert(thing, name.clone());
            }
            return;
        }
        let (slot, part) = ((offset / SLOT_IDS) as usize, offset % SLOT_IDS);
        let Some(setting) = specs.get(slot) else {
            return;
        };
        if setting.kind == Kind::Keys && cfg!(windows) {
            // Hotkeys are recorded from the keyboard: a click starts listening,
            // the clear button unbinds.
            match (part, event) {
                (0, Event::Clicked { .. }) => self.recording = Some((thing, setting.key.clone())),
                (1, Event::Clicked { .. }) => {
                    self.commit(thing, setting, &specs, serde_json::json!(""))
                }
                _ => {}
            }
            return;
        }
        let draft_key = format!("{thing}/{}", setting.key);
        let proposed = if setting.kind == Kind::Color && part > 0 {
            // A slider or a preset swatch: the colour changes, so the text
            // the user typed no longer says what it is.
            let current = bones_consent::values(&specs, &self.settings.chosen(&thing));
            let rgb = current
                .get(&setting.key)
                .and_then(Value::as_str)
                .and_then(color::parse)
                .unwrap_or([0; 3]);
            let (hue, saturation, value) = self.hsv(&draft_key, rgb);
            let rgb = match (part, event) {
                (1, Event::ValueChanged { value: v, .. }) => {
                    self.hues.insert(draft_key.clone(), *v);
                    color::from_hsv(*v, saturation, value)
                }
                (2, Event::ValueChanged { value: v, .. }) => {
                    color::from_hsv(hue, *v / 100.0, value)
                }
                (3, Event::ValueChanged { value: v, .. }) => {
                    color::from_hsv(hue, saturation, *v / 100.0)
                }
                (4.., Event::Clicked { .. }) => {
                    match color::PRESETS
                        .get(part as usize - 4)
                        .and_then(|p| color::parse(p))
                    {
                        Some(rgb) => rgb,
                        None => return,
                    }
                }
                _ => return,
            };
            self.drafts.remove(&draft_key);
            serde_json::json!(color::hex(rgb))
        } else {
            match event {
                Event::Toggled { on, .. } => serde_json::json!(on),
                Event::ValueChanged { value, .. } => {
                    serde_json::json!((f64::from(*value) * 100.0).round() / 100.0)
                }
                Event::Selected { index, .. } => match setting.options.get(*index as usize) {
                    Some(option) => serde_json::json!(option),
                    None => return,
                },
                Event::Changed { text, .. } => {
                    self.drafts.insert(draft_key.clone(), text.clone());
                    serde_json::json!(text)
                }
                _ => return,
            }
        };
        self.commit(thing, setting, &specs, proposed);
    }

    /// Stores `proposed` as the value of `setting` when it is valid, and says
    /// why on the setting when it is not.
    fn commit(&mut self, thing: String, setting: &Setting, specs: &[Setting], proposed: Value) {
        let draft_key = format!("{thing}/{}", setting.key);
        let checked = setting.accept(&proposed).and_then(|value| match &value {
            Value::String(text) if setting.kind == Kind::Keys && !text.is_empty() => {
                home_feeling_desktop::Hotkey::parse(text)?;
                let current = bones_consent::values(specs, &self.settings.chosen(&thing));
                let taken = specs.iter().find(|other| {
                    other.kind == Kind::Keys
                        && other.key != setting.key
                        && current.get(&other.key) == Some(&value)
                });
                match taken {
                    Some(other) => Err(format!("Already used for {}", other.title())),
                    None => Ok(value.clone()),
                }
            }
            _ => Ok(value),
        });
        match checked {
            Ok(value) => {
                self.refused.remove(&draft_key);
                match self.settings.choose(&thing, &setting.key, value) {
                    Ok(()) => self.unapplied = Some((thing, Instant::now())),
                    Err(err) => self.notice = Some(format!("Could not save settings: {err}")),
                }
            }
            Err(why) => {
                self.refused.insert(draft_key, why);
            }
        }
    }

    /// While a hotkey setting listens, reads the keyboard once per frame.
    fn poll_recording(&mut self) {
        use home_feeling_desktop::Capture;
        let Some((thing, key)) = self.recording.clone() else {
            return;
        };
        let text = match home_feeling_desktop::capture_hotkey() {
            Capture::Waiting => return,
            Capture::Combo(text) => text,
            Capture::Cleared => String::new(),
            Capture::Cancelled | Capture::Unsupported => {
                self.recording = None;
                return;
            }
        };
        self.recording = None;
        if let Ok(manifest) = self.manifest(&thing) {
            if let Some(setting) = manifest.settings.iter().find(|s| s.key == key) {
                self.commit(thing, setting, &manifest.settings, serde_json::json!(text));
            }
        }
    }

    /// Hue, saturation, and brightness of a colour; a grey keeps the hue the
    /// user last chose, which its red, green, and blue cannot tell.
    fn hsv(&self, key: &str, rgb: [u8; 3]) -> (f32, f32, f32) {
        let (hue, saturation, value) = color::to_hsv(rgb);
        if saturation < 0.01 || value < 0.01 {
            (
                self.hues.get(key).copied().unwrap_or(hue),
                saturation,
                value,
            )
        } else {
            (hue, saturation, value)
        }
    }

    /// A square of one colour, shown by an uploaded image. Clicking it reports
    /// `Clicked` with `id` unless that is 0.
    fn swatch(&mut self, rgb: [u8; 3], size: f32, id: u32) -> Node {
        let handle = ((u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2])) + 1;
        self.swatches_wanted.insert(handle);
        Image {
            handle,
            width: size,
            height: size,
            fit: Fit::Fill,
            corner: 5.0,
        }
        .node(id)
    }

    /// Uploads the swatches the tree now shows and drops those it stopped showing.
    fn sync_swatches(&mut self, frame: &mut Frame<'_>) {
        for handle in self.swatches_wanted.difference(&self.swatches) {
            let rgb = handle - 1;
            let pixel = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255];
            frame.set_image(*handle, 2, 2, pixel.repeat(4));
        }
        for handle in self.swatches.difference(&self.swatches_wanted) {
            frame.free_image(*handle);
        }
        self.swatches = std::mem::take(&mut self.swatches_wanted);
    }

    /// The controls of one setting, in a card.
    fn setting_card(&mut self, thing: &str, id: u32, setting: &Setting, value: &Value) -> Node {
        let draft_key = format!("{thing}/{}", setting.key);
        let title = || {
            Strong {
                text: setting.title().into(),
            }
            .node(0)
        };
        let mut card = Vec::new();
        match setting.kind {
            Kind::Bool => card.push(
                Checkbox {
                    label: setting.title().into(),
                    checked: value.as_bool().unwrap_or(false),
                }
                .node(id),
            ),
            Kind::Int | Kind::Number => {
                let number = value.as_f64().unwrap_or(0.0);
                let shown = if number.fract() == 0.0 {
                    format!("{number}")
                } else {
                    format!("{number:.2}")
                };
                let (min, max) = setting.range();
                card.push(
                    Strong {
                        text: format!("{} · {shown}", setting.title()),
                    }
                    .node(0),
                );
                card.push(
                    Slider {
                        value: number as f32,
                        min: min as f32,
                        max: max as f32,
                    }
                    .node(id),
                );
            }
            Kind::Choice => {
                card.push(title());
                card.push(
                    Select {
                        selected: setting
                            .options
                            .iter()
                            .position(|o| Some(o.as_str()) == value.as_str())
                            .map(|i| i as u32),
                        placeholder: "Choose".into(),
                        options: setting.options.clone(),
                    }
                    .node(id),
                );
            }
            Kind::Keys if cfg!(windows) => {
                let text = value.as_str().unwrap_or_default();
                let listening = self
                    .recording
                    .as_ref()
                    .is_some_and(|(t, k)| t == thing && *k == setting.key);
                let label = if listening {
                    "Press the keys…".to_string()
                } else if text.is_empty() {
                    "Not set · click to record".to_string()
                } else {
                    pretty_keys(text)
                };
                card.push(title());
                card.push(Node::container(
                    Layout::Horizontal,
                    vec![
                        Button::new(label).node(id),
                        Button::new("Clear").disabled(text.is_empty()).node(id + 1),
                    ],
                ));
                if listening {
                    card.push(
                        Weak {
                            text:
                                "Hold the modifiers and press a key. Esc cancels, Delete unbinds."
                                    .into(),
                        }
                        .node(0),
                    );
                }
            }
            Kind::String | Kind::Keys => {
                let text = self
                    .drafts
                    .get(&draft_key)
                    .cloned()
                    .unwrap_or_else(|| value.as_str().unwrap_or_default().to_string());
                card.push(title());
                card.push(TextEdit::new(text).node(id));
            }
            Kind::Color => {
                let rgb = value.as_str().and_then(color::parse).unwrap_or([0; 3]);
                let text = self
                    .drafts
                    .get(&draft_key)
                    .cloned()
                    .unwrap_or_else(|| value.as_str().unwrap_or_default().to_string());
                let (hue, saturation, brightness) = self.hsv(&draft_key, rgb);
                card.push(title());
                card.push(Node::container(
                    Layout::Horizontal,
                    vec![
                        self.swatch(rgb, 2.0 * SWATCH, 0),
                        TextEdit::new(text).node(id),
                    ],
                ));
                let presets = color::PRESETS
                    .iter()
                    .enumerate()
                    .map(|(i, preset)| {
                        let rgb = color::parse(preset).unwrap_or([0; 3]);
                        self.swatch(rgb, SWATCH, id + 4 + i as u32)
                    })
                    .collect();
                card.push(Node::container(Layout::Wrap, presets));
                for (part, name, shown, now, max) in [
                    (1, "Hue", format!("{hue:.0}°"), hue, 360.0),
                    (
                        2,
                        "Saturation",
                        format!("{:.0}%", saturation * 100.0),
                        saturation * 100.0,
                        100.0,
                    ),
                    (
                        3,
                        "Brightness",
                        format!("{:.0}%", brightness * 100.0),
                        brightness * 100.0,
                        100.0,
                    ),
                ] {
                    card.push(
                        Weak {
                            text: format!("{name} {shown}"),
                        }
                        .node(0),
                    );
                    card.push(
                        Slider {
                            value: now,
                            min: 0.0,
                            max,
                        }
                        .node(id + part),
                    );
                }
            }
        }
        if let Some(why) = self.refused.get(&draft_key) {
            card.push(Strong { text: why.clone() }.node(0));
        }
        if !setting.description.is_empty() {
            card.push(
                Weak {
                    text: setting.description.clone(),
                }
                .node(0),
            );
        }
        Node::container(Layout::Frame, card)
    }

    /// The Settings tab: a sub-tab for each section, and the selected
    /// section's settings, one card each.
    fn settings_section(&mut self, thing: &str, index: u32, specs: &[Setting]) -> Vec<Node> {
        let sections = section_names(specs);
        let selected = self
            .sections
            .get(thing)
            .filter(|name| sections.contains(name))
            .or(sections.first())
            .cloned()
            .unwrap_or_default();
        let base = SETTINGS + index * THING_IDS;
        let mut nodes = Vec::new();
        if sections.len() > 1 {
            let tabs = sections
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    Selectable {
                        label: name.clone(),
                        selected: *name == selected,
                    }
                    .node(base + SECTION_TAB + i as u32)
                })
                .collect();
            nodes.push(Node::container(Layout::Wrap, tabs));
            nodes.push(Space { amount: 4.0 }.node(0));
        }
        let current = bones_consent::values(specs, &self.settings.chosen(thing));
        for (slot, setting) in specs.iter().enumerate().take(MAX_SLOTS) {
            if section_of(setting) != selected {
                continue;
            }
            let value = current[&setting.key].clone();
            nodes.push(self.setting_card(thing, base + slot as u32 * SLOT_IDS, setting, &value));
        }
        nodes.push(Space { amount: 4.0 }.node(0));
        nodes.push(Button::new("Reset all settings to defaults").node(base + RESET));
        nodes
    }

    fn answer(&mut self, thing: &str, answer: PromptAnswer) {
        let Ok(manifest) = self.manifest(thing) else {
            return;
        };
        let granted = match answer {
            PromptAnswer::Allow => manifest.requests.clone(),
            PromptAnswer::Refuse => Vec::new(),
        };
        if let Err(err) = self.consent.answer(thing, &manifest, &granted) {
            self.notice = Some(format!("Could not save the answer: {err}"));
            return;
        }
        let deny = self.consent.deny_list(thing, &manifest);
        self.control.set_permissions(thing, Permissions::deny(deny));
        if self.settings.enabled(thing) {
            self.control.start(thing);
        }
    }

    fn set_enabled(&mut self, thing: &str, on: bool) {
        if let Err(err) = self.settings.set_enabled(thing, on) {
            self.notice = Some(format!("Could not save settings: {err}"));
        }
        if !on {
            self.control.stop(thing);
        } else if !self.pending(thing) {
            self.control.start(thing);
        }
    }

    fn handle_tray(&mut self) {
        let Some(tray) = &self.tray else { return };
        for action in tray.actions() {
            match action {
                TrayAction::Show => {
                    self.background.show();
                    if let Some(window) = &self.window {
                        let _ = window.raise();
                    }
                }
                TrayAction::ReloadAll => self.reload_all(),
                TrayAction::Quit => {
                    self.background.allow_close();
                    if let Some(exit) = &self.exit {
                        exit.request();
                    }
                }
            }
        }
    }

    fn state_text(&mut self, thing: &str, state: GuestState) -> String {
        if !self.settings.enabled(thing) {
            return "off".into();
        }
        if self.pending(thing) {
            return "waiting for permission".into();
        }
        match state {
            GuestState::Running => "running".into(),
            GuestState::Faulted => "faulted".into(),
            GuestState::Stopped => "stopped".into(),
        }
    }

    fn list_panel(&mut self) -> Node {
        let records = self.control.records();
        self.rows = records.iter().map(|r| r.name.clone()).collect();
        if self.selected.is_none() {
            self.selected = self.rows.first().cloned();
        }
        let mut children = vec![
            Heading {
                text: "Things".into(),
            }
            .node(0),
            Button::new("Reload all").node(RELOAD),
            Button::new("Open Things folder").node(OPEN_FOLDER),
            Separator.node(0),
        ];
        for (index, record) in records.iter().enumerate() {
            let base = ROWS + index as u32 * 10;
            let state = self.state_text(&record.name, record.state);
            children.push(
                Selectable {
                    label: format!("{} · {state}", record.name),
                    selected: self.selected.as_deref() == Some(record.name.as_str()),
                }
                .node(base + SELECT),
            );
        }
        if records.is_empty() {
            children.push(
                Weak {
                    text: "No Things in the folder.".into(),
                }
                .node(0),
            );
        }
        children.push(Separator.node(0));
        children.extend(self.app_section());
        Node::container(
            Layout::Panel {
                side: PanelSide::Left,
            },
            vec![Node::container(Layout::Vertical, children)],
        )
    }

    fn detail_panel(&mut self) -> Node {
        let mut children = Vec::new();
        if let Some(notice) = &self.notice {
            children.push(
                Weak {
                    text: notice.clone(),
                }
                .node(0),
            );
        }
        for file in &self.offered {
            children.push(
                Weak {
                    text: format!("A newer built-in {file} is beside yours as {file}.new."),
                }
                .node(0),
            );
        }
        let selected = self.selected.clone();
        let record = selected
            .as_ref()
            .and_then(|name| self.control.records().into_iter().find(|r| &r.name == name));
        if let Some(record) = record {
            let index = self
                .rows
                .iter()
                .position(|n| n == &record.name)
                .unwrap_or(0) as u32;
            let base = ROWS + index * 10;
            children.push(
                Heading {
                    text: record.name.clone(),
                }
                .node(0),
            );
            let mut specs = Vec::new();
            match self.manifest(&record.name) {
                Ok(manifest) => {
                    specs = manifest.settings.clone();
                    if !manifest.description.is_empty() {
                        children.push(
                            Label {
                                text: manifest.description.clone(),
                            }
                            .node(0),
                        );
                    }
                    let unknown = self.consent.unknown(&manifest);
                    if !unknown.is_empty() {
                        children.push(
                            Weak {
                                text: format!(
                                    "Requests unknown capabilities: {}",
                                    unknown.join(", ")
                                ),
                            }
                            .node(0),
                        );
                    }
                    if self.consent.status(&record.name, &manifest) == Status::Pending {
                        children.push(self.consent.prompt(&record.name, &manifest, base + PROMPT));
                    }
                }
                Err(err) => children.push(Label { text: err }.node(0)),
            }
            let tab = match self.tabs.get(&record.name) {
                Some(Tab::Settings) if !specs.is_empty() => Tab::Settings,
                _ => Tab::Overview,
            };
            if !specs.is_empty() {
                children.push(Node::container(
                    Layout::Horizontal,
                    vec![
                        Selectable {
                            label: "Overview".into(),
                            selected: tab == Tab::Overview,
                        }
                        .node(base + TAB_OVERVIEW),
                        Selectable {
                            label: "Settings".into(),
                            selected: tab == Tab::Settings,
                        }
                        .node(base + TAB_SETTINGS),
                    ],
                ));
            }
            children.push(Separator.node(0));
            match tab {
                Tab::Settings => {
                    children.extend(self.settings_section(&record.name, index, &specs))
                }
                Tab::Overview => {
                    children.push(
                        Checkbox {
                            label: "Enabled".into(),
                            checked: self.settings.enabled(&record.name),
                        }
                        .node(base + ENABLE),
                    );
                    if let Some(error) = &record.last_error {
                        children.push(
                            Strong {
                                text: "Last error".into(),
                            }
                            .node(0),
                        );
                        children.push(
                            Label {
                                text: error.clone(),
                            }
                            .node(0),
                        );
                    }
                    children.push(Strong { text: "Log".into() }.node(0));
                    for line in record.log.iter().rev().take(100) {
                        children.push(
                            Label {
                                text: format!("[{}] {}", line.level, line.text),
                            }
                            .node(0),
                        );
                    }
                }
            }
        }
        // The whole page scrolls: the settings alone outgrow the window.
        Node::container(
            Layout::Panel {
                side: PanelSide::Central,
            },
            vec![Node::container(
                Layout::Scroll,
                vec![Node::container(Layout::Vertical, children)],
            )],
        )
    }
}

impl ThingsWindow {
    /// Version, updates, and installation (UC-INSTALL, UC-UPDATE).
    fn app_section(&mut self) -> Vec<Node> {
        while let Ok(state) = self.updates.results.try_recv() {
            if state == UpdateState::Installed {
                self.notice = Some(match installation::register() {
                    Ok(()) => "Installed. Start home-feeling from your application list.".into(),
                    Err(err) => format!("Installed, but not added to the application list: {err}"),
                });
            }
            self.update = Some(state);
        }
        let mut nodes = vec![Weak {
            text: format!("{} {}", installation::NAME, installation::VERSION),
        }
        .node(0)];
        if cfg!(windows) {
            let app = self.settings.app();
            nodes.push(
                Checkbox {
                    label: "Start with Windows".into(),
                    checked: app.autostart,
                }
                .node(AUTOSTART),
            );
            nodes.push(
                Checkbox {
                    label: "Start in the tray".into(),
                    checked: app.start_minimized,
                }
                .node(START_MINIMIZED),
            );
        }
        if !installation::running_installed() {
            nodes.push(Button::new("Install").node(INSTALL));
            return nodes;
        }
        match &self.update {
            Some(UpdateState::Available(version)) => {
                nodes.push(Button::new(format!("Update to {version}")).node(STAGE_UPDATE))
            }
            Some(UpdateState::Staged(version)) => nodes.push(
                Label {
                    text: format!("{version} is ready; restart the app to use it."),
                }
                .node(0),
            ),
            Some(UpdateState::UpToDate) => {
                nodes.push(
                    Weak {
                        text: "Up to date.".into(),
                    }
                    .node(0),
                );
                nodes.push(Button::new("Check for updates").node(CHECK_UPDATE));
            }
            Some(UpdateState::Failed(reason)) => {
                nodes.push(
                    Label {
                        text: format!("Update failed: {reason}"),
                    }
                    .node(0),
                );
                nodes.push(Button::new("Check for updates").node(CHECK_UPDATE));
            }
            _ => nodes.push(Button::new("Check for updates").node(CHECK_UPDATE)),
        }
        if installation::is_registered() {
            nodes.push(Button::new("Unregister").node(UNREGISTER));
        }
        nodes
    }

    /// Starting with the session, and starting in the tray.
    fn set_startup(&mut self, id: u32, on: bool) {
        let mut app = self.settings.app();
        if id == AUTOSTART {
            app.autostart = on;
        } else {
            app.start_minimized = on;
        }
        self.notice = match autostart::set(app.autostart, app.start_minimized) {
            Ok(()) => self
                .settings
                .set_app(AppSettings { ..app })
                .err()
                .map(|err| format!("Could not save settings: {err}")),
            Err(err) => Some(format!("Could not change the startup entry: {err}")),
        };
    }

    fn app_action(&mut self, id: u32) {
        match id {
            CHECK_UPDATE => {
                self.update = None;
                let _ = self.updates.requests.send(Action::Check);
            }
            STAGE_UPDATE => {
                let _ = self.updates.requests.send(Action::Stage);
            }
            INSTALL => {
                self.notice = Some("Installing...".into());
                let _ = self.updates.requests.send(Action::Install);
            }
            UNREGISTER => {
                self.notice = Some(match installation::unregister() {
                    Ok(()) => format!(
                        "Removed from the application list. Delete {} to finish; your Things folder stays.",
                        installation::install_dir().unwrap_or_default().display()
                    ),
                    Err(err) => format!("Could not unregister: {err}"),
                });
            }
            _ => {}
        }
    }
}

/// `alt+shift+left` as `Alt + Shift + Left`.
fn pretty_keys(text: &str) -> String {
    text.split('+')
        .map(|part| {
            if part == "capslock" {
                return "Caps Lock".to_string();
            }
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" + ")
}

/// The section a setting is listed under.
fn section_of(setting: &Setting) -> &str {
    if setting.section.is_empty() {
        GENERAL
    } else {
        &setting.section
    }
}

/// The sections of a Thing's settings, in the order they first appear.
fn section_names(specs: &[Setting]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for setting in specs {
        if !names.iter().any(|name| name == section_of(setting)) {
            names.push(section_of(setting).to_string());
        }
    }
    names
}

impl UiView for ThingsWindow {
    fn init(&mut self, ctx: &mut bones_kernel::bus::ModuleContext) -> Result<(), String> {
        ctx.subscribe(bones_kernel::messages::instance::Launched::TOPIC);
        self.window = ctx.get_service::<WindowControl>().cloned();
        self.exit = ctx.get_service::<ExitRequest>().cloned();
        self.tray = Tray::new();
        Ok(())
    }

    fn on_message(&mut self, envelope: &bones_kernel::bus::Envelope) {
        // A later launch handed over to this copy: bring the window back,
        // unless it was only the session starting the app again.
        if envelope.topic == bones_kernel::messages::instance::Launched::TOPIC {
            use bones_kernel::messages::DecodeMessage;
            let quiet = bones_kernel::messages::instance::Launched::decode(&envelope.payload)
                .is_ok_and(|launch| launch.args.contains(&"--minimized"));
            if !quiet {
                self.background.show();
            }
        }
    }

    fn on_event(&mut self, event: Event) {
        let id = match &event {
            Event::Clicked { id }
            | Event::Toggled { id, .. }
            | Event::ValueChanged { id, .. }
            | Event::Selected { id, .. }
            | Event::Changed { id, .. } => *id,
            _ => return,
        };
        match id {
            id if id >= SETTINGS => self.setting_event(&event),
            RELOAD => self.reload_all(),
            OPEN_FOLDER => {
                let _ = std::fs::create_dir_all(&self.things_dir);
                if let Err(err) = open::that(&self.things_dir) {
                    self.notice = Some(format!("Could not open the folder: {err}"));
                }
            }
            CHECK_UPDATE | STAGE_UPDATE | INSTALL | UNREGISTER => self.app_action(id),
            AUTOSTART | START_MINIMIZED => {
                if let Event::Toggled { on, .. } = event {
                    self.set_startup(id, on);
                }
            }
            id if id >= ROWS => {
                let index = ((id - ROWS) / 10) as usize;
                let Some(thing) = self.rows.get(index).cloned() else {
                    return;
                };
                let base = ROWS + index as u32 * 10;
                if let Some(answer) = PromptAnswer::from_event(base + PROMPT, &event) {
                    self.answer(&thing, answer);
                } else if id == base + ENABLE {
                    if let Event::Toggled { on, .. } = event {
                        self.set_enabled(&thing, on);
                    }
                } else if id == base + SELECT {
                    self.selected = Some(thing);
                } else if id == base + TAB_OVERVIEW {
                    self.tabs.insert(thing, Tab::Overview);
                } else if id == base + TAB_SETTINGS {
                    self.tabs.insert(thing, Tab::Settings);
                }
            }
            _ => {}
        }
    }

    fn view(&mut self, frame: &mut Frame<'_>) -> Spec {
        self.background.tick();
        self.handle_tray();
        self.poll_recording();
        self.apply_settled();
        let list = self.list_panel();
        let detail = self.detail_panel();
        self.sync_swatches(frame);
        Spec {
            root: Root::Screen,
            body: Node::container(Layout::Vertical, vec![list, detail]),
        }
    }
}
