# ADR-004: OS control is mechanism in Rust and policy in Lua

## Problem

Tiling needs fast, platform-specific calls: window events, positions, and hotkeys. The user wants to change how tiling behaves by editing a script.

## Decision

`home-feeling-desktop` is a native module offering mechanisms on the bus: list and watch windows and monitors, move and resize windows, set focus, register global hotkeys, draw overlays, and report which of these the platform supports. Decisions such as layout, gaps, and when to float live in Lua Things, starting with the tiler.

## Rationale

Platform code and its edge cases, such as invisible frame borders and elevated windows, stay in Rust where they can be tested, and the behaviour stays editable. Later Things reuse the same mechanisms. Reporting capabilities lets a Thing explain itself where a mechanism is missing, such as on Linux in v1.

## Rejected alternatives

- The whole tiler in Rust: faster, but changing the layout would need a rebuild.
- Raw OS calls exposed to Lua: breaks the sandbox and repeats platform work in every Thing.
