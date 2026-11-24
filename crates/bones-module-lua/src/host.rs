//! The module: catalog, loading, reload, quarantine, and lifecycle events.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bones_kernel_infra::bus::{
    BudgetLimits, Bus, Endpoint, EndpointBudget, Envelope, Handler, Module, ModuleContext,
    Registry, Respond,
};
use bones_kernel_infra::logging::Logger;
use bones_messages::lifecycle::{Event, LifecycleEvent};
use bones_messages::{EncodeMessage, Message};

use crate::control::{Command, GuestState, LuaControl};
use crate::guest::{Guest, HostApi};
use crate::permissions::Permissions;

/// The module's own endpoint, and the sender stamped on lifecycle events.
const ENDPOINT: &str = "lua-host";

/// A loaded guest, shared by its bus endpoint and its direct-send target.
#[derive(Default)]
struct Cell {
    guest: Option<Guest>,
    fault: Option<String>,
}

#[derive(Clone, Default)]
struct CellHandle(Arc<Mutex<Cell>>);

impl CellHandle {
    fn run(&self, call: impl FnOnce(&Guest) -> Result<Option<Vec<u8>>, String>) -> Option<Vec<u8>> {
        let mut cell = self.0.lock().unwrap();
        if cell.fault.is_some() {
            return None;
        }
        let result = call(cell.guest.as_ref()?);
        match result {
            Ok(reply) => reply,
            Err(err) => {
                cell.fault = Some(err);
                None
            }
        }
    }
}

impl Handler for CellHandle {
    fn handle(&mut self, envelope: &Envelope) {
        self.run(|guest| guest.deliver(envelope));
    }
}

impl Respond for CellHandle {
    fn respond(&self, sender: &str, payload: &[u8]) -> Option<Vec<u8>> {
        self.run(|guest| guest.respond(sender, payload))
    }
}

struct Running {
    endpoint: Endpoint,
    cell: CellHandle,
    budget: EndpointBudget,
}

/// The Lua guest host. Configure it, register it after the modules guests
/// call during `init`, and keep its [`control`](Self::control) handle.
pub struct LuaHost {
    logger: Logger,
    guests_dir: Option<PathBuf>,
    startup: Option<Vec<String>>,
    permissions: HashMap<String, Permissions>,
    budget_limits: BudgetLimits,
    init_budget: Duration,
    call_budget: Duration,
    control: LuaControl,
    bus: Option<Bus>,
    registry: Option<Registry>,
    catalog: HashMap<String, PathBuf>,
    running: HashMap<String, Running>,
}

impl LuaHost {
    pub fn new(logger: Logger) -> Self {
        Self {
            logger,
            guests_dir: None,
            startup: None,
            permissions: HashMap::new(),
            budget_limits: BudgetLimits::default(),
            init_budget: Duration::from_millis(500),
            call_budget: Duration::from_millis(50),
            control: LuaControl::default(),
            bus: None,
            registry: None,
            catalog: HashMap::new(),
            running: HashMap::new(),
        }
    }

