//! The handle a composition root keeps to drive the host and read what each
//! guest is doing. Commands are queued and applied at the start of the next
//! frame, so they never run inside a dispatch.
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::permissions::Permissions;

/// How many log lines each guest keeps.
const LOG_LINES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestState {
    /// Not running: never started, disabled, or unloaded.
    Stopped,
    Running,
    /// Quarantined after an error, a time budget, or a message budget.
    Faulted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub level: String,
    pub text: String,
}

/// What the host knows about one guest. It outlives reloads.
#[derive(Debug, Clone)]
pub struct GuestRecord {
    pub name: String,
    pub state: GuestState,
    /// Why the guest last failed to load or was quarantined; cleared by a
    /// successful load.
    pub last_error: Option<String>,
    pub log: VecDeque<LogLine>,
}

impl GuestRecord {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            state: GuestState::Stopped,
            last_error: None,
            log: VecDeque::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Command {
    /// Rescan the catalog, then reload every running or faulted guest.
    ReloadAll,
    Start(String),
    Stop(String),
    /// Replace a guest's deny-list; a running guest is reloaded to apply it.
    SetPermissions(String, Permissions),
}

#[derive(Default)]
pub(crate) struct Inner {
    pub(crate) commands: Vec<Command>,
    pub(crate) records: BTreeMap<String, GuestRecord>,
}

/// Cheap to clone; every clone drives the same host.
#[derive(Clone, Default)]
pub struct LuaControl {
    pub(crate) inner: Arc<Mutex<Inner>>,
}

impl LuaControl {
    /// Rescans the guests folder and reloads every running or faulted guest.
    pub fn reload_all(&self) {
        self.push(Command::ReloadAll);
    }

    pub fn start(&self, name: &str) {
        self.push(Command::Start(name.to_string()));
    }

    pub fn stop(&self, name: &str) {
        self.push(Command::Stop(name.to_string()));
    }

    pub fn set_permissions(&self, name: &str, permissions: Permissions) {
        self.push(Command::SetPermissions(name.to_string(), permissions));
    }

    /// Every guest in the catalog, by name.
    pub fn records(&self) -> Vec<GuestRecord> {
        self.inner
            .lock()
            .unwrap()
            .records
            .values()
            .cloned()
            .collect()
    }

    fn push(&self, command: Command) {
        self.inner.lock().unwrap().commands.push(command);
    }

    pub(crate) fn take_commands(&self) -> Vec<Command> {
        std::mem::take(&mut self.inner.lock().unwrap().commands)
    }

    pub(crate) fn update(&self, name: &str, change: impl FnOnce(&mut GuestRecord)) {
        let mut inner = self.inner.lock().unwrap();
        let record = inner
            .records
            .entry(name.to_string())
            .or_insert_with(|| GuestRecord::new(name));
        change(record);
    }

    pub(crate) fn log(&self, name: &str, level: &str, text: &str) {
        self.update(name, |record| {
            if record.log.len() == LOG_LINES {
                record.log.pop_front();
            }
            record.log.push_back(LogLine {
                level: level.to_string(),
                text: text.to_string(),
            });
        });
    }

    pub(crate) fn retain(&self, names: &[String]) {
        self.inner
            .lock()
            .unwrap()
            .records
            .retain(|name, record| names.contains(name) || record.state != GuestState::Stopped);
    }
}
