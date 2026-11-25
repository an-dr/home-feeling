# Changelog

Each release lists what changed for people who use home-feeling. Versions follow [Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-11-25

The first release: a desktop app built on bones whose tools, called Things, are Lua scripts you edit and reload, with a tiling window manager for Windows as the first Thing.

### Things

- A Thing is a folder with `init.lua` and `manifest.toml`; Reload all picks up your edits, and a broken edit keeps the previous version running and shows the error with its file and line.
- Things run sandboxed on the bones message bus under a time and message budget; one that hangs or floods the bus is stopped while the rest keep running.
- A Thing asks for capabilities such as moving windows, global hotkeys, or drawing overlays, and runs only after you allow them.
- A Thing declares its settings in its manifest, and the Things window draws a page for them: numbers, choices, colours, and hotkeys you record by pressing them.

### Tiler (Windows)

- Tiles windows on each monitor in a dwindle layout, with a configurable gap and a minimum tile size.
- A window that does not fit, or refuses its tile, joins a stack: a tile shared by several windows, one shown at a time, with a tab strip and a badge whose menu picks the window to show.
- Hotkeys move focus, swap tiles, float or tile a window, and show the next window in a stack; Caps Lock can serve as the float key without turning on capitals.
- Drag a window onto another tile's edge to split it, onto its middle to stack, or off its stack to float it; a preview shows where it will go, also when you bring it back to its own tile.
- Dragging the edge between tiles moves the split, and wins over sizes the tiler learned from windows; a window that really cannot shrink is learned again.
- A window you minimize and restore returns to its place, beside the same neighbour or in the same stack.
- A frame marks the focused window and floating windows, with the system's corner radius by default; frames hide while you drag and fade back in after, and nothing is drawn over a window in full screen.
- Trace window positions, under the tiler's Diagnostics settings, records window events and moves to a file for diagnosing layout problems.

### App

- A tray icon keeps the app running when its window is closed; one copy runs per user.
- The home-feeling settings page holds Start with Windows, Start in the tray, updates, and installation.
- Installs per user without administrator rights on Windows x64, Windows ARM64, and Linux x64, and updates itself from GitHub releases; your Things folder is never touched by an update.
- On Linux the app and the Things window run; the tiler reports that window management is not supported there.

[0.1.0]: https://github.com/an-dr/home-feeling/releases/tag/v0.1.0
