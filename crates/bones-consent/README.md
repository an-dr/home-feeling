# bones-consent

Turns what a bones guest requests into what it may do ([ADR-003](../../docs/adr/ADR-003-capabilities-are-requested-by-the-guest-and-granted-by-the-user.md)). It names no guest runtime and no product, so it serves Lua and WASM hosts alike and can move into bones.

- The composition root declares **capabilities**: an id, a label the user reads, and the bus patterns it covers.
- A guest's `manifest.toml` **requests** capability ids:

  ```toml
  description = "Tiles windows"
  requests = ["windows", "hotkeys", "overlays"]
  ```

- The user's answer is stored per guest. An unchanged request keeps its answer; a changed one asks again.
- `deny_list` gives the patterns of every capability not granted, ready for the host's deny-list. A guest waiting for an answer is denied every capability.
- `prompt` builds the approval prompt as a bones widget subtree, and `PromptAnswer::from_event` reads the user's click.

Run the tests with `cargo test -p bones-consent`.
