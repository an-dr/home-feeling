# home-feeling

[![build](https://github.com/an-dr/home-feeling/actions/workflows/build.yml/badge.svg)](https://github.com/an-dr/home-feeling/actions/workflows/build.yml) [![Powered by bones](https://img.shields.io/badge/powered%20by-bones-e8e2d4?labelColor=2b2b2b)](https://github.com/an-dr/bones)

Extensible desktop tools for Windows and Linux, built on [bones](https://github.com/an-dr/bones). Each tool is a **Thing**: a Lua script you edit in any text editor and reload with one button. The first Thing is a tiling window manager for Windows.

## Features

- **Things in Lua:** each Thing is a folder with `init.lua` and a `manifest.toml`. Edit, save, press Reload all. A broken edit keeps the previous version running and shows the error with file and line.
- **Sandboxed:** a Thing reaches the system only through the bones message bus, under a time budget. A Thing that hangs or floods the bus is stopped and the rest keep running.
- **You approve what a Thing may do:** a Thing asks for capabilities such as moving windows or global hotkeys, and runs only with the ones you allow.
- **Tiler (Windows):** a dwindle layout on each monitor, with a configurable gap, a minimum tile size below which a new window floats, hotkeys to move focus, swap windows and toggle floating, and a border around the focused window.
- **Self-updating:** installs per user, with no administrator rights, and never touches your Things folder.

## Install

Releases are published from version tags. Windows x64 or ARM64, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/an-dr/home-feeling/main/scripts/install.ps1 | iex
```

Linux x64 with glibc 2.35 or newer, which also needs `curl` and `unzip`:

```sh
curl -fsSL https://raw.githubusercontent.com/an-dr/home-feeling/main/scripts/install.sh | sh
```

Both verify the download's checksum, install into `~/.home-feeling`, and add the app to your application list. On Linux the core and the Things window work; the tiler reports that window management is not supported, and there is no tray icon yet.

## Use it

1. Start home-feeling. The Things window lists your Things; on Windows a tray icon brings it back.
2. Select the tiler and press **Allow** to grant what it asks for.
3. Start with Windows, Start in the tray, and updates are on the **home-feeling settings** page, below the list of Things.
4. Change the gap, minimum size, active window frame (on or off, colour, thickness, corners), or hotkeys under the Thing's **Settings** in the Things window. Changes apply a moment after you make them.

Default tiler hotkeys: `Alt+Arrow` moves focus, `Alt+Shift+Arrow` swaps with the neighbour, `Alt+Shift+Space` toggles floating.

If the tiler places a window wrongly, turn on **Trace window positions** under the tiler's **Settings → Diagnostics**, reproduce the problem, and turn it off. The app writes every window event the system reported and every move the tiler requested, one JSON object per line, to `%LOCALAPPDATA%\home-feeling\window-trace.jsonl` (`~/.local/share/home-feeling/window-trace.jsonl` on Linux). Window titles are included, so check the file before you share it. Tracing is off by default and stays on across restarts until you turn it off.

Your Things live in `%APPDATA%\home-feeling\things` on Windows and `~/.config/home-feeling/things` on Linux. [things/README.md](things/README.md) describes a Thing's files.

## Build from source

```sh
git clone --recurse-submodules https://github.com/an-dr/home-feeling
cd home-feeling
cargo run -p home-feeling
```

Needs a Rust toolchain, a C and C++ compiler, CMake, and Ninja, because bones builds SDL3 and mlua builds Luau from source. On Linux, run `cargo xtask linux-packages` inside `vendor/bones` to list the packages it also needs.

Run the tests with `cargo test --workspace`. To package and publish, see [docs/releasing.md](docs/releasing.md).

With [just](https://github.com/casey/just) installed, `just` lists shortcuts for these commands: `just run`, `just test`, `just check` for the checks CI runs, `just package` for a release zip, `just dist` to unpack this machine's release into `dist/` and run it from there, and others.

## Documentation

Start at [docs/index.md](docs/index.md) for use cases, architecture, and decisions.

## License

MIT. See [LICENSE](LICENSE).
