//! The Things window (UC-THINGS-01 to 05): every Thing with its state, an
//! enable switch, the consent prompt when one waits, and the selected Thing's
//! last error and log. Each frame it describes itself as a bones widget tree.
use std::collections::HashMap;
use std::path::PathBuf;

use bones_consent::{Consent, Manifest, PromptAnswer, Status};
use bones_kernel::exit::ExitRequest;
use bones_kernel::messages::ui::kinds::{
    Button, Checkbox, Heading, Label, Selectable, Separator, Strong, Weak,
};
use bones_kernel::messages::ui::{Event, Layout, Node, PanelSide, Root, Spec, WidgetProps};
use bones_kernel::ui::{Frame, UiView};
use bones_kernel::window_control::WindowControl;
use bones_module_lua::{GuestState, LuaControl, Permissions};

use crate::settings::Settings;
use crate::tray::{Tray, TrayAction};

/// The module name, under which bones shows this window's trees.
pub const NAME: &str = "home-feeling";

const RELOAD: u32 = 1;
const OPEN_FOLDER: u32 = 2;
/// Each Thing's row owns ten ids from here: select, allow, refuse, enable.
const ROWS: u32 = 1000;
const SELECT: u32 = 0;
const PROMPT: u32 = 0; // the prompt's buttons are +1 (allow) and +2 (refuse)
const ENABLE: u32 = 3;

pub struct ThingsWindow {
    control: LuaControl,
    consent: Consent,
    settings: Settings,
    things_dir: PathBuf,
    /// Each Thing's manifest as last read, or why it could not be read.
    manifests: HashMap<String, Result<Manifest, String>>,
    /// The Thing names in row order at the last frame.
    rows: Vec<String>,
    selected: Option<String>,
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
    ) -> Self {
        Self {
            control,
            consent,
            settings,
            things_dir,
            manifests: HashMap::new(),
            rows: Vec::new(),
            selected: None,
            offered,
            notice: None,
            tray: None,
            window: None,
            exit: None,
        }
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
        for record in self.control.records() {
            let name = record.name;
            if let Ok(manifest) = self.manifest(&name) {
                let deny = self.consent.deny_list(&name, &manifest);
                self.control.set_permissions(&name, Permissions::deny(deny));
                if self.consent.status(&name, &manifest) == Status::Pending {
                    self.control.stop(&name);
                }
            }
        }
        self.control.reload_all();
        self.notice = Some("Reloaded all Things.".into());
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
                    if let Some(window) = &self.window {
                        let _ = window.raise();
                    }
                }
                TrayAction::ReloadAll => self.reload_all(),
                TrayAction::Quit => {
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
            match self.manifest(&record.name) {
                Ok(manifest) => {
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
            let lines: Vec<Node> = record
                .log
                .iter()
                .rev()
                .take(100)
                .map(|line| {
                    Label {
                        text: format!("[{}] {}", line.level, line.text),
                    }
                    .node(0)
                })
                .collect();
            children.push(Node::container(Layout::Scroll, lines));
        }
        Node::container(
            Layout::Panel {
                side: PanelSide::Central,
            },
            vec![Node::container(Layout::Vertical, children)],
        )
    }
}

impl UiView for ThingsWindow {
    fn init(&mut self, ctx: &mut bones_kernel::bus::ModuleContext) -> Result<(), String> {
        self.window = ctx.get_service::<WindowControl>().cloned();
        self.exit = ctx.get_service::<ExitRequest>().cloned();
        self.tray = Tray::new();
        Ok(())
    }

    fn on_event(&mut self, event: Event) {
        let id = match &event {
            Event::Clicked { id } | Event::Toggled { id, .. } => *id,
            _ => return,
        };
        match id {
            RELOAD => self.reload_all(),
            OPEN_FOLDER => {
                let _ = std::fs::create_dir_all(&self.things_dir);
                if let Err(err) = open::that(&self.things_dir) {
                    self.notice = Some(format!("Could not open the folder: {err}"));
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
                }
            }
            _ => {}
        }
    }

    fn view(&mut self, _frame: &mut Frame<'_>) -> Spec {
        self.handle_tray();
        let list = self.list_panel();
        let detail = self.detail_panel();
        Spec {
            root: Root::Screen,
            body: Node::container(Layout::Vertical, vec![list, detail]),
        }
    }
}
