# ADR-001: Things are Lua guests on the bones bus

## Problem

Things must be editable in a plain text editor and reloaded without restarting the app, and the scripting support should be able to move into bones later. Binding a Rust API straight into Lua would tie every Thing to this application's types.

## Decision

A Thing is a Lua guest on the bones bus. It reaches the rest of the system only by subscribing, publishing, sending, and logging, the same four operations a bones WASM extension has, and it draws only through `ui/spec` widget trees. Operating-system access comes from native modules that answer on the bus.

## Rationale

The bus already makes guests and native modules indistinguishable, so a Lua guest gets every module, present and future, without new bindings. The same boundary makes sandboxing, hot reload, and deny-lists possible, and keeps the Lua host free of anything specific to home-feeling.

## Rejected alternatives

- Rust functions bound directly into Lua: quicker to write and with no message hop, but tied to this app, not upstreamable, and with no sandbox boundary.
- WASM extensions: already in bones, but a compiled `.wasm` cannot be edited in a text editor.
