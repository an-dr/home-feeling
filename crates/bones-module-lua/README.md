# bones-module-lua

A bones native module that hosts Lua guests ("Things" in home-feeling) on the bus with the contract of `bones-module-wasm`: the same four host operations, lifecycle events on `core/lifecycle`, time budget and quarantine, atomic reload, and deny-lists. It names nothing of home-feeling, so it can move into bones unchanged ([ADR-002](../../docs/adr/ADR-002-the-lua-host-mirrors-the-wasm-host-and-uses-luau.md)).

## A guest

A guest is a folder holding `init.lua` and, optionally, `config.lua`, which returns a table. The folder name is the guest's name and bus endpoint.

```lua
-- init.lua
function init()
  bones.subscribe("core/tick")
  bones.info("started with gap " .. tostring(config.gap))
end

function on_message(topic, sender, payload)
  -- payload is a byte string; bones.json.decode reads JSON payloads
end

function on_tick(dt) end
function shutdown() end
```

| `bones.` | Does |
| --- | --- |
| `subscribe(pattern)` | Subscribes to a topic or `prefix/*` |
| `publish(topic, payload)` | Publishes a string, or a table encoded as JSON |
| `send(endpoint, payload)` | Calls an endpoint directly; returns the reply, or `nil` and an error |
| `log(level, text)`, `info`, `warn`, `error`, `debug` | Writes to the guest's log |
| `json.encode(value)`, `json.decode(text)` | JSON helpers |
| `name` | The guest's name |

Luau has no file, process, or network library, so a guest reaches the outside only through the bus.

## Composing it

```rust
let lua = bones_module_lua::LuaHost::new(logger).guests_dir(things_dir);
let control = lua.control(); // reload, enable, statuses, from the app
kernel.module(lua)
```

Run the tests with `cargo test -p bones-module-lua`.
