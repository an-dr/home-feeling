# ADR-006: Distribution follows crossWall

## Problem

The app must install without administrator rights on Windows and Linux and update itself, while the user's Things folder survives every update.

## Decision

Distribute the way [crossWall](https://github.com/an-dr/cross-wall) does: a per-user install under `~/.home-feeling`, a stable launcher built on `bones-launcher` that starts the newest installed version, packages from `cargo xtask package`, install scripts that verify checksums, and GitHub Releases published by CI from a version tag, which installed apps update from. Built-in Things are copied to the user's Things folder on first run, and updates never overwrite them.

## Rationale

The approach is proven on these platforms with the bones upgrade and launcher crates. The launcher lets the running version be replaced and keeps the previous one for recovery.

## Rejected alternatives

- System installers such as MSI or distribution packages: need administrator rights and a channel per format.
- Shipping Things only inside the install folder: updates would overwrite the user's edits.
