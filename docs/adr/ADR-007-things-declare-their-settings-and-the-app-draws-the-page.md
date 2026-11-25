# ADR-007: Things declare their settings and the app draws the page

## Problem

A Thing's options lived in a `config.lua` the user had to edit by hand. Every Thing would need its own way to show options, and a typo in a file is the only feedback.

## Decision

A Thing lists its settings in `manifest.toml` as `[[settings]]` tables: a dotted `key`, a `type` (bool, int, number, string, color, choice, keys), a `default`, and optional label, description, section, range, or options. The Things window builds its controls from that list, checks each value against its declaration, keeps what the user chose in the app's own settings file, and applies a change after a short pause by reloading the Thing. The Thing reads the merged values, nested by the dots in the keys, as its global `config`. A `config.lua` is still read as the lowest layer for keys no manifest declares.

## Rationale

Every Thing gets a settings page for free, with the same look, validation, and reset. The defaults live beside the Thing, so a Thing author writes no interface code, and a manifest with a default of the wrong type is rejected when it is read.

## Rejected alternatives

- Each Thing drawing its own settings panel: more flexible, but every Thing repeats the work and the pages drift apart.
- Writing the user's choices back into `config.lua`: the file would stop being the author's, and a built-in update could never replace it.
