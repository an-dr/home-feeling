# ADR-003: Capabilities are requested by the guest and granted by the user

## Problem

A Thing can move windows and capture hotkeys, so a Thing someone else wrote is as powerful as any program. bones lets only the composition root deny a guest topics (bones ADR-042), and rejects a manifest that grants its own permissions, because a guard the guarded code writes guards nothing.

## Decision

A Thing's manifest lists the capabilities it requests. `bones-consent` shows the request to the user, stores the answer per guest and request, and produces the deny-list for everything not granted. The composition root gives that deny-list to the guest host. A new Thing, or one whose request changes, waits until the user answers. `bones-consent` names neither Lua nor home-feeling, so it serves the WASM host as well and can move into bones.

## Rationale

The manifest asks and the user decides, so the guard is still set by the composition root, on the user's behalf. Keeping consent in its own crate lets bones adopt it beside `bones-module-lua`; that move needs a bones ADR extending ADR-042 with user-approved requests.

## Rejected alternatives

- Full trust: simplest, but a downloaded Thing could do anything.
- Enforcing the manifest without asking: catches mistakes, not malice.
- Consent inside the app only: simpler, but not reusable by bones or other apps.
