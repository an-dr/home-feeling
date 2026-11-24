# Tiling

The built-in tiler Thing. Windows only in v1.

## UC-TILE-01: Windows arrange themselves

- **Actor**: Desktop user.
- **Goal**: Have application windows fill each monitor without overlapping.
- **Flow**: Open, close, minimise or restore windows as usual.
- **Success**: Each monitor's managed windows are tiled in a dwindle layout: every new window splits the focused tile along its longer side. A configured gap separates windows from each other and from the monitor edge. Each monitor is tiled on its own.

## UC-TILE-02: Small tiles float instead

- **Actor**: Desktop user.
- **Goal**: Never get a window squeezed too small to use.
- **Flow**: Open another window on a monitor that is already full.
- **Success**: If the new window's tile would be narrower than the configured minimum width or shorter than the minimum height, the window floats at its own size instead, and the existing tiles stay as they were.

## UC-TILE-03: Drive the layout from the keyboard

- **Actor**: Desktop user.
- **Goal**: Move focus and windows without the mouse.
- **Flow**: Press the configured hotkeys.
- **Success**: Focus moves to the neighbour in the pressed direction, also across monitors. The focused window swaps with that neighbour. A toggle key turns the focused window between tiled and floating.

## UC-TILE-04: See which window has focus

- **Actor**: Desktop user.
- **Goal**: Tell at a glance which window keyboard actions apply to.
- **Flow**: Move focus by hotkey or mouse.
- **Success**: A border in the configured colour and width surrounds the focused window, drawn above the desktop without taking clicks or focus, and follows the window as it moves.

## UC-TILE-05: The tiler is unavailable on Linux

- **Actor**: Desktop user.
- **Goal**: Understand why tiling does nothing on Linux.
- **Flow**: Enable the tiling Thing on Linux.
- **Success**: The Things window reports that window management is not supported on this platform. No other Thing is affected.
