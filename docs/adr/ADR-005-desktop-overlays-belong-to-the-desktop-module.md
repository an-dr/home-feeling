# ADR-005: Desktop overlays belong to the desktop module

## Problem

The tiler draws a border around the focused window, which needs transparent, click-through, always-on-top windows over other applications. The bones ui module draws only inside the application's own windows.

## Decision

In v1, overlays are a capability of `home-feeling-desktop`. A Thing asks for simple shapes, such as borders and rectangles, at screen positions, and the module draws them in windows that take no clicks, focus, or taskbar entry. Overlays a Thing owns are removed when it stops.

## Rationale

It delivers the focus border without waiting for an engine change, beside the window positions it follows. A small shape vocabulary keeps the messages simple and the drawing cheap.

## Rejected alternatives

- Adding desktop overlays to bones first: the right long-term home, but it would block v1 on an engine release. The capability can move there later.
- Full widget trees in overlays: more than a border needs.
