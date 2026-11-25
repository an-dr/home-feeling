# Built-in Things

The Things that ship with home-feeling. On first run the app copies each into your Things folder, where it is yours to edit; press Reload all in the Things window to apply a change. A file you edited is never overwritten: a newer built-in version is put beside it as `<file>.new`.

| Thing | Does | Asks for |
| --- | --- | --- |
| [tiler](tiler/) | Tiles windows on each monitor in a dwindle layout, with gaps, a minimum tile size below which windows float, keyboard focus and swap, and a focus border. Windows only. | windows, hotkeys, overlays |

A Thing is a folder with `init.lua` and a `manifest.toml` listing the capabilities it asks for and its settings.

The settings are `[[settings]]` tables with a dotted `key`, a `type` (`bool`, `int`, `number`, `string`, `color`, `choice`, `keys`), and a `default`; `label`, `description`, `section`, `min`, `max`, and `options` are optional. The Things window draws a control for each, and `init.lua` reads the values, nested by the dots in the keys, from the global `config`. A `config.lua` returning a table is still read beneath them for undeclared keys. The Lua API is in [crates/bones-module-lua](../crates/bones-module-lua/README.md); the desktop topics are in [crates/home-feeling-desktop](../crates/home-feeling-desktop/src/protocol.rs).
