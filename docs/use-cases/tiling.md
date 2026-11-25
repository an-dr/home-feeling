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
- **Success**: Focus moves to the neighbour in the pressed direction, also across monitors. The focused window swaps with that neighbour. A toggle key turns the focused window between tiled and floating. Caps Lock is a second toggle key: while the tiler owns it, it fires the toggle and never turns capitals on. A floating window is moved freely by dragging and never tiles by a drop; only the toggle key tiles it.

## UC-TILE-04: See which window has focus

- **Actor**: Desktop user.
- **Goal**: Tell at a glance which window keyboard actions apply to.
- **Flow**: Move focus by hotkey or mouse.
- **Success**: A border in the configured colour, thickness, and corner radius surrounds the focused window, or none when the user turned it off, drawn above the desktop without taking clicks or focus, and follows the window as it moves.

## UC-TILE-05: The tiler is unavailable on Linux

- **Actor**: Desktop user.
- **Goal**: Understand why tiling does nothing on Linux.
- **Flow**: Enable the tiling Thing on Linux.
- **Success**: The Things window reports that window management is not supported on this platform. No other Thing is affected.

## UC-TILE-06: Choose a window in a stack

- **Actor**: Desktop user.
- **Goal**: Reach any of several windows that share one tile.
- **Flow**: Click the round badge with the window count in the top left corner of the tile. A menu lists the stack's windows, a small rounded card, each row with the window's icon, title, and program, the shown one marked with a bar and bold type. Choose one.
- **Success**: The chosen window is shown in the tile and focused, and the others stay minimized. Dismissing the menu changes nothing.
- **Enlarging**: A stacked window that is maximized leaves its stack, which shows the next window and counts one fewer; restored, it rejoins that stack.
- **Stacking on purpose**: Dropping a dragged window on the middle third of a tile stacks it there even when there is room for its own tile; a setting makes that swap the two tiles instead, and another makes every new window join the focused tile. Stacks are kept when the monitors change; only the windows of a monitor that went away move to another one, each stack together.
- **Also**: The badge sits above its window only, so windows in front of that window cover it too. Dragging the shown window out of the stack onto another tile's edge puts it in a new cell there, onto an edge of its own tile splits that tile, the window taking that side, onto another tile's middle adds it to that tile's stack, and onto no tile floats it where it was dropped; the old stack's count goes down by one, and its badge goes when one window is left.
