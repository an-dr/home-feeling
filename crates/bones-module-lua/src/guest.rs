//! One Lua instance: its host API, its time budget, and calls into its
//! callbacks. Luau has no file, process, or network library, so the `bones`
//! table is the guest's only way out.
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bones_kernel_infra::bus::{Bus, Endpoint, EndpointBudget, Envelope, Registry, SendError};
use bones_kernel_infra::logging::Logger;
use mlua::{Function, Lua, LuaSerdeExt, MultiValue, SerializeOptions, Value, VmState};

use crate::control::LuaControl;
use crate::permissions::Permissions;

/// A guest may not allocate more than this.
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// Everything the `bones` table needs to reach the engine.
#[derive(Clone)]
pub(crate) struct HostApi {
    pub name: String,
    pub bus: Bus,
    pub registry: Registry,
    pub endpoint: Endpoint,
    pub budget: EndpointBudget,
    pub permissions: Permissions,
    pub logger: Logger,
    pub control: LuaControl,
}

impl HostApi {
    fn log(&self, level: &str, text: &str) {
        match level {
            "error" => self.logger.error(&self.name, text),
            "warn" => self.logger.warn(&self.name, text),
            "debug" => self.logger.debug(&self.name, text),
            _ => self.logger.info(&self.name, text),
        }
        self.control.log(&self.name, level, text);
    }
}

/// Writes the entries of the JSON object `over` into `table`, going into
/// tables on both sides instead of replacing them.
fn overlay(lua: &Lua, table: &mlua::Table, over: &serde_json::Value) -> mlua::Result<()> {
    let Some(entries) = over.as_object() else {
        return Ok(());
    };
    for (key, value) in entries {
        if value.is_object() {
            if let Value::Table(inner) = table.get::<Value>(key.as_str())? {
                overlay(lua, &inner, value)?;
                continue;
            }
        }
        table.set(key.as_str(), lua.to_value(value)?)?;
    }
    Ok(())
}

/// When the current call must end; `None` while the guest is idle.
type Deadline = Arc<Mutex<Option<Instant>>>;

pub(crate) struct Guest {
    lua: Lua,
    deadline: Deadline,
    call_budget: Duration,
}

impl Guest {
    /// Reads `dir/config.lua` and `dir/init.lua`, then runs `init()` within
    /// `init_budget`. Errors name the file and line.
    pub fn load(
        dir: &Path,
        api: HostApi,
        settings: Option<serde_json::Value>,
        init_budget: Duration,
        call_budget: Duration,
    ) -> Result<Self, String> {
        let lua = Lua::new();
        lua.set_memory_limit(MEMORY_LIMIT)
            .map_err(|err| err.to_string())?;
        let deadline: Deadline = Arc::default();
        let watched = deadline.clone();
        lua.set_interrupt(move |_| match *watched.lock().unwrap() {
            Some(end) if Instant::now() > end => Err(mlua::Error::runtime("time budget exceeded")),
            _ => Ok(VmState::Continue),
        });
        install_api(&lua, api.clone()).map_err(|err| err.to_string())?;

        let guest = Self {
            lua,
            deadline,
            call_budget,
        };
        let name = &api.name;
        let config = guest.run_file(dir, name, "config.lua", init_budget, true)?;
        let config = match config {
            Value::Nil => Value::Table(guest.lua.create_table().map_err(|e| e.to_string())?),
            value => value,
        };
        if let (Value::Table(table), Some(settings)) = (&config, &settings) {
            overlay(&guest.lua, table, settings)
                .map_err(|err| format!("{name}/settings: {err}"))?;
        }
        guest
            .lua
            .globals()
            .set("config", config)
            .map_err(|err| err.to_string())?;
        guest.run_file(dir, name, "init.lua", init_budget, false)?;
        guest.call_with("init", (), init_budget)?;
        Ok(guest)
    }

    fn run_file(
        &self,
        dir: &Path,
        name: &str,
        file: &str,
        budget: Duration,
        optional: bool,
    ) -> Result<Value, String> {
        let path = dir.join(file);
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(_) if optional && !path.exists() => return Ok(Value::Nil),
            Err(err) => return Err(format!("{name}/{file}: {err}")),
        };
        self.within(budget, || {
            self.lua
                .load(source)
                .set_name(format!("@{name}/{file}"))
                .eval::<Value>()
        })
    }

    /// Calls the global `function` if the guest defines it.
    pub fn call<A: mlua::IntoLuaMulti>(
        &self,
        function: &str,
        args: A,
    ) -> Result<Option<Vec<u8>>, String> {
        self.call_with(function, args, self.call_budget)
    }

    fn call_with<A: mlua::IntoLuaMulti>(
        &self,
        function: &str,
        args: A,
        budget: Duration,
    ) -> Result<Option<Vec<u8>>, String> {
        let Some(callback) = self
            .lua
            .globals()
            .get::<Option<Function>>(function)
            .map_err(|err| err.to_string())?
        else {
            return Ok(None);
        };
        let result: MultiValue = self.within(budget, || callback.call(args))?;
        match result.into_iter().next() {
            None | Some(Value::Nil) => Ok(None),
            Some(value) => payload_bytes(&self.lua, value)
                .map(Some)
                .map_err(|e| e.to_string()),
        }
    }

    /// Delivers one bus message: `on_tick(dt)` for ticks, `on_message`
    /// otherwise. Returns `on_message`'s reply.
    pub fn deliver(&self, envelope: &Envelope) -> Result<Option<Vec<u8>>, String> {
        if let Some(dt) = bones_kernel_infra::runner::read_tick_dt(envelope) {
            return self.call("on_tick", dt);
        }
        let payload = self
            .lua
            .create_string(&envelope.payload)
            .map_err(|err| err.to_string())?;
        self.call(
            "on_message",
            (envelope.topic.as_str(), envelope.sender.as_str(), payload),
        )
    }

    pub fn respond(&self, sender: &str, payload: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let payload = self
            .lua
            .create_string(payload)
            .map_err(|err| err.to_string())?;
        self.call("on_message", ("", sender, payload))
    }

    fn within<T>(
        &self,
        budget: Duration,
        run: impl FnOnce() -> mlua::Result<T>,
    ) -> Result<T, String> {
        let previous = self
            .deadline
            .lock()
            .unwrap()
            .replace(Instant::now() + budget);
        let result = run();
        *self.deadline.lock().unwrap() = previous;
        result.map_err(|err| describe(&err))
    }
}

