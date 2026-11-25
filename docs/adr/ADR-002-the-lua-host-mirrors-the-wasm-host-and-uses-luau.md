# ADR-002: The Lua host mirrors the WASM host and uses Luau

## Problem

The Lua host needs a contract and a Lua engine. A contract of its own would split the bones extension model in two, and the engine must be able to stop a script that runs too long.

## Decision

`bones-module-lua` is a native module with the contract of `bones-module-wasm`: the same lifecycle and `core/lifecycle` events, time budget and quarantine, reload that keeps the old instance on failure, and deny-lists enforced on the guest's endpoint. It names nothing of home-feeling. The engine is Luau, embedded through `mlua`, with one Lua state per guest, a memory limit, and its interrupt callback enforcing the time budget.

## Rationale

Matching the WASM host means a Thing author learns one model and bones can adopt the module without changing it. Luau's standard library has no file, process, or network access, so the `bones` table is a guest's only way out, and its interrupt callback gives the watchdog a place to stop a runaway script.

## Rejected alternatives

- LuaJIT: faster, but its standard library and FFI reach files, processes, and native code.
- piccolo, a Lua written in Rust: pure Rust, but still experimental.
- Rhai: easy to embed from Rust, but less familiar to Thing authors than Lua and with a smaller ecosystem.
