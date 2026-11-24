# home-feeling-desktop

A bones native module giving the bus mechanisms over the desktop: windows, monitors, global hotkeys, and overlays ([ADR-004](../../docs/adr/ADR-004-os-control-is-mechanism-in-rust-and-policy-in-lua.md), [ADR-005](../../docs/adr/ADR-005-desktop-overlays-belong-to-the-desktop-module.md)). Windows only in v1; elsewhere every capability reports unsupported.

Payloads are JSON, so Lua Things read and write them directly. The topics, the query endpoint, and their payloads are listed in [src/protocol.rs](src/protocol.rs).

Hotkeys and overlays belong to the guest that asked for them. They are released when it stops or faults, and on reload, those its replacement does not ask for again.

Run the tests with `cargo test -p home-feeling-desktop`. `cargo test -p home-feeling-desktop -- --ignored` also checks the Windows backend against the real desktop and briefly draws a red rectangle.