/// The message without mlua's wrapping, keeping the `file:line:` prefix.
fn describe(err: &mlua::Error) -> String {
    match err {
        mlua::Error::CallbackError { cause, .. } => describe(cause),
        mlua::Error::RuntimeError(text) | mlua::Error::SyntaxError { message: text, .. } => {
            text.clone()
        }
        other => other.to_string(),
    }
}

/// A payload as bytes: strings as they are, tables as JSON, nil as empty.
fn payload_bytes(lua: &Lua, value: Value) -> mlua::Result<Vec<u8>> {
    Ok(match value {
        Value::Nil => Vec::new(),
        Value::String(text) => text.as_bytes().to_vec(),
        Value::Table(_) => {
            serde_json::to_vec(&to_json(lua, value)?).map_err(mlua::Error::external)?
        }
        other => other.to_string()?.into_bytes(),
    })
}

/// A Lua value as JSON. Luau numbers are all floats, so whole numbers are
/// written as integers, which is what receivers parsing integers expect.
fn to_json(lua: &Lua, value: Value) -> mlua::Result<serde_json::Value> {
    fn integral(value: serde_json::Value) -> serde_json::Value {
        use serde_json::Value as J;
        match value {
            J::Number(n) => match n.as_f64() {
                Some(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => J::from(f as i64),
                _ => J::Number(n),
            },
            J::Array(items) => J::Array(items.into_iter().map(integral).collect()),
            J::Object(map) => J::Object(map.into_iter().map(|(k, v)| (k, integral(v))).collect()),
            other => other,
        }
    }
    Ok(integral(lua.from_value(value)?))
}

fn install_api(lua: &Lua, api: HostApi) -> mlua::Result<()> {
    let bones = lua.create_table()?;
    bones.set("name", api.name.clone())?;
    let warned: Arc<Mutex<HashSet<String>>> = Arc::default();

    let refuse = {
        let api = api.clone();
        let warned = warned.clone();
        move |target: &str| {
            if warned.lock().unwrap().insert(target.to_string()) {
                api.log(
                    "warn",
                    &format!("permission refused: '{target}' is denied to this Thing"),
                );
            }
        }
    };

    let (a, r) = (api.clone(), refuse.clone());
    bones.set(
        "subscribe",
        lua.create_function(move |_, pattern: String| {
            if a.permissions.allows_subscription(&pattern) {
                a.endpoint.subscribe(pattern);
            } else {
                r(&pattern);
            }
            Ok(())
        })?,
    )?;

    let (a, r) = (api.clone(), refuse.clone());
    bones.set(
        "publish",
        lua.create_function(move |lua, (topic, payload): (String, Value)| {
            if !a.permissions.allows(&topic) {
                r(&topic);
                return Ok(());
            }
            if a.budget.accept_publish() {
                a.bus.publish(Envelope {
                    topic,
                    sender: a.name.clone(),
                    correlation: None,
                    payload: payload_bytes(lua, payload)?,
                });
            }
            Ok(())
        })?,
    )?;

    let (a, r) = (api.clone(), refuse);
    bones.set(
        "send",
        lua.create_function(move |lua, (endpoint, payload): (String, Value)| {
            if !a.permissions.allows(&endpoint) {
                r(&endpoint);
                return Ok((Value::Nil, Some("unknown-endpoint".to_string())));
            }
            let bytes = payload_bytes(lua, payload)?;
            match a.registry.call(&a.name, &endpoint, &bytes) {
                Ok(reply) => Ok((Value::String(lua.create_string(&reply)?), None)),
                Err(SendError::UnknownEndpoint) => {
                    Ok((Value::Nil, Some("unknown-endpoint".into())))
                }
                Err(SendError::Cycle) => Ok((Value::Nil, Some("cycle".into()))),
                Err(SendError::Timeout) => Ok((Value::Nil, Some("timeout".into()))),
            }
        })?,
    )?;

    for level in ["debug", "info", "warn", "error"] {
        let a = api.clone();
        bones.set(
            level,
            lua.create_function(move |_, text: String| {
                a.log(level, &text);
                Ok(())
            })?,
        )?;
    }
    let a = api;
    bones.set(
        "log",
        lua.create_function(move |_, (level, text): (String, String)| {
            a.log(&level, &text);
            Ok(())
        })?,
    )?;

    let json = lua.create_table()?;
    json.set(
        "encode",
        lua.create_function(|lua, value: Value| {
            serde_json::to_string(&to_json(lua, value)?).map_err(mlua::Error::external)
        })?,
    )?;
    json.set(
        "decode",
        lua.create_function(|lua, text: mlua::String| {
            let json: serde_json::Value =
                serde_json::from_slice(&text.as_bytes()).map_err(mlua::Error::external)?;
            let options = SerializeOptions::new()
                .serialize_none_to_null(false)
                .serialize_unit_to_null(false);
            lua.to_value_with(&json, options)
        })?,
    )?;
    bones.set("json", json)?;
    lua.globals().set("bones", bones)
}