    /// Catalogs every folder in `dir` that holds an `init.lua`; the folder
    /// name is the guest's name.
    pub fn guests_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.guests_dir = Some(dir.into());
        self
    }

    /// Starts only these at startup; the rest wait for
    /// [`LuaControl::start`]. Without it every catalogued guest starts.
    pub fn startup(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.startup = Some(names.into_iter().collect());
        self
    }

    /// Denies the guest called `name` the topics and endpoints in `permissions`.
    pub fn permissions(mut self, name: impl Into<String>, permissions: Permissions) -> Self {
        self.permissions.insert(name.into(), permissions);
        self
    }

    /// The wall-clock budgets for loading and for each later call.
    pub fn budgets(mut self, init: Duration, call: Duration) -> Self {
        self.init_budget = init;
        self.call_budget = call;
        self
    }

    pub fn budget_limits(mut self, limits: BudgetLimits) -> Self {
        self.budget_limits = limits;
        self
    }

    pub fn control(&self) -> LuaControl {
        self.control.clone()
    }

    fn scan(&mut self) {
        let Some(dir) = &self.guests_dir else { return };
        self.catalog = find_guests(dir);
        let names: Vec<String> = self.catalog.keys().cloned().collect();
        self.control.retain(&names);
        for name in names {
            self.control.update(&name, |_| {});
        }
    }

    fn lifecycle(&self, name: &str, event: Event) {
        if let Some(bus) = &self.bus {
            bus.publish(Envelope {
                topic: LifecycleEvent::TOPIC.to_string(),
                sender: ENDPOINT.to_string(),
                correlation: None,
                payload: LifecycleEvent {
                    event,
                    extension: name,
                }
                .encode(),
            });
        }
    }

    /// Loads `name` into a fresh endpoint. On success it replaces any running
    /// instance, which shuts down afterwards; on failure the running instance
    /// stays.
    fn load(&mut self, name: &str) {
        let (Some(bus), Some(registry)) = (self.bus.clone(), self.registry.clone()) else {
            return;
        };
        let Some(dir) = self.catalog.get(name).cloned() else {
            self.control.update(name, |record| {
                record.last_error = Some("no such Thing folder".into());
            });
            return;
        };
        let replacing = self.running.contains_key(name);
        if replacing {
            self.lifecycle(name, Event::Reloading);
        }
        let cell = CellHandle::default();
        let budget = EndpointBudget::new(self.budget_limits);
        let endpoint = bus.register_with_budget(name.to_string(), cell.clone(), budget.clone());
        let api = HostApi {
            name: name.to_string(),
            bus: bus.clone(),
            registry: registry.clone(),
            endpoint: endpoint.clone(),
            budget: budget.clone(),
            permissions: self.permissions.get(name).cloned().unwrap_or_default(),
            logger: self.logger.clone(),
            control: self.control.clone(),
        };
        match Guest::load(&dir, api, self.init_budget, self.call_budget) {
            Ok(guest) => {
                cell.0.lock().unwrap().guest = Some(guest);
                registry.insert(name.to_string(), Arc::new(cell.clone()));
                let previous = self.running.insert(
                    name.to_string(),
                    Running {
                        endpoint,
                        cell,
                        budget,
                    },
                );
                if let Some(old) = previous {
                    self.shut_down(old);
                }
                self.control.update(name, |record| {
                    record.state = GuestState::Running;
                    record.last_error = None;
                });
                self.lifecycle(
                    name,
                    if replacing {
                        Event::Reloaded
                    } else {
                        Event::Loaded
                    },
                );
            }
            Err(err) => {
                bus.unregister(&endpoint);
                self.logger.error(name, &format!("failed to load: {err}"));
                self.control
                    .log(name, "error", &format!("failed to load: {err}"));
                self.control.update(name, |record| {
                    record.last_error = Some(err);
                    if !replacing {
                        record.state = GuestState::Faulted;
                    }
                });
                if !replacing {
                    self.lifecycle(name, Event::Faulted);
                }
            }
        }
    }

    /// Calls `shutdown()` and releases the endpoint, keeping the registry
    /// entry, which a replacement may already hold.
    fn shut_down(&self, running: Running) {
        running.cell.run(|guest| guest.call("shutdown", ()));
        if let Some(bus) = &self.bus {
            bus.unregister(&running.endpoint);
        }
    }

    fn stop(&mut self, name: &str) {
        if let Some(running) = self.running.remove(name) {
            self.shut_down(running);
            if let Some(registry) = &self.registry {
                registry.remove(name);
            }
            self.lifecycle(name, Event::Stopped);
        }
        self.control
            .update(name, |record| record.state = GuestState::Stopped);
    }

    fn quarantine(&mut self, name: &str, reason: String) {
        if let Some(running) = self.running.remove(name) {
            if let Some(bus) = &self.bus {
                bus.unregister(&running.endpoint);
            }
            if let Some(registry) = &self.registry {
                registry.remove(name);
            }
        }
        self.logger.error(name, &format!("quarantined: {reason}"));
        self.control
            .log(name, "error", &format!("quarantined: {reason}"));
        self.control.update(name, |record| {
            record.state = GuestState::Faulted;
            record.last_error = Some(reason);
        });
        self.lifecycle(name, Event::Faulted);
    }

    fn apply(&mut self, command: Command) {
        match command {
            Command::ReloadAll => {
                self.scan();
                let faulted: Vec<String> = self
                    .control
                    .records()
                    .into_iter()
                    .filter(|record| record.state == GuestState::Faulted)
                    .map(|record| record.name)
                    .collect();
                let mut names: Vec<String> = self.running.keys().cloned().collect();
                names.extend(faulted);
                names.sort();
                names.dedup();
                for name in names {
                    self.load(&name);
                }
            }
            Command::Start(name) => {
                if !self.running.contains_key(&name) {
                    self.scan();
                    self.load(&name);
                }
            }
            Command::Stop(name) => self.stop(&name),
            Command::SetPermissions(name, permissions) => {
                self.permissions.insert(name.clone(), permissions);
                if self.running.contains_key(&name) {
                    self.load(&name);
                }
            }
        }
    }
}

fn find_guests(dir: &Path) -> HashMap<String, PathBuf> {
    let mut found = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.join("init.lua").is_file() {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                found.insert(name.to_string(), path);
            }
        }
    }
    found
}

impl Handler for LuaHost {
    fn handle(&mut self, _envelope: &Envelope) {}
}

impl Module for LuaHost {
    fn name(&self) -> &str {
        ENDPOINT
    }

    fn init(&mut self, ctx: &mut ModuleContext) -> Result<(), String> {
        self.bus = Some(
            ctx.get_service::<Bus>()
                .cloned()
                .ok_or("lua needs the bus service")?,
        );
        self.registry = Some(
            ctx.get_service::<Registry>()
                .cloned()
                .ok_or("lua needs the registry service")?,
        );
        self.scan();
        let mut names: Vec<String> = match &self.startup {
            Some(names) => names.clone(),
            None => self.catalog.keys().cloned().collect(),
        };
        names.sort();
        for name in names {
            self.load(&name);
        }
        Ok(())
    }

    fn before_tick(&mut self) {
        for command in self.control.take_commands() {
            self.apply(command);
        }
    }

    fn after_tick(&mut self) {
        let mut faults = Vec::new();
        for (name, running) in &self.running {
            if let Some(fault) = running.cell.0.lock().unwrap().fault.clone() {
                faults.push((name.clone(), fault));
            } else if running.budget.has_exceeded() {
                let drops = running.budget.get_drop_counters();
                faults.push((
                    name.clone(),
                    format!(
                        "message budget exceeded ({drops:?}; limits {} inbound, {} publishes per frame)",
                        self.budget_limits.max_inbound, self.budget_limits.max_publishes
                    ),
                ));
            }
        }
        for (name, reason) in faults {
            self.quarantine(&name, reason);
        }
    }

    fn begin_shutdown(&mut self) {
        let names: Vec<String> = self.running.keys().cloned().collect();
        for name in names {
            self.stop(&name);
        }
    }
}
